//! The request handlers. The daemon and the in-process CLI both call
//! [`handle`], so they give the same answers.

use crate::activity::{self, EntryKind};
use crate::config;
use crate::doc::assembly::{Instance, Pose, Rev};
use crate::doc::files::{
    Feature, Manifest, PartFile, PartKind, Placement, Primitive, PrimitiveType, number,
};
use crate::doc::id::Id;
use crate::doc::project::{
    MANIFEST, Part, feature_path, parameters_path, part_dir, part_path, write_json,
};
use crate::doc::reference::Reference;
use crate::doc::sketch::{OnFace, OriginPlane, Plane, Sketch};
use crate::doc::solid::{Boolean, Combine};
use crate::doc::sweep::{Angle, Extent, Extrude, Revolve, RevolveAxis};
use crate::entity::{self, EntityRef};
use crate::error::{Code, Error, Result};
use crate::expr::Env;
use crate::live::{self, CadLive, Live, SelectMode};
use crate::mesh;
use crate::notify::Topic;
use crate::param;
use crate::proto::{Answer, FeatureFields, PlacementArg, Position, Request, Transfer, VersionInfo};
use crate::rebuild;
use crate::session::Session;
use crate::solid::Solids;
use crate::units::{Quantity, Unit};
use crate::view::{Camera, DEFAULT_PROJECTION, StandardView};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Creates a project in `root`. Not a session request: there is no
/// project to open until this has run.
pub fn init(root: &Path, default_unit: Option<Unit>) -> Result<Value> {
    let manifest = root.join(MANIFEST);
    if manifest.exists() {
        return Err(Error::new(
            Code::ProjectExists,
            format!("{} already holds a project", root.display()),
        )
        .with("path", root));
    }
    let m = Manifest::new(default_unit.unwrap_or(Unit::Mm));
    write_json(&manifest, &m.to_json())?;
    crate::drivers::write_attributes(root)?;
    Ok(json!({"root": root, "default_unit": m.default_unit}))
}

/// Answers one request against `session`, after bringing it up to date
/// with the files, as a run with no daemon does.
pub fn handle(session: &mut Session, req: Request) -> Result<Value> {
    handle_live(session, None, None, req)
}

/// Answers one request. `live` is the daemon's live session (`None` in
/// process) and `me` the participant asking, if its connection said hello.
pub fn handle_live(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    req: Request,
) -> Result<Value> {
    answer_live(session, live, me, req).map(|a| a.result)
}

/// [`handle_live`], with any bytes the answer carries.
pub fn answer_live(
    session: &mut Session,
    mut live: Option<&mut Live>,
    me: Option<&str>,
    req: Request,
) -> Result<Answer> {
    let external = session.refresh_changes()?;
    let changed = !external.is_empty();
    let before = live.as_deref().map(Live::snapshot);
    if let Some(l) = live.as_deref_mut() {
        if changed {
            activity::record_external(l, external);
        }
        if let Some(me) = me {
            l.touch(me);
        }
    }
    let mutates = req.mutates();
    // A daemon keeps what its writes change, for the log and for undo;
    // undo and redo log themselves.
    let (method, params) = (req.method(), req.params());
    let logs_itself = matches!(req, Request::Undo { .. } | Request::Redo { .. });
    let files_before = match (&live, mutates && !logs_itself) {
        (Some(_), true) => Some(session.snapshot()),
        _ => None,
    };
    let result = match req {
        Request::MeshGet {
            part,
            feature,
            features,
            quality,
            transfer: Some(t @ (Transfer::Binary | Transfer::Compact)),
            rev,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            let only = wanted(feature.as_deref(), &features);
            let compact = t == Transfer::Compact;
            let mut a = mesh_binary(s, &part, only.as_ref(), quality.as_deref(), compact)?;
            a.result = versioned(noted(Ok(a.result), commit))?;
            Ok(a)
        }),
        Request::Render {
            part,
            camera,
            participant,
            view,
            width,
            height,
            highlight,
            edges,
            labels,
            sketches,
            rev,
            frame,
            compare,
            hidden,
            origin,
        } => {
            // What the participant's window hides, else the one human's
            // (where an agent's `hide` went), else the caller's.
            let hidden = match hidden {
                Some(refs) => crate::visibility::reach_all(session, &refs)?,
                None => match live.as_deref_mut() {
                    Some(l) => {
                        let who = l.view_target(participant.as_deref(), me)?;
                        l.get(&who)?.view.hidden.clone()
                    }
                    None => Default::default(),
                },
            };
            let target = participant.as_deref().map(|p| (p, live.as_deref()));
            // A participant's render shows what their window shows.
            let part = part.or_else(|| {
                let l = live.as_deref()?;
                Some(
                    l.get(participant.as_deref()?)
                        .ok()?
                        .view
                        .focus
                        .as_ref()?
                        .to_string(),
                )
            });
            let shown = match live.as_deref() {
                Some(l) => l
                    .participants()
                    .flat_map(|p| p.highlights.clone())
                    .collect(),
                None => Vec::new(),
            };
            let ask = RenderAsk {
                part,
                camera,
                view,
                width: width.unwrap_or(800),
                height: height.unwrap_or(600),
                highlight,
                edges: edges.unwrap_or(true),
                labels: labels.unwrap_or(false),
                sketches: sketches.unwrap_or(true),
                rev,
                frame,
                compare,
                hidden,
                origin,
            };
            render(session, target, shown, ask)
        }
        Request::Export {
            part,
            rev,
            format,
            out,
            schema,
            unit,
            time_stamp,
            file_name,
            chord,
            angle,
            max_edge,
            ascii,
        } => crate::export::export(
            session,
            crate::export::Ask {
                part,
                rev,
                format,
                out,
                schema,
                unit,
                time_stamp,
                file_name,
                chord,
                angle,
                max_edge,
                ascii,
            },
        ),
        req => dispatch(session, live.as_deref_mut(), me, req).map(Answer::json),
    };
    if let (Some(l), Some(files_before), true) = (live.as_deref_mut(), files_before, result.is_ok())
    {
        let files = activity::diff(&files_before, &session.snapshot());
        let who = l.caller(me);
        if !files.is_empty() {
            activity::record(l, EntryKind::Change, &who, &method, params, None, files);
        } else if method == "release.sync" {
            activity::record(l, EntryKind::Refs, &who, &method, params, None, files);
        }
    }
    if let (Some(l), Some(before)) = (live, before) {
        // Entities that no longer exist drop out of every selection.
        if changed || mutates {
            l.prune(|r| r.resolve(session).is_ok());
        }
        l.announce(&before);
        publish(l, session);
        // What the request used may go now that it has answered.
        session.trim();
    }
    result
}

fn dispatch(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    req: Request,
) -> Result<Value> {
    match req {
        Request::ProjectInit { .. } => Err(Error::new(
            Code::ProjectExists,
            format!("{} already holds a project", session.root().display()),
        )
        .with("path", session.root())),
        Request::ProjectInfo {} => Ok(project_info(session)),
        Request::PartCreate { part, kind } => part_create(session, &part, kind.as_deref()),
        Request::PartDelete {
            part,
            expected_revision,
        } => part_delete(session, &part, expected_revision.as_deref()),
        Request::PartList {} => Ok(part_list(session)),
        Request::FeatureAdd {
            part,
            ty,
            params,
            placement,
            fields,
            id,
            position,
            expected_revision,
        } => feature_add(
            session,
            &part,
            &ty,
            Given {
                params,
                placement,
                fields,
            },
            id.as_deref(),
            &position,
            expected_revision.as_deref(),
        ),
        Request::FeatureEdit {
            part,
            id,
            params,
            placement,
            fields,
            expected_revision,
        } => feature_edit(
            session,
            &part,
            &id,
            Given {
                params,
                placement,
                fields,
            },
            expected_revision.as_deref(),
        ),
        Request::FeatureDelete {
            part,
            id,
            expected_revision,
        } => feature_delete(session, &part, &id, expected_revision.as_deref()),
        Request::FeatureMove {
            part,
            id,
            position,
            expected_revision,
        } => feature_move(session, &part, &id, &position, expected_revision.as_deref()),
        Request::FeatureList { part } => feature_list(session, &part),
        Request::PdmInit {} => crate::pdm::init(session),
        Request::PdmConfig {} => crate::pdm::config(session),
        Request::PdmConfigure {
            revisions,
            release_branch,
            pattern,
            properties,
            force,
            expected_revision,
        } => crate::pdm::configure(
            session,
            crate::pdm::Change {
                revisions,
                release_branch,
                pattern,
                properties,
                force,
            },
            expected_revision.as_deref(),
        ),
        Request::PropsGet { part } => crate::pdm::get(session, &part),
        Request::BomGet {
            part,
            rev,
            flat,
            depth,
            format,
        } => crate::bom::get(
            session,
            &crate::bom::Ask {
                part: part.as_deref(),
                rev: rev.as_deref(),
                flat,
                depth,
                format: format.as_deref(),
            },
        ),
        Request::BomDiff { part, from, to } => {
            crate::bom::diff(session, part.as_deref(), from.as_deref(), to.as_deref())
        }
        Request::BomWhereUsed {
            part,
            label,
            working,
            direct,
        } => crate::bom::where_used(session, &part, label.as_deref(), working, direct),
        Request::BomRenumber {
            part,
            items,
            compact,
            expected_revision,
        } => crate::bom::renumber(
            session,
            &part,
            &items,
            compact,
            expected_revision.as_deref(),
        ),
        Request::ReleaseList { part } => versioned(crate::release::list(session, part.as_deref())),
        Request::ReleaseSync {} => crate::release::sync(session),
        Request::ReleaseCheck { results } => crate::release::check(session, results),
        Request::ReleasePropose {
            part,
            note,
            expected_revision,
            dry_run,
        } => {
            let who = participant_name(live, me);
            crate::release::propose(
                session,
                &who,
                &part,
                note,
                expected_revision.as_deref(),
                dry_run,
            )
        }
        Request::ReleaseObsolete {
            part,
            label,
            note,
            expected_revision,
        } => {
            let who = participant_name(live, me);
            crate::release::obsolete(
                session,
                &who,
                &part,
                &label,
                note,
                expected_revision.as_deref(),
            )
        }
        Request::PropsSet {
            part,
            properties,
            expected_revision,
        } => crate::pdm::set(session, &part, properties, expected_revision.as_deref()),
        Request::ParamList { part } => param::list(session, &part),
        Request::ParamSet {
            part,
            parameters,
            expected_revision,
        } => param::set(session, &part, parameters, expected_revision.as_deref()),
        Request::ParamDelete {
            part,
            name,
            expected_revision,
        } => param::delete(session, &part, &name, expected_revision.as_deref()),
        Request::ParamRename {
            part,
            from,
            to,
            expected_revision,
        } => param::rename(session, &part, &from, &to, expected_revision.as_deref()),
        Request::SketchEdit {
            part,
            sketch,
            ops,
            expected_revision,
        } => crate::sketch::edit(session, &part, &sketch, &ops, expected_revision.as_deref()),
        Request::SketchSolve { part, sketch, ops } => {
            crate::sketch::solve_request(session, &part, &sketch, &ops)
        }
        Request::SketchGet {
            part,
            sketch,
            quality,
        } => {
            let quality = match quality {
                Some(q) => mesh::Quality::parse(&q)?,
                None => mesh::Quality::preferred(),
            };
            crate::sketch::get(session, &part, &sketch, quality)
        }
        Request::AssemblyMove {
            part,
            instance,
            placement,
            translate,
            axis,
            angle,
            expected_revision,
        } => crate::assembly::moved(
            session,
            &part,
            &instance,
            crate::assembly::Move {
                placement,
                translate,
                axis,
                angle,
            },
            expected_revision.as_deref(),
        ),
        Request::AssemblyTree { part, path, depth } => {
            crate::assembly::tree(session, &part, path.as_deref(), depth)
        }
        Request::Interference { part, instances } => {
            crate::interference::run(session, &part, &instances)
        }
        Request::Measure { entities, rev } => {
            crate::measure::run(session, &entities, rev.as_deref())
        }
        Request::Clearance {
            part,
            instances,
            below,
        } => crate::clearance::run(session, &part, &instances, below.as_deref()),
        Request::Mass { of, tolerance, rev } => {
            crate::mass::run(session, &of, tolerance, rev.as_deref())
        }
        Request::MaterialsList {} => crate::materials::list(session),
        Request::MaterialsSet {
            name,
            density,
            colour,
            default,
            expected_revision,
        } => crate::materials::set(
            session,
            &name,
            &density,
            colour.as_deref(),
            default,
            expected_revision.as_deref(),
        ),
        Request::MaterialsDelete {
            name,
            expected_revision,
        } => crate::materials::delete(session, &name, expected_revision.as_deref()),
        Request::AssemblySolve {
            part,
            moves,
            component,
        } => crate::mate::solve_request(
            session,
            &part,
            moves
                .into_iter()
                .map(|m| {
                    (
                        m.instance,
                        crate::assembly::Move {
                            placement: m.placement,
                            translate: m.translate,
                            axis: m.axis,
                            angle: m.angle,
                        },
                    )
                })
                .collect(),
            component,
        ),
        Request::FeatureShow { part, id } => {
            let (part, fid) = (part_id(&part)?, feature_id(&id)?);
            let p = session.project().part(&part)?;
            let mut v = feature_report(&part, &fid, feature(p, &fid)?);
            // The revision it was read at: an edit made from it says so.
            v["revision"] = json!(p.revision);
            Ok(v)
        }
        Request::MergeStatus {} => crate::validate::status(session),
        Request::MergeResolve {
            resolutions,
            expected_revisions,
        } => crate::validate::resolve(session, &resolutions, &expected_revisions),
        Request::DaemonStatus {} | Request::DaemonShutdown {} => Err(Error::new(
            Code::Usage,
            "only a running daemon answers daemon requests",
        )),
        Request::Import {
            path,
            part,
            into,
            product,
            dry_run,
            expected_revision,
            update,
        } => crate::import::import(
            session,
            crate::import::Ask {
                path,
                part,
                into,
                product,
                dry_run,
                expected: expected_revision,
                update,
            },
        ),
        Request::Rebuild { part, rev } => {
            let (commit, s) = session.at(rev.as_deref())?;
            noted(rebuild(s, part.as_deref()), commit)
        }
        Request::Inspect {
            part,
            feature,
            entity,
            rev,
        } => {
            let (commit, s) = session.at(rev.as_deref())?;
            versioned(noted(
                inspect(s, &part, feature.as_deref(), entity.as_deref()),
                commit,
            ))
        }
        Request::CheckFormat {} => check_format(session),
        Request::Check {
            part,
            against,
            results,
        } => crate::check::run(session, part.as_deref(), against.as_deref(), results),
        Request::Diff {
            from,
            to,
            part,
            model,
            positions,
            all,
        } => {
            let opts = crate::diff::Options {
                part,
                positions,
                all,
            };
            versioned(diff(
                session,
                from.as_deref().unwrap_or("HEAD"),
                to.as_deref().unwrap_or("working"),
                model,
                &opts,
            ))
        }
        Request::SessionHello {
            kind,
            name,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("session.hello"))?;
            let id = live.hello(kind, name.as_deref(), participant.as_deref())?;
            Ok(json!({"participant": id, "session": live.list()}))
        }
        // With no daemon, nobody is attached.
        Request::ParticipantList {} => {
            Ok(live.map_or_else(|| json!({"participants": []}), |l| l.list()))
        }
        // A daemon's connection answers this itself: it needs the socket.
        Request::Subscribe { .. } => match live {
            None => Err(live::no_live_session("subscribe")),
            Some(_) => Err(Error::new(Code::Usage, "subscribe needs a connection")),
        },
        Request::SelectionGet { participant } => {
            let target = match &live {
                Some(l) => l.target(participant.as_deref())?,
                None => None,
            };
            Ok(match target {
                Some(p) => json!({"participant": p.id, "selection": p.selection}),
                None => json!({"participant": null, "selection": []}),
            })
        }
        Request::SelectionSet { entities, mode } => {
            let live = live.ok_or_else(|| live::no_live_session("selection.set"))?;
            // What is being removed may no longer exist.
            let refs = match mode {
                SelectMode::Remove => entities
                    .iter()
                    .map(|s| EntityRef::parse(s))
                    .collect::<Result<Vec<_>>>()?,
                _ => entity::parse_all(session, &entities)?,
            };
            let id = live.caller(me);
            let selection = live.select(&id, refs, mode)?;
            Ok(json!({"participant": id, "selection": selection}))
        }
        Request::CameraGet { participant } => {
            let target = match &live {
                Some(l) => l.target(participant.as_deref())?,
                None => None,
            };
            Ok(match target {
                Some(p) => json!({"participant": p.id, "camera": p.view.camera}),
                None => json!({"participant": null, "camera": null}),
            })
        }
        Request::CameraSet {
            camera,
            view,
            fit,
            normal_to,
            part,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("camera.set"))?;
            let id = live.caller(me);
            let camera: Camera = match camera {
                Some(_) if view.is_some() || !fit.is_empty() || normal_to.is_some() => {
                    return Err(Error::new(
                        Code::Usage,
                        "give a camera, or a view and/or entities to fit, not both",
                    ));
                }
                Some(c) => c.resolve(session.project().manifest.default_unit)?,
                None if view.is_some() && normal_to.is_some() => {
                    return Err(Error::new(
                        Code::Usage,
                        "give a view or a face to look normal to, not both",
                    ));
                }
                None => {
                    let me = live.get(&id)?;
                    let (current, focus) = (me.view.camera.clone(), me.view.focus.clone());
                    framed(
                        session,
                        current.as_ref(),
                        Look {
                            view: view.as_deref(),
                            normal_to: normal_to.as_deref(),
                            part: part.as_deref(),
                            focus: focus.as_ref(),
                        },
                        &fit,
                    )?
                }
            };
            live.get_mut(&id)?.view.camera = Some(camera.clone());
            Ok(json!({"participant": id, "camera": camera}))
        }
        Request::ViewList { part } => crate::views::list(session, part.as_deref()),
        Request::ViewSave {
            part,
            view,
            camera,
            participant,
            expected_revision,
        } => {
            let camera = match camera {
                Some(c) => c.resolve(session.project().manifest.default_unit)?,
                None => {
                    let live = live.ok_or_else(|| live::no_live_session("view.save"))?;
                    let who = live.compare_target(participant.as_deref(), me)?;
                    live.get(&who)?.view.camera.clone().ok_or_else(|| {
                        Error::new(
                            Code::InvalidParam,
                            format!("participant {who} has no camera to save"),
                        )
                        .with("participant", &who)
                    })?
                }
            };
            crate::views::save(session, &part, &view, &camera, expected_revision.as_deref())
        }
        Request::ViewDelete {
            part,
            view,
            expected_revision,
        } => crate::views::delete(session, &part, &view, expected_revision.as_deref()),
        Request::ViewRename {
            part,
            from,
            to,
            expected_revision,
        } => crate::views::rename(session, &part, &from, &to, expected_revision.as_deref()),
        Request::CompareGet { participant } => {
            let target = match &live {
                Some(l) => l.target(participant.as_deref())?,
                None => None,
            };
            let Some(p) = target else {
                return Ok(json!({"participant": null, "compare": null}));
            };
            let (id, rev) = (p.id.clone(), p.view.compare.clone());
            // The commit the revision names now; none if it no longer
            // names one.
            let compare = rev.map(|rev| {
                let commit = crate::git::repo(session.root())
                    .and_then(|r| r.locate(&rev))
                    .ok()
                    .map(|(c, _)| c);
                json!({"rev": rev, "commit": commit})
            });
            Ok(json!({"participant": id, "compare": compare}))
        }
        Request::CompareSet {
            rev,
            off,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("compare.set"))?;
            let rev = match (rev, off) {
                (Some(r), false) => Some(r),
                (None, true) => None,
                _ => {
                    return Err(Error::new(
                        Code::Usage,
                        "give a revision to compare with, or off, not both",
                    ));
                }
            };
            if rev.as_deref() == Some("working") {
                return Err(Error::new(
                    Code::InvalidParam,
                    "a compare is against a Git revision: the working files are what it compares",
                )
                .with("rev", "working"));
            }
            let id = live.compare_target(participant.as_deref(), me)?;
            // Refused unless the revision has the project; it is then
            // ready for the diff and meshes the window asks for next.
            let commit = match &rev {
                Some(r) => session.at(Some(r))?.0,
                None => None,
            };
            live.set_compare(&id, rev.clone())?;
            let compare = rev.map(|rev| json!({"rev": rev, "commit": commit}));
            Ok(json!({"participant": id, "compare": compare}))
        }
        Request::ViewFocus {
            part,
            project,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("view.focus"))?;
            let part = match (part, project) {
                (Some(p), false) => {
                    let id = part_id(&p)?;
                    session.project().part(&id)?;
                    Some(Some(id))
                }
                (None, true) => Some(None),
                (None, false) => None,
                (Some(_), true) => {
                    return Err(Error::new(
                        Code::Usage,
                        "give a part to show, or project, not both",
                    ));
                }
            };
            let id = live.compare_target(participant.as_deref(), me)?;
            if let Some(focus) = part {
                live.set_focus(&id, focus)?;
            }
            let focus = live.get(&id)?.view.focus.clone();
            let shows = match &focus {
                Some(p) => vec![p.clone()],
                None => crate::assembly::shown_parts(session.project()),
            };
            Ok(json!({"participant": id, "focus": focus, "shows": shows}))
        }
        Request::VisibilitySet {
            hide,
            show,
            isolate,
            all,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("visibility.set"))?;
            let id = live.compare_target(participant.as_deref(), me)?;
            let change = crate::visibility::Change {
                all,
                isolate,
                show,
                hide,
            };
            let hidden = crate::visibility::apply(session, &live.get(&id)?.view.hidden, &change)?;
            live.set_hidden(&id, hidden)?;
            Ok(json!({"participant": id, "hidden": live.get(&id)?.view.hidden}))
        }
        Request::HighlightSet {
            entities,
            style,
            label,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("highlight.set"))?;
            let refs = entity::parse_all(session, &entities)?;
            let id = live.caller(me);
            let highlights = live.highlight(&id, style, label, refs)?;
            Ok(json!({"participant": id, "highlights": highlights}))
        }
        Request::HighlightClear { style } => {
            let live = live.ok_or_else(|| live::no_live_session("highlight.clear"))?;
            let id = live.caller(me);
            let highlights = live.clear_highlights(&id, style)?;
            Ok(json!({"participant": id, "highlights": highlights}))
        }
        Request::ActivityLog { limit, before } => {
            let entries = live.map_or_else(Vec::new, |l| l.log.page(limit.unwrap_or(50), before));
            Ok(json!({ "entries": entries }))
        }
        Request::Undo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("undo"))?;
            let who = live.caller(me);
            activity::undo(live, session, &who, entry, false)
        }
        Request::Redo { entry } => {
            let live = live.ok_or_else(|| live::no_live_session("redo"))?;
            let who = live.caller(me);
            activity::undo(live, session, &who, entry, true)
        }
        Request::MeshGet {
            part,
            feature,
            features,
            quality,
            transfer: _,
            rev,
        } => {
            let (commit, s) = session.at(rev.as_deref())?;
            let only = wanted(feature.as_deref(), &features);
            versioned(noted(
                mesh_get(s, &part, only.as_ref(), quality.as_deref()),
                commit,
            ))
        }
        // A render's answer is bytes: `answer_live` handles it.
        Request::Render { .. } => Err(Error::new(Code::Internal, "render answers with bytes")),
        Request::Export { .. } => Err(Error::new(Code::Internal, "export answers with bytes")),
        // The session is current: doctor reuses its rebuilds (#125).
        Request::Doctor {} => {
            let root = session.root().to_owned();
            crate::doctor::report_in(&mut crate::doctor::Builds::live(&root, session))
        }
        Request::DoctorRepair { repairs, params } => {
            let params = crate::doctor::Params::from_json(&params)?;
            let r = crate::doctor::repair(session.root(), &repairs, &params)?;
            session.reload()?;
            Ok(r)
        }
        Request::HistoryLog { part, limit } => {
            let part = part.as_deref().map(part_id).transpose()?;
            if let Some(p) = &part {
                session.project().part(p)?;
            }
            let files = session.snapshot();
            crate::git::history(
                session.root(),
                &files,
                part.as_ref().map(Id::as_str),
                limit.unwrap_or(50),
            )
        }
        Request::CacheStats {} => Ok(cache_stats(session.disk().map(|d| d.as_ref()), None)),
        Request::CacheClear {} => {
            let disk = session.disk().map(|d| d.as_ref());
            let removed = disk.map(crate::cache::Disk::clear).unwrap_or_default();
            Ok(cache_stats(disk, Some(removed)))
        }
        Request::CacheLogin { token } => {
            crate::cache::token::store(Some(token.trim()))?;
            cache_login(session.disk().map(|d| d.as_ref()))
        }
        Request::CacheLogout {} => {
            crate::cache::token::store(None)?;
            let mut v = cache_login(session.disk().map(|d| d.as_ref()))?;
            // So a token the variable still sends isn't read as one saved.
            v["removed"] = json!(true);
            Ok(v)
        }
        Request::CacheWarm { part } => cache_warm(session, part.as_deref()),
        Request::ConfigGet { key, scope } => {
            let scope = scope.as_deref().map(config::Scope::parse).transpose()?;
            let mut v = config::get(
                session.project().manifest.default_unit,
                key.as_deref(),
                scope,
            )?;
            v["project_revision"] = json!(config::project_revision(session.root())?);
            Ok(v)
        }
        Request::ConfigSet {
            key,
            value,
            scope,
            expected_revision,
        } => {
            let scope = scope.as_deref().map(config::Scope::parse).transpose()?;
            let v = match config::validate(&key, &value, scope)? {
                config::Scope::Project => {
                    config::check_project_revision(session.root(), expected_revision.as_deref())?;
                    let unit = Unit::parse(value.as_str().expect("validated")).expect("validated");
                    // The rest of the manifest (the project's ID) stays.
                    let m = Manifest {
                        default_unit: unit,
                        ..session.project().manifest.clone()
                    };
                    write_json(&session.root().join(MANIFEST), &m.to_json())?;
                    session.reload()?;
                    let mut v = config::get(unit, Some(&key), None)?;
                    v["project_revision"] = json!(config::project_revision(session.root())?);
                    v
                }
                config::Scope::User => config::set_user(&key, value)?,
            };
            if key == "cache.max_bytes"
                && let Some(d) = session.disk()
            {
                d.set_max_bytes(
                    v["value"]
                        .as_u64()
                        .unwrap_or(crate::cache::DEFAULT_MAX_BYTES),
                );
                d.trim_in_background();
            }
            // A daemon's cache takes the shared cache as it is now set.
            if key.starts_with("cache.remote")
                && let Some(d) = session.disk()
            {
                d.set_remote(crate::cache::remote_preference(&d.identity()));
            }
            if let Some(live) = live {
                if key == "daemon.poll_ms" {
                    live.poll_ms = v["value"].as_u64().unwrap_or(250);
                }
                let note = json!({"key": key, "scope": v["scope"], "value": v["value"],
                                  "default": v["default"]});
                live.hub.send(Topic::Project, "config.changed", &note);
            }
            Ok(v)
        }
        Request::AppOpen { participant } => {
            let live = live.ok_or_else(|| live::no_live_session("app.open"))?;
            let asked: Vec<String> = match participant {
                Some(id) => {
                    let p = live.get(&id)?;
                    if p.kind != live::Kind::Human {
                        return Err(Error::new(
                            Code::InvalidParam,
                            format!("participant {id} is not an app window"),
                        )
                        .with("participant", id));
                    }
                    vec![id]
                }
                None => live.humans().map(|p| p.id.clone()).collect(),
            };
            for id in &asked {
                live.hub.send_to(id, "app.open", &json!({}));
            }
            Ok(json!({ "asked": asked }))
        }
        Request::ActivitySet { status, entities } => {
            let live = live.ok_or_else(|| live::no_live_session("activity.set"))?;
            let refs = entity::parse_all(session, &entities)?;
            let id = live.caller(me);
            live.set_status(&id, &status, refs)?;
            Ok(json!({"participant": id, "status": live.get(&id)?.status}))
        }
    }
}

/// [`handle_live`] behind a panic boundary. `slot` holds the session
/// between requests; it is opened on first use, and dropped after a panic
/// so the next request starts again from the files. The live session is
/// kept: who is attached doesn't depend on the files.
pub fn handle_safely(
    slot: &mut Option<Session>,
    live: Option<&mut Live>,
    me: Option<&str>,
    root: &Path,
    req: Request,
) -> Result<Answer> {
    let watch = live.is_some();
    guarded(slot, root, watch, |s| answer_live(s, live, me, req))
}

/// The display meshes a `mesh.get` of the working files needs that the
/// cache doesn't hold yet, which the daemon makes without its lock, so a
/// window fetching a fine mesh of a large part doesn't hold up every other
/// request (#91). `handle` then finds them cached.
pub fn prepare(
    slot: &mut Option<Session>,
    req: &Request,
) -> Option<ryu_session::Prepared<Session>> {
    let Request::MeshGet {
        part,
        feature,
        features,
        quality,
        rev,
        ..
    } = req
    else {
        return None;
    };
    let only = wanted(feature.as_deref(), features);
    if rev.as_deref().is_some_and(|r| r != "working") {
        return None;
    }
    let session = slot.as_mut()?;
    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mesh_pending(session, part, only.as_ref(), quality.as_deref())
    }));
    let pending = match found {
        Ok(p) => p?,
        // As `guarded`: the request opens a new session and says why.
        Err(_) => {
            *slot = None;
            return None;
        }
    };
    let budget = mesh_budget();
    Some(Box::new(move || {
        let made = pending.make();
        Box::new(move |s: &mut Session| s.meshes.put(made, budget))
    }))
}

/// The meshes [`mesh_binary`] and [`mesh_get`] would make; `None` when the
/// cache has them all, or the request fails (`handle` says how).
fn mesh_pending(
    session: &mut Session,
    part: &str,
    only: Option<&BTreeSet<String>>,
    quality: Option<&str>,
) -> Option<mesh::Pending> {
    let (quality, items): (_, Vec<(Feature, crate::rebuild::Built)>) = if is_assembly(session, part)
    {
        let (pid, _, quality) = mesh_args(part, None, quality).ok()?;
        let model = session
            .model(&pid, &crate::doc::assembly::Rev::Working)
            .ok()?;
        // Each body once, before anything is copied: a root places
        // 100,000 occurrences of 10,000 bodies.
        let mut seen = BTreeSet::new();
        let items = model
            .occurrences
            .iter()
            .filter(|o| only.is_none_or(|w| w.contains(&crate::assembly::path_text(&o.path))))
            .flat_map(|o| &o.solids)
            .filter(|s| seen.insert(s.built.hash.clone()))
            .filter(|s| !session.meshes.has(&s.built.hash, quality))
            .map(|s| (s.feature.as_ref().clone(), s.built.as_ref().clone()))
            .collect();
        (quality, items)
    } else {
        let one = one_of(only).ok()?;
        let (pid, only, quality) = mesh_args(part, one, quality).ok()?;
        let p = session.project().part(&pid).ok()?;
        let solids = Solids::of(p);
        let features: BTreeMap<Id, Feature> = p
            .features
            .iter()
            .filter_map(|(id, f)| Some((id.clone(), f.as_ref().ok()?.clone())))
            .collect();
        let result = session.rebuilt(&pid).ok()?;
        let items = result
            .shown(&solids)
            .into_iter()
            .filter(|s| only.as_ref().is_none_or(|o| o == s.solid))
            .filter_map(|s| {
                let (f, b) = s.state?;
                Some((features.get(f)?.clone(), b.as_ref().clone()))
            })
            .collect();
        (quality, items)
    };
    let refs: Vec<(&Feature, &crate::rebuild::Built)> = items.iter().map(|(f, b)| (f, b)).collect();
    session.meshes.pending(&refs, quality)
}

/// The display mesh cache's budget in bytes.
pub(crate) fn mesh_budget() -> usize {
    config::user("display.mesh_cache_mib")
        .as_u64()
        .unwrap_or(256) as usize
        * 1024
        * 1024
}

fn guarded<T>(
    slot: &mut Option<Session>,
    root: &Path,
    watch: bool,
    f: impl FnOnce(&mut Session) -> Result<T>,
) -> Result<T> {
    let session = match slot {
        Some(s) => s,
        None => {
            // A daemon's first request trims what earlier ones left.
            let disk = crate::cache::Disk::for_user().map(std::sync::Arc::new);
            if let Some(d) = &disk {
                d.trim_in_background();
            }
            let mut s = Session::open(root)?.with_disk(disk);
            // A daemon's session lives on, within its memory budget.
            if watch {
                let max = config::user("session.max_memory")
                    .as_u64()
                    .unwrap_or_else(crate::budget::default_max);
                s = s.with_budget(max);
            }
            // A daemon's session lives on: file events narrow its checks,
            // its markers beside its log.
            if watch {
                let markers = crate::endpoint::Endpoint::for_project(root)?
                    .log
                    .with_extension("watch");
                s = s.with_watch(&markers);
            }
            slot.insert(s)
        }
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(session))) {
        Ok(r) => r,
        Err(panic) => {
            *slot = None;
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            Err(Error::new(Code::Internal, format!("internal error: {msg}")).with("panic", msg))
        }
    }
}

/// `cache.stats`' answer, and `cache.clear`'s with what it `removed`:
/// whether there is a disk cache, its folder and bound, what it holds,
/// and its shared cache server's state.
fn cache_stats(disk: Option<&crate::cache::Disk>, removed: Option<crate::cache::Stats>) -> Value {
    let mut v = local_stats(disk, removed);
    v["remote"] = remote_stats(disk);
    v
}

/// `cache.login`'s and `cache.logout`'s answer, the token as it now is
/// put behind the daemon's cache: where the token is read from, what the
/// server says it allows (or why it couldn't say), and the remote's state.
/// `cache.logout` adds `removed: true`.
fn cache_login(disk: Option<&crate::cache::Disk>) -> Result<Value> {
    use crate::cache::token;
    if let Some(d) = disk {
        d.set_remote(crate::cache::remote_preference(&d.identity()));
    }
    let from = token::token().map(|(_, o)| o.as_str());
    let mut v = json!({
        "file": token::path()?,
        "token": from,
        "remote": remote_stats(disk),
    });
    // The variable wins over the file.
    if from == Some("variable") {
        v["overridden_by"] = json!(token::TOKEN_VAR);
    }
    if let Some(r) = disk.and_then(crate::cache::Disk::remote) {
        v["server"] = match r.info() {
            Ok(info) => json!({"write": info["write"], "entries": info["entries"]}),
            Err(e) => json!({"error": e}),
        };
    }
    Ok(v)
}

/// `cache.warm`: `part` (or every part) rebuilt, the parts that aren't
/// assemblies side by side and then each assembly's model, which places
/// what it holds. Its remote reads come a depth at a time; what the remote
/// lacks is built and sent behind, as any rebuild's is. A failing part is
/// left to `rebuild` to report.
fn cache_warm(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let parts: Vec<Id> = match part {
        Some(p) => {
            let id = part_id(p)?;
            session.project().part(&id)?;
            vec![id]
        }
        None => session.project().parts.keys().cloned().collect(),
    };
    let disk = session.disk().cloned();
    let counts = |d: &Option<std::sync::Arc<crate::cache::Disk>>| {
        let d = d.as_deref();
        let remote = d.and_then(crate::cache::Disk::remote).map(|r| r.stats());
        let n = |k: &str| remote.as_ref().and_then(|r| r[k].as_u64()).unwrap_or(0);
        (
            d.map_or(0, crate::cache::Disk::stored),
            n("hits"),
            n("bytes_in"),
        )
    };
    let before = counts(&disk);
    session.rebuild_parts(&parts)?;
    for id in &parts {
        let assembly = session
            .project()
            .part(id)?
            .file
            .as_ref()
            .is_ok_and(|f| f.kind == PartKind::Assembly);
        if assembly {
            let _ = session.model(id, &Rev::Working);
        }
    }
    let after = counts(&disk);
    Ok(json!({
        "parts": parts.len(),
        "built": after.0 - before.0,
        "fetched": after.1 - before.1,
        "fetched_bytes": after.2 - before.2,
        "remote": remote_stats(disk.as_deref()),
    }))
}

/// What `cache.stats` says of the remote: what it has done in this
/// process, else why there is none.
fn remote_stats(disk: Option<&crate::cache::Disk>) -> Value {
    if let Some(r) = disk.and_then(crate::cache::Disk::remote) {
        let mut v = r.stats();
        v["enabled"] = json!(true);
        return v;
    }
    let url = config::user("cache.remote");
    let url = url.as_str().unwrap_or_default();
    let note = match disk {
        _ if url.is_empty() => return json!({"enabled": false}),
        None => "there is no disk cache on this machine to put it behind",
        Some(d) if !d.identity().shared => {
            "a development build never uses a shared cache: only released builds share entries"
        }
        Some(_) => "it isn't in use",
    };
    json!({"enabled": false, "url": url, "note": note})
}

fn local_stats(disk: Option<&crate::cache::Disk>, removed: Option<crate::cache::Stats>) -> Value {
    let mut v = match disk {
        Some(d) => {
            let mut v = d.stats().to_json();
            v["enabled"] = json!(true);
            v["folder"] = json!(d.root());
            v["max_bytes"] = json!(d.max_bytes());
            // A release's entries are shared with its builds everywhere.
            v["shared"] = json!(d.identity().shared);
            v
        }
        None => {
            let mut v = crate::cache::Stats::default().to_json();
            v["enabled"] = json!(false);
            v
        }
    };
    if let Some(r) = removed {
        v["removed"] = r.to_json();
    }
    v
}

fn project_info(session: &Session) -> Value {
    let p = session.project();
    json!({
        "root": p.root,
        "default_unit": p.manifest.default_unit,
        "parts": p.parts.keys().collect::<Vec<_>>(),
        "version": VersionInfo::current(),
    })
}

/// The name of the participant asking: the live session's, else the one
/// the caller gave (the CLI's `--as`), else the default agent's.
fn participant_name(live: Option<&mut Live>, me: Option<&str>) -> String {
    match live {
        Some(l) => {
            let id = l.caller(me);
            l.get(&id)
                .map_or_else(|_| live::DEFAULT_AGENT.to_owned(), |p| p.name.clone())
        }
        None => me.unwrap_or(live::DEFAULT_AGENT).to_owned(),
    }
}

pub(crate) fn part_id(s: &str) -> Result<Id> {
    Id::parse(s, "part")
}

/// Refuses to write when the agent's view of the part is out of date.
pub(crate) fn check_revision(part: &Part, expected: Option<&str>) -> Result<()> {
    match expected {
        Some(e) if e != part.revision => Err(Error::new(
            Code::Conflict,
            format!(
                "part {} changed since revision {e}; read it again and retry",
                part.id
            ),
        )
        .with("part", &part.id)
        .with("expected", e)
        .with("actual", &part.revision)),
        _ => Ok(()),
    }
}

fn part_create(session: &mut Session, part: &str, kind: Option<&str>) -> Result<Value> {
    let id = part_id(part)?;
    let kind = kind
        .map(PartKind::parse)
        .transpose()?
        .unwrap_or(PartKind::Part);
    if session.project().parts.contains_key(&id) {
        return Err(
            Error::new(Code::DuplicateId, format!("part {id} already exists")).with("part", &id),
        );
    }
    crate::pdm::check_not_retired(session.root(), &id)?;
    write_json(
        &part_path(session.root(), &id),
        &PartFile {
            kind,
            ..PartFile::default()
        }
        .to_json(),
    )?;
    session.reload()?;
    let revision = &session.project().part(&id)?.revision;
    Ok(json!({"part": id, "kind": kind.as_str(), "revision": revision}))
}

fn part_delete(session: &mut Session, part: &str, expected: Option<&str>) -> Result<Value> {
    let id = part_id(part)?;
    check_revision(session.project().part(&id)?, expected)?;
    // An assembly placing the part as it is would have nothing to place;
    // a release it places is read from history.
    let placed = &id;
    let users: Vec<String> = session
        .project()
        .parts
        .iter()
        .flat_map(|(a, p)| {
            p.order()
                .iter()
                .filter_map(move |fid| match p.features.get(fid) {
                    Some(Ok(Feature::Instance(i)))
                        if i.part == *placed && i.rev == Rev::Working =>
                    {
                        Some(format!("{a}/{fid}"))
                    }
                    _ => None,
                })
        })
        .collect();
    if !users.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!("assemblies place {id}: {}", users.join(", ")),
        )
        .with("part", &id)
        .with("instances", users));
    }
    let dir = part_dir(session.root(), &id);
    fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, &e))?;
    session.reload()?;
    Ok(json!({"part": id, "deleted": true}))
}

fn part_list(session: &Session) -> Value {
    let parts: Vec<Value> = session
        .project()
        .parts
        .values()
        .map(|p| {
            let kind = part_kind(p);
            let mut v = json!({"part": p.id, "kind": kind.as_str(),
                               "features": p.order().len(), "revision": p.revision});
            if let Err(e) = &p.file {
                v["error"] = json!(e);
            }
            // What its rebuild was, where this session built it at its
            // revision: a window lists 10,000 parts without asking each.
            if let Some((hash, failed)) = session.summary(&p.id) {
                v["hash"] = json!(hash);
                v["failed"] = json!(failed);
            }
            v
        })
        .collect();
    // What a window shows when it isn't told: the project's one root
    // assembly, else every part that isn't an assembly.
    let model = crate::assembly::shown_parts(session.project());
    json!({ "parts": parts, "model": model })
}

pub(crate) fn feature_id(s: &str) -> Result<Id> {
    Id::parse(s, "feature")
}

/// The part's feature list, or why it can't be used.
pub(crate) fn order(part: &Part) -> Result<Vec<Id>> {
    part.file
        .as_ref()
        .map(|f| f.features.clone())
        .map_err(Clone::clone)
}

/// A feature of `part`, or the error that keeps it from loading.
pub(crate) fn feature<'a>(part: &'a Part, id: &Id) -> Result<&'a Feature> {
    match part.features.get(id) {
        Some(Ok(f)) => Ok(f),
        Some(Err(e)) => Err(e.clone()),
        None => Err(no_feature(part, id)),
    }
}

fn no_feature(part: &Part, id: &Id) -> Error {
    Error::new(
        Code::NotFound,
        format!("no feature {id} in part {}", part.id),
    )
    .with("part", &part.id)
    .with("feature", id)
}

/// How a feature appears in output: every quantity as written and in
/// meters, every reference as `<part>/<feature>/<name>`.
pub fn feature_report(part: &Id, id: &Id, f: &Feature) -> Value {
    let mut v = match f {
        Feature::Primitive(p) => primitive_report(p),
        Feature::Sketch(s) => sketch_report(part, s),
        Feature::Extrude(e) => {
            let mut v = e.fields();
            match &e.extent {
                Extent::Blind(q) | Extent::Symmetric(q) => v["extent"]["length"] = q.report(),
                Extent::ThroughAll { .. } => {}
                Extent::UpTo { target, offset } => {
                    v["extent"]["target"] = json!(target.text(part));
                    if let Some(q) = offset {
                        v["extent"]["offset"] = q.report();
                    }
                }
            }
            v
        }
        Feature::Boolean(b) => b.fields(),
        Feature::Instance(i) => i.fields(),
        Feature::Import(i) => i.fields(),
        Feature::Mate(m) => {
            let mut v = m.fields();
            for (side, c) in [("a", &m.a), ("b", &m.b)] {
                v[side]["text"] = json!(connector_text(part, c));
            }
            if let Some(q) = &m.angle {
                v["angle"] = q.report();
            }
            if let Some(q) = &m.distance {
                v["distance"] = q.report();
            }
            v
        }
        Feature::InstancePattern(x) => {
            let mut v = x.fields();
            for (role, c) in x.connectors() {
                v[role]["text"] = json!(connector_text(part, c));
            }
            v
        }
        Feature::Fillet(_) | Feature::Chamfer(_) => blend_report(part, f),
        Feature::Hole(h) => hole_report(part, h),
        Feature::Pattern(x) => pattern_report(part, x),
        Feature::Revolve(r) => {
            let mut v = r.fields();
            if let Angle::Of(q) = &r.angle {
                v["angle"] = q.report();
            }
            if let RevolveAxis::Edge(e) = &r.axis {
                v["axis"] = json!(e.text(part));
            }
            v
        }
    };
    v["id"] = json!(id);
    v["type"] = json!(f.type_name());
    v
}

/// A fillet or chamfer as output reports it: its file's fields, every
/// size as written and in meters (an angle in radians), every edge and
/// face as `<part>/<feature>/<name>`.
fn blend_report(part: &Id, f: &Feature) -> Value {
    use crate::doc::blend::ChamferForm;
    let mut v = match f {
        Feature::Fillet(x) => x.fields(),
        Feature::Chamfer(x) => x.fields(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    let sizes: Vec<Vec<(&str, &Quantity)>> = match f {
        Feature::Fillet(x) => x
            .groups
            .iter()
            .map(|g| vec![("radius", &g.radius)])
            .collect(),
        Feature::Chamfer(x) => x
            .groups
            .iter()
            .map(|g| match &g.form {
                ChamferForm::Equal { d } => vec![("d", d)],
                ChamferForm::Distances { d1, d2, .. } => vec![("d1", d1), ("d2", d2)],
                ChamferForm::DistanceAngle { d, angle, .. } => vec![("d", d), ("angle", angle)],
            })
            .collect(),
        _ => unreachable!("a fillet or a chamfer"),
    };
    for (i, group) in sizes.into_iter().enumerate() {
        for (k, q) in group {
            v["groups"][i][k] = q.report();
        }
    }
    for (role, r) in f.references() {
        let at = format!("/{}", role.replace('.', "/"));
        *v.pointer_mut(&at).expect("its own reference") = json!(r.text(part));
    }
    v
}

/// A hole as output reports it: its file's fields, every size as written
/// and in meters (an angle in radians), an up-to target as
/// `<part>/<feature>/<name>`.
fn hole_report(part: &Id, h: &crate::doc::hole::Hole) -> Value {
    use crate::doc::hole::{Bottom, Depth, HoleForm};
    let mut v = h.fields();
    v["diameter"] = h.diameter.report();
    match &h.form {
        HoleForm::Simple => {}
        HoleForm::Counterbore { diameter, depth } => {
            v["counterbore"] = json!({"diameter": diameter.report(), "depth": depth.report()});
        }
        HoleForm::Countersink { diameter, angle } => {
            v["countersink"] = json!({"diameter": diameter.report(), "angle": angle.report()});
        }
    }
    match &h.depth {
        Depth::Blind(q) => v["depth"] = json!({"blind": q.report()}),
        Depth::ThroughAll => {}
        Depth::UpTo { target, offset } => {
            v["depth"]["up_to"]["target"] = json!(target.text(part));
            if let Some(q) = offset {
                v["depth"]["up_to"]["offset"] = q.report();
            }
        }
    }
    if let Bottom::Drill(q) = &h.bottom {
        v["bottom"] = json!({"drill": q.report()});
    }
    v
}

/// A pattern as output reports it: its file's fields, every spacing,
/// count and angle as written and as a value, every reference as
/// `<part>/<feature>/<name>`.
fn pattern_report(part: &Id, x: &crate::doc::pattern::Pattern) -> Value {
    use crate::doc::pattern::Kind;
    let mut v = x.fields();
    match &x.kind {
        Kind::Linear { first, second } => {
            v["spacing"] = first.spacing.report();
            v["count"] = first.count.report();
            if let Some(r) = second {
                v["spacing2"] = r.spacing.report();
                v["count2"] = r.count.report();
            }
        }
        Kind::Circular { count, angle, .. } => {
            v["count"] = count.report();
            v["angle"] = angle.report();
        }
        Kind::Mirror { .. } => {}
    }
    for (role, r) in x.references() {
        v[role] = json!(r.text(part));
    }
    v
}

/// A placement as output reports it: quantities as written and in meters.
fn placement_report(p: &Placement) -> Value {
    json!({
        "origin": p.origin.iter().map(Quantity::report).collect::<Vec<_>>(),
        "z": p.z.map(number),
        "x": p.x.map(number),
    })
}

/// A sketch as output reports it: its file's fields, with every dimension
/// as written and evaluated.
fn sketch_report(part: &Id, s: &Sketch) -> Value {
    let mut v = s.fields();
    if let Plane::Placement(p) = &s.plane {
        v["placement"] = placement_report(p);
    }
    reference_texts(part, s, &mut v);
    for (id, c) in &s.constraints {
        if let Some(q) = &c.value {
            v["constraints"][id.as_str()]["value"] = q.report();
        }
    }
    v
}

/// Rewrites a sketch's references in `v`, its fields as a file has them,
/// as `<part>/<feature>/<name>`.
pub(crate) fn reference_texts(part: &Id, s: &Sketch, v: &mut Value) {
    if let Plane::Face(on) = &s.plane {
        v["face"] = json!(on.face.text(part));
        if let Some(r) = &on.origin {
            v["origin"] = json!(r.text(part));
        }
        if let Some(r) = &on.x {
            v["x"] = json!(r.text(part));
        }
    }
    for (id, e) in &s.entities {
        if let Some(r) = &e.projected {
            v["entities"][id.as_str()]["projected"] = json!(r.text(part));
        }
    }
}

fn primitive_report(f: &Primitive) -> Value {
    let params: Map<String, Value> = f
        .params
        .iter()
        .map(|(k, q)| (k.clone(), q.report()))
        .collect();
    let mut v = json!({"params": params, "placement": placement_report(&f.placement)});
    f.combine.write(&mut v);
    v
}

/// Applies the parts of `arg` that are given to `p`, its expressions
/// reading `env`.
fn apply_placement(p: &mut Placement, arg: PlacementArg, unit: Unit, env: &Env) -> Result<()> {
    if let Some(origin) = arg.origin {
        for (i, v) in origin.iter().enumerate() {
            p.origin[i] = Quantity::from_json_in(v, unit, &|n| env.get(n))
                .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
        }
    }
    if let Some(z) = arg.z {
        p.z = z;
    }
    if let Some(x) = arg.x {
        p.x = x;
    }
    // Refuse a placement that could never build, before it is written.
    rebuild::placement(p).map(|_| ())
}

/// Where `position` puts a feature in `order` (which doesn't hold it).
fn insert_at(order: &[Id], position: &Position) -> Result<usize> {
    match (&position.after, position.first) {
        (Some(_), true) => Err(Error::new(Code::Usage, "give after or first, not both")),
        (None, true) => Ok(0),
        (None, false) => Ok(order.len()),
        (Some(after), false) => {
            let after = feature_id(after)?;
            order
                .iter()
                .position(|f| *f == after)
                .map(|i| i + 1)
                .ok_or_else(|| {
                    Error::new(
                        Code::NotFound,
                        format!("no feature {after} to insert after"),
                    )
                    .with("feature", &after)
                })
        }
    }
}

/// Writes a feature file, then the part's list: a crash in between leaves
/// at worst an unlisted feature file, which `check_format` reports.
pub(crate) fn save(
    session: &mut Session,
    part: &Id,
    writes: &[(&Id, &Feature)],
    order: Vec<Id>,
) -> Result<String> {
    for (id, f) in writes {
        write_json(&feature_path(session.root(), part, id), &f.to_json())?;
    }
    // A part the write makes the assembly place takes the next item number;
    // one it no longer places loses its own.
    let p = session.project().part(part)?;
    let items = p.file.as_ref().map(|f| f.items.clone()).unwrap_or_default();
    let feature = |fid: &Id| match writes.iter().find(|(w, _)| *w == fid) {
        Some((_, f)) => Some(*f),
        None => p.features.get(fid)?.as_ref().ok(),
    };
    let after = ryupdm::items::placed_by(&order, feature);
    let whole = order.iter().all(|f| feature(f).is_some());
    let items = ryupdm::items::assigned(&items, &ryupdm::items::placed(p), &after, whole);
    write_json(
        &part_path(session.root(), part),
        &PartFile {
            kind: part_kind(p),
            features: order,
            items,
        }
        .to_json(),
    )?;
    session.reload()?;
    Ok(session.project().part(part)?.revision.clone())
}

/// What a `feature.add` or `feature.edit` gives besides the feature's
/// place: each part left out keeps its current or default value.
struct Given {
    params: Map<String, Value>,
    placement: Option<PlacementArg>,
    fields: FeatureFields,
}

impl Given {
    /// The names of the fields given.
    fn present(&self) -> Vec<String> {
        let mut out: Vec<String> = self.fields.given().keys().cloned().collect();
        if !self.params.is_empty() {
            out.push("params".into());
        }
        if self.placement.is_some() {
            out.push("placement".into());
        }
        out
    }

    /// Refuses the fields `ty` doesn't take.
    fn only(&self, ty: &str, allowed: &[&str]) -> Result<()> {
        match self
            .present()
            .into_iter()
            .find(|f| !allowed.contains(&f.as_str()))
        {
            None => Ok(()),
            Some(name) => Err(
                Error::new(Code::InvalidParam, format!("a {ty} has no {name}")).with("param", name),
            ),
        }
    }
}

/// The fields a sketch takes.
const SKETCH: [&str; 5] = ["plane", "placement", "face", "face_origin", "face_x"];

/// The fields a primitive, an extrude, a revolve and a boolean take.
const PRIMITIVE: [&str; 4] = ["params", "placement", "operation", "targets"];
const EXTRUDE: [&str; 6] = [
    "sketch",
    "loops",
    "extent",
    "reverse",
    "operation",
    "targets",
];
const REVOLVE: [&str; 7] = [
    "sketch",
    "loops",
    "axis",
    "angle",
    "symmetric",
    "operation",
    "targets",
];
const BOOLEAN: [&str; 4] = ["op", "target", "targets", "tools"];
const INSTANCE: [&str; 4] = ["of", "rev", "placement", "fixed"];
const MATE: [&str; 7] = [
    "form", "a", "b", "a_offset", "b_offset", "angle", "distance",
];
const INSTANCE_PATTERN: [&str; 13] = [
    "form",
    "instances",
    "direction",
    "spacing",
    "count",
    "direction2",
    "spacing2",
    "count2",
    "reverse",
    "reverse2",
    "axis",
    "angle",
    "skip",
];
const FILLET: [&str; 4] = ["groups", "propagate", "radius", "edges"];
const CHAMFER: [&str; 9] = [
    "groups",
    "propagate",
    "form",
    "d",
    "d1",
    "d2",
    "angle",
    "face",
    "edges",
];

const PATTERN: [&str; 15] = [
    "form",
    "features",
    "direction",
    "spacing",
    "count",
    "direction2",
    "spacing2",
    "count2",
    "reverse",
    "reverse2",
    "axis",
    "angle",
    "plane",
    "skip",
    "targets",
];

/// A connector as a file holds it, from its text: an entity of an
/// occurrence of assembly `asm`, `<asm>/<path>/<solid>/<name>`, or an
/// occurrence's own frame, `<asm>/<path>`. `param` names the field for
/// errors.
fn connector_json(t: &str, asm: &Id, param: &str) -> Result<Value> {
    let r = EntityRef::parse(t)
        .map_err(|e| e.with("param", param))?
        .in_assembly();
    if r.part != *asm {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{t} is in {}, not assembly {asm}: a connector is on an occurrence of the assembly",
                r.part
            ),
        )
        .with("param", param));
    }
    if r.path.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{t} names no instance: give <assembly>/<instance>[/…]/<solid>/<name>"),
        )
        .with("param", param));
    }
    let at = if r.is_occurrence() {
        json!("origin")
    } else {
        json!({"feature": r.feature, "name": r.name})
    };
    Ok(json!({"instance": r.path, "at": at}))
}

/// A connector's text, as requests give it.
pub fn connector_text(asm: &Id, c: &crate::doc::mate::Connector) -> String {
    let path = c.path().join("/");
    match &c.at {
        crate::doc::mate::At::Origin => format!("{asm}/{path}"),
        crate::doc::mate::At::Entity(r) => format!("{asm}/{path}/{}/{}", r.feature, r.name),
    }
}

/// A mate from `current`'s fields (none for a new one) with what was
/// given laid over them: connectors as text, each keeping its offset
/// unless one is given (`{}` for none); `angle` or `distance` `""` lets it
/// go; a new form keeps only the driven values it takes.
fn mate_from(
    current: Option<&crate::doc::mate::Mate>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::mate::{Form, Mate};
    let f = &given.fields;
    let mut v = current.map_or_else(|| json!({}), Mate::fields);
    let o = v.as_object_mut().expect("an object");
    match (&f.form, current) {
        (Some(form), _) => {
            let Some(form) = Form::parse(form) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{form:?} isn't a mate: fastened, revolute, slider, cylindrical, planar or ball"),
                )
                .with("param", "form"));
            };
            o.insert("form".into(), json!(form.as_str()));
            if !form.takes_angle() {
                o.remove("angle");
            }
            if !form.takes_distance() {
                o.remove("distance");
            }
        }
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "a mate names its form: fastened, revolute, slider, cylindrical, planar or ball",
            )
            .with("param", "form"));
        }
        (None, Some(_)) => {}
    }
    for (side, text, offset) in [("a", &f.a, &f.a_offset), ("b", &f.b, &f.b_offset)] {
        if let Some(t) = text {
            let mut c = connector_json(t, part, side)?;
            if let Some(off) = o.get(side).and_then(|c| c.get("offset")) {
                c["offset"] = off.clone();
            }
            o.insert(side.into(), c);
        }
        if let Some(off) = offset {
            let Some(c) = o.get_mut(side) else {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{side}_offset moves connector {side}: give {side} too"),
                )
                .with("param", format!("{side}_offset")));
            };
            if *off == json!({}) {
                c.as_object_mut().expect("a connector").remove("offset");
            } else {
                c["offset"] = off.clone();
            }
        }
        if !o.contains_key(side) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("a mate joins two connectors: give {side}"),
            )
            .with("param", side));
        }
    }
    for (k, x) in [("angle", &f.angle), ("distance", &f.distance)] {
        match x {
            Some(Value::String(t)) if t.is_empty() => {
                o.remove(k);
            }
            Some(x) => {
                o.insert(k.into(), x.clone());
            }
            None => {}
        }
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let m = Mate::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::Mate(Box::new(m)))
}

/// An instance pattern from `current`'s fields (none for a new one, whose
/// form, left out, is circular with an axis, else linear) with what was
/// given laid over them, as a part's pattern takes them; directions and
/// the axis are `x`, `y`, `z` or a connector's text, whose z they take.
fn instance_pattern_from(
    current: Option<&crate::doc::mate::InstancePattern>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::mate::InstancePattern;
    let f = &given.fields;
    let fresh = || json!({"form": if f.axis.is_some() { "circular" } else { "linear" }});
    let mut v = current.map_or_else(fresh, InstancePattern::fields);
    let o = v.as_object_mut().expect("an object");
    const SECOND: [&str; 4] = ["direction2", "spacing2", "count2", "reverse2"];
    if let Some(form) = &f.form {
        let takes: &[&str] = match form.as_str() {
            "linear" => &[
                "direction",
                "spacing",
                "count",
                "reverse",
                "direction2",
                "spacing2",
                "count2",
                "reverse2",
                "skip",
            ],
            "circular" => &["axis", "count", "angle", "skip"],
            _ => &[],
        };
        o.retain(|k, _| takes.contains(&k.as_str()) || k == "instances");
        o.insert("form".into(), json!(form));
    }
    if f.direction2.as_deref() == Some("") {
        o.retain(|k, _| !SECOND.contains(&k.as_str()));
    }
    for (k, x) in [
        ("direction", &f.direction),
        ("direction2", &f.direction2),
        ("axis", &f.axis),
    ] {
        if let Some(t) = x.as_deref().filter(|t| !t.is_empty()) {
            let d = if t.contains('/') {
                connector_json(t, part, k)?
            } else {
                json!(t)
            };
            o.insert(k.into(), d);
        }
    }
    for (k, x) in [
        ("spacing", &f.spacing),
        ("count", &f.count),
        ("spacing2", &f.spacing2),
        ("count2", &f.count2),
        ("angle", &f.angle),
    ] {
        if let Some(x) = x {
            o.insert(k.into(), x.clone());
        }
    }
    for (k, x) in [("reverse", f.reverse), ("reverse2", f.reverse2)] {
        match x {
            Some(true) => {
                o.insert(k.into(), json!(true));
            }
            Some(false) => {
                o.remove(k);
            }
            None => {}
        }
    }
    match &f.skip {
        Some(x) if x == &json!([]) => {
            o.remove("skip");
        }
        Some(x) => {
            o.insert("skip".into(), x.clone());
        }
        None => {}
    }
    if let Some(x) = &f.instances {
        o.insert("instances".into(), json!(x));
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = InstancePattern::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::InstancePattern(Box::new(p)))
}

/// A pattern from `current`'s fields (none for a new one, whose form,
/// left out, is what its fields say: a mirror with a plane, circular with
/// an axis, else linear) with what was given laid over them. A new form
/// keeps only the fields it takes; references (a direction, an axis, a
/// plane) are given as text and kept as a file holds them; `direction2`
/// `""` drops the second direction, and no skips, or false, is the
/// default.
fn pattern_from(
    current: Option<&crate::doc::pattern::Pattern>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::pattern::Pattern;
    let f = &given.fields;
    let fresh = || {
        let form = match (&f.plane, &f.axis) {
            (Some(_), _) => "mirror",
            (None, Some(_)) => "circular",
            _ => "linear",
        };
        json!({ "form": form })
    };
    let mut v = current.map_or_else(fresh, Pattern::fields);
    let o = v.as_object_mut().expect("an object");
    const SECOND: [&str; 4] = ["direction2", "spacing2", "count2", "reverse2"];
    if let Some(form) = &f.form {
        let takes: &[&str] = match form.as_str() {
            "linear" => &[
                "direction",
                "spacing",
                "count",
                "reverse",
                "direction2",
                "spacing2",
                "count2",
                "reverse2",
                "skip",
            ],
            "circular" => &["axis", "count", "angle", "skip"],
            _ => &[],
        };
        o.retain(|k, _| {
            takes.contains(&k.as_str()) || ["features", "targets"].contains(&k.as_str())
        });
        o.insert("form".into(), json!(form));
    }
    // A reference given as text; an origin axis or plane as its name.
    let entity = |t: &str, at: &str| -> Result<Value> {
        if t.contains('/') {
            Ok(Reference::parse(t, part)
                .map_err(|e| e.with("pointer", at))?
                .to_json())
        } else {
            Ok(json!(t))
        }
    };
    if f.direction2.as_deref() == Some("") {
        o.retain(|k, _| !SECOND.contains(&k.as_str()));
    }
    for (k, x) in [
        ("direction", &f.direction),
        ("direction2", &f.direction2),
        ("axis", &f.axis),
        ("plane", &f.plane),
    ] {
        if let Some(t) = x.as_deref().filter(|t| !t.is_empty()) {
            o.insert(k.into(), entity(t, &format!("/{k}"))?);
        }
    }
    for (k, x) in [
        ("spacing", &f.spacing),
        ("count", &f.count),
        ("spacing2", &f.spacing2),
        ("count2", &f.count2),
        ("angle", &f.angle),
    ] {
        if let Some(x) = x {
            o.insert(k.into(), x.clone());
        }
    }
    for (k, x) in [("reverse", f.reverse), ("reverse2", f.reverse2)] {
        match x {
            Some(true) => {
                o.insert(k.into(), json!(true));
            }
            Some(false) => {
                o.remove(k);
            }
            None => {}
        }
    }
    match &f.skip {
        Some(x) if x == &json!([]) => {
            o.remove("skip");
        }
        Some(x) => {
            o.insert("skip".into(), x.clone());
        }
        None => {}
    }
    if let Some(x) = &f.features {
        o.insert("features".into(), json!(x));
    }
    if let Some(t) = &f.targets {
        o.insert("targets".into(), json!(t));
    }
    // What the form needs and nothing gave it.
    let needs: &[(&str, &str)] = match o.get("form").and_then(Value::as_str) {
        Some("linear") => &[
            (
                "direction",
                "an origin axis, a straight edge or a planar face",
            ),
            ("spacing", "the distance between instances"),
            ("count", "how many instances, the seed's included"),
        ],
        Some("circular") => &[
            (
                "axis",
                "an origin axis, a straight or circular edge, or a cylindrical or conical face",
            ),
            ("count", "how many instances, the seed's included"),
            (
                "angle",
                "the span: 360 deg spaces the count evenly round a whole turn, less spans first to last",
            ),
        ],
        Some("mirror") => &[("plane", "an origin plane or a planar face")],
        _ => &[],
    };
    let form = o.get("form").and_then(Value::as_str).unwrap_or_default();
    if let Some((k, what)) = needs.iter().find(|(k, _)| !o.contains_key(*k)) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("a {form} pattern needs {k}: {what}"),
        )
        .with("param", *k)
        .with("pointer", format!("/{k}")));
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = Pattern::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::Pattern(p))
}

const HOLE: [&str; 9] = [
    "sketch",
    "points",
    "form",
    "diameter",
    "counterbore",
    "countersink",
    "depth",
    "bottom",
    "targets",
];

/// A hole from `current`'s fields (none for a new one, which is simple)
/// with what was given laid over them. A counterbore or countersink given
/// in part keeps the rest; a new form keeps only its own; an up-to target
/// is given as text and kept as a file holds it; no points is every point
/// and a flat bottom the default, so neither is written.
fn hole_from(
    current: Option<&crate::doc::hole::Hole>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::hole::Hole;
    let mut v = current.map_or_else(|| json!({"form": "simple"}), Hole::fields);
    let o = v.as_object_mut().expect("an object");
    let f = &given.fields;
    if let Some(form) = &f.form {
        o.insert("form".into(), json!(form));
        for k in ["counterbore", "countersink"] {
            if form != k {
                o.remove(k);
            }
        }
    }
    for (k, x) in [
        ("counterbore", &f.counterbore),
        ("countersink", &f.countersink),
    ] {
        match (o.get_mut(k), x) {
            (Some(Value::Object(old)), Some(Value::Object(new))) => old.extend(new.clone()),
            (_, Some(x)) => {
                o.insert(k.into(), x.clone());
            }
            (_, None) => {}
        }
    }
    if let Some(s) = &f.sketch {
        o.insert("sketch".into(), json!(s));
    }
    match &f.points {
        Some(p) if p.is_empty() => {
            o.remove("points");
        }
        Some(p) => {
            o.insert("points".into(), json!(p));
        }
        None => {}
    }
    if let Some(d) = &f.diameter {
        o.insert("diameter".into(), d.clone());
    }
    if let Some(d) = &f.depth {
        let mut d = d.clone();
        if let Some(Value::String(t)) = d.pointer("/up_to/target") {
            let r =
                Reference::parse(t, part).map_err(|e| e.with("pointer", "/depth/up_to/target"))?;
            d["up_to"]["target"] = r.to_json();
        }
        o.insert("depth".into(), d);
    }
    match &f.bottom {
        Some(b) if b == "flat" => {
            o.remove("bottom");
        }
        Some(b) => {
            o.insert("bottom".into(), b.clone());
        }
        None => {}
    }
    if let Some(t) = &f.targets {
        o.insert("targets".into(), json!(t));
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let h = Hole::from_fields(&mut o, unit, env)?;
    o.finish()?;
    Ok(Feature::Hole(h))
}

/// An instance: `current`'s fields (none for a new one), with what was
/// given laid over them. A new instance needs `of`; its revision is
/// `working` and its pose the assembly's frame unless given. The origin
/// takes quantities and expressions, held as meters.
fn instance_from(
    current: Option<&Instance>,
    given: &Given,
    unit: Unit,
    env: &Env,
) -> Result<Instance> {
    let f = &given.fields;
    let part = match (&f.of, current) {
        (Some(of), _) => Id::parse(of, "part").map_err(|e| e.with("param", "of"))?,
        (None, Some(c)) => c.part.clone(),
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "an instance names the part it places (of)",
            )
            .with("param", "of"));
        }
    };
    let rev = match f.rev.as_deref() {
        None => current.map_or(Rev::Working, |c| c.rev.clone()),
        Some("working") => Rev::Working,
        Some(l) => {
            Rev::Label(crate::doc::assembly::label_text(l).map_err(|e| e.with("param", "rev"))?)
        }
    };
    let mut pose = current.map_or(Pose::IDENTITY, |c| c.pose);
    if let Some(arg) = &given.placement {
        if let Some(origin) = &arg.origin {
            for (i, v) in origin.iter().enumerate() {
                let q = Quantity::from_json_in(v, unit, &|n| env.get(n))
                    .map_err(|e| e.with("pointer", format!("/placement/origin/{i}")))?;
                if q.dim() != crate::units::Dim::LENGTH {
                    return Err(Error::new(
                        Code::UnitMismatch,
                        format!("an instance's origin is a length, not {}", q.text()),
                    )
                    .with("pointer", format!("/placement/origin/{i}")));
                }
                pose.origin_m[i] = crate::doc::sketch::clean(q.meters());
            }
        }
        if let Some(z) = arg.z {
            pose.z = z.map(crate::doc::sketch::clean);
        }
        if let Some(x) = arg.x {
            pose.x = x.map(crate::doc::sketch::clean);
        }
    }
    // Refuse a pose that could never build, before it is written.
    crate::assembly::pose_rigid(&pose)?;
    // Which use of an imported product it is, while it places that part.
    let import = current
        .filter(|c| c.part == part)
        .and_then(|c| c.import.clone());
    Ok(Instance {
        part,
        rev,
        pose,
        fixed: f.fixed.unwrap_or(current.is_some_and(|c| c.fixed)),
        import,
    })
}

/// Refuses instance `id` of assembly `pid`, listed in `list`, if its part
/// doesn't exist (`not_found`), if placing it would make the assembly
/// place itself (`assembly_cycle`), or if its label doesn't resolve now
/// (`label_not_found`, `label_not_official`).
fn check_instance(
    session: &mut Session,
    pid: &Id,
    list: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<()> {
    let i = f.instance().expect("an instance");
    if !session.project().parts.contains_key(&i.part) {
        return Err(
            Error::new(Code::NotFound, format!("no part {} to place", i.part))
                .with("part", &i.part)
                .with("param", "of"),
        );
    }
    match &i.rev {
        Rev::Working => {
            let mut project = session.project().clone();
            let q = proposed(project.part(pid)?, list, &[(id, f)]);
            project.parts.insert(pid.clone(), q);
            let cycles = crate::assembly::Cycles::of(&project, &crate::assembly::Pins::new());
            let (a, c) = ((pid.clone(), Rev::Working), (i.part.clone(), Rev::Working));
            if cycles.closes(&a, &c) {
                return Err(crate::assembly::cycle_error(&cycles, &a, &c).with("feature", id));
            }
        }
        Rev::Label(l) => {
            crate::release::pinned(session, &i.part, l).map_err(|e| e.with("feature", id))?;
        }
    }
    Ok(())
}

/// A primitive's operation and targets: `current`'s, with what was
/// given laid over them (`new` drops the targets).
fn combine_from(current: &Combine, given: &Given) -> Result<Combine> {
    let mut v = json!({});
    current.write(&mut v);
    let f = &given.fields;
    if let Some(op) = &f.operation {
        v["operation"] = json!(op);
        if op == "new" {
            v.as_object_mut().expect("an object").remove("targets");
        }
    }
    if let Some(t) = &f.targets {
        v["targets"] = json!(t);
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let c = Combine::from_fields(&mut o)?;
    o.finish()?;
    Ok(c)
}

/// A boolean from `current`'s fields (none for a new one) with what was
/// given laid over them. Its target may come as `target`, or as the one
/// entry of `targets`.
fn boolean_from(current: Option<&Boolean>, given: &Given) -> Result<Boolean> {
    let mut v = current.map_or_else(|| json!({}), Boolean::fields);
    let f = &given.fields;
    let target = match (&f.target, &f.targets) {
        (Some(_), Some(_)) => {
            return Err(Error::new(Code::Usage, "give a boolean's target once"));
        }
        (Some(t), None) => Some(t.clone()),
        (None, Some(ts)) => match ts.as_slice() {
            [t] => Some(t.clone()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a boolean has one target, not {}", ts.len()),
                )
                .with("param", "targets"));
            }
        },
        (None, None) => None,
    };
    if let Some(op) = &f.op {
        v["op"] = json!(op);
    }
    if let Some(t) = target {
        v["target"] = json!(t);
    }
    if let Some(t) = &f.tools {
        v["tools"] = json!(t);
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let b = Boolean::from_fields(&mut o)?;
    o.finish()?;
    Ok(b)
}

/// A fillet or chamfer (`ty`) from `current`'s fields (none for a new
/// one) with what was given laid over them: its `groups` whole, or its
/// one group's fields (`radius`, or `form`, `d`, `d1`, `d2`, `angle` and
/// `face`; and `edges`), and `propagate`. Edges and faces are given as
/// text and kept as a file holds them; a new form keeps only what it
/// takes.
fn blend_from(
    ty: &str,
    current: Option<&Feature>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    use crate::doc::blend::{Chamfer, Fillet};
    let f = &given.fields;
    let reference = |v: &Value, at: &str| -> Result<Value> {
        match v {
            Value::String(t) => Ok(Reference::parse(t, part)
                .map_err(|e| e.with("pointer", at))?
                .to_json()),
            v => Ok(v.clone()),
        }
    };
    let mut v = match current {
        Some(Feature::Fillet(x)) => x.fields(),
        Some(Feature::Chamfer(x)) => x.fields(),
        _ if ty == "chamfer" => json!({"groups": [{"form": "equal"}]}),
        _ => json!({"groups": [{}]}),
    };
    let sizes = [
        ("radius", &f.radius),
        ("d", &f.d),
        ("d1", &f.d1),
        ("d2", &f.d2),
        ("angle", &f.angle),
    ];
    let one = sizes.iter().any(|(_, x)| x.is_some())
        || f.edges.is_some()
        || f.form.is_some()
        || f.face.is_some();
    if let Some(groups) = &f.groups {
        if one {
            return Err(Error::new(
                Code::Usage,
                format!("give a {ty}'s groups, or its one group's fields, not both"),
            ));
        }
        let mut groups = groups.clone();
        if let Value::Array(gs) = &mut groups {
            for (i, g) in gs.iter_mut().enumerate() {
                if let Some(Value::Array(es)) = g.get_mut("edges") {
                    for (j, e) in es.iter_mut().enumerate() {
                        *e = reference(e, &format!("/groups/{i}/edges/{j}"))?;
                    }
                }
                if let Some(face) = g.get_mut("face") {
                    *face = reference(face, &format!("/groups/{i}/face"))?;
                }
            }
        }
        v["groups"] = groups;
    } else if one {
        let gs = v["groups"].as_array_mut().expect("groups");
        if gs.len() != 1 {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "this {ty} has {} groups: give them all, as groups",
                    gs.len()
                ),
            )
            .with("param", "groups"));
        }
        let g = gs[0].as_object_mut().expect("a group");
        if let Some(form) = &f.form {
            let takes: &[&str] = match form.as_str() {
                "equal" => &["d", "edges"],
                "distances" => &["d1", "d2", "face", "edges"],
                "distance_angle" => &["d", "angle", "face", "edges"],
                _ => &["edges"],
            };
            g.retain(|k, _| takes.contains(&k.as_str()));
            g.insert("form".into(), json!(form));
        }
        for (k, x) in sizes {
            if let Some(x) = x {
                g.insert(k.into(), x.clone());
            }
        }
        if let Some(face) = &f.face {
            g.insert("face".into(), reference(&json!(face), "/groups/0/face")?);
        }
        if let Some(edges) = &f.edges {
            let edges: Vec<Value> = edges
                .iter()
                .enumerate()
                .map(|(j, e)| reference(&json!(e), &format!("/groups/0/edges/{j}")))
                .collect::<Result<_>>()?;
            g.insert("edges".into(), json!(edges));
        }
    }
    match f.propagate {
        Some(false) => v["propagate"] = json!(false),
        Some(true) => {
            v.as_object_mut().expect("an object").remove("propagate");
        }
        None => {}
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let out = match ty {
        "fillet" => Feature::Fillet(Fillet::from_fields(&mut o, unit, env)?),
        _ => Feature::Chamfer(Chamfer::from_fields(&mut o, unit, env)?),
    };
    o.finish()?;
    Ok(out)
}

/// The solids a feature put at `at` in `order` could act on: founded
/// before it and not consumed.
fn solids_before(p: &Part, order: &[Id], at: usize) -> Vec<Id> {
    Solids::of_order(p, &order[..at])
        .tips()
        .map(|(s, _)| s.clone())
        .collect()
}

/// Refuses a write that would leave a feature acting on what isn't a
/// solid before it in `q` (the part as the write leaves it): the written
/// features, those that name them, and those that name a written
/// boolean's tools.
fn check_solids(q: &Part, written: &[&Id]) -> Result<()> {
    let solids = Solids::of(q);
    let mut named: Vec<&Id> = written.to_vec();
    for w in written {
        if let Some(Ok(Feature::Boolean(b))) = q.features.get(*w) {
            named.extend(&b.tools);
        }
    }
    for id in q.order() {
        let Some(Ok(f)) = q.features.get(id) else {
            continue;
        };
        let targets = f.targets();
        let concerned = written.contains(&id) || targets.iter().any(|t| named.contains(t));
        if targets.is_empty() || !concerned {
            continue;
        }
        crate::graph::check_order(q, q.order(), id, f)?;
        for t in targets {
            if let Some(by) = solids.consumed_by(t, id) {
                return Err(rebuild::consumed(id, t, by));
            }
            if !solids.is_solid(t) {
                return Err(rebuild::not_a_solid(q, &solids, id, "targets", t));
            }
        }
    }
    Ok(())
}

/// The part as a write would leave it: `writes` in place, listed as
/// `order`.
fn proposed(p: &Part, order: &[Id], writes: &[(&Id, &Feature)]) -> Part {
    let mut q = p.clone();
    for (id, f) in writes {
        q.features.insert((*id).clone(), Ok((*f).clone()));
    }
    q.file = Ok(PartFile {
        kind: part_kind(p),
        features: order.to_vec(),
        items: p.file.as_ref().map(|f| f.items.clone()).unwrap_or_default(),
    });
    q
}

/// `invalid_param` for `feature.add` or `feature.edit` of an import:
/// its file, product and body come from the file, through `import`.
fn import_not_written_here(how: &str) -> Error {
    Error::new(
        Code::InvalidParam,
        format!(
            "an import can't be {how}ed with feature.{how}: import a STEP file with `import` (`--into` adds its bodies to a part)"
        ),
    )
    .with("type", "import")
}

/// Whether `part` is an assembly, as its part file says; a part whose
/// file can't be read is taken for a part.
pub(crate) fn part_kind(part: &Part) -> PartKind {
    part.file.as_ref().map_or(PartKind::Part, |f| f.kind)
}

/// An extrude or revolve from its fields: the current ones (for an edit)
/// with those given laid over them.
fn sweep_from(
    ty: &str,
    current: Option<Value>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Feature> {
    // A new extrude's extent is blind unless it says otherwise.
    let fresh = match ty {
        "extrude" => json!({"extent": {"type": "blind"}}),
        _ => json!({}),
    };
    let mut v = current.unwrap_or(fresh);
    let o = v.as_object_mut().expect("an object");
    if given.fields.operation.as_deref() == Some("new") {
        o.remove("targets");
    }
    for (k, x) in given.fields.given() {
        // No loops is every loop, and false the default: nothing to write.
        if (k == "loops" && x == json!([])) || x == json!(false) {
            o.remove(&k);
            continue;
        }
        let x = match (k.as_str(), x) {
            // An up-to target is given as text, and kept as a file holds it.
            ("extent", Value::Object(mut e)) => {
                if let Some(Value::String(t)) = e.get("target") {
                    let r = Reference::parse(t, part)
                        .map_err(|e| e.with("pointer", "/extent/target"))?;
                    e.insert("target".into(), r.to_json());
                }
                Value::Object(e)
            }
            // An axis given as text is an edge.
            ("axis", Value::String(t)) if t.contains('/') => Reference::parse(&t, part)
                .map_err(|e| e.with("pointer", "/axis"))?
                .to_json(),
            (_, x) => x,
        };
        // Part of an extent keeps the rest; a new type keeps only what it
        // takes (blind and symmetric share a length).
        match (o.get_mut(&k), x) {
            (Some(Value::Object(old)), Value::Object(new)) if k == "extent" => {
                if let Some(t) = new.get("type").and_then(Value::as_str) {
                    let takes: &[&str] = match t {
                        "up_to" => &["target", "offset"],
                        "through_all" => &["both"],
                        _ => &["length"],
                    };
                    old.retain(|f, _| takes.contains(&f.as_str()));
                }
                old.extend(new)
            }
            (_, x) => {
                o.insert(k, x);
            }
        }
    }
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let f = match ty {
        "extrude" => Feature::Extrude(Extrude::from_fields(&mut o, unit, env)?),
        _ => Feature::Revolve(Revolve::from_fields(&mut o, env)?),
    };
    o.finish()?;
    Ok(f)
}

/// The listed features that sweep `sketch` or drill at its points.
pub(crate) fn sweeps_of(part: &Part, sketch: &Id) -> Vec<Id> {
    part.order()
        .iter()
        .filter(|f| {
            part.features
                .get(*f)
                .and_then(|r| r.as_ref().ok())
                .and_then(Feature::swept)
                == Some(sketch)
        })
        .cloned()
        .collect()
}

/// Refuses a sweep or a hole that names what isn't there: its sketch must
/// be a sketch listed before it in `order`, with the entities it names,
/// its axis a line and a hole's points points. Whether the loops make a
/// profile only rebuild can tell.
fn check_sweep(part: &Part, order: &[Id], id: &Id, f: &Feature) -> Result<()> {
    let sketch = f.swept().expect("a sweep");
    check_sketch(part, order, id, sketch)?;
    let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
        return Ok(());
    };
    // A hole's points must be points of its sketch; whether it has any
    // to drill when it lists none only rebuild can tell.
    if let Feature::Hole(h) = f {
        for (i, e) in h.points.iter().enumerate() {
            match sk.entities.get(e).map(|x| &x.geometry) {
                None => {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/points/{i}")),
                    );
                }
                Some(crate::doc::sketch::Geometry::Point { .. }) => {}
                Some(g) => {
                    return Err(Error::new(
                        Code::ReferenceWrongKind,
                        format!("a hole drills at points; {e} is a {}", g.type_name()),
                    )
                    .with("entity", e)
                    .with("pointer", format!("/points/{i}")));
                }
            }
        }
        return Ok(());
    }
    let (loops, axis) = match f {
        Feature::Extrude(e) => (&e.loops, None),
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Line(l) => (&r.loops, Some(l)),
            RevolveAxis::Edge(_) => (&r.loops, None),
        },
        _ => unreachable!("a sweep"),
    };
    for (field, e) in loops
        .iter()
        .map(|e| ("loops", e))
        .chain(axis.map(|a| ("axis", a)))
    {
        match sk.entities.get(e).map(|x| &x.geometry) {
            None => {
                return Err(
                    Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                        .with("sketch", sketch)
                        .with("entity", e)
                        .with("pointer", format!("/{field}")),
                );
            }
            Some(g)
                if field == "axis" && !matches!(g, crate::doc::sketch::Geometry::Line { .. }) =>
            {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!("an axis is a line; {e} is a {}", g.type_name()),
                )
                .with("entity", e)
                .with("pointer", "/axis"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Refuses a sketch that isn't a sketch listed before `id` in `order`.
fn check_sketch(part: &Part, order: &[Id], id: &Id, sketch: &Id) -> Result<()> {
    let at = |f: &Id| order.iter().position(|x| x == f);
    match (at(sketch), at(id)) {
        (None, _) => {
            return Err(Error::new(
                Code::NotFound,
                format!("part {} has no feature {sketch} to sweep", part.id),
            )
            .with("sketch", sketch));
        }
        (Some(s), Some(me)) if s >= me => {
            return Err(Error::new(
                Code::ReferenceForward,
                format!("{id} would sweep {sketch}, which comes after it"),
            )
            .with("feature", id)
            .with("sketch", sketch));
        }
        _ => {}
    }
    match part.features.get(sketch) {
        Some(Ok(f)) if f.sketch().is_none() => Err(Error::new(
            Code::ReferenceWrongKind,
            format!("{sketch} is a {}, not a sketch", f.type_name()),
        )
        .with("sketch", sketch)),
        _ => Ok(()),
    }
}

/// A new feature's placement from what was given. With z given and x left
/// out, x defaults to world X, or to world Y when z lies along X; either
/// way the file records it.
fn new_placement(arg: PlacementArg, unit: Unit, env: &Env) -> Result<Placement> {
    let mut placed = Placement::identity(unit);
    let along_x = arg.x.is_none()
        && arg.z.is_some_and(|z| {
            let mut p = Placement::identity(unit);
            p.z = z;
            rebuild::placement(&p).is_err_and(|e| e.details["kernel"] == "Degenerate")
        });
    if along_x {
        placed.x = [0.0, 1.0, 0.0];
    }
    apply_placement(&mut placed, arg, unit, env)?;
    Ok(placed)
}

/// A sketch's plane after an edit: an origin plane, its placement with
/// what was given applied, or a face with its origin and `x`. `current`
/// is `None` for a new sketch.
fn sketch_plane(
    current: Option<&Plane>,
    given: &Given,
    part: &Id,
    unit: Unit,
    env: &Env,
) -> Result<Plane> {
    let f = &given.fields;
    let chosen = [
        f.plane.is_some(),
        given.placement.is_some(),
        f.face.is_some(),
    ];
    if chosen.iter().filter(|c| **c).count() > 1 {
        return Err(Error::new(
            Code::Usage,
            "give a sketch one of a plane, a placement and a face",
        ));
    }
    // A face's origin or x, or none: the default.
    let reference = |s: &Option<String>| -> Result<Option<Option<Reference>>> {
        match s.as_deref() {
            None => Ok(None),
            Some("") => Ok(Some(None)),
            Some(t) => Reference::parse(t, part).map(|r| Some(Some(r))),
        }
    };
    let (origin, x) = (reference(&f.face_origin)?, reference(&f.face_x)?);
    let plane = match (&f.plane, &given.placement, &f.face) {
        (Some(p), _, _) => {
            Plane::Origin(OriginPlane::parse(p).map_err(|e| e.with("pointer", "/plane"))?)
        }
        (_, Some(arg), _) => {
            let mut p = match current.and_then(|c| c.placement(unit)) {
                Some(p) => p,
                None => new_placement(arg.clone(), unit, env)?,
            };
            apply_placement(&mut p, arg.clone(), unit, env)?;
            Plane::Placement(p)
        }
        (_, _, Some(face)) => Plane::Face(OnFace {
            face: Reference::parse(face, part)?,
            origin: None,
            x: None,
        }),
        _ => current.cloned().unwrap_or(Plane::Origin(OriginPlane::Xy)),
    };
    match plane {
        Plane::Face(mut on) => {
            // A new face keeps no old origin or x.
            let old = match current {
                Some(Plane::Face(c)) if f.face.is_none() => Some(c),
                _ => None,
            };
            on.origin = origin.unwrap_or_else(|| old.and_then(|c| c.origin.clone()));
            on.x = x.unwrap_or_else(|| old.and_then(|c| c.x.clone()));
            Ok(Plane::Face(on))
        }
        p if origin.is_some() || x.is_some() => {
            let _ = p;
            Err(Error::new(
                Code::Usage,
                "face_origin and face_x go with a sketch on a face",
            ))
        }
        p => Ok(p),
    }
}

/// Refuses a feature whose references don't resolve to what they must
/// now, in the part's current rebuild; `order` is the list as the write
/// leaves it.
fn check_references(
    session: &mut Session,
    part: &Id,
    order: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<()> {
    if f.references().is_empty() {
        return Ok(());
    }
    let p = session.project().part(part)?.clone();
    let results = session.rebuilt(part)?;
    let checked = check_resolved(&p, order, id, f, results);
    checked.map_err(|e| session.explain_in(part, order, e))
}

/// [`check_references`] against `results`.
fn check_resolved(
    p: &Part,
    order: &[Id],
    id: &Id,
    f: &Feature,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    match f {
        Feature::Sketch(s) => crate::sketch::context(p, order, id, s, results).map(|_| ()),
        // What the target is; how far it is only rebuild can tell.
        Feature::Extrude(e) => match &e.extent {
            Extent::UpTo { target, .. } => check_up_to(p, order, id, "target", target, results),
            _ => Ok(()),
        },
        Feature::Hole(h) => match &h.depth {
            crate::doc::hole::Depth::UpTo { target, .. } => {
                check_up_to(p, order, id, "depth", target, results)
            }
            _ => Ok(()),
        },
        // What its directions, axis and plane are; where the instances
        // land only rebuild can tell.
        Feature::Pattern(x) => {
            let q = proposed(p, order, &[(id, f)]);
            crate::pattern::motions(&q, id, x, results).map(|_| ())
        }
        // Edges of one solid, a face where a face goes, no edge twice; what
        // the kernel makes of them only rebuild can tell.
        Feature::Fillet(_) | Feature::Chamfer(_) => {
            rebuild::check_blend(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // That the edge is straight; whether it lies in the sketch's plane
        // only rebuild can tell.
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Edge(e) => {
                let found = crate::reference::resolve(p, order, id, "axis", e, results)?;
                crate::reference::line(p, id, "axis", e, found).map(|_| ())
            }
            RevolveAxis::Line(_) => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Refuses an up-to target (`role` in feature `id`) that isn't a planar
/// face or a vertex now.
fn check_up_to(
    p: &Part,
    order: &[Id],
    id: &Id,
    role: &str,
    target: &Reference,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    use crate::reference::{kind, resolve, wrong_kind};
    use ryukei::topo::EntityId;
    let found = resolve(p, order, id, role, target, results)?;
    match found.1 {
        EntityId::Vertex(_) => Ok(()),
        EntityId::Face(_) => crate::reference::plane(p, id, role, target, found).map(|_| ()),
        e => Err(wrong_kind(
            p,
            id,
            role,
            target,
            kind(e),
            "a planar face or a vertex",
        )),
    }
}

/// Every feature type `feature.add` makes.
const FEATURE_TYPES: [&str; 16] = [
    "box",
    "cylinder",
    "cone",
    "sphere",
    "torus",
    "sketch",
    "extrude",
    "revolve",
    "boolean",
    "fillet",
    "chamfer",
    "hole",
    "pattern",
    "instance",
    "mate",
    "instance_pattern",
];

fn feature_add(
    session: &mut Session,
    part: &str,
    ty: &str,
    mut given: Given,
    id: Option<&str>,
    position: &Position,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let env = p.env.clone();
    let at = insert_at(&list, position)?;
    // A feature its part can't hold is refused before its fields are read.
    if FEATURE_TYPES.contains(&ty) {
        let fid = match id {
            Some(id) => feature_id(id)?,
            None => {
                let taken: BTreeSet<&str> = p.features.keys().map(Id::as_str).collect();
                Id::generate(ty, |s| taken.contains(s))
            }
        };
        if let Some(e) = crate::assembly::kind_error_of(p, &fid, ty) {
            return Err(e);
        }
    }
    // A pattern with no targets acts on its first seed's.
    if ty == "pattern" && given.fields.targets.is_none() {
        let seed = given.fields.features.as_ref().and_then(|s| s.first());
        let theirs = seed
            .and_then(|s| Id::parse(s, "feature").ok())
            .and_then(|s| p.features.get(&s).cloned())
            .and_then(|f| f.ok());
        match (seed, theirs) {
            (_, Some(f)) if !f.targets().is_empty() => {
                given.fields.targets = Some(f.targets().iter().map(|t| t.to_string()).collect());
            }
            // Its seed's own fault says what's wrong with it.
            (Some(seed), Some(f)) => {
                return Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!(
                        "{}its seed {seed} is a {} that joins or cuts nothing; a pattern repeats joins, cuts and holes",
                        id.map(|i| format!("pattern {i}: ")).unwrap_or_default(),
                        f.type_name()
                    ),
                )
                .with("seed", seed)
                .with("pointer", "/features/0"));
            }
            (Some(seed), None)
                if Id::parse(seed, "feature").is_ok_and(|s| !p.features.contains_key(&s)) =>
            {
                return Err(Error::new(
                    Code::NotFound,
                    format!("a pattern's seed {seed} isn't a feature of part {pid}"),
                )
                .with("seed", seed)
                .with("pointer", "/features/0"));
            }
            _ => {}
        }
    }
    // A join, cut, intersect or hole with no targets acts on the part's
    // one solid; the file names it all the same.
    let acts = ty == "hole"
        || given
            .fields
            .operation
            .as_deref()
            .is_some_and(|o| o != "new");
    if acts && given.fields.targets.is_none() {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.targets = Some(vec![one.to_string()]),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {} names the solids it acts on (targets): the part has {} before it",
                        if ty == "hole" {
                            "hole"
                        } else {
                            given.fields.operation.as_deref().unwrap_or_default()
                        },
                        before.len()
                    ),
                )
                .with("param", "targets")
                .with("solids", before));
            }
        }
    }
    let f = match ty {
        "sketch" => {
            given.only(ty, &SKETCH)?;
            Feature::Sketch(Sketch::new(sketch_plane(None, &given, &pid, unit, &env)?))
        }
        "extrude" | "revolve" => {
            given.only(ty, if ty == "extrude" { &EXTRUDE } else { &REVOLVE })?;
            sweep_from(ty, None, &given, &pid, unit, &env)?
        }
        "boolean" => {
            given.only(ty, &BOOLEAN)?;
            Feature::Boolean(boolean_from(None, &given)?)
        }
        "instance" => {
            given.only(ty, &INSTANCE)?;
            Feature::Instance(instance_from(None, &given, unit, &env)?)
        }
        "mate" => {
            given.only(ty, &MATE)?;
            mate_from(None, &given, &pid, unit, &env)?
        }
        "instance_pattern" => {
            given.only(ty, &INSTANCE_PATTERN)?;
            instance_pattern_from(None, &given, &pid, unit, &env)?
        }
        "fillet" | "chamfer" => {
            given.only(ty, if ty == "fillet" { &FILLET } else { &CHAMFER })?;
            blend_from(ty, None, &given, &pid, unit, &env)?
        }
        "hole" => {
            given.only(ty, &HOLE)?;
            hole_from(None, &given, &pid, unit, &env)?
        }
        "pattern" => {
            given.only(ty, &PATTERN)?;
            pattern_from(None, &given, &pid, unit, &env)?
        }
        "import" => return Err(import_not_written_here("add")),
        _ => {
            let ty = PrimitiveType::parse(ty)?;
            given.only(ty.as_str(), &PRIMITIVE)?;
            let combine = combine_from(&Combine::NEW, &given)?;
            Feature::Primitive(Primitive {
                ty,
                params: Primitive::params_from_json(ty, &Value::Object(given.params), unit, &env)?,
                placement: new_placement(given.placement.unwrap_or_default(), unit, &env)?,
                combine,
            })
        }
    };
    let fid = match id {
        Some(id) => feature_id(id)?,
        None => {
            // A set, not a scan per candidate: in a list of thousands the
            // first free number is thousands of candidates in (issue #25).
            let taken: BTreeSet<&str> = p.features.keys().map(Id::as_str).collect();
            Id::generate(f.type_name(), |s| taken.contains(s))
        }
    };
    if p.features.contains_key(&fid) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("part {pid} already has a feature {fid}"),
        )
        .with("feature", &fid));
    }
    list.insert(at, fid.clone());
    if let Some(e) = crate::assembly::kind_error(p, &fid, &f) {
        return Err(e);
    }
    if f.instance().is_some() {
        check_instance(session, &pid, &list, &fid, &f)?;
    }
    if !f.connectors().is_empty() || f.instance_pattern().is_some() {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
    }
    let p = session.project().part(&pid)?;
    if f.swept().is_some() {
        check_sweep(p, &list, &fid, &f)?;
    }
    check_solids(&proposed(p, &list, &[(&fid, &f)]), &[&fid])?;
    if let Feature::Pattern(x) = &f {
        crate::pattern::check_seeds(&proposed(p, &list, &[(&fid, &f)]), &fid, x)?;
    }
    check_references(session, &pid, &list, &fid, &f)?;
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let revision = &session.project().part(&pid)?.revision;
    let mut v = json!({"feature": feature_report(&pid, &fid, &f), "revision": revision});
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
}

fn feature_edit(
    session: &mut Session,
    part: &str,
    id: &str,
    given: Given,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let list = order(p)?;
    let old = feature(p, &fid)?.clone();
    let f = match old.clone() {
        Feature::Primitive(mut f) => {
            given.only(f.ty.as_str(), &PRIMITIVE)?;
            f.combine = combine_from(&f.combine, &given)?;
            let mut merged: Map<String, Value> = f
                .params
                .iter()
                .map(|(k, q)| (k.clone(), Value::String(q.text())))
                .collect();
            for (k, v) in given.params {
                if v.is_null() {
                    merged.remove(&k);
                } else {
                    merged.insert(k, v);
                }
            }
            f.params = Primitive::params_from_json(f.ty, &Value::Object(merged), unit, &p.env)?;
            apply_placement(
                &mut f.placement,
                given.placement.unwrap_or_default(),
                unit,
                &p.env,
            )?;
            Feature::Primitive(f)
        }
        // Its geometry changes through sketch.edit; the plane moves the
        // sketch without changing its solve.
        Feature::Sketch(mut s) => {
            given.only("sketch", &SKETCH)?;
            s.plane = sketch_plane(Some(&s.plane), &given, &pid, unit, &p.env)?;
            Feature::Sketch(s)
        }
        Feature::Instance(i) => {
            given.only("instance", &INSTANCE)?;
            Feature::Instance(instance_from(Some(&i), &given, unit, &p.env)?)
        }
        Feature::Mate(m) => {
            given.only("mate", &MATE)?;
            mate_from(Some(&m), &given, &pid, unit, &p.env)?
        }
        Feature::InstancePattern(x) => {
            given.only("instance_pattern", &INSTANCE_PATTERN)?;
            instance_pattern_from(Some(&x), &given, &pid, unit, &p.env)?
        }
        Feature::Boolean(b) => {
            given.only("boolean", &BOOLEAN)?;
            Feature::Boolean(boolean_from(Some(&b), &given)?)
        }
        f @ (Feature::Fillet(_) | Feature::Chamfer(_)) => {
            let ty = f.type_name();
            given.only(ty, if ty == "fillet" { &FILLET } else { &CHAMFER })?;
            blend_from(ty, Some(&f), &given, &pid, unit, &p.env)?
        }
        Feature::Pattern(x) => {
            given.only("pattern", &PATTERN)?;
            pattern_from(Some(&x), &given, &pid, unit, &p.env)?
        }
        Feature::Hole(h) => {
            given.only("hole", &HOLE)?;
            let f = hole_from(Some(&h), &given, &pid, unit, &p.env)?;
            check_sweep(p, &list, &fid, &f)?;
            f
        }
        Feature::Import(_) => return Err(import_not_written_here("edit").with("feature", &fid)),
        f @ (Feature::Extrude(_) | Feature::Revolve(_)) => {
            let ty = f.type_name();
            given.only(ty, if ty == "extrude" { &EXTRUDE } else { &REVOLVE })?;
            let current = match &f {
                Feature::Extrude(e) => e.fields(),
                Feature::Revolve(r) => r.fields(),
                _ => unreachable!("a sweep"),
            };
            let f = sweep_from(ty, Some(current), &given, &pid, unit, &p.env)?;
            check_sweep(p, &list, &fid, &f)?;
            f
        }
    };
    // A solid's founder that stops founding it leaves what acts on it, or
    // refers to it, with nothing.
    let founded = |f: &Feature| f.combine().is_some_and(|c| c.is_new());
    if founded(&old) && !founded(&f) {
        let users = crate::graph::dependents(p, &fid);
        if !users.is_empty() {
            return Err(Error::new(
                Code::HasDependents,
                format!(
                    "{fid} founds a solid that {} act on or refer to; change those first",
                    users.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
                ),
            )
            .with("feature", &fid)
            .with("features", users));
        }
    }
    if f.instance().is_some() && f != old {
        check_instance(session, &pid, &list, &fid, &f)?;
    }
    if (!f.connectors().is_empty() || f.instance_pattern().is_some()) && f != old {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
    }
    let p = session.project().part(&pid)?;
    check_solids(&proposed(p, &list, &[(&fid, &f)]), &[&fid])?;
    if let Feature::Pattern(x) = &f {
        crate::pattern::check_seeds(&proposed(p, &list, &[(&fid, &f)]), &fid, x)?;
    }
    // References already there were checked when they were written; a
    // fillet or chamfer that now grows its edges, or stops, may reach an
    // edge twice.
    let propagates = |f: &Feature| match f {
        Feature::Fillet(x) => x.propagate,
        Feature::Chamfer(x) => x.propagate,
        _ => false,
    };
    if f.references() != old.references() || propagates(&f) != propagates(&old) {
        check_references(session, &pid, &list, &fid, &f)?;
    }
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    let before = crate::sketch::current(session, &pid)?;
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let sketches = crate::sketch::keep_current(session, &pid, &before)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let revision = &session.project().part(&pid)?.revision;
    let mut v = json!({"feature": feature_report(&pid, &fid, &f), "revision": revision});
    if !sketches.is_empty() {
        v["sketches"] = json!(sketches);
    }
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
}

fn feature_delete(
    session: &mut Session,
    part: &str,
    id: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    // A broken feature file can still be deleted.
    if !p.features.contains_key(&fid) {
        return Err(no_feature(p, &fid));
    }
    let users = crate::graph::dependents(p, &fid);
    if !users.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "{fid} is read by {}; delete or change those first",
                users.iter().map(Id::as_str).collect::<Vec<_>>().join(", ")
            ),
        )
        .with("feature", &fid)
        .with("features", users));
    }
    let mut list = order(p)?;
    list.retain(|f| *f != fid);
    if part_kind(p) == PartKind::Assembly {
        let q = proposed(p, &list, &[]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    let assemblies = crate::mate::current(session, &pid)?;
    // The list first: a crash then leaves an unlisted file, never a listed
    // feature with no file.
    save(session, &pid, &[], list)?;
    let path = feature_path(session.root(), &pid, &fid);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(&path, &e)),
    }
    session.reload()?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let revision = &session.project().part(&pid)?.revision;
    let mut v = json!({"deleted": fid, "revision": revision});
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
}

fn feature_move(
    session: &mut Session,
    part: &str,
    id: &str,
    position: &Position,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let from = list
        .iter()
        .position(|f| *f == fid)
        .ok_or_else(|| no_feature(p, &fid))?;
    if position.after.is_none() && !position.first {
        return Err(Error::new(Code::Usage, "give after or first"));
    }
    if position.after.as_deref() == Some(fid.as_str()) {
        return Err(Error::new(Code::Usage, "a feature can't move after itself"));
    }
    list.remove(from);
    list.insert(insert_at(&list, position)?, fid.clone());
    // Everything must still come after what it reads, and act only on
    // solids not yet consumed.
    for f in &list {
        if let Some(Ok(feature)) = p.features.get(f)
            && (*f == fid || crate::graph::inputs(feature).contains(&&fid))
        {
            crate::graph::check_order(p, &list, f, feature)?;
        }
    }
    check_solids(&proposed(p, &list, &[]), &[&fid])?;
    let revision = save(session, &pid, &[], list.clone())?;
    Ok(json!({"features": list, "revision": revision}))
}

/// `feature.list`: each feature as listed, `ok` when it builds (issue
/// #123: a file that loads may still fail to build), with the rebuild's
/// error where it doesn't and `blocked` where only a feature it needs
/// failed. A part that can't be rebuilt at all lists what loads.
fn feature_list(session: &mut Session, part: &str) -> Result<Value> {
    let pid = part_id(part)?;
    let built: BTreeMap<Id, std::result::Result<(), Error>> = match session.rebuilt(&pid) {
        Ok(r) => r
            .features
            .iter()
            .map(|(id, o)| (id.clone(), o.as_ref().map(|_| ()).map_err(Error::clone)))
            .collect(),
        Err(_) => BTreeMap::new(),
    };
    let p = session.project().part(&pid)?;
    let features: Vec<Value> = order(p)?
        .iter()
        .map(|id| match feature(p, id) {
            Ok(f) => {
                let mut v = json!({"id": id, "type": f.type_name(), "ok": true});
                if let Some(Err(e)) = built.get(id) {
                    v["ok"] = json!(false);
                    v["error"] = json!(e);
                    if e.code == Code::DependencyFailed {
                        v["blocked"] = json!(true);
                    }
                }
                let uses = crate::graph::inputs(f);
                if !uses.is_empty() {
                    v["uses"] = json!(uses);
                }
                v
            }
            Err(e) => {
                let mut v = json!({"id": id, "ok": false, "error": e});
                if let Some(t) = e.details.get("type").filter(|t| t.is_string()) {
                    v["type"] = t.clone();
                }
                v
            }
        })
        .collect();
    Ok(json!({"part": p.id, "features": features, "orphans": p.orphans(), "revision": p.revision}))
}

/// A part's rebuild as agents and subscribers see it: each feature's
/// status, hash and counts.
fn part_report(session: &mut Session, id: &Id, occurrences: bool) -> Result<Value> {
    let part = session.project().part(id)?;
    let revision = part.revision.clone();
    let assembly = part_kind(part) == PartKind::Assembly;
    let solids = Solids::of(part);
    // A feature that doesn't load may still say what it is: a conflict
    // record's type where the merge left it alone.
    let types: BTreeMap<Id, Value> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let ty = match f {
                Ok(f) => json!(f.type_name()),
                Err(e) => e.details.get("type").filter(|t| t.is_string())?.clone(),
            };
            Some((fid.clone(), ty))
        })
        .collect();
    // What each feature reads, for showing dependencies.
    let uses: BTreeMap<Id, Vec<Id>> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let ins = crate::graph::inputs(f.as_ref().ok()?);
            (!ins.is_empty()).then(|| (fid.clone(), ins.into_iter().cloned().collect()))
        })
        .collect();
    // What each feature does to solids, and the tools a boolean consumes:
    // for showing the chains.
    let ops: BTreeMap<Id, (&'static str, Vec<Id>)> = part
        .features
        .iter()
        .filter_map(|(fid, f)| match f.as_ref().ok()? {
            Feature::Boolean(b) => Some((fid.clone(), (b.op.as_str(), b.tools.clone()))),
            Feature::Hole(_) => Some((fid.clone(), ("cut", Vec::new()))),
            // A pattern does what its seeds do.
            Feature::Pattern(x) => {
                let seed = part.features.get(x.features.first()?)?.as_ref().ok()?;
                let op = match seed {
                    Feature::Hole(_) => "cut",
                    f => f.combine()?.operation.as_str(),
                };
                Some((fid.clone(), (op, Vec::new())))
            }
            f => {
                let c = f.combine().filter(|c| !c.is_new())?;
                Some((fid.clone(), (c.operation.as_str(), Vec::new())))
            }
        })
        .collect();
    // A mate's form and connectors, as requests give them, for showing it
    // whatever it rebuilt to.
    let mates: BTreeMap<Id, Value> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let m = f.as_ref().ok()?.mate()?;
            Some((
                fid.clone(),
                json!({"form": m.form.as_str(), "a": connector_text(id, &m.a),
                       "b": connector_text(id, &m.b)}),
            ))
        })
        .collect();
    let counts = |b: &crate::rebuild::Built| {
        let c = b.body.counts();
        json!({"faces": c.faces, "edges": c.edges, "vertices": c.vertices})
    };
    // An import's file, hash, product and body, as written: shown
    // whether it built or not.
    let imports: BTreeMap<Id, Value> = part
        .features
        .iter()
        .filter_map(|(fid, f)| match f.as_ref().ok()? {
            Feature::Import(i) => Some((fid.clone(), crate::import::written(i))),
            _ => None,
        })
        .collect();
    // Instances an update kept though their usage left the file.
    let kept = if assembly {
        crate::import::usage_warnings(session.project(), id)
    } else {
        Vec::new()
    };
    let r = session.rebuilt(id)?;
    let features: Vec<Value> = r
        .features
        .iter()
        .map(|(fid, b)| {
            let mut v = match b {
                Ok(o @ crate::rebuild::Output::Solids(_)) => {
                    let mut v = json!({"id": fid, "type": types.get(fid), "ok": true,
                                       "hash": o.hash()});
                    if let crate::rebuild::Output::Solids(m) = o
                        && let Some(i) = &m.import
                    {
                        v["import"] = i.as_ref().clone();
                    }
                    match o.body() {
                        Some(b) => v["counts"] = counts(b),
                        None => {
                            v["bodies"] = o
                                .solids()
                                .iter()
                                .map(|(s, b)| {
                                    json!({"solid": s, "hash": b.hash, "counts": counts(b)})
                                })
                                .collect();
                        }
                    }
                    v
                }
                Ok(crate::rebuild::Output::Instance(p)) => {
                    let mut v = json!({"id": fid, "type": types.get(fid), "ok": true,
                                       "hash": p.hash, "instance": p.to_json()});
                    // What its mates leave it, when the assembly has any.
                    if let Some(b) = r.solve.as_ref().and_then(|s| s.instance(fid)) {
                        v["instance"]["solve"] = crate::mate::body_json(b);
                    }
                    v
                }
                Ok(crate::rebuild::Output::Mate(m)) => {
                    json!({"id": fid, "type": types.get(fid), "ok": true, "hash": m.hash,
                           "mate": m.to_json()})
                }
                Ok(crate::rebuild::Output::Copies(c)) => {
                    json!({"id": fid, "type": types.get(fid), "ok": true, "hash": c.hash,
                           "copies": c.to_json(fid)})
                }
                // A sketch makes no body: its state, not a body hash.
                Ok(crate::rebuild::Output::Sketch(s)) => {
                    json!({"id": fid, "type": types.get(fid), "ok": true,
                           "sketch": {"state": s.report.state_str(), "dof": s.report.dof,
                                      "stale": s.stale}})
                }
                Err(e) => json!({"id": fid, "type": types.get(fid), "ok": false, "error": e}),
            };
            if let Some(u) = uses.get(fid) {
                v["uses"] = json!(u);
            }
            if let Some(Value::Object(m)) = imports.get(fid) {
                if !v["import"].is_object() {
                    v["import"] = json!({});
                }
                for (k, x) in m {
                    v["import"][k] = x.clone();
                }
            }
            if let Some(Value::Object(m)) = mates.get(fid) {
                if !v["mate"].is_object() {
                    v["mate"] = json!({});
                }
                for (k, x) in m {
                    v["mate"][k] = x.clone();
                }
            }
            let of = solids.solids_of(fid);
            if !of.is_empty() {
                v["solids"] = json!(of);
            }
            if let Some((op, tools)) = ops.get(fid) {
                v["operation"] = json!(op);
                if !tools.is_empty() {
                    v["consumes"] = json!(tools);
                }
            }
            v
        })
        .collect();
    // The model: each solid with its chain, and its body as it shows.
    let shown: Vec<Value> = r
        .shown(&solids)
        .into_iter()
        .map(|s| {
            let mut v = json!({"solid": s.solid, "ok": s.error.is_none(),
                               "features": solids.chain(s.solid)});
            if let Some((f, b)) = s.state {
                v["shown"] = json!(f);
                v["hash"] = json!(b.hash);
                v["counts"] = counts(b);
            }
            if let Some(e) = s.error {
                v["error"] = json!(e);
            }
            v
        })
        .collect();
    let mut v = json!({"part": id, "revision": revision, "hash": r.hash,
                       "failed": r.failed(), "features": features, "solids": shown});
    let mut warnings = r.warnings(id);
    // An assembly's model is what its instances place.
    warnings.extend(kept);
    if assembly {
        let model = crate::assembly::Model::of_assembly(r);
        v["kind"] = json!("assembly");
        v["occurrence_count"] = json!(model.occurrences.len());
        if occurrences {
            let all: Vec<Value> = model
                .occurrences
                .iter()
                .map(crate::assembly::Occurrence::to_json)
                .collect();
            v["occurrences"] = json!(all);
        }
        if let Some(solve) = &r.solve {
            v["solve"] = solve.to_json();
        }
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok(v)
}

/// Tells subscribers what changed since they were last told: the project
/// (manifest and parts) and every part whose revision moved, rebuilt.
/// While nobody watches, it only notes the current state, so a new
/// subscriber hears of changes made after it subscribed.
pub fn publish(live: &mut Live, session: &mut Session) {
    let watched = live.hub.is_watched();
    let p = session.project();
    let project = json!({
        "default_unit": p.manifest.default_unit,
        "parts": p.parts.keys().collect::<Vec<_>>(),
    });
    if watched && live.seen.project != project {
        live.hub.send(Topic::Project, "project.changed", &project);
    }
    live.seen.project = project;
    // A merge: when it starts, ends, or what it leaves to settle changes.
    let merge = crate::validate::key(session);
    if watched && live.seen.merge != merge {
        let params = match merge {
            None => json!({"merging": false}),
            Some(_) => match crate::validate::status(session) {
                Ok(s) => json!({"merging": true, "status": s}),
                Err(e) => json!({"merging": true, "error": e}),
            },
        };
        live.hub.send(Topic::Merge, "merge.changed", &params);
    }
    live.seen.merge = merge;
    if !watched {
        // Noted only: a subscriber hears of what changes from now on.
        live.seen.pdm = None;
        live.seen.at = None;
        live.seen.parts = session
            .project()
            .parts
            .iter()
            .map(|(id, part)| (id.clone(), part.revision.clone()))
            .collect();
        return;
    }
    // Release states and parts move with the files and the release
    // branch's refs, and with nothing else: while neither has, a request
    // has nothing to tell (issue #25: this ran over every part and
    // assembly on every request a window was attached for).
    let at = (session.epoch(), crate::release::refs(session));
    if live.seen.at.as_ref() == Some(&at) {
        return;
    }
    live.seen.at = Some(at);
    // Release states: from records, parts and the release branch's refs.
    // The first reading after a subscription is what it starts from.
    let now = crate::release::summary(session);
    if let Some(seen) = &live.seen.pdm {
        let changed: Vec<&Value> = now
            .iter()
            .filter(|(id, v)| seen.get(*id) != Some(*v))
            .map(|(_, v)| v)
            .collect();
        if !changed.is_empty() {
            live.hub
                .send(Topic::Pdm, "pdm.changed", &json!({"parts": changed}));
        }
    }
    live.seen.pdm = Some(now);
    let mut revisions: BTreeMap<Id, String> = session
        .project()
        .parts
        .iter()
        .map(|(id, part)| (id.clone(), part.revision.clone()))
        .collect();
    // An assembly changes when what it places does, its files or not: its
    // rebuild's hash says so, and its labels' pins, which move with the
    // release branch and not with any file (issue #105).
    revisions.extend(session.assembly_keys());
    let mut parts = Vec::new();
    for (id, rev) in &revisions {
        match live.seen.parts.get(id) {
            Some(was) if was == rev => continue,
            // Noted while nobody watched, from its files alone, which
            // haven't changed: what it places changing before the
            // subscription isn't news.
            Some(was) if !was.contains('+') && rev.split('+').next() == Some(was.as_str()) => {
                continue;
            }
            _ => {}
        }
        // An assembly's occurrences aren't sent: a window asks for those
        // of the assembly it shows.
        let mut report =
            part_report(session, id, false).unwrap_or_else(|e| json!({"part": id, "error": e}));
        if let Ok(Err(e)) = session.project().part(id).map(|p| &p.file) {
            report["error"] = json!(e);
        }
        parts.push(report);
    }
    let removed: Vec<&Id> = live
        .seen
        .parts
        .keys()
        .filter(|id| !revisions.contains_key(*id))
        .collect();
    if !parts.is_empty() || !removed.is_empty() {
        let params = json!({"parts": parts, "removed": removed});
        live.hub.send(Topic::Model, "model.changed", &params);
    }
    live.seen.parts = revisions;
}

/// The daemon's periodic check while anyone watches: picks up changes made
/// behind its back (another process, an editor, `git checkout`) and tells
/// subscribers. Errors wait for the next request to report them.
pub fn tick(slot: &mut Option<Session>, live: &mut Live, root: &Path) {
    if live.hub.is_watched() {
        settle(slot, live, root);
    }
}

/// Brings the session and what subscribers know up to date with the
/// files.
pub fn settle(slot: &mut Option<Session>, live: &mut Live, root: &Path) {
    let _ = guarded(slot, root, true, |s| {
        let before = live.snapshot();
        let external = s.refresh_changes()?;
        if !external.is_empty() {
            activity::record_external(live, external);
            live.prune(|r| r.resolve(s).is_ok());
        }
        live.announce(&before);
        publish(live, s);
        s.trim();
        Ok(Value::Null)
    });
}

/// An answer whose shape is an interface: it says its schema.
fn versioned(r: Result<Value>) -> Result<Value> {
    r.map(|mut v| {
        v["schema"] = json!(crate::OUTPUT_SCHEMA);
        v
    })
}

/// A request's answer, with the commit it read when it named a revision:
/// in the result, or in a failure's details.
fn noted(r: Result<Value>, commit: Option<String>) -> Result<Value> {
    let Some(commit) = commit else {
        return r;
    };
    match r {
        Ok(mut v) => {
            v["commit"] = json!(commit);
            Ok(v)
        }
        Err(mut e) => {
            if e.details.is_null() {
                e.details = json!({});
            }
            if e.details.is_object() {
                e.details["commit"] = json!(commit);
            }
            Err(e)
        }
    }
}

fn rebuild(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let ids: Vec<Id> = match part {
        Some(p) => vec![part_id(p)?],
        None => session.project().parts.keys().cloned().collect(),
    };
    let mut parts = Vec::new();
    let mut warnings = Vec::new();
    let mut failed = 0;
    for id in ids {
        // A part whose list can't be read can't be rebuilt.
        order(session.project().part(&id)?)?;
        let mut report = part_report(session, &id, true)?;
        failed += report["failed"].as_u64().unwrap_or(0) as usize;
        if let Some(Value::Array(w)) = report.as_object_mut().and_then(|o| o.remove("warnings")) {
            warnings.extend(w);
        }
        parts.push(report);
    }
    let mut report = json!({ "parts": parts });
    if !warnings.is_empty() {
        report["warnings"] = json!(warnings);
    }
    if failed > 0 {
        // Every feature's status still reaches the agent, in the details.
        return Err(Error {
            code: Code::RebuildFailed,
            message: format!("{failed} feature(s) failed to rebuild"),
            details: report,
        });
    }
    Ok(report)
}

/// The display meshes of a part's solids as the model shows them,
/// `(solid, body, mesh)` by founder; a solid that never built has its
/// error instead. `only` keeps one solid; a sketch has none.
pub fn meshes(
    session: &mut Session,
    part: &Id,
    only: Option<&Id>,
    quality: mesh::Quality,
) -> Result<Vec<(Id, Result<mesh::Meshed>)>> {
    let p = session.project().part(part)?;
    let solids = Solids::of(p);
    if let Some(f) = only {
        let found = feature(p, f)?;
        if found.sketch().is_none() && !solids.is_solid(f) {
            return Err(rebuild::not_a_solid(p, &solids, f, "feature", f));
        }
    }
    let features: BTreeMap<Id, Feature> = p
        .features
        .iter()
        .filter_map(|(id, f)| Some((id.clone(), f.as_ref().ok()?.clone())))
        .collect();
    let budget = mesh_budget();
    let result = session.rebuilt(part)?;
    // The feature each shown state is from sizes its mesh.
    let built: Vec<(Id, Result<(Id, crate::rebuild::Built)>)> = result
        .shown(&solids)
        .into_iter()
        .filter(|s| only.is_none_or(|o| o == s.solid))
        .map(|s| {
            let r = match (s.state, s.error) {
                (Some((f, b)), _) => Ok((f.clone(), b.as_ref().clone())),
                (None, Some(e)) => Err(e.clone()),
                (None, None) => unreachable!("a solid that never built failed"),
            };
            (s.solid.clone(), r)
        })
        .collect();
    let ok: Vec<(&Feature, &crate::rebuild::Built)> = built
        .iter()
        .filter_map(|(_, b)| {
            let (f, b) = b.as_ref().ok()?;
            Some((&features[f], b))
        })
        .collect();
    let mut made = session.meshes.get_all(&ok, quality, budget).into_iter();
    Ok(built
        .into_iter()
        .map(|(id, b)| {
            let r = b.map(|(_, b)| mesh::Meshed {
                body: b.body,
                hash: b.hash,
                mesh: made.next().expect("one mesh per built body"),
            });
            (id, r)
        })
        .collect())
}

/// The points of the given entities, or of the whole model when there
/// are none, from coarse display meshes: what a camera frames.
pub fn model_points(session: &mut Session, refs: &[EntityRef]) -> Result<Vec<[f64; 3]>> {
    let mut points = Vec::new();
    let mut grow = |p: &ryukei::geom::Point3| points.push([p.x, p.y, p.z]);
    let coarse = mesh::Quality::Coarse;
    // The whole model: each part shown, an assembly as all its
    // occurrences (an occurrence reference with no path).
    let mut asked = refs.to_vec();
    if refs.is_empty() {
        for part in crate::assembly::shown_parts(session.project()) {
            if part_kind(session.project().part(&part)?) == PartKind::Assembly {
                asked.push(EntityRef {
                    feature: part.clone(),
                    part,
                    path: Vec::new(),
                    name: String::new(),
                });
                continue;
            }
            for (_, m) in meshes(session, &part, None, coarse)? {
                if let Ok(Ok(mesh)) = m.as_ref().map(|m| m.mesh.mesh.as_ref()) {
                    mesh.positions.iter().for_each(&mut grow);
                }
            }
        }
    }
    for r in &asked {
        let p = session.project().part(&r.part)?;
        if part_kind(p) == PartKind::Assembly {
            let r = r.in_assembly();
            let model = session.model(&r.part, &crate::doc::assembly::Rev::Working)?;
            let chosen: Vec<&crate::assembly::Occurrence> = model
                .occurrences
                .iter()
                .filter(|o| match r.is_occurrence() {
                    true => o.path.starts_with(&r.path),
                    false => o.path == r.path,
                })
                .collect();
            for o in chosen {
                let solids: Vec<&crate::assembly::Solid> = o
                    .solids
                    .iter()
                    .filter(|s| r.is_occurrence() || s.founder == r.feature)
                    .collect();
                for (s, m) in solids
                    .iter()
                    .zip(occurrence_meshes(session, &solids, coarse))
                {
                    let Ok(mesh) = &m.mesh else { continue };
                    let mut placed = |q: &ryukei::geom::Point3| grow(&o.pose.apply_point(*q));
                    match r.is_occurrence() {
                        true => mesh.positions.iter().for_each(&mut placed),
                        false => named_points(&s.built.body, mesh, &r.name, &mut placed),
                    }
                }
            }
            continue;
        }
        // A sketch entity shows by its points.
        if let Some(sk) = session
            .rebuilt(&r.part)?
            .output(&r.feature)
            .and_then(|o| o.sketch())
        {
            let id = Id::parse(&r.name, "entity")?;
            for [x, y, z] in crate::sketch::entity_points(sk, &id) {
                grow(&ryukei::geom::Point3::new(x, y, z));
            }
            continue;
        }
        for (_, m) in meshes(session, &r.part, Some(&r.feature), coarse)? {
            let Ok(m) = m else { continue };
            let Ok(mesh) = &m.mesh.mesh else { continue };
            named_points(&m.body, mesh, &r.name, &mut grow);
        }
    }
    Ok(points)
}

/// The mesh points of the face, edge or vertex of `body` named `name`.
fn named_points(
    body: &ryukei::topo::Body,
    mesh: &ryukei::mesh::BodyMesh,
    name: &str,
    grow: &mut impl FnMut(&ryukei::geom::Point3),
) {
    let named = |n: String| n == name;
    for f in &mesh.faces {
        if named(body.face(f.face).expect("live").name.to_string()) {
            for c in &mesh.corners[f.corners.start as usize..f.corners.end as usize] {
                grow(&mesh.positions[c.position as usize]);
            }
        }
    }
    for e in &mesh.edges {
        if named(body.edge(e.edge).expect("live").name.to_string()) {
            e.positions
                .iter()
                .for_each(|p| grow(&mesh.positions[*p as usize]));
        }
    }
    for (i, v) in body.vertices().enumerate() {
        if named(body.vertex(v).expect("live").name.to_string()) {
            grow(&mesh.positions[i]);
        }
    }
}

/// The display meshes of occurrences' solids, in order, each body meshed
/// once however many occurrences place it.
pub fn occurrence_meshes(
    session: &mut Session,
    solids: &[&crate::assembly::Solid],
    quality: mesh::Quality,
) -> Vec<std::sync::Arc<mesh::DisplayMesh>> {
    let budget = mesh_budget();
    let items: Vec<(&Feature, &crate::rebuild::Built)> = solids
        .iter()
        .map(|s| (s.feature.as_ref(), s.built.as_ref()))
        .collect();
    session.meshes.get_all(&items, quality, budget)
}

/// How a camera is to look: a view's name (standard, else a named view of
/// `part`, else of `focus`, else of the project's model when it is one
/// part), or normal to a face.
struct Look<'a> {
    view: Option<&'a str>,
    normal_to: Option<&'a str>,
    part: Option<&'a str>,
    focus: Option<&'a Id>,
}

/// A camera from a standard view, a named view or one normal to a face,
/// and/or framing entities. With none of those, it keeps `current`'s
/// direction (iso when there is none). A named view is restored as saved
/// unless there are entities to frame, which it frames keeping its
/// direction.
fn framed(
    session: &mut Session,
    current: Option<&Camera>,
    look: Look<'_>,
    fit: &[String],
) -> Result<Camera> {
    let named = match look.view {
        Some(v) if StandardView::named(v).is_none() => {
            let part = crate::views::part_for(session, look.part, look.focus)?;
            Some(crate::views::named(session, part.as_ref(), v)?)
        }
        _ => None,
    };
    if let Some(saved) = &named
        && fit.is_empty()
    {
        return Ok(saved.clone());
    }
    let view = look.view.and_then(StandardView::named);
    let along = look
        .normal_to
        .map(|face| normal_to_face(session, face))
        .transpose()?;
    let refs = entity::parse_all(session, fit)?;
    let points = model_points(session, &refs)?;
    if points.is_empty() {
        return Err(Error::new(Code::InvalidParam, "there is nothing to frame"));
    }
    let projection = current.map_or(DEFAULT_PROJECTION, |c| c.projection);
    Ok(match (named, along, view, current) {
        (Some(saved), ..) => crate::view::refit_points(&saved, &points, 1.0),
        (None, Some((back, up)), ..) => {
            crate::view::frame_points_along(&points, back, up, projection, 1.0)
        }
        // Square: windows that follow the camera may be any shape.
        (None, None, None, Some(c)) => crate::view::refit_points(c, &points, 1.0),
        (None, None, v, _) => {
            crate::view::frame_points(&points, v.unwrap_or(StandardView::Iso), projection, 1.0)
        }
    })
}

/// The direction (towards the eye) and up of a view normal to the face
/// `reference` names (`<part>/<solid>/<name>`, or an occurrence's face,
/// placed by its pose): along a planar face's outward normal or a
/// revolved face's axis, its longest straight edge level.
fn normal_to_face(session: &mut Session, reference: &str) -> Result<([f64; 3], [f64; 3])> {
    use ryukei::geom::{Curve, Surface};
    use ryukei::measure::Entity;
    let located = crate::measure::locate(session, reference)?;
    let not_a_face = || {
        Error::new(
            Code::InvalidParam,
            format!(
                "{reference} isn't a face: look normal to a planar face or a face of revolution"
            ),
        )
        .with("entity", reference)
    };
    let [l] = located.as_slice() else {
        return Err(not_a_face());
    };
    let Entity::Face(f) = l.entity else {
        return Err(not_a_face());
    };
    let body = &l.built.body;
    let face = body.face(f).ok_or_else(not_a_face)?;
    let surface = body.surface(face.surface).ok_or_else(not_a_face)?;
    let axis = match surface {
        Surface::Plane(p) => {
            let z = p.frame().z().get().to_array();
            match face.sense {
                ryukei::topo::Sense::Forward => z,
                ryukei::topo::Sense::Reversed => z.map(|x| -x),
            }
        }
        Surface::Cylinder(_) | Surface::Cone(_) | Surface::Torus(_) => {
            surface.frame().expect("analytic").z().get().to_array()
        }
        Surface::Revolution(r) => r.frame().z().get().to_array(),
        _ => {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{reference} is neither planar nor a face of revolution: no direction is normal to it"),
            )
            .with("entity", reference));
        }
    };
    // The longest straight edge, the first of equals in loop order.
    let mut edge: Option<([f64; 3], f64)> = None;
    for lp in body.face_loops(f) {
        for fin in body.loop_fins(lp) {
            let Some(e) = body.fin(fin).and_then(|x| body.edge(x.edge)) else {
                continue;
            };
            if !matches!(body.curve(e.curve), Some(Curve::Line(_))) {
                continue;
            }
            let (Some(a), Some(b)) = (body.vertex(e.start), body.vertex(e.end)) else {
                continue;
            };
            let d = (b.point - a.point).to_array();
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if edge.is_none_or(|(_, best)| len > best) {
                edge = Some((d, len));
            }
        }
    }
    let placed = |v: [f64; 3]| {
        l.pose
            .apply_vector(ryukei::geom::Vector3::new(v[0], v[1], v[2]))
            .to_array()
    };
    Ok(crate::view::normal_to(
        placed(axis),
        edge.map(|(d, _)| placed(d)),
    ))
}

/// A render request, unpacked.
struct RenderAsk {
    part: Option<String>,
    camera: Option<crate::view::CameraArg>,
    view: Option<String>,
    width: u32,
    height: u32,
    highlight: Vec<String>,
    edges: bool,
    labels: bool,
    sketches: bool,
    rev: Option<String>,
    frame: Vec<String>,
    compare: Option<String>,
    /// What to leave out: occurrences and solids, at the leaves.
    hidden: std::collections::BTreeSet<String>,
    /// Draw the origin's axes and planes.
    origin: bool,
}

/// How a compare paints one rendered solid: wholly new when the other
/// side doesn't have it, else its faces new or changed by name.
fn compare_paint(
    theirs: Option<&mesh::Meshed>,
    ours: &mesh::Meshed,
) -> (
    Option<crate::render::Rgb>,
    BTreeMap<String, crate::render::Rgb>,
) {
    use crate::render as r;
    let Some(theirs) = theirs else {
        return (Some(r::COMPARE_NEW), BTreeMap::new());
    };
    let mut faces = BTreeMap::new();
    if theirs.hash != ours.hash {
        let d = crate::diff::bodies(&theirs.body, &ours.body);
        for (kind, color) in [("new", r::COMPARE_NEW), ("changed", r::COMPARE_CHANGED)] {
            for n in d["faces"][kind].as_array().into_iter().flatten() {
                if let Some(n) = n.as_str() {
                    faces.insert(n.to_owned(), color);
                }
            }
        }
    }
    (None, faces)
}

/// Adds `m`'s faces to `g` as ghosts, where `pose` puts them: those
/// `gone` says in the removed colour, the rest in the ghost colour.
fn ghost(
    g: &mut crate::render::Geometry,
    m: &mesh::Meshed,
    pose: &ryukei::geom::Rigid,
    gone: impl Fn(&str) -> bool,
) {
    use crate::render as r;
    let Ok(mesh) = &m.mesh.mesh else { return };
    let moved = *pose != ryukei::geom::Rigid::IDENTITY;
    let base = g.positions.len() as u32;
    g.positions.extend(mesh.positions.iter().map(|p| {
        let p = if moved { pose.apply_point(*p) } else { *p };
        [p.x, p.y, p.z]
    }));
    for f in &mesh.faces {
        let name = m.body.face(f.face).expect("live").name.to_string();
        let color = if gone(&name) {
            r::COMPARE_REMOVED
        } else {
            r::COMPARE_GHOST
        };
        for t in &mesh.triangles[f.triangles.start as usize..f.triangles.end as usize] {
            let c = t.map(|i| mesh.corners[i as usize]);
            g.ghosts.push(r::Triangle {
                p: c.map(|c| base + c.position),
                n: c.map(|c| {
                    let n = c.normal.get();
                    let n = if moved { pose.apply_vector(n) } else { n };
                    [n.x as f32, n.y as f32, n.z as f32]
                }),
                color,
            });
        }
    }
}

/// The names of the faces `theirs` has and `ours` doesn't.
fn gone_faces(theirs: &mesh::Meshed, ours: &mesh::Meshed) -> std::collections::BTreeSet<String> {
    let d = crate::diff::bodies(&theirs.body, &ours.body);
    d["faces"]["gone"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str().map(str::to_owned))
        .collect()
}

/// Solids' meshes by part and founder.
type SolidMeshes = BTreeMap<(Id, Id), mesh::Meshed>;

/// Each solid's mesh at a revision, by part and founder, for the parts
/// given (those the revision has), and the commit it names.
fn solids_at(
    session: &mut Session,
    rev: &str,
    parts: &[Id],
) -> Result<(Option<String>, SolidMeshes)> {
    let (commit, s) = session.at(Some(rev))?;
    let mut out = BTreeMap::new();
    for part in parts {
        if s.project().part(part).is_err() {
            continue;
        }
        let Ok(list) = meshes(s, part, None, mesh::Quality::Normal) else {
            continue;
        };
        for (fid, m) in list {
            if let Ok(m) = m
                && m.mesh.mesh.is_ok()
            {
                out.insert((part.clone(), fid), m);
            }
        }
    }
    Ok((commit, out))
}

/// An assembly's occurrences' solids at a revision, by occurrence path
/// and founder: where each is and its mesh.
type OccurrenceMeshes = BTreeMap<(String, Id), (ryukei::geom::Rigid, mesh::Meshed)>;

/// [`OccurrenceMeshes`] of `part` at `rev`: empty where the revision has
/// no such assembly or it doesn't build.
fn occurrences_at(session: &mut Session, rev: &str, part: &Id) -> Result<OccurrenceMeshes> {
    let (_, s) = session.at(Some(rev))?;
    let mut out = BTreeMap::new();
    let assembly = s
        .project()
        .part(part)
        .is_ok_and(|p| part_kind(p) == PartKind::Assembly);
    if !assembly {
        return Ok(out);
    }
    let Ok(model) = s.model(part, &crate::doc::assembly::Rev::Working) else {
        return Ok(out);
    };
    for o in &model.occurrences {
        let path = crate::assembly::path_text(&o.path);
        let solids: Vec<&crate::assembly::Solid> = o.solids.iter().collect();
        let made = occurrence_meshes(s, &solids, mesh::Quality::Normal);
        for (sol, m) in solids.iter().zip(made) {
            if m.mesh.is_err() {
                continue;
            }
            let m = mesh::Meshed {
                body: sol.built.body.clone(),
                hash: sol.built.hash.clone(),
                mesh: m,
            };
            out.insert((path.clone(), sol.founder.clone()), (o.pose, m));
        }
    }
    Ok(out)
}

/// The parts a render draws: the one asked for, or the project's model
/// ([`crate::assembly::shown_parts`]).
fn render_parts(session: &Session, part: Option<&str>) -> Result<Vec<Id>> {
    Ok(match part {
        Some(p) => {
            let id = part_id(p)?;
            session.project().part(&id)?;
            vec![id]
        }
        None => crate::assembly::shown_parts(session.project()),
    })
}

/// Renders the model. `target` names a participant whose camera to use
/// (with the live session to find it in, if any); `shown` are the
/// participants' highlights, drawn in their styles.
fn render(
    session: &mut Session,
    target: Option<(&str, Option<&Live>)>,
    shown: Vec<live::Highlight>,
    ask: RenderAsk,
) -> Result<Answer> {
    use crate::render::{self as r, Geometry, Options};
    r::check_size(ask.width, ask.height)?;
    let parts = render_parts(session.at(ask.rev.as_deref())?.1, ask.part.as_deref())?;
    // Other revisions' models only widen the framing.
    let mut extra: Vec<[f64; 3]> = Vec::new();
    for rev in &ask.frame {
        for m in solids_at(session, rev, &parts)?.1.values() {
            if let Ok(mesh) = &m.mesh.mesh {
                extra.extend(mesh.positions.iter().map(|p| [p.x, p.y, p.z]));
            }
        }
    }
    let other = match &ask.compare {
        Some(rev) => Some((rev.clone(), solids_at(session, rev, &parts)?)),
        None => None,
    };
    // A compare's assemblies: the other side's occurrences, and this
    // side's as they are drawn, to paint and ghost them (issue #135).
    let mut theirs_occurrences: BTreeMap<Id, OccurrenceMeshes> = BTreeMap::new();
    if let Some(rev) = &ask.compare {
        for part in &parts {
            theirs_occurrences.insert(part.clone(), occurrences_at(session, rev, part)?);
        }
    }
    let mut seen_occurrences: BTreeMap<(Id, String, Id), (ryukei::geom::Rigid, mesh::Meshed)> =
        BTreeMap::new();
    // The rendered side's solids, for the ghosts.
    let mut seen = SolidMeshes::new();
    let (commit, session) = session.at(ask.rev.as_deref())?;
    // What to colour, and how: every participant's highlights in their
    // styles, then the request's own as notes.
    let mut marks: BTreeMap<String, (r::Rgb, Option<String>)> = BTreeMap::new();
    for h in &shown {
        let color = match h.style {
            live::Style::Pending => r::PENDING,
            live::Style::Changed => r::CHANGED,
            live::Style::Note => r::NOTE,
        };
        for e in &h.entities {
            marks.insert(e.to_string(), (color, h.label.clone()));
        }
    }
    for e in entity::parse_all(session, &ask.highlight)? {
        marks.insert(e.to_string(), (r::NOTE, None));
    }
    let mut g = Geometry::default();
    // Renders don't follow anyone's display quality, so they compare
    // across machines.
    for part in &parts {
        // An assembly draws its occurrences where its instances put them.
        if part_kind(session.project().part(part)?) == PartKind::Assembly {
            let model = session.model(part, &crate::doc::assembly::Rev::Working)?;
            for o in &model.occurrences {
                let path = crate::assembly::path_text(&o.path);
                if ask.hidden.contains(&format!("{part}/{path}")) {
                    continue;
                }
                let solids: Vec<&crate::assembly::Solid> = o.solids.iter().collect();
                let made = occurrence_meshes(session, &solids, mesh::Quality::Normal);
                for (s, m) in solids.iter().zip(made) {
                    let m = mesh::Meshed {
                        body: s.built.body.clone(),
                        hash: s.built.hash.clone(),
                        mesh: m,
                    };
                    let key = (path.clone(), s.founder.clone());
                    let (whole, faces) = match theirs_occurrences.get(part) {
                        Some(theirs) => compare_paint(theirs.get(&key).map(|(_, t)| t), &m),
                        None => (None, BTreeMap::new()),
                    };
                    draw_body(
                        &mut g,
                        &m,
                        &o.pose,
                        &format!("{part}/{path}/{}", s.founder),
                        &marks,
                        (whole, &faces),
                        (ask.labels, ask.edges),
                    );
                    seen_occurrences.insert((part.clone(), key.0, key.1), (o.pose, m));
                }
            }
            continue;
        }
        for (fid, m) in meshes(session, part, None, mesh::Quality::Normal)? {
            let Ok(m) = m else { continue };
            if m.mesh.mesh.is_err() || ask.hidden.contains(&format!("{part}/{fid}")) {
                continue;
            }
            seen.insert((part.clone(), fid.clone()), m.clone());
            let (whole, faces) = match &other {
                Some((_, (_, theirs))) => {
                    compare_paint(theirs.get(&(part.clone(), fid.clone())), &m)
                }
                None => (None, BTreeMap::new()),
            };
            draw_body(
                &mut g,
                &m,
                &ryukei::geom::Rigid::IDENTITY,
                &format!("{part}/{fid}"),
                &marks,
                (whole, &faces),
                (ask.labels, ask.edges),
            );
        }
        if ask.sketches {
            sketches(session, part, &marks, ask.labels, &mut g)?;
        }
    }
    if let Some((_, (_, theirs))) = &other {
        for (key, t) in theirs {
            match seen.get(key) {
                Some(ours) if ours.hash == t.hash => {}
                Some(ours) => {
                    let gone = gone_faces(t, ours);
                    ghost(&mut g, t, &ryukei::geom::Rigid::IDENTITY, |n| {
                        gone.contains(n)
                    });
                }
                None => ghost(&mut g, t, &ryukei::geom::Rigid::IDENTITY, |_| true),
            }
        }
    }
    for (part, theirs) in &theirs_occurrences {
        for ((path, founder), (pose, t)) in theirs {
            // What this render leaves out leaves no ghost either.
            if ask.hidden.contains(&format!("{part}/{path}")) {
                continue;
            }
            match seen_occurrences.get(&(part.clone(), path.clone(), founder.clone())) {
                Some((p, ours)) if ours.hash == t.hash && p == pose => {}
                // Only moved: all of it where it was.
                Some((_, ours)) if ours.hash == t.hash => ghost(&mut g, t, pose, |_| false),
                Some((_, ours)) => {
                    let gone = gone_faces(t, ours);
                    ghost(&mut g, t, pose, |n| gone.contains(n));
                }
                None => ghost(&mut g, t, pose, |_| true),
            }
        }
    }
    if ask.origin {
        g.add_origin();
    }
    let camera = match (ask.camera, target, ask.view.as_deref()) {
        (Some(c), _, _) => c.resolve(session.project().manifest.default_unit)?,
        (None, Some((p, live)), _) => {
            let live =
                live.ok_or_else(|| live::no_live_session("render with a participant's camera"))?;
            live.get(p)?.view.camera.clone().ok_or_else(|| {
                Error::new(Code::InvalidParam, format!("participant {p} has no camera"))
                    .with("participant", p)
            })?
        }
        // A named view of the rendered part, as saved.
        (None, None, Some(v)) if StandardView::named(v).is_none() => {
            let part = match parts.as_slice() {
                [one] => Some(one),
                _ => None,
            };
            crate::views::named(session, part, v)?
        }
        (None, None, view) => {
            let view = view
                .map(StandardView::parse)
                .transpose()?
                .unwrap_or(StandardView::Iso);
            let points: Vec<[f64; 3]> = g.positions.iter().chain(&extra).copied().collect();
            if points.is_empty() {
                crate::view::frame([-0.5; 3], [0.5; 3], view, DEFAULT_PROJECTION)
            } else {
                let aspect = f64::from(ask.width) / f64::from(ask.height);
                crate::view::frame_points(&points, view, DEFAULT_PROJECTION, aspect)
            }
        }
    };
    let options = Options {
        silhouettes: ask.edges,
        triad: true,
    };
    let img = r::render(&g, &camera, ask.width, ask.height, options)?;
    let mut result =
        json!({"part": ask.part, "width": ask.width, "height": ask.height, "camera": camera});
    if let Some(c) = commit {
        result["commit"] = json!(c);
    }
    if let Some((rev, (commit, _))) = other {
        result["compare"] = json!({"rev": rev, "commit": commit});
    }
    Ok(Answer::bytes(result, "image/png", img.png()))
}

/// Draws one body into a render's geometry where `pose` puts it: its
/// faces in their marks' or paint's colours, its edges, and its marked
/// vertices, labelled when asked. Marks are keyed by `prefix/<name>`.
fn draw_body(
    g: &mut crate::render::Geometry,
    m: &mesh::Meshed,
    pose: &ryukei::geom::Rigid,
    prefix: &str,
    marks: &BTreeMap<String, (crate::render::Rgb, Option<String>)>,
    (whole, faces): (
        Option<crate::render::Rgb>,
        &BTreeMap<String, crate::render::Rgb>,
    ),
    (labels, edges): (bool, bool),
) {
    use crate::render::{self as r, Label, Line, Triangle};
    let Ok(mesh) = &m.mesh.mesh else { return };
    // A part's own bodies stay exactly as meshed.
    let moved = *pose != ryukei::geom::Rigid::IDENTITY;
    let positions: Vec<ryukei::geom::Point3> = mesh
        .positions
        .iter()
        .map(|p| if moved { pose.apply_point(*p) } else { *p })
        .collect();
    let base = g.positions.len() as u32;
    g.positions
        .extend(positions.iter().map(|p| [p.x, p.y, p.z]));
    let key = |name: String| format!("{prefix}/{name}");
    let label =
        |g: &mut crate::render::Geometry, name: &str, text: &Option<String>, pts: &[u32]| {
            if !labels || pts.is_empty() {
                return;
            }
            let n = pts.len() as f64;
            let mut at = [0.0; 3];
            for p in pts {
                let q = positions[*p as usize];
                at = [at[0] + q.x / n, at[1] + q.y / n, at[2] + q.z / n];
            }
            let text = text.clone().unwrap_or_else(|| name[..8].to_owned());
            g.labels.push(Label { at, text });
        };
    for f in &mesh.faces {
        let name = m.body.face(f.face).expect("live").name.to_string();
        let mark = marks.get(&key(name.clone()));
        // Highlights over the compare's paint, as in the app.
        let color = mark
            .map(|(c, _)| *c)
            .or(whole)
            .or_else(|| faces.get(&name).copied())
            .unwrap_or(r::SURFACE);
        for t in &mesh.triangles[f.triangles.start as usize..f.triangles.end as usize] {
            let c = t.map(|i| mesh.corners[i as usize]);
            g.triangles.push(Triangle {
                p: c.map(|c| base + c.position),
                n: c.map(|c| {
                    let n = c.normal.get();
                    let n = if moved { pose.apply_vector(n) } else { n };
                    [n.x as f32, n.y as f32, n.z as f32]
                }),
                color,
            });
        }
        if let Some((_, text)) = mark {
            let pts: Vec<u32> = mesh.corners[f.corners.start as usize..f.corners.end as usize]
                .iter()
                .map(|c| c.position)
                .collect();
            label(g, &name, text, &pts);
        }
    }
    for e in &mesh.edges {
        let name = m.body.edge(e.edge).expect("live").name.to_string();
        let points: Vec<u32> = e.positions.iter().map(|p| base + p).collect();
        match marks.get(&key(name.clone())) {
            Some((color, text)) => {
                label(g, &name, text, &e.positions);
                g.lines.push(Line {
                    points,
                    color: *color,
                    width: 3.0,
                    dash: false,
                });
            }
            None if edges => g.lines.push(Line {
                points,
                color: r::EDGE,
                width: 1.0,
                dash: false,
            }),
            None => {}
        }
    }
    for (i, v) in m.body.vertices().enumerate() {
        let name = m.body.vertex(v).expect("live").name.to_string();
        if let Some((color, text)) = marks.get(&key(name.clone())) {
            label(g, &name, text, &[i as u32]);
            g.lines.push(Line {
                points: vec![base + i as u32],
                color: *color,
                width: 7.0,
                dash: false,
            });
        }
    }
}

/// Draws `part`'s sketches into `g`: curves in their solve states'
/// colours (construction dashed), points as dots, highlighted ones in
/// their marks' colours, and with `labels` each dimension's label.
fn sketches(
    session: &mut Session,
    part: &Id,
    marks: &BTreeMap<String, (crate::render::Rgb, Option<String>)>,
    labels: bool,
    g: &mut crate::render::Geometry,
) -> Result<()> {
    use crate::render::{self as r, Label, Line};
    let unit = session.project().manifest.default_unit;
    let built = session.rebuilt(part)?;
    for (fid, out) in &built.features {
        let Some(sk) = out.as_ref().ok().and_then(|o| o.sketch()) else {
            continue;
        };
        let d = crate::sketch_view::drawing(sk, mesh::Quality::Normal.angle(), unit);
        let mark = |e: &Id| marks.get(&format!("{part}/{fid}/{e}")).map(|m| m.0);
        for c in &d.curves {
            let base = g.positions.len() as u32;
            g.positions.extend(&c.points);
            let (color, width) = match mark(&c.entity) {
                Some(m) => (m, 3.0),
                None => (r::sketch_color(c.state), 1.5),
            };
            g.lines.push(Line {
                points: (base..g.positions.len() as u32).collect(),
                color,
                width,
                dash: c.construction,
            });
        }
        for p in &d.dots {
            g.positions.push(p.at);
            let (color, width) = match mark(&p.entity) {
                Some(m) => (m, 7.0),
                None => (r::sketch_color(p.state), 4.0),
            };
            g.lines.push(Line {
                points: vec![g.positions.len() as u32 - 1],
                color,
                width,
                dash: false,
            });
        }
        if labels {
            g.labels.extend(d.labels.into_iter().map(|l| Label {
                at: l.at,
                text: l.text,
            }));
        }
    }
    Ok(())
}

/// The part, feature and quality a mesh request names.
fn mesh_args(
    part: &str,
    only: Option<&str>,
    quality: Option<&str>,
) -> Result<(Id, Option<Id>, mesh::Quality)> {
    let quality = match quality {
        Some(q) => mesh::Quality::parse(q)?,
        None => mesh::Quality::preferred(),
    };
    Ok((part_id(part)?, only.map(feature_id).transpose()?, quality))
}

/// What a `mesh.get` asks for: a solid or occurrence (`feature`), several
/// of an assembly's occurrences (`features`), or (`None`) everything.
fn wanted(feature: Option<&str>, features: &[String]) -> Option<BTreeSet<String>> {
    let all: BTreeSet<String> = feature
        .map(str::to_owned)
        .into_iter()
        .chain(features.iter().cloned())
        .collect();
    (!all.is_empty()).then_some(all)
}

/// The one solid a part's `mesh.get` names, if any: several are an
/// assembly's occurrences only.
fn one_of(only: Option<&BTreeSet<String>>) -> Result<Option<&str>> {
    match only.map(|w| w.iter().collect::<Vec<_>>()).as_deref() {
        None => Ok(None),
        Some([one]) => Ok(Some(one.as_str())),
        Some(_) => Err(Error::new(
            Code::InvalidParam,
            "features names several of an assembly's occurrences; a part's solid is feature",
        )
        .with("pointer", "/features")),
    }
}

/// Display meshes, each with its label.
type Labelled = Vec<(String, Result<mesh::Meshed>)>;

/// An assembly's display meshes: each body its occurrences place, once,
/// labelled by the first occurrence that places it (`<path>/<solid>`), or
/// only the occurrence at path `only`; and the occurrences chosen, which
/// say where each body goes.
fn assembly_meshes(
    session: &mut Session,
    pid: &Id,
    only: Option<&BTreeSet<String>>,
    quality: mesh::Quality,
) -> Result<(Labelled, Vec<Value>)> {
    let model = session.model(pid, &crate::doc::assembly::Rev::Working)?;
    let mut found = BTreeSet::new();
    let chosen: Vec<&crate::assembly::Occurrence> = model
        .occurrences
        .iter()
        .filter(|o| {
            only.is_none_or(|w| {
                let path = crate::assembly::path_text(&o.path);
                w.contains(&path) && found.insert(path)
            })
        })
        .collect();
    if let Some(p) = only.and_then(|w| w.iter().find(|p| !found.contains(*p))) {
        return Err(Error::new(
            Code::NotFound,
            format!("no occurrence {p} in assembly {pid}"),
        )
        .with("part", pid)
        .with("occurrence", p));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut solids: Vec<(String, &crate::assembly::Solid)> = Vec::new();
    for o in &chosen {
        for s in &o.solids {
            if seen.insert(s.built.hash.clone()) {
                let path = crate::assembly::path_text(&o.path);
                solids.push((format!("{path}/{}", s.founder), s));
            }
        }
    }
    let refs: Vec<&crate::assembly::Solid> = solids.iter().map(|(_, s)| *s).collect();
    let made = occurrence_meshes(session, &refs, quality);
    let bodies = solids
        .into_iter()
        .zip(made)
        .map(|((label, s), m)| {
            let m = mesh::Meshed {
                body: s.built.body.clone(),
                hash: s.built.hash.clone(),
                mesh: m,
            };
            (label, Ok(m))
        })
        .collect();
    let occurrences = chosen.iter().map(|o| o.to_json()).collect();
    Ok((bodies, occurrences))
}

/// Whether `part` is an assembly.
fn is_assembly(session: &Session, part: &str) -> bool {
    part_id(part)
        .ok()
        .and_then(|id| session.project().parts.get(&id).map(part_kind))
        == Some(PartKind::Assembly)
}

/// Meshes as bytes: `ryucad.mesh.1`, or `ryucad.mesh.2` when `compact`.
fn mesh_binary(
    session: &mut Session,
    part: &str,
    only: Option<&BTreeSet<String>>,
    quality: Option<&str>,
    compact: bool,
) -> Result<Answer> {
    fn bytes<K: AsRef<str>>(
        bodies: &[(K, Result<mesh::Meshed>)],
        compact: bool,
    ) -> (&'static str, Vec<u8>) {
        match compact {
            true => (mesh::COMPACT, mesh::encode_compact(bodies)),
            false => (mesh::ENCODING, mesh::encode(bodies)),
        }
    }
    if is_assembly(session, part) {
        let (pid, _, quality) = mesh_args(part, None, quality)?;
        let (bodies, occurrences) = assembly_meshes(session, &pid, only, quality)?;
        let result = json!({"part": pid, "quality": quality.as_str(), "occurrences": occurrences});
        let (encoding, b) = bytes(&bodies, compact);
        return Ok(Answer::bytes(result, encoding, b));
    }
    let (pid, only, quality) = mesh_args(part, one_of(only)?, quality)?;
    let bodies = meshes(session, &pid, only.as_ref(), quality)?;
    let result = json!({"part": pid, "quality": quality.as_str()});
    let (encoding, b) = bytes(&bodies, compact);
    Ok(Answer::bytes(result, encoding, b))
}

fn mesh_get(
    session: &mut Session,
    part: &str,
    only: Option<&BTreeSet<String>>,
    quality: Option<&str>,
) -> Result<Value> {
    if is_assembly(session, part) {
        let (pid, _, quality) = mesh_args(part, None, quality)?;
        let (bodies, occurrences) = assembly_meshes(session, &pid, only, quality)?;
        let bodies: Vec<Value> = bodies
            .into_iter()
            .filter_map(|(label, m)| {
                let m = m.ok()?;
                let mut v = mesh::to_json(&m.body, &m.mesh);
                v["feature"] = json!(label);
                v["hash"] = json!(m.hash);
                Some(v)
            })
            .collect();
        return Ok(
            json!({"part": pid, "quality": quality.as_str(), "bodies": bodies,
                         "occurrences": occurrences}),
        );
    }
    let (pid, only, quality) = mesh_args(part, one_of(only)?, quality)?;
    let bodies: Vec<Value> = meshes(session, &pid, only.as_ref(), quality)?
        .into_iter()
        .map(|(id, r)| match r {
            Ok(m) => {
                let mut v = mesh::to_json(&m.body, &m.mesh);
                v["feature"] = json!(id);
                v["hash"] = json!(m.hash);
                v
            }
            Err(e) => json!({"feature": id, "ok": false, "error": e}),
        })
        .collect();
    // Each body is a solid, by its founder (`feature`, as entity
    // references name it).
    Ok(json!({"part": pid, "quality": quality.as_str(), "bodies": bodies}))
}

/// `inspect` of an assembly: each occurrence (or the one at path `only`)
/// with its part, revision and pose and each solid's body by stable name;
/// with `entity`, a full reference, just that entity.
fn inspect_assembly(
    session: &mut Session,
    pid: &Id,
    only: Option<&str>,
    entity: Option<&str>,
) -> Result<Value> {
    let model = session.model(pid, &crate::doc::assembly::Rev::Working)?;
    let chosen: Vec<&crate::assembly::Occurrence> = model
        .occurrences
        .iter()
        .filter(|o| only.is_none_or(|p| crate::assembly::path_text(&o.path) == p))
        .collect();
    if let Some(p) = only
        && chosen.is_empty()
    {
        return Err(Error::new(
            Code::NotFound,
            format!("no occurrence {p} in assembly {pid}"),
        )
        .with("part", pid)
        .with("occurrence", p));
    }
    if let Some(given) = entity {
        let r = EntityRef::parse(given)?.in_assembly();
        if r.part != *pid || r.is_occurrence() {
            return Err(Error::new(
                Code::NotFound,
                format!("{given} isn't an entity of an occurrence of {pid}"),
            )
            .with("entity", given));
        }
        for o in &chosen {
            if o.path != r.path {
                continue;
            }
            for sd in o.solids.iter().filter(|sd| sd.founder == r.feature) {
                let b = crate::inspect::body(&sd.feature, &sd.built);
                for (list, kind) in [("faces", "face"), ("edges", "edge"), ("vertices", "vertex")] {
                    if let Some(e) = b[list]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|e| e["name"] == r.name.as_str())
                    {
                        return Ok(json!({"occurrence": crate::assembly::path_text(&o.path),
                                         "part": o.part, "solid": sd.founder,
                                         "kind": kind, "entity": e}));
                    }
                }
            }
        }
        return Err(Error::new(
            Code::NotFound,
            format!("no entity {given} in assembly {pid}"),
        )
        .with("entity", given));
    }
    let occurrences: Vec<Value> = chosen
        .iter()
        .map(|o| {
            let bodies: Vec<Value> = o
                .solids
                .iter()
                .map(|sd| {
                    let mut v = crate::inspect::body(&sd.feature, &sd.built);
                    v["solid"] = json!(sd.founder);
                    v
                })
                .collect();
            let mut v = o.to_json();
            v["bodies"] = json!(bodies);
            v
        })
        .collect();
    Ok(json!({"part": pid, "occurrences": occurrences}))
}

fn inspect(
    session: &mut Session,
    part: &str,
    only: Option<&str>,
    entity: Option<&str>,
) -> Result<Value> {
    if is_assembly(session, part) {
        return inspect_assembly(session, &part_id(part)?, only, entity);
    }
    let pid = part_id(part)?;
    let only = only.map(feature_id).transpose()?;
    if let Some(f) = &only {
        feature(session.project().part(&pid)?, f)?;
    }
    let features: Vec<(Id, Feature)> = session
        .project()
        .part(&pid)?
        .features
        .iter()
        .filter_map(|(id, f)| Some((id.clone(), f.as_ref().ok()?.clone())))
        .collect();
    let solids = Solids::of(session.project().part(&pid)?);
    let result = session.rebuilt(&pid)?;
    let of = |id: &Id| {
        &features
            .iter()
            .find(|(i, _)| i == id)
            .expect("built features loaded")
            .1
    };
    let mut bodies = Vec::new();
    let mut put = |id: &Id, solid: &Id, b: &crate::rebuild::Built| {
        let mut v = crate::inspect::body(of(id), b);
        v["feature"] = json!(id);
        v["solid"] = json!(solid);
        bodies.push(v);
    };
    match &only {
        // The solids a feature leaves, as it leaves them.
        Some(f) => {
            if let Some(out) = result.output(f) {
                for (solid, b) in out.solids() {
                    put(f, solid, b);
                }
            }
        }
        // The model: each solid as it shows.
        None => {
            for shown in result.shown(&solids) {
                if let Some((f, b)) = shown.state {
                    put(f, shown.solid, b);
                }
            }
        }
    }
    if let Some(given) = entity {
        // A bare name, or a full reference as `selection` and Copy
        // reference give it: then only its own part and feature count.
        let (name, in_feature) = if given.contains('/') {
            let r = crate::entity::EntityRef::parse(given)?;
            if r.part != pid {
                return Err(Error::new(
                    Code::NotFound,
                    format!("{given} is in part {}, not {pid}", r.part),
                )
                .with("entity", given));
            }
            (r.name, Some(r.feature))
        } else {
            (given.to_owned(), None)
        };
        // Just the one entity, wherever it is. A reference names its
        // solid.
        for b in &bodies {
            if in_feature
                .as_ref()
                .is_some_and(|f| b["solid"] != f.as_str())
            {
                continue;
            }
            for (list, kind) in [("faces", "face"), ("edges", "edge"), ("vertices", "vertex")] {
                if let Some(e) = b[list]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == name.as_str())
                {
                    return Ok(json!({"feature": b["feature"], "solid": b["solid"],
                                     "kind": kind, "entity": e}));
                }
            }
        }
        return Err(
            Error::new(Code::NotFound, format!("no entity {given} in part {pid}"))
                .with("entity", given),
        );
    }
    Ok(json!({"part": pid, "bodies": bodies}))
}

/// A revision's files, and how the answer names it: the working files
/// (`working`) or a commit.
fn files_at(session: &mut Session, rev: &str) -> Result<(Value, activity::Files)> {
    if rev == "working" {
        return Ok((json!({"rev": rev}), session.snapshot()));
    }
    let r = crate::git::revision(session.root(), rev)?;
    Ok((json!({"rev": rev, "commit": r.commit}), r.files))
}

/// Every part of a revision, rebuilt.
fn rebuilt_at(session: &mut Session, rev: &str) -> Result<BTreeMap<String, crate::diff::Built>> {
    let (_, s) = session.at(Some(rev))?;
    let ids: Vec<Id> = s.project().parts.keys().cloned().collect();
    let mut out = BTreeMap::new();
    for id in ids {
        let part = s.project().part(&id)?.clone();
        let r = s.rebuilt(&id)?;
        out.insert(id.to_string(), crate::diff::Built::of(&part, r));
    }
    Ok(out)
}

/// `diff`: what changed from one revision to another.
pub(crate) fn diff(
    session: &mut Session,
    from: &str,
    to: &str,
    model: bool,
    opts: &crate::diff::Options,
) -> Result<Value> {
    if let Some(p) = &opts.part {
        part_id(p)?;
    }
    let (a, fa) = files_at(session, from)?;
    let (b, fb) = files_at(session, to)?;
    let releases = crate::release::history(session)
        .map(|(_, r)| r)
        .unwrap_or_default();
    let mut v = crate::diff::structural(&fa, &fb, opts, &releases);
    v["from"] = a;
    v["to"] = b;
    if model {
        let from = rebuilt_at(session, from)?;
        let to = rebuilt_at(session, to)?;
        crate::diff::model(&mut v, &from, &to, opts);
        // Each imported file that differs, by product.
        if let Some(Value::Array(files)) = v.get_mut("imports") {
            for f in files {
                let path = f["path"].as_str().unwrap_or_default().to_owned();
                let (a, b) = (fa.get(&path), fb.get(&path));
                f["products"] =
                    crate::import::products_diff(&path, a.map(|x| &x[..]), b.map(|x| &x[..]));
            }
        }
    }
    Ok(v)
}

fn check_format(session: &Session) -> Result<Value> {
    let p = session.project();
    let root = &p.root;
    let rel = |path: &Path| crate::doc::project::relative(root, path);
    let mut non_canonical = Vec::new();
    let mut errors = Vec::new();
    let mut orphans = Vec::new();
    let mut check = |path: &Path| {
        if let Ok(text) = fs::read_to_string(path)
            && crate::doc::canonical::parse(&text).is_ok()
            && !crate::doc::canonical::is_canonical(&text)
        {
            non_canonical.push(rel(path));
        }
    };
    check(&root.join(MANIFEST));
    check(&root.join(crate::doc::project::PDM_CONFIG));
    for part in p.parts.values() {
        check(&part_path(root, &part.id));
        check(&crate::doc::project::properties_path(root, &part.id));
        if let Err(e) = &part.file {
            errors.push(json!(e));
        }
        match &part.parameters {
            Some(Ok(_)) => check(&parameters_path(root, &part.id)),
            Some(Err(e)) => errors.push(json!(e)),
            None => {}
        }
        for (fid, f) in &part.features {
            check(&feature_path(root, &part.id, fid));
            if let Err(e) = f {
                errors.push(json!(e));
            }
        }
        orphans.extend(
            part.orphans()
                .into_iter()
                .map(|f| rel(&feature_path(root, &part.id, f))),
        );
    }
    let clean =
        non_canonical.is_empty() && errors.is_empty() && orphans.is_empty() && p.unknown.is_empty();
    Ok(
        json!({"clean": clean, "non_canonical": non_canonical, "errors": errors,
              "orphans": orphans, "unknown": p.unknown}),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh project and its session.
    pub(crate) fn fresh() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path(), None).unwrap();
        let s = Session::open(dir.path()).unwrap();
        (dir, s)
    }

    pub(crate) fn call(s: &mut Session, method: &str, params: Value) -> Result<Value> {
        handle(s, Request::from_rpc(method, Some(params))?)
    }

    /// The pixels of a render of `s`'s project, RGB.
    pub(crate) fn render_pixels(dir: &std::path::Path, s: Session, params: Value) -> Vec<[u8; 3]> {
        let req = Request::from_rpc("render", Some(params)).unwrap();
        let a = handle_safely(&mut Some(s), None, None, dir, req).unwrap();
        let png = a.binary.unwrap();
        let mut r = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        buf[..info.buffer_size()]
            .chunks(4)
            .map(|p| [p[0], p[1], p[2]])
            .collect()
    }

    #[test]
    fn renders_draw_sketches() {
        use crate::render::{NOTE, SKETCH_WELL};
        let (dir, mut s) = crate::sketch::tests::with_sketch();
        crate::sketch::tests::edit(&mut s, crate::sketch::tests::rectangle_ops()).unwrap();
        let px = |c: crate::render::Rgb| c.map(|x| (x * 255.0 + 0.5) as u8);
        let count = |params: Value, c: [u8; 3]| {
            let s = Session::open(dir.path()).unwrap();
            render_pixels(dir.path(), s, params)
                .into_iter()
                .filter(|p| *p == c)
                .count()
        };
        let base = json!({"view": "top", "width": 120, "height": 90});
        // Fully defined: drawn in the well-defined colour.
        assert!(count(base.clone(), px(SKETCH_WELL)) > 50);
        let mut off = base.clone();
        off["sketches"] = json!(false);
        assert_eq!(count(off, px(SKETCH_WELL)), 0);
        // A highlighted sketch entity takes its highlight's colour.
        let mut lit = base;
        lit["highlight"] = json!(["p/s/l1"]);
        assert!(count(lit, px(NOTE)) > 20);
        drop(s);
    }

    #[test]
    fn init_once() {
        let dir = tempfile::tempdir().unwrap();
        let r = init(dir.path(), Some(Unit::In)).unwrap();
        assert_eq!(r["default_unit"], "in");
        assert_eq!(
            init(dir.path(), None).unwrap_err().code,
            Code::ProjectExists
        );
        let mut s = Session::open(dir.path()).unwrap();
        let e = call(&mut s, "project.init", json!({})).unwrap_err();
        assert_eq!(e.code, Code::ProjectExists);
    }

    #[test]
    fn parts() {
        let (_dir, mut s) = fresh();
        let r = call(&mut s, "part.create", json!({"part": "bracket"})).unwrap();
        assert_eq!(r["part"], "bracket");
        let rev = r["revision"].as_str().unwrap().to_owned();
        let e = call(&mut s, "part.create", json!({"part": "bracket"})).unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
        let e = call(&mut s, "part.create", json!({"part": "Bracket"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidId);
        let list = call(&mut s, "part.list", json!({})).unwrap();
        assert_eq!(
            list["parts"],
            json!([{"part": "bracket", "kind": "part", "features": 0, "revision": rev}])
        );
        assert_eq!(list["model"], json!(["bracket"]));
        // Once built, a part's summary: its hash and failures.
        let built = call(&mut s, "rebuild", json!({"part": "bracket"})).unwrap();
        let list = call(&mut s, "part.list", json!({})).unwrap();
        assert_eq!(list["parts"][0]["hash"], built["parts"][0]["hash"]);
        assert_eq!(list["parts"][0]["failed"], 0);
        let info = call(&mut s, "project.info", json!({})).unwrap();
        assert_eq!(info["parts"], json!(["bracket"]));
        assert_eq!(info["default_unit"], "mm");
        let e = call(
            &mut s,
            "part.delete",
            json!({"part": "bracket", "expected_revision": "nope"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["actual"].as_str()),
            (Code::Conflict, Some(rev.as_str()))
        );
        call(
            &mut s,
            "part.delete",
            json!({"part": "bracket", "expected_revision": rev}),
        )
        .unwrap();
        let e = call(&mut s, "part.delete", json!({"part": "bracket"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn assemblies_are_parts_of_their_own_kind() {
        let (dir, mut s) = fresh();
        let r = call(
            &mut s,
            "part.create",
            json!({"part": "frame", "kind": "assembly"}),
        )
        .unwrap();
        assert_eq!(r["kind"], "assembly");
        let text = fs::read_to_string(dir.path().join("parts/frame/part.json")).unwrap();
        assert!(text.contains("\"part_kind\": \"assembly\""), "{text}");
        assert!(text.contains("\"format\": 2"), "{text}");
        let list = call(&mut s, "part.list", json!({})).unwrap();
        assert_eq!(list["parts"][0]["kind"], "assembly");
        let e = call(
            &mut s,
            "part.create",
            json!({"part": "x", "kind": "drawing"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert!(!dir.path().join("parts/x").exists());
    }

    fn ids(v: &Value) -> Vec<&str> {
        v["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_str().unwrap())
            .collect()
    }

    fn with_part() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        (dir, s)
    }

    #[test]
    fn add_list_show() {
        let (dir, mut s) = with_part();
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "params": {"radius": 5, "height": "2 in"},
                   "placement": {"origin": [0, 0, "1 m"]}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["id"], "cylinder-1");
        assert_eq!(
            r["feature"]["params"]["radius"],
            json!({"value": "5 mm", "m": 0.005})
        );
        assert_eq!(
            r["feature"]["placement"]["origin"][2],
            json!({"value": "1 m", "m": 1.0})
        );
        call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "ball", "first": true})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "after": "ball"}),
        )
        .unwrap();
        let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        assert_eq!(ids(&list), ["ball", "sphere-1", "cylinder-1"]);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "ball"})).unwrap();
        assert_eq!(show["type"], "sphere");
        let list = call(&mut s, "part.list", json!({})).unwrap();
        assert_eq!(show["revision"], list["parts"][0]["revision"]);
        // The file on disk is canonical and explicit.
        let text = fs::read_to_string(dir.path().join("parts/p/features/cylinder-1.json")).unwrap();
        assert!(crate::doc::canonical::is_canonical(&text));
        assert!(text.contains("\"5 mm\""));
    }

    #[test]
    fn add_refuses_bad_input_before_writing() {
        let (dir, mut s) = with_part();
        for (params, code) in [
            (
                json!({"part": "p", "type": "prism"}),
                Code::UnknownFeatureType,
            ),
            (
                json!({"part": "p", "type": "sphere", "params": {}}),
                Code::InvalidParam,
            ),
            (
                json!({"part": "p", "type": "sphere", "params": {"radius": "1 parsec"}}),
                Code::InvalidQuantity,
            ),
            (
                json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [0, 0, 0]}}),
                Code::InvalidPlacement,
            ),
            (
                json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "after": "ghost"}),
                Code::NotFound,
            ),
            (
                json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "after": "x", "first": true}),
                Code::Usage,
            ),
            (
                json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "Bad"}),
                Code::InvalidId,
            ),
            (
                json!({"part": "q", "type": "sphere", "params": {"radius": 1}}),
                Code::NotFound,
            ),
        ] {
            let e = call(&mut s, "feature.add", params.clone()).unwrap_err();
            assert_eq!(e.code, code, "{params}");
        }
        assert_eq!(fs::read_dir(dir.path().join("parts/p")).unwrap().count(), 1);
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
    }

    #[test]
    fn x_follows_z_when_left_out() {
        let (_dir, mut s) = with_part();
        let r = call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [-2, 0, 0]}})).unwrap();
        assert_eq!(r["feature"]["placement"]["x"], json!([0, 1, 0]));
        let r = call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [0, 1, 0]}})).unwrap();
        assert_eq!(r["feature"]["placement"]["x"], json!([1, 0, 0]));
        // Given explicitly, a parallel x is still refused.
        let e = call(&mut s, "feature.add", json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "placement": {"z": [1, 0, 0], "x": [3, 0, 0]}})).unwrap_err();
        assert_eq!(e.code, Code::InvalidPlacement);
    }

    #[test]
    fn edit_merges() {
        let (_dir, mut s) = with_part();
        call(&mut s, "feature.add", json!({"part": "p", "type": "cone", "params": {"radius": 5, "height": 9, "top_radius": 2}, "id": "c"})).unwrap();
        let r = call(&mut s, "feature.edit", json!({"part": "p", "id": "c", "params": {"height": "1 cm", "top_radius": null}, "placement": {"x": [0, 1, 0]}})).unwrap();
        let f = &r["feature"];
        assert_eq!(f["params"]["height"]["value"], "1 cm");
        assert_eq!(f["params"]["radius"]["value"], "5 mm");
        assert!(f["params"].get("top_radius").is_none());
        assert_eq!(f["placement"]["x"], json!([0, 1, 0]));
        assert_eq!(f["placement"]["z"], json!([0, 0, 1]));
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "c", "params": {"radius": null}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn move_and_delete() {
        let (dir, mut s) = with_part();
        for id in ["a", "b", "c"] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": id}),
            )
            .unwrap();
        }
        let r = call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "a", "after": "c"}),
        )
        .unwrap();
        assert_eq!(r["features"], json!(["b", "c", "a"]));
        let r = call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "a", "first": true}),
        )
        .unwrap();
        assert_eq!(r["features"], json!(["a", "b", "c"]));
        for bad in [
            json!({"part": "p", "id": "a"}),
            json!({"part": "p", "id": "a", "after": "a"}),
        ] {
            assert_eq!(
                call(&mut s, "feature.move", bad).unwrap_err().code,
                Code::Usage
            );
        }
        call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap();
        let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        assert_eq!(ids(&list), ["a", "c"]);
        assert!(!dir.path().join("parts/p/features/b.json").exists());
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn writes_check_the_revision() {
        let (dir, mut s) = with_part();
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
        )
        .unwrap();
        let rev = r["revision"].as_str().unwrap().to_owned();
        // Someone else edits the feature on disk.
        let path = dir.path().join("parts/p/features/a.json");
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("\"1 mm\"", "\"2 mm\"");
        fs::write(&path, text).unwrap();
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "a", "params": {"radius": 3}, "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "a"})).unwrap();
        assert_eq!(show["params"]["radius"]["value"], "2 mm");
    }

    #[test]
    fn broken_features_are_listed_with_their_error() {
        let (dir, mut s) = with_part();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
        )
        .unwrap();
        fs::write(dir.path().join("parts/p/features/a.json"), "{").unwrap();
        fs::write(dir.path().join("parts/p/features/z.json"), "{}").unwrap();
        let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        assert_eq!(list["features"][0]["ok"], false);
        assert_eq!(list["features"][0]["error"]["code"], "invalid_json");
        assert_eq!(list["orphans"], json!(["z"]));
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "a", "params": {}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidJson);
    }

    #[test]
    fn features_that_fail_to_build_are_listed_failed() {
        // Issue #123: a file that loads but doesn't build was listed ok.
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let unit = |id: &str, x: i32| {
            json!({"part": "p", "type": "box", "id": id, "params": {"x": 10, "y": 10, "z": 10},
                   "placement": {"origin": [x, 0, 0]}})
        };
        call(&mut s, "feature.add", unit("a", 0)).unwrap();
        // A cut far from the box takes nothing: its file loads, but it
        // fails to build.
        let mut cut = unit("c", 100);
        cut["operation"] = json!("cut");
        call(&mut s, "feature.add", cut).unwrap();
        // One after it on the same solid is blocked by it.
        let mut d = unit("d", 5);
        d["operation"] = json!("cut");
        call(&mut s, "feature.add", d).unwrap();
        let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        let f = &list["features"];
        assert_eq!(f[0]["ok"], true);
        assert_eq!(f[1]["ok"], false);
        assert_eq!(f[1]["error"]["code"], "operation_no_effect");
        assert!(f[1].get("blocked").is_none());
        assert_eq!(f[2]["ok"], false);
        assert_eq!(f[2]["blocked"], true);
        // Moved onto the box, it builds.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "c", "placement": {"origin": [5, 5, 0]}}),
        )
        .unwrap();
        let list = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        assert!(
            list["features"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["ok"] == true)
        );
    }

    #[test]
    fn operations_through_the_protocol() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let add = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.add", f)
        };
        let unit = |id: &str, x: i32| {
            json!({"type": "box", "id": id, "params": {"x": 10, "y": 10, "z": 10},
                   "placement": {"origin": [x, 0, 0]}})
        };
        add(&mut s, unit("a", 0)).unwrap();
        // With one solid, a cut acts on it; the file says so.
        let mut cut = unit("c", 5);
        cut["operation"] = json!("cut");
        let r = add(&mut s, cut).unwrap();
        assert_eq!(r["feature"]["operation"], "cut");
        assert_eq!(r["feature"]["targets"], json!(["a"]));
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "c"})).unwrap();
        assert_eq!(show["targets"], json!(["a"]));
        // With two, it must say which.
        add(&mut s, unit("b", 20)).unwrap();
        let mut join = unit("j", 25);
        join["operation"] = json!("join");
        let e = add(&mut s, join.clone()).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["solids"], json!(["a", "b"]));
        join["targets"] = json!(["b"]);
        add(&mut s, join).unwrap();
        // What isn't a solid, or comes after, is refused.
        let mut bad = unit("x", 0);
        bad["operation"] = json!("cut");
        bad["targets"] = json!(["c"]);
        let e = add(&mut s, bad.clone()).unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        assert_eq!(e.details["solids"], json!(["a"]));
        bad["targets"] = json!(["b"]);
        bad["first"] = json!(true);
        assert_eq!(add(&mut s, bad).unwrap_err().code, Code::ReferenceForward);
        // A founder that stops founding leaves what acts on it with nothing.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "operation": "join", "targets": ["a"]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::HasDependents, "{e}");
        assert_eq!(e.details["features"], json!(["j"]));
        // A cut made new again drops its targets.
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "c", "operation": "new"}),
        )
        .unwrap();
        assert!(r["feature"].get("targets").is_none());
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "c", "operation": "cut", "targets": ["a"]}),
        )
        .unwrap();
        // A boolean takes its target as target, or as targets' one entry.
        let r = add(
            &mut s,
            json!({"type": "boolean", "id": "u", "op": "unite", "targets": ["a"],
                   "tools": ["b"]}),
        )
        .unwrap();
        assert_eq!(r["feature"]["target"], "a");
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "u"})).unwrap();
        assert_eq!(show["tools"], json!(["b"]));
        let e = call(&mut s, "rebuild", json!({"part": "p"}));
        assert!(e.is_ok(), "{e:?}");
        // After it, b is the boolean's: nothing acts on it.
        let mut late = unit("late", 22);
        late["operation"] = json!("cut");
        late["targets"] = json!(["b"]);
        let e = add(&mut s, late).unwrap_err();
        assert_eq!(e.code, Code::BodyConsumed, "{e}");
        assert_eq!(e.details["consumed_by"], "u");
        // Nor can the boolean move before what acts on its tools.
        let e = call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "u", "after": "b"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::BodyConsumed, "{e}");
        // Fields a type doesn't take are refused.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "u", "operation": "cut"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = add(
            &mut s,
            json!({"type": "boolean", "op": "unite", "target": "a",
                                   "targets": ["a"], "tools": ["c"]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // A founder others act on can't go.
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "a"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
    }

    #[test]
    fn fillets_and_chamfers_through_the_protocol() {
        use crate::sketch::tests::{edge_between, face_along, vertex_at, with_box};
        let (dir, mut s, body) = with_box();
        let add = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.add", f)
        };
        let edit = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.edit", f)
        };
        let edge = |a: [f64; 3], b: [f64; 3]| {
            let (a, b) = (vertex_at(&body, a), vertex_at(&body, b));
            format!("p/b/{}", edge_between(&body, &a, &b))
        };
        // The box's vertical edge at its far corner, and its top front edge.
        let up = edge([0.02, 0.01, 0.0], [0.02, 0.01, 0.005]);
        let front = edge([0.0, 0.0, 0.005], [0.02, 0.0, 0.005]);
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        let r = add(
            &mut s,
            json!({"type": "fillet", "id": "f", "radius": "1 mm", "edges": [up]}),
        )
        .unwrap();
        assert_eq!(
            r["feature"]["groups"],
            json!([{"radius": {"value": "1 mm", "m": 0.001}, "edges": [up]}])
        );
        // The file keeps the reference as {feature, name}, and no default.
        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("parts/p/features/f.json")).unwrap(),
        )
        .unwrap();
        let name = up.rsplit('/').next().unwrap();
        assert_eq!(
            file["groups"][0]["edges"],
            json!([{"feature": "b", "name": name}])
        );
        assert!(file.get("propagate").is_none());
        assert_eq!(file["format"], 3);
        // One group's fields edit it; propagate is written only when false.
        let r = edit(&mut s, json!({"id": "f", "radius": 2, "propagate": false})).unwrap();
        assert_eq!(r["feature"]["groups"][0]["radius"]["value"], "2 mm");
        assert_eq!(r["feature"]["propagate"], false);
        let r = edit(&mut s, json!({"id": "f", "propagate": true})).unwrap();
        assert!(r["feature"].get("propagate").is_none());
        // Several sizes are several groups, given whole.
        let groups = json!([{"radius": "2 mm", "edges": [up]},
                            {"radius": "0.5 mm", "edges": [front]}]);
        let r = edit(&mut s, json!({"id": "f", "groups": groups})).unwrap();
        assert_eq!(r["feature"]["groups"][1]["edges"], json!([front]));
        let e = edit(&mut s, json!({"id": "f", "radius": 3})).unwrap_err();
        assert_eq!(
            (e.code, e.details["param"].as_str()),
            (Code::InvalidParam, Some("groups"))
        );
        let e = edit(&mut s, json!({"id": "f", "radius": 3, "groups": groups})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // A chamfer: equal unless it says otherwise; a new form keeps only
        // what it takes.
        // (The left back vertical edge, clear of the fillet: where a chamfer
        // would meet it the kernel has no corner yet.)
        let back = edge([0.0, 0.01, 0.0], [0.0, 0.01, 0.005]);
        let r = add(
            &mut s,
            json!({"type": "chamfer", "id": "c", "d": "0.5 mm", "edges": [back]}),
        )
        .unwrap();
        assert_eq!(r["feature"]["groups"][0]["form"], "equal");
        let r = edit(
            &mut s,
            json!({"id": "c", "form": "distance_angle", "angle": 30, "face": top}),
        )
        .unwrap();
        let g = &r["feature"]["groups"][0];
        assert_eq!(g["d"]["value"], "0.5 mm");
        assert_eq!(g["angle"]["value"], "30 deg");
        assert_eq!(g["face"], top);
        let r = edit(&mut s, json!({"id": "c", "form": "equal", "d": 1})).unwrap();
        assert!(r["feature"]["groups"][0].get("face").is_none());
        call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        // What can't be blended now is refused before anything is written.
        let e = add(
            &mut s,
            json!({"type": "fillet", "radius": 1, "edges": [top]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        // The fillet took the corner edge: naming it after says so.
        let e = add(
            &mut s,
            json!({"type": "fillet", "radius": 1, "edges": [up]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceUnresolved, "{e}");
        assert_eq!(e.details["removed_by"], "f");
        let e = add(
            &mut s,
            json!({"type": "fillet", "after": "b",
                   "groups": [{"radius": 1, "edges": [up]}, {"radius": 2, "edges": [up]}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::EdgeListedTwice, "{e}");
        add(
            &mut s,
            json!({"type": "box", "id": "o", "params": {"x": 1, "y": 1, "z": 1},
                   "placement": {"origin": [30, 0, 0]}}),
        )
        .unwrap();
        let other = {
            let o = call(&mut s, "inspect", json!({"part": "p", "feature": "o"})).unwrap();
            let o = &o["bodies"][0];
            let (a, b) = (
                vertex_at(o, [0.03, 0.0, 0.0]),
                vertex_at(o, [0.03, 0.0, 0.001]),
            );
            format!("p/o/{}", edge_between(o, &a, &b))
        };
        let e = add(
            &mut s,
            json!({"type": "fillet", "radius": 0.1, "edges": [front, other]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::TargetsMixed, "{e}");
        // Fields a fillet doesn't take, and a box's founder others round.
        let e = add(
            &mut s,
            json!({"type": "fillet", "radius": 1, "d": 1, "edges": [front]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "b"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
        assert_eq!(e.details["features"], json!(["f", "c"]));
    }

    #[test]
    fn holes_through_the_protocol() {
        use crate::sketch::tests::{edge_between, face_along, vertex_at};
        let (dir, mut s) = crate::hole::tests::drilled();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"]
            [0]
        .clone();
        let add = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.add", f)
        };
        let edit = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            f["id"] = json!("h");
            call(s, "feature.edit", f)
        };
        // A hole cuts the part's one solid when it names none; the file
        // names it all the same, and writes no defaults.
        let r = add(
            &mut s,
            json!({"type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
                   "diameter": 4, "depth": {"blind": 3}}),
        )
        .unwrap();
        let h = &r["feature"];
        assert_eq!(h["targets"], json!(["b"]));
        assert_eq!(h["diameter"], json!({"value": "4 mm", "m": 0.004}));
        assert_eq!(h["depth"], json!({"blind": {"value": "3 mm", "m": 0.003}}));
        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("parts/p/features/h.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (&file["format"], &file["form"], &file["points"]),
            (&json!(3), &json!("simple"), &json!(["p1"]))
        );
        assert!(file.get("bottom").is_none());
        // A counterbore needs its own fields; given in part, it keeps the
        // rest; a new form drops what it doesn't take.
        let e = edit(&mut s, json!({"form": "counterbore"})).unwrap_err();
        assert_eq!(e.details["pointer"], "/counterbore", "{e}");
        let r = edit(
            &mut s,
            json!({"form": "counterbore", "counterbore": {"diameter": 8, "depth": 1}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["counterbore"]["diameter"]["value"], "8 mm");
        let r = edit(&mut s, json!({"counterbore": {"depth": 2}})).unwrap();
        let c = &r["feature"]["counterbore"];
        assert_eq!(
            (&c["diameter"]["value"], &c["depth"]["value"]),
            (&json!("8 mm"), &json!("2 mm"))
        );
        let r = edit(&mut s, json!({"form": "simple"})).unwrap();
        assert!(r["feature"].get("counterbore").is_none());
        // A drill point, then flat again; every free point, then p1 again.
        let r = edit(&mut s, json!({"bottom": {"drill": 118}})).unwrap();
        assert_eq!(r["feature"]["bottom"]["drill"]["value"], "118 deg");
        let r = edit(&mut s, json!({"bottom": "flat", "points": []})).unwrap();
        assert!(r["feature"].get("bottom").is_none() && r["feature"].get("points").is_none());
        // Up to the box's bottom face, given as text and kept as a file
        // holds it.
        let bottom = format!("p/b/{}", face_along(&body, [0.0, 0.0, -1.0]));
        let r = edit(
            &mut s,
            json!({"points": ["p1"], "depth": {"up_to": {"target": bottom, "offset": "-1"}}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["depth"]["up_to"]["target"], bottom);
        assert_eq!(r["feature"]["depth"]["up_to"]["offset"]["value"], "-1 mm");
        let file: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("parts/p/features/h.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(file["depth"]["up_to"]["target"]["feature"], "b");
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        assert!(
            r["parts"][0]["features"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["ok"] == true),
            "{r}"
        );
        // What can be judged now is refused before anything is written.
        let edge = {
            let (a, b) = (
                vertex_at(&body, [0.0, 0.0, 0.0]),
                vertex_at(&body, [0.02, 0.0, 0.0]),
            );
            format!("p/b/{}", edge_between(&body, &a, &b))
        };
        for (f, code) in [
            (
                json!({"depth": {"up_to": {"target": edge}}}),
                Code::ReferenceWrongKind,
            ),
            (json!({"points": ["p9"]}), Code::NotFound),
            (json!({"points": ["l1"]}), Code::ReferenceWrongKind),
            (json!({"sketch": "b"}), Code::ReferenceWrongKind),
            (json!({"radius": 1}), Code::InvalidParam),
        ] {
            let e = edit(&mut s, f.clone()).unwrap_err();
            assert_eq!(e.code, code, "{f}: {e}");
        }
        // What it drills at, and its sketch, stay while it does.
        let e = call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": "p1"}]}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["features"]),
            (Code::HasDependents, &json!(["h"]))
        );
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "s"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
        // With two solids a hole names its targets.
        add(
            &mut s,
            json!({"type": "box", "id": "o", "params": {"x": 1, "y": 1, "z": 1},
                   "placement": {"origin": [30, 0, 0]}}),
        )
        .unwrap();
        let e = add(
            &mut s,
            json!({"type": "hole", "sketch": "s", "diameter": 1, "depth": "through_all"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["param"]),
            (Code::InvalidParam, &json!("targets"))
        );
    }

    #[test]
    fn patterns_through_the_protocol() {
        use crate::sketch::tests::{face_along, vertex_at};
        let (dir, mut s) = crate::hole::tests::drilled();
        let body = call(&mut s, "inspect", json!({"part": "p", "feature": "b"})).unwrap()["bodies"]
            [0]
        .clone();
        let add = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.add", f)
        };
        let edit = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            f["id"] = json!("pat");
            call(s, "feature.edit", f)
        };
        let file = |dir: &tempfile::TempDir| -> Value {
            serde_json::from_str(
                &std::fs::read_to_string(dir.path().join("parts/p/features/pat.json")).unwrap(),
            )
            .unwrap()
        };
        add(
            &mut s,
            json!({"type": "hole", "id": "h", "sketch": "s", "points": ["p1"],
                   "diameter": 2, "depth": {"blind": 3}}),
        )
        .unwrap();
        // A pattern acts on its seeds' targets when it names none.
        let r = add(
            &mut s,
            json!({"type": "pattern", "id": "pat", "features": ["h"], "direction": "x",
                   "spacing": 3, "count": 3}),
        )
        .unwrap();
        let f = &r["feature"];
        assert_eq!(f["targets"], json!(["b"]));
        assert_eq!(f["count"], json!({"value": "3", "n": 3.0}));
        assert_eq!(f["spacing"], json!({"value": "3 mm", "m": 0.003}));
        assert_eq!(
            (&file(&dir)["form"], &file(&dir)["format"]),
            (&json!("linear"), &json!(3))
        );
        // A second direction, a skip, then none of either again.
        let r = edit(
            &mut s,
            json!({"direction2": "y", "spacing2": 3, "count2": 2, "reverse2": true,
                   "skip": [[1, 1]]}),
        )
        .unwrap();
        assert_eq!(r["feature"]["skip"], json!([[1, 1]]));
        assert_eq!(r["feature"]["reverse2"], true);
        call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let e = edit(&mut s, json!({"direction2": ""})).unwrap_err();
        assert_eq!(e.details["pointer"], "/skip/0", "{e}");
        let r = edit(&mut s, json!({"direction2": "", "skip": []})).unwrap();
        assert!(r["feature"].get("direction2").is_none() && r["feature"].get("skip").is_none());
        // A new form keeps only what it takes: round the z axis, then a
        // mirror in the box's top face, given as text.
        let r = edit(
            &mut s,
            json!({"form": "circular", "axis": "z", "count": 4, "angle": 90}),
        )
        .unwrap();
        assert!(r["feature"].get("direction").is_none());
        assert_eq!(r["feature"]["angle"]["value"], "90 deg");
        assert_eq!(r["feature"]["features"], json!(["h"]));
        let top = format!("p/b/{}", face_along(&body, [0.0, 0.0, 1.0]));
        let r = edit(&mut s, json!({"form": "mirror", "plane": top})).unwrap();
        assert_eq!(r["feature"]["plane"], top);
        assert!(r["feature"].get("count").is_none());
        assert_eq!(file(&dir)["plane"]["feature"], "b");
        // What can be judged now is refused before anything is written.
        let vertex = format!("p/b/{}", vertex_at(&body, [0.0, 0.0, 0.0]));
        for (f, code) in [
            (json!({"features": ["b"]}), Code::ReferenceWrongKind),
            (json!({"plane": vertex}), Code::ReferenceWrongKind),
            (json!({"features": ["nope"]}), Code::NotFound),
            (json!({"radius": 1}), Code::InvalidParam),
            (
                json!({"form": "linear", "direction": "w", "spacing": 1, "count": 2}),
                Code::InvalidDocument,
            ),
        ] {
            let e = edit(&mut s, f.clone()).unwrap_err();
            assert_eq!(e.code, code, "{f}: {e}");
        }
        // A new pattern whose seed can't give it targets says why (#102).
        for (seed, code) in [("nope", Code::NotFound), ("b", Code::ReferenceWrongKind)] {
            let e = add(
                &mut s,
                json!({"type": "pattern", "id": "bad", "features": [seed], "direction": "x",
                       "spacing": 3, "count": 3}),
            )
            .unwrap_err();
            assert_eq!(
                (e.code, &e.details["seed"], &e.details["pointer"]),
                (code, &json!(seed), &json!("/features/0")),
                "{seed}: {e}"
            );
        }
        // What a form needs and wasn't given is a parameter to give (#98).
        for (f, param) in [
            (json!({"axis": "z", "count": 4}), "angle"),
            (json!({"axis": "z", "angle": 90}), "count"),
            (
                json!({"form": "linear", "spacing": 3, "count": 3}),
                "direction",
            ),
            (json!({"direction": "x", "count": 3}), "spacing"),
            (json!({"form": "mirror"}), "plane"),
        ] {
            let mut f = f;
            f["type"] = json!("pattern");
            f["features"] = json!(["h"]);
            let e = add(&mut s, f.clone()).unwrap_err();
            assert_eq!(
                (e.code, &e.details["param"]),
                (Code::InvalidParam, &json!(param)),
                "{f}: {e}"
            );
        }
        // A new pattern's form, left out, is what its fields say.
        let r = add(
            &mut s,
            json!({"type": "pattern", "id": "turn", "features": ["h"], "axis": "z",
                   "count": 2, "angle": 90}),
        )
        .unwrap();
        assert_eq!(r["feature"]["form"], "circular");
        call(&mut s, "feature.delete", json!({"part": "p", "id": "turn"})).unwrap();
        // A seed acting elsewhere than the pattern does.
        add(
            &mut s,
            json!({"type": "box", "id": "o", "params": {"x": 1, "y": 1, "z": 1},
                   "placement": {"origin": [30, 0, 0]}, "first": true}),
        )
        .unwrap();
        let e = edit(&mut s, json!({"targets": ["o"]})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidParam, &json!("/targets"))
        );
        // Its seeds stay while it repeats them.
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "h"})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["features"]),
            (Code::HasDependents, &json!(["pat"]))
        );
    }

    #[test]
    fn the_model_is_its_solids() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let add = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.add", f).unwrap()
        };
        add(
            &mut s,
            json!({"type": "box", "id": "a", "params": {"x": 10, "y": 10, "z": 10}}),
        );
        add(
            &mut s,
            json!({"type": "box", "id": "c", "operation": "cut",
                   "params": {"x": 4, "y": 20, "z": 20}, "placement": {"origin": [8, -5, -5]}}),
        );
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let part = &r["parts"][0];
        let solid = &part["solids"][0];
        assert_eq!(part["solids"].as_array().unwrap().len(), 1);
        assert_eq!(solid["solid"], "a");
        assert_eq!(solid["features"], json!(["a", "c"]));
        assert_eq!((&solid["ok"], &solid["shown"]), (&json!(true), &json!("c")));
        assert_eq!(part["features"][1]["solids"], json!(["a"]));
        assert_eq!(part["features"][1]["operation"], "cut");
        assert!(part["features"][0].get("operation").is_none());
        assert_eq!(solid["hash"], part["features"][1]["hash"]);
        // Meshes are the solids', named by founder.
        let m = call(&mut s, "mesh.get", json!({"part": "p"})).unwrap();
        assert_eq!(m["bodies"].as_array().unwrap().len(), 1);
        assert_eq!(m["bodies"][0]["feature"], "a");
        assert_eq!(m["bodies"][0]["hash"], solid["hash"]);
        let e = call(&mut s, "mesh.get", json!({"part": "p", "feature": "c"})).unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind);
        assert_eq!(e.details["solids"], json!(["a"]));
        // Inspect shows the model, or any feature's state.
        let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        let b = &v["bodies"][0];
        assert_eq!((&b["solid"], &b["feature"]), (&json!("a"), &json!("c")));
        let cut_face = b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["origin"]["feature"] == "c")
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned();
        let own = call(&mut s, "inspect", json!({"part": "p", "feature": "a"})).unwrap();
        assert_eq!(own["bodies"][0]["counts"]["faces"], 6);
        // A reference names the solid, and finds what the model shows.
        let one = call(
            &mut s,
            "inspect",
            json!({"part": "p", "entity": format!("p/a/{cut_face}")}),
        )
        .unwrap();
        assert_eq!((&one["solid"], &one["kind"]), (&json!("a"), &json!("face")));
        let mut live = Live::new();
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human"}),
        )
        .unwrap()["participant"]
            .as_str()
            .unwrap()
            .to_owned();
        let picked = format!("p/a/{cut_face}");
        let r = call_live(
            &mut s,
            &mut live,
            Some(&human),
            "selection.set",
            json!({"entities": [picked]}),
        )
        .unwrap();
        assert_eq!(r["selection"], json!([picked]));
        // The cut's own ID names no solid.
        let e = call_live(
            &mut s,
            &mut live,
            Some(&human),
            "selection.set",
            json!({"entities": [format!("p/c/{cut_face}")]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        assert_eq!(e.details["errors"][0]["details"]["solids"], json!(["a"]));
        // A tip that fails leaves the solid showing its last good state,
        // and failed.
        add(
            &mut s,
            json!({"type": "box", "id": "miss", "operation": "cut", "targets": ["a"],
                   "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [50, 0, 0]}}),
        );
        let e = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
        let solid = &e.details["parts"][0]["solids"][0];
        assert_eq!(
            (&solid["ok"], &solid["shown"]),
            (&json!(false), &json!("c"))
        );
        assert_eq!(solid["error"]["code"], "operation_no_effect");
        let m = call(&mut s, "mesh.get", json!({"part": "p"})).unwrap();
        assert_eq!(m["bodies"][0]["hash"], solid["hash"]);
    }

    #[test]
    fn rebuild_and_inspect() {
        let (_dir, mut s) = with_part();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "params": {"x": 1, "y": 2, "z": 3}, "id": "b"}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "s"}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({})).unwrap();
        let part = &r["parts"][0];
        assert_eq!(part["failed"], 0);
        assert_eq!(part["features"][1]["type"], "sphere");
        assert_eq!(
            part["features"][0]["counts"],
            json!({"faces": 6, "edges": 12, "vertices": 8})
        );
        let all = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        assert_eq!(all["bodies"].as_array().unwrap().len(), 2);
        let one = call(&mut s, "inspect", json!({"part": "p", "feature": "s"})).unwrap();
        assert_eq!(one["bodies"][0]["feature"], "s");
        let name = all["bodies"][0]["edges"][3]["name"].as_str().unwrap();
        let e = call(&mut s, "inspect", json!({"part": "p", "entity": name})).unwrap();
        assert_eq!(
            (e["feature"].as_str(), e["kind"].as_str()),
            (Some("b"), Some("edge"))
        );
        let e = call(&mut s, "inspect", json!({"part": "p", "entity": "0"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        for (list, kind) in [("faces", "face"), ("edges", "edge"), ("vertices", "vertex")] {
            let name = all["bodies"][0][list][0]["name"].as_str().unwrap();
            let e = call(&mut s, "inspect", json!({"part": "p", "entity": name})).unwrap();
            assert_eq!(e["kind"], kind);
            // The full reference, as Copy reference and `selection` give it.
            let full = format!("p/b/{name}");
            let e = call(&mut s, "inspect", json!({"part": "p", "entity": full})).unwrap();
            assert_eq!(
                (e["kind"].as_str(), e["feature"].as_str()),
                (Some(kind), Some("b"))
            );
        }
        let elsewhere = format!("q/b/{name}");
        let e = call(&mut s, "inspect", json!({"part": "p", "entity": elsewhere})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let wrong_feature = format!("p/s/{name}");
        let e = call(
            &mut s,
            "inspect",
            json!({"part": "p", "entity": wrong_feature}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    /// Part `p` with parameter `width` and the rectangle sketch `s`,
    /// written as a file.
    pub(crate) fn with_sketch() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = with_part();
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"width": "20 mm"}}),
        )
        .unwrap();
        let root = dir.path();
        write_json(
            &root.join("parts/p/features/s.json"),
            &crate::doc::fixtures::rectangle(),
        )
        .unwrap();
        write_json(
            &root.join("parts/p/part.json"),
            &PartFile::new(vec![Id::parse("s", "feature").unwrap()]).to_json(),
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn sketches_are_added_on_a_plane() {
        let (dir, mut s) = with_part();
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch"}),
        )
        .unwrap();
        assert_eq!(
            r["feature"],
            json!({"id": "sketch-1", "type": "sketch", "plane": "xy",
                   "entities": {}, "constraints": {}})
        );
        let text = fs::read_to_string(dir.path().join("parts/p/features/sketch-1.json")).unwrap();
        assert!(text.contains("\"format\": 2"));
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "side", "plane": "zx"}),
        )
        .unwrap();
        assert_eq!(r["feature"]["plane"], "zx");
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "up",
                   "placement": {"origin": [0, 0, "5 mm"]}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["placement"]["origin"][2]["m"], 0.005);
        assert!(r["feature"].get("plane").is_none());
        // Moved to another plane, and off it again.
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "side", "placement": {"origin": [0, "1 mm", 0]}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["placement"]["z"], json!([0, 1, 0]));
        assert_eq!(r["feature"]["placement"]["x"], json!([0, 0, 1]));
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "side", "plane": "yz"}),
        )
        .unwrap();
        assert_eq!(r["feature"]["plane"], "yz");
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "side"})).unwrap();
        assert_eq!(show["plane"], "yz");
        for (method, params, code) in [
            (
                "feature.add",
                json!({"type": "sketch", "plane": "xz"}),
                Code::InvalidParam,
            ),
            (
                "feature.add",
                json!({"type": "sketch", "params": {"x": 1}}),
                Code::InvalidParam,
            ),
            (
                "feature.add",
                json!({"type": "sketch", "plane": "xy",
                                   "placement": {"z": [0, 0, 1]}}),
                Code::Usage,
            ),
            (
                "feature.add",
                json!({"type": "sphere", "params": {"radius": 1},
                                   "plane": "xy"}),
                Code::InvalidParam,
            ),
            (
                "feature.edit",
                json!({"id": "side", "params": {"x": 1}}),
                Code::InvalidParam,
            ),
        ] {
            let mut params = params;
            params["part"] = json!("p");
            let e = call(&mut s, method, params.clone()).unwrap_err();
            assert_eq!(e.code, code, "{params}");
        }
    }

    #[test]
    fn sketches_rebuild_to_their_state() {
        let (dir, mut s) = with_sketch();
        let r = call(&mut s, "rebuild", json!({})).unwrap();
        let f = &r["parts"][0]["features"][0];
        assert_eq!(
            (&f["type"], &f["ok"], &f["sketch"]),
            (
                &json!("sketch"),
                &json!(true),
                &json!({"state": "under", "dof": 4, "stale": false})
            )
        );
        // A sketch has no body: no hash, no mesh, nothing to inspect.
        assert!(f.get("hash").is_none());
        assert!(r.get("warnings").is_none());
        let m = call(&mut s, "mesh.get", json!({"part": "p"})).unwrap();
        assert_eq!(m["bodies"], json!([]));
        let i = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        assert_eq!(i["bodies"], json!([]));
        // A parameter edited behind ryucad's back makes the sketch stale.
        let params =
            |w: &str| json!({"format": 1, "kind": "ryucad.parameters", "parameters": {"width": w}});
        let path = dir.path().join("parts/p/parameters.json");
        write_json(&path, &params("15 mm")).unwrap();
        let r = call(&mut s, "rebuild", json!({})).unwrap();
        assert_eq!(r["parts"][0]["features"][0]["sketch"]["stale"], true);
        assert_eq!(r["warnings"][0]["code"], "sketch_stale");
        assert_eq!(r["warnings"][0]["feature"], "s");
        // One it can't solve fails alone.
        write_json(&path, &params("50 mm")).unwrap();
        let e = call(&mut s, "rebuild", json!({})).unwrap_err();
        let f = &e.details["parts"][0]["features"][0];
        assert_eq!(f["error"]["code"], "sketch_unsolved");
    }

    /// The rebuild report of feature `f` in part `p`.
    fn built(s: &mut Session, f: &str) -> Value {
        let r = match call(s, "rebuild", json!({"part": "p"})) {
            Ok(r) => r,
            Err(e) => e.details,
        };
        r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == f)
            .unwrap()
            .clone()
    }

    #[test]
    fn sketches_extrude_and_revolve_into_bodies() {
        let (dir, mut s) = with_sketch();
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "sketch": "s",
                   "extent": {"type": "blind", "length": "width / 4"}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["id"], "extrude-1");
        assert_eq!(
            r["feature"]["extent"]["length"],
            json!({"value": "width / 4", "m": 0.005, "expr": true})
        );
        let text = fs::read_to_string(dir.path().join("parts/p/features/extrude-1.json")).unwrap();
        assert!(
            !text.contains("reverse") && !text.contains("loops"),
            "{text}"
        );
        let b = built(&mut s, "extrude-1");
        assert_eq!(b["counts"], json!({"faces": 6, "edges": 12, "vertices": 8}));
        // About its left side, a full turn: a solid cylinder.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s",
                   "axis": "l4", "angle": "full"}),
        )
        .unwrap();
        let b = built(&mut s, "r");
        assert_eq!(b["ok"], true, "{b}");
        assert_eq!(b["counts"]["faces"], 3);
        // A quarter turn, both ways, has two more faces: its ends.
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "r", "angle": 90, "symmetric": true}),
        )
        .unwrap();
        assert_eq!(r["feature"]["angle"]["value"], "90 deg");
        assert_eq!(built(&mut s, "r")["counts"]["faces"], 5);
        // Reversed and symmetric extrudes, and back.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "extrude-1", "reverse": true}),
        )
        .unwrap();
        assert_eq!(built(&mut s, "extrude-1")["ok"], true);
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "extrude-1", "reverse": false}),
        )
        .unwrap();
        assert!(r["feature"].get("reverse").is_none());
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "r"})).unwrap();
        assert_eq!(
            (show["axis"].as_str(), show["symmetric"].as_bool()),
            (Some("l4"), Some(true))
        );
    }

    #[test]
    fn sweeps_refuse_what_cant_build() {
        let (dir, mut s) = with_sketch();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": 1}}),
        )
        .unwrap();
        let blind = json!({"type": "blind", "length": "5 mm"});
        for (params, code) in [
            (
                json!({"type": "extrude", "sketch": "ghost", "extent": blind}),
                Code::NotFound,
            ),
            (
                json!({"type": "extrude", "sketch": "ball", "extent": blind}),
                Code::ReferenceWrongKind,
            ),
            (
                json!({"type": "extrude", "sketch": "s", "extent": blind, "first": true}),
                Code::ReferenceForward,
            ),
            (
                json!({"type": "extrude", "sketch": "s", "extent": {"type": "blind", "length": 0}}),
                Code::InvalidExtent,
            ),
            (
                json!({"type": "extrude", "sketch": "s", "extent": blind, "axis": "l1"}),
                Code::InvalidParam,
            ),
            (
                json!({"type": "extrude", "sketch": "s"}),
                Code::InvalidDocument,
            ),
            (
                json!({"type": "revolve", "sketch": "s", "axis": "l4", "angle": "400 deg"}),
                Code::InvalidExtent,
            ),
        ] {
            let mut params = params;
            params["part"] = json!("p");
            let e = call(&mut s, "feature.add", params.clone()).unwrap_err();
            assert_eq!(e.code, code, "{params}: {e}");
        }
        for (params, code) in [
            (json!({"axis": "l9", "angle": 90}), Code::NotFound),
            (json!({"axis": "p1", "angle": 90}), Code::ReferenceWrongKind),
            (
                json!({"axis": "l4", "angle": 90, "loops": ["q"]}),
                Code::NotFound,
            ),
        ] {
            let mut params = params;
            params["part"] = json!("p");
            params["type"] = json!("revolve");
            params["sketch"] = json!("s");
            let e = call(&mut s, "feature.add", params.clone()).unwrap_err();
            assert_eq!(e.code, code, "{params}: {e}");
        }
        // What only the rebuild can tell, each in the sketch's terms: the
        // diagonal crosses the rectangle, and isn't on a loop.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": "l5",
                    "angle": 90}),
        )
        .unwrap();
        let b = built(&mut s, "r");
        assert_eq!(b["error"]["code"], "profile_invalid");
        assert_eq!(b["error"]["details"]["entities"][1], "l5");
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "r", "loops": ["l5"]}),
        )
        .unwrap();
        let b = built(&mut s, "r");
        assert_eq!(b["error"]["code"], "profile_invalid");
        assert_eq!(b["error"]["details"]["entities"], json!(["l5"]));
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "r", "axis": "l4", "loops": ["l2"]}),
        )
        .unwrap();
        assert_eq!(built(&mut s, "r")["ok"], true);
        // A sketch that fails takes its sweeps with it.
        let params =
            |w: &str| json!({"format": 1, "kind": "ryucad.parameters", "parameters": {"width": w}});
        write_json(
            &dir.path().join("parts/p/parameters.json"),
            &params("50 mm"),
        )
        .unwrap();
        let b = built(&mut s, "r");
        assert_eq!(b["error"]["code"], "dependency_failed");
        assert_eq!(b["error"]["details"]["dependency"], "s");
        assert_eq!(b["error"]["details"]["cause"], "sketch_unsolved");
    }

    #[test]
    fn inspect_says_where_sweeps_came_from() {
        let (_dir, mut s) = with_sketch();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                    "extent": {"length": "5 mm"}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": "l4",
                    "angle": 90}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "full", "sketch": "s", "axis": "l4",
                    "angle": "full"}),
        )
        .unwrap();
        for f in ["e", "r", "full"] {
            let v = call(&mut s, "inspect", json!({"part": "p", "feature": f})).unwrap();
            let b = &v["bodies"][0];
            for list in ["faces", "edges", "vertices"] {
                for e in b[list].as_array().unwrap() {
                    assert_eq!(e["origin"]["sketch"], "s", "{f} {list}: {e}");
                    assert_eq!(e["origin"]["feature"], f, "{f} {list}: {e}");
                }
            }
        }
        let v = call(&mut s, "inspect", json!({"part": "p", "feature": "e"})).unwrap();
        let b = &v["bodies"][0];
        let origins: Vec<&Value> = b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| &f["origin"])
            .collect();
        assert!(origins.contains(&&json!({"feature": "e", "sketch": "s", "entity": "l1"})));
        assert!(
            origins
                .contains(&&json!({"feature": "e", "sketch": "s", "cap": "start", "loop": "p1"}))
        );
        assert!(
            origins.contains(&&json!({"feature": "e", "sketch": "s", "cap": "end", "loop": "p1"}))
        );
        let near = |a: &Value, want: [f64; 3]| {
            for k in 0..3 {
                assert!(
                    (a[k].as_f64().unwrap() - want[k]).abs() < 1e-9,
                    "{a} != {want:?}"
                );
            }
        };
        near(&b["bounds"]["min"], [0.0, 0.0, 0.0]);
        near(&b["bounds"]["max"], [0.02, 0.01, 0.005]);
        // An origin is found by name too.
        let side = b["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["origin"] == json!({"feature": "e", "sketch": "s", "entity": "l2"}))
            .unwrap();
        let one = call(
            &mut s,
            "inspect",
            json!({"part": "p", "entity": format!("p/e/{}", side["name"].as_str().unwrap())}),
        )
        .unwrap();
        assert_eq!(one["entity"]["origin"]["entity"], "l2");
        // Display meshes are sized to the body, not a metre.
        let m = call(
            &mut s,
            "mesh.get",
            json!({"part": "p", "feature": "e", "quality": "coarse"}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 20, "y": 10, "z": 5}}),
        )
        .unwrap();
        let b = call(
            &mut s,
            "mesh.get",
            json!({"part": "p", "feature": "b", "quality": "coarse"}),
        )
        .unwrap();
        // The same box as a primitive: the same chord.
        let chord = m["bodies"][0]["params"]["chord"].as_f64().unwrap();
        let want = b["bodies"][0]["params"]["chord"].as_f64().unwrap();
        assert!((chord - want).abs() < 1e-9 * want, "{chord} vs {want}");
    }

    #[test]
    fn sweeps_keep_their_sketch_before_them() {
        let (_dir, mut s) = with_sketch();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": 1}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "revolve", "id": "r", "sketch": "s", "axis": "l4",
                    "angle": 90, "loops": ["l1"]}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "e", "sketch": "s",
                    "extent": {"type": "symmetric", "length": "4 mm"}}),
        )
        .unwrap();
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "s"})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["features"]),
            (Code::HasDependents, &json!(["r", "e"]))
        );
        let e = call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "s", "after": "e"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceForward);
        let e = call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "r", "first": true}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceForward);
        // Moving what nothing sweeps, or a sweep that stays behind its sketch, is fine.
        call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "ball", "first": true}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.move",
            json!({"part": "p", "id": "r", "after": "e"}),
        )
        .unwrap();
        // What a sweep names stays in its sketch.
        for gone in ["l4", "l1"] {
            let e = call(
                &mut s,
                "sketch.edit",
                json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": gone}]}),
            )
            .unwrap_err();
            assert_eq!(
                (e.code, &e.details["entity"]),
                (Code::HasDependents, &json!(gone))
            );
        }
        // An extent's length alone keeps its type.
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "e", "extent": {"length": "8 mm"}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["extent"]["type"], "symmetric");
        assert_eq!(r["feature"]["extent"]["length"]["m"], 0.008);
    }

    #[test]
    fn a_failing_feature_fails_the_rebuild_but_reports_everything() {
        let (_dir, mut s) = with_part();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "ok"}),
        )
        .unwrap();
        call(&mut s, "feature.add", json!({"part": "p", "type": "torus", "params": {"major_radius": 1, "minor_radius": 1}, "id": "bad"})).unwrap();
        let e = call(&mut s, "rebuild", json!({"part": "p"})).unwrap_err();
        assert_eq!(e.code, Code::RebuildFailed);
        let features = &e.details["parts"][0]["features"];
        assert_eq!(features[0]["ok"], true);
        assert_eq!(features[1]["error"]["code"], "rebuild_failed");
        // Inspect still shows what did build.
        let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        assert_eq!(v["bodies"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn check_format_reports_every_kind_of_problem() {
        let (dir, mut s) = with_part();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": 1}, "id": "a"}),
        )
        .unwrap();
        assert_eq!(
            call(&mut s, "check_format", json!({})).unwrap()["clean"],
            true
        );
        let root = dir.path();
        let a = root.join("parts/p/features/a.json");
        let compact = serde_json::to_string(
            &crate::doc::canonical::parse(&fs::read_to_string(&a).unwrap()).unwrap(),
        )
        .unwrap();
        fs::write(&a, compact).unwrap();
        fs::write(
            root.join("parts/p/features/orphan.json"),
            fs::read_to_string(&a).unwrap(),
        )
        .unwrap();
        fs::write(root.join("parts/p/features/broken.json"), "{").unwrap();
        fs::write(root.join("parts/p/stray.txt"), "").unwrap();
        // ryupdm's files are known, and must be canonical too.
        fs::write(root.join("parts/p/properties.json"), "{\"name\":\"x\"}").unwrap();
        fs::write(root.join("ryupdm.json"), "{\"format\":1}").unwrap();
        let r = call(&mut s, "check_format", json!({})).unwrap();
        assert_eq!(r["clean"], false);
        for f in [
            "parts/p/features/a.json",
            "parts/p/properties.json",
            "ryupdm.json",
        ] {
            assert!(
                r["non_canonical"].as_array().unwrap().contains(&json!(f)),
                "{f}: {r}"
            );
        }
        assert_eq!(
            r["orphans"],
            json!([
                "parts/p/features/broken.json",
                "parts/p/features/orphan.json"
            ])
        );
        assert_eq!(r["errors"][0]["code"], "invalid_json");
        assert_eq!(r["unknown"], json!(["parts/p/stray.txt"]));
    }

    #[test]
    fn a_panic_is_an_internal_error_and_drops_the_session() {
        let (dir, _) = fresh();
        let mut slot = None;
        let r = handle_safely(&mut slot, None, None, dir.path(), Request::PartList {})
            .unwrap()
            .result;
        assert_eq!(r["parts"], json!([]));
        assert!(slot.is_some());
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let e = guarded::<Value>(&mut slot, dir.path(), false, |_| panic!("boom")).unwrap_err();
        std::panic::set_hook(prev);
        assert_eq!(
            (e.code, e.details["panic"].as_str()),
            (Code::Internal, Some("boom"))
        );
        assert!(slot.is_none());
        assert!(handle_safely(&mut slot, None, None, dir.path(), Request::PartList {}).is_ok());
    }

    #[test]
    fn daemon_requests_need_a_daemon() {
        let (_dir, mut s) = fresh();
        let e = call(&mut s, "daemon.status", json!({})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
    }

    /// `call` with the daemon's live session, as participant `me`.
    pub(crate) fn call_live(
        s: &mut Session,
        live: &mut Live,
        me: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        handle_live(s, Some(live), me, Request::from_rpc(method, Some(params))?)
    }

    #[test]
    fn without_a_daemon_nobody_is_attached() {
        let (_dir, mut s) = fresh();
        let e = call(&mut s, "session.hello", json!({"kind": "agent"})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
        let r = call(&mut s, "participant.list", json!({})).unwrap();
        assert_eq!(r, json!({"participants": []}));
        let e = call(&mut s, "app.open", json!({})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
    }

    #[test]
    fn hello_registers_a_participant() {
        let (_dir, mut s) = fresh();
        let mut live = Live::new();
        let r = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human", "name": "Dana"}),
        )
        .unwrap();
        let id = r["participant"].as_str().unwrap().to_owned();
        assert_eq!(
            r["session"]["participants"],
            json!([{"id": id, "kind": "human", "name": "Dana", "status": null, "selection": [],
                    "camera": null, "highlights": [], "compare": null, "focus": null, "hidden": []}])
        );
        let list = call_live(&mut s, &mut live, Some(&id), "participant.list", json!({})).unwrap();
        assert_eq!(list, r["session"]);
    }

    #[test]
    fn sketch_entities_are_selected_highlighted_and_framed() {
        let (_dir, mut s) = with_sketch();
        let mut live = Live::new();
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human"}),
        )
        .unwrap()["participant"]
            .as_str()
            .unwrap()
            .to_owned();
        let me = Some(human.as_str());
        let r = call_live(
            &mut s,
            &mut live,
            me,
            "selection.set",
            json!({"entities": ["p/s/l1", "p/s/p3"]}),
        )
        .unwrap();
        assert_eq!(r["selection"], json!(["p/s/l1", "p/s/p3"]));
        call_live(
            &mut s,
            &mut live,
            me,
            "highlight.set",
            json!({"entities": ["p/s/l2"], "style": "pending"}),
        )
        .unwrap();
        let e = call_live(
            &mut s,
            &mut live,
            me,
            "selection.set",
            json!({"entities": ["p/s/l9"]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // Framing a line takes its two ends.
        let r = call_live(
            &mut s,
            &mut live,
            me,
            "camera.set",
            json!({"fit": ["p/s/l1"]}),
        )
        .unwrap();
        assert!(r["camera"]["target"][0].as_f64().unwrap() > 0.0);
        // An entity that goes drops out of the selection.
        call_live(
            &mut s,
            &mut live,
            None,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [{"op": "remove", "id": "l3"},
                                                            {"op": "remove", "id": "p3"},
                                                            {"op": "remove", "id": "l2"},
                                                            {"op": "remove", "id": "l5"}]}),
        )
        .unwrap();
        let r = call_live(&mut s, &mut live, None, "selection.get", json!({})).unwrap();
        assert_eq!(r["selection"], json!(["p/s/l1"]));
    }

    #[test]
    fn selections_follow_the_model() {
        let (_dir, mut s) = with_part();
        let mut live = Live::new();
        for id in ["a", "b"] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "box", "id": id, "params": {"x": 1, "y": 1, "z": 1}}),
            )
            .unwrap();
        }
        let face = |s: &mut Session, f: &str| {
            let v = call(s, "inspect", json!({"part": "p", "feature": f})).unwrap();
            format!(
                "p/{f}/{}",
                v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
            )
        };
        let (fa, fb) = (face(&mut s, "a"), face(&mut s, "b"));
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human"}),
        )
        .unwrap()["participant"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = call_live(
            &mut s,
            &mut live,
            Some(&human),
            "selection.set",
            json!({"entities": [fa, fb]}),
        )
        .unwrap();
        assert_eq!(r["selection"], json!([fa, fb]));
        // Anyone asking without naming a participant gets the human's.
        let r = call_live(&mut s, &mut live, None, "selection.get", json!({})).unwrap();
        assert_eq!(
            (&r["participant"], &r["selection"]),
            (&json!(human), &json!([fa, fb]))
        );
        // A reference that doesn't resolve is refused, never matched.
        let e = call_live(
            &mut s,
            &mut live,
            Some(&human),
            "selection.set",
            json!({"entities": ["p/a/0123456789abcdef0123456789abcdef"], "mode": "add"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // Editing keeps names, so the selection stays; deleting drops it.
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.edit",
            json!({"part": "p", "id": "a", "params": {"x": 2}}),
        )
        .unwrap();
        let sel = |s: &mut Session, live: &mut Live| {
            call_live(s, live, None, "selection.get", json!({})).unwrap()["selection"].clone()
        };
        assert_eq!(sel(&mut s, &mut live), json!([fa, fb]));
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.delete",
            json!({"part": "p", "id": "a"}),
        )
        .unwrap();
        assert_eq!(sel(&mut s, &mut live), json!([fb]));
        // Removing works even for references that no longer resolve.
        call_live(
            &mut s,
            &mut live,
            Some(&human),
            "selection.set",
            json!({"entities": [fa, fb], "mode": "remove"}),
        )
        .unwrap();
        assert_eq!(sel(&mut s, &mut live), json!([]));
        // Without a daemon there is no selection to change.
        let e = call(&mut s, "selection.set", json!({"entities": []})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
        let r = call(&mut s, "selection.get", json!({})).unwrap();
        assert_eq!(r, json!({"participant": null, "selection": []}));
    }

    /// The face of solid `b` of part `p` whose outward normal is `n`.
    fn face_facing(s: &mut Session, n: [f64; 3]) -> String {
        let v = call(s, "inspect", json!({"part": "p", "feature": "b"})).unwrap();
        let face = v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let z = &f["surface"]["frame"]["z"];
                let k = if f["reversed"] == true { -1.0 } else { 1.0 };
                f["surface"]["type"] == "plane"
                    && (0..3).all(|i| (z[i].as_f64().unwrap() * k - n[i]).abs() < 1e-12)
            })
            .unwrap();
        format!("p/b/{}", face["name"].as_str().unwrap())
    }

    #[test]
    fn cameras_restore_named_views_and_look_normal_to_faces() {
        let (dir, mut s) = with_part();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 40, "y": 20, "z": 10},
                   "placement": {"origin": [30, 30, 5]}}),
        )
        .unwrap();
        let mut live = Live::new();
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human"}),
        )
        .unwrap()["participant"]
            .clone();
        let me = Some(human.as_str().unwrap());
        let set =
            |s: &mut Session, live: &mut Live, p: Value| call_live(s, live, me, "camera.set", p);
        // Every cube direction frames the model.
        for (name, _) in crate::view::STANDARD_VIEWS {
            let r = set(&mut s, &mut live, json!({"view": name})).unwrap();
            assert!(r["camera"]["eye"].is_array(), "{name}");
        }
        // Looking at the box and the origin beside it.
        let camera = json!({"eye": ["120 mm", "-80 mm", "80 mm"], "target": ["20 mm", "20 mm", "5 mm"],
                            "up": [0, 0, 1], "projection": {"orthographic": {"height": "120 mm"}}});
        let mine = set(&mut s, &mut live, json!({"camera": camera})).unwrap()["camera"].clone();
        // An agent saves what the one human sees.
        let saved = call_live(
            &mut s,
            &mut live,
            None,
            "view.save",
            json!({"part": "p", "view": "detail"}),
        )
        .unwrap();
        assert_eq!(saved["camera"], mine);
        set(&mut s, &mut live, json!({"view": "iso"})).unwrap();
        // Restored as saved, bit for bit: the project's model is one part.
        let r = set(&mut s, &mut live, json!({"view": "detail"})).unwrap();
        assert_eq!(r["camera"], mine);
        let r = set(&mut s, &mut live, json!({"view": "detail", "part": "p"})).unwrap();
        assert_eq!(r["camera"], mine);
        // Framing entities from a named view keeps its direction.
        let top = face_facing(&mut s, [0.0, 0.0, 1.0]);
        let r = set(&mut s, &mut live, json!({"view": "detail", "fit": [top]})).unwrap();
        assert_eq!(r["camera"]["up"], mine["up"]);
        assert_ne!(r["camera"]["eye"], mine["eye"]);
        let e = set(&mut s, &mut live, json!({"view": "side"})).unwrap_err();
        assert_eq!(
            (e.code, e.details["views"].clone()),
            (Code::ViewNotFound, json!(["detail"]))
        );
        // Normal to the top: looking down, the long edge (along x) level.
        let r = set(&mut s, &mut live, json!({"normal_to": top})).unwrap();
        let c: Camera = serde_json::from_value(r["camera"].clone()).unwrap();
        let (right, _, forward) = c.basis();
        assert!((forward[2] + 1.0).abs() < 1e-12, "{forward:?}");
        assert!((right[0].abs() - 1.0).abs() < 1e-12, "{right:?}");
        // The participant's projection is kept.
        assert!(matches!(
            c.projection,
            crate::view::Projection::Orthographic { .. }
        ));
        // The front face (normal -y): looking along +y with z up.
        let front = face_facing(&mut s, [0.0, -1.0, 0.0]);
        let r = set(&mut s, &mut live, json!({"normal_to": front})).unwrap();
        let c: Camera = serde_json::from_value(r["camera"].clone()).unwrap();
        let (right, up, forward) = c.basis();
        assert!((forward[1] - 1.0).abs() < 1e-12 && (right[0] - 1.0).abs() < 1e-12);
        assert!((up[2] - 1.0).abs() < 1e-12, "{up:?}");
        let e = set(&mut s, &mut live, json!({"normal_to": "p/b"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        let e = set(
            &mut s,
            &mut live,
            json!({"normal_to": front, "view": "top"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // A render restores the named view too, and draws the origin.
        let size = |v: Value| {
            let mut v = v;
            v["width"] = json!(320);
            v["height"] = json!(240);
            v
        };
        let named = render_pixels(dir.path(), s, size(json!({"view": "detail"})));
        let s = Session::open(dir.path()).unwrap();
        let again = render_pixels(dir.path(), s, size(json!({"camera": camera})));
        assert_eq!(named, again);
        let s = Session::open(dir.path()).unwrap();
        let with_origin = render_pixels(
            dir.path(),
            s,
            size(json!({"view": "detail", "origin": true})),
        );
        assert_ne!(with_origin, named);
        let red = |px: &[[u8; 3]]| {
            px.iter()
                .filter(|p| p[0] > 150 && p[1] < 90 && p[2] < 90)
                .count()
        };
        assert!(
            red(&with_origin) > red(&named),
            "the x axis is drawn red: {} against {}",
            red(&with_origin),
            red(&named)
        );
    }

    #[test]
    fn cameras_highlights_and_status() {
        let (_dir, mut s) = with_part();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 5}}),
        )
        .unwrap();
        let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        let face = format!(
            "p/s/{}",
            v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
        );
        let mut live = Live::new();
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human"}),
        )
        .unwrap()["participant"]
            .clone();
        let me = Some(human.as_str().unwrap());
        let camera = json!({"eye": [0, "-100 mm", 0], "target": [0, 0, 0], "up": [0, 0, 1],
                            "projection": {"perspective": {"fov_deg": 30}}});
        let r = call_live(
            &mut s,
            &mut live,
            me,
            "camera.set",
            json!({"camera": camera}),
        )
        .unwrap();
        assert_eq!(r["camera"]["eye"], json!([0.0, -0.1, 0.0]));
        // Framing a face keeps the direction and looks at the face's box.
        let set =
            |s: &mut Session, live: &mut Live, p: Value| call_live(s, live, me, "camera.set", p);
        let r = set(&mut s, &mut live, json!({"fit": [face]})).unwrap();
        let (eye, target) = (&r["camera"]["eye"], &r["camera"]["target"]);
        assert!(eye[1].as_f64().unwrap() < target[1].as_f64().unwrap());
        assert!(target[2].as_f64().unwrap().abs() < 1e-3);
        let r = set(&mut s, &mut live, json!({"view": "top"})).unwrap();
        assert_eq!(r["camera"]["up"], json!([0.0, 1.0, 0.0]));
        // The participant's projection is kept.
        assert_eq!(r["camera"]["projection"]["perspective"]["fov_deg"], 30.0);
        let e = set(&mut s, &mut live, json!({"view": "top", "camera": camera})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
        let r = set(&mut s, &mut live, json!({"camera": camera})).unwrap();
        assert_eq!(r["camera"]["eye"], json!([0.0, -0.1, 0.0]));
        let r = call_live(&mut s, &mut live, None, "camera.get", json!({})).unwrap();
        assert_eq!(
            (&r["participant"], &r["camera"]["eye"]),
            (&human, &json!([0.0, -0.1, 0.0]))
        );
        // An agent (no hello) highlights and reports status as itself.
        let r = call_live(
            &mut s,
            &mut live,
            None,
            "highlight.set",
            json!({"entities": [face], "style": "pending", "label": "next"}),
        )
        .unwrap();
        assert_eq!(
            r["highlights"],
            json!([{"style": "pending", "label": "next", "entities": [face]}])
        );
        let agent = r["participant"].clone();
        assert_ne!(agent, human);
        let r = call_live(
            &mut s,
            &mut live,
            None,
            "activity.set",
            json!({"status": "measuring", "entities": [face]}),
        )
        .unwrap();
        assert_eq!(
            r["status"],
            json!({"text": "measuring", "entities": [face]})
        );
        let list = call_live(&mut s, &mut live, None, "participant.list", json!({})).unwrap();
        let a = list["participants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == agent)
            .unwrap()
            .clone();
        assert_eq!(a["status"]["text"], "measuring");
        assert_eq!(a["highlights"][0]["label"], "next");
        let r = call_live(&mut s, &mut live, None, "highlight.clear", json!({})).unwrap();
        assert_eq!(r["highlights"], json!([]));
        for (method, params) in [
            ("camera.set", json!({"camera": camera})),
            ("highlight.set", json!({"entities": [face]})),
            ("highlight.clear", json!({})),
            ("activity.set", json!({"status": "x"})),
        ] {
            let e = call(&mut s, method, params).unwrap_err();
            assert_eq!(e.code, Code::NoLiveSession, "{method}");
        }
        let r = call(&mut s, "camera.get", json!({})).unwrap();
        assert_eq!(r, json!({"participant": null, "camera": null}));
    }

    #[test]
    fn an_agent_puts_the_human_into_a_compare() {
        use crate::git::tests::git;
        let (dir, mut s) = fresh();
        let mut live = Live::new();
        let e = call_live(
            &mut s,
            &mut live,
            None,
            "compare.set",
            json!({"rev": "HEAD"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotAGitRepo);
        git(dir.path(), &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "start"]);
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human", "name": "Dana"}),
        )
        .unwrap()["participant"]
            .clone();
        // An agent (no hello) sets it: it goes to the one human.
        let set =
            |s: &mut Session, live: &mut Live, p: Value| call_live(s, live, None, "compare.set", p);
        let r = set(&mut s, &mut live, json!({"rev": "main"})).unwrap();
        assert_eq!(r["participant"], human);
        assert_eq!(r["compare"]["rev"], "main");
        let commit = r["compare"]["commit"].as_str().unwrap().to_owned();
        assert_eq!(commit.len(), 40);
        let r = call_live(&mut s, &mut live, None, "compare.get", json!({})).unwrap();
        assert_eq!(
            r,
            json!({"participant": human, "compare": {"rev": "main", "commit": commit}})
        );
        let list = call_live(&mut s, &mut live, None, "participant.list", json!({})).unwrap();
        let of = |id: &Value| {
            list["participants"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == *id)
                .unwrap()["compare"]
                .clone()
        };
        assert_eq!(of(&human), json!({"rev": "main"}));
        // Revisions that don't resolve, and the working files, are refused;
        // the compare stays as it was.
        for (p, code) in [
            (json!({"rev": "nowhere"}), Code::RevNotFound),
            (json!({"rev": "working"}), Code::InvalidParam),
            (json!({}), Code::Usage),
            (json!({"rev": "HEAD", "off": true}), Code::Usage),
            (
                json!({"off": true, "participant": "p-ffffffff"}),
                Code::NotFound,
            ),
        ] {
            let e = set(&mut s, &mut live, p.clone()).unwrap_err();
            assert_eq!(e.code, code, "{p}");
        }
        let r = call_live(&mut s, &mut live, None, "compare.get", json!({})).unwrap();
        assert_eq!(r["compare"]["rev"], "main");
        // Named, the agent compares itself; off ends the human's.
        let r = set(
            &mut s,
            &mut live,
            json!({"rev": "HEAD", "participant": human}),
        )
        .unwrap();
        assert_eq!(r["compare"]["rev"], "HEAD");
        let r = set(&mut s, &mut live, json!({"off": true})).unwrap();
        assert_eq!(r, json!({"participant": human, "compare": null}));
        // Without a daemon nobody compares.
        let e = call(&mut s, "compare.set", json!({"rev": "HEAD"})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
        let r = call(&mut s, "compare.get", json!({})).unwrap();
        assert_eq!(r, json!({"participant": null, "compare": null}));
    }

    /// The notifications queued for `sub` so far, as (method, params).
    pub(crate) fn drain(sub: &crate::notify::Subscriber) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        // `next` blocks when empty, so close a clone's view by sending a
        // marker first.
        loop {
            let line = sub.next().unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["method"] == "marker" {
                return out;
            }
            out.push((
                v["method"].as_str().unwrap().to_owned(),
                v["params"].clone(),
            ));
        }
    }

    #[test]
    fn the_log_records_changes_and_who_made_them() {
        let (dir, mut s) = with_part();
        let mut live = Live::new();
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human", "name": "Dana"}),
        )
        .unwrap()["participant"]
            .clone();
        call_live(
            &mut s,
            &mut live,
            human.as_str(),
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        // Reads and refused writes aren't logged.
        call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
        call_live(&mut s, &mut live, None, "part.create", json!({"part": "p"})).unwrap_err();
        // An edit behind the daemon's back is logged as external.
        let mut other = Session::open(dir.path()).unwrap();
        call(
            &mut other,
            "feature.edit",
            json!({"part": "p", "id": "s", "params": {"radius": 2}}),
        )
        .unwrap();
        call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let e = log["entries"].as_array().unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0]["kind"], "change");
        assert_eq!(e[0]["method"], "feature.add");
        assert_eq!(e[0]["participant"]["name"], "Dana");
        assert_eq!(e[0]["params"]["id"], "s");
        assert_eq!(
            e[0]["files"],
            json!([{"path": "parts/p/features/s.json", "change": "created"},
                   {"path": "parts/p/part.json", "change": "modified"}])
        );
        assert_eq!(e[1]["kind"], "external");
        assert_eq!(e[1]["participant"], Value::Null);
        assert_eq!(
            e[1]["files"],
            json!([{"path": "parts/p/features/s.json", "change": "modified"}])
        );
        let page = call_live(&mut s, &mut live, None, "activity.log", json!({"limit": 1})).unwrap();
        assert_eq!(page["entries"][0]["seq"], e[1]["seq"]);
        let r = call(&mut s, "activity.log", json!({})).unwrap();
        assert_eq!(r, json!({"entries": []}));
    }

    #[test]
    fn undo_and_redo_go_through_the_files() {
        let (dir, mut s) = with_part();
        let mut live = Live::new();
        let human = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human"}),
        )
        .unwrap()["participant"]
            .as_str()
            .unwrap()
            .to_owned();
        let h = Some(human.as_str());
        let radius = |s: &mut Session| {
            call(s, "feature.show", json!({"part": "p", "id": "s"}))
                .map(|v| v["params"]["radius"]["value"].clone())
        };
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        call_live(
            &mut s,
            &mut live,
            h,
            "feature.edit",
            json!({"part": "p", "id": "s", "params": {"radius": 2}}),
        )
        .unwrap();
        // The human undoes their own edit, then redoes it.
        let r = call_live(&mut s, &mut live, h, "undo", json!({})).unwrap();
        assert_eq!(r["entry"]["kind"], "undo");
        assert_eq!(radius(&mut s).unwrap(), "1 mm");
        assert!(r["revisions"]["p"].is_string());
        let e = call_live(&mut s, &mut live, h, "undo", json!({})).unwrap_err();
        assert_eq!(e.code, Code::NothingToUndo);
        call_live(&mut s, &mut live, h, "redo", json!({})).unwrap();
        assert_eq!(radius(&mut s).unwrap(), "2 mm");
        let e = call_live(&mut s, &mut live, h, "redo", json!({})).unwrap_err();
        assert_eq!(e.code, Code::NothingToUndo);
        // A redo can itself be undone.
        call_live(&mut s, &mut live, h, "undo", json!({})).unwrap();
        assert_eq!(radius(&mut s).unwrap(), "1 mm");
        call_live(&mut s, &mut live, h, "redo", json!({})).unwrap();
        // The agent can't undo its add while the human's edit stands on
        // top of it, by default or by number.
        let e = call_live(&mut s, &mut live, None, "undo", json!({})).unwrap_err();
        assert_eq!(e.code, Code::UndoConflict);
        assert_eq!(e.details["files"], json!(["parts/p/features/s.json"]));
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let add = log["entries"][0]["seq"].as_u64().unwrap();
        let e = call_live(&mut s, &mut live, None, "undo", json!({"entry": add})).unwrap_err();
        assert_eq!(e.code, Code::UndoConflict);
        // An edit behind the daemon's back can't be undone, and blocks
        // undoing what it overwrote.
        let mut other = Session::open(dir.path()).unwrap();
        call(
            &mut other,
            "feature.edit",
            json!({"part": "p", "id": "s", "params": {"radius": 3}}),
        )
        .unwrap();
        call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let entries = log["entries"].as_array().unwrap();
        let external = entries.last().unwrap()["seq"].as_u64().unwrap();
        let e = call_live(&mut s, &mut live, None, "undo", json!({"entry": external})).unwrap_err();
        assert_eq!(e.code, Code::NothingToUndo);
        let e = call_live(&mut s, &mut live, h, "undo", json!({})).unwrap_err();
        assert_eq!(e.code, Code::UndoConflict);
        // Undoing a part's creation removes its folder too.
        call_live(&mut s, &mut live, None, "part.create", json!({"part": "q"})).unwrap();
        assert!(dir.path().join("parts/q").is_dir());
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert!(!dir.path().join("parts/q").exists());
        // Undoing a deletion brings every file back.
        call_live(&mut s, &mut live, None, "part.delete", json!({"part": "p"})).unwrap();
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert_eq!(radius(&mut s).unwrap(), "3 mm");
        let e = call(&mut s, "undo", json!({})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
    }

    #[test]
    fn setting_a_preference_tells_subscribers() {
        crate::config::tests::with_config(|_| {
            let (_dir, mut s) = fresh();
            let mut live = Live::new();
            let sub = live.hub.subscribe("p-x", &[Topic::Project]);
            call_live(
                &mut s,
                &mut live,
                None,
                "config.set",
                json!({"key": "daemon.poll_ms", "value": 100}),
            )
            .unwrap();
            assert_eq!(live.poll_ms, 100);
            live.hub.send(Topic::Project, "marker", &json!({}));
            let got = drain(&sub);
            assert_eq!(got[0].0, "config.changed");
            assert_eq!(
                got[0].1,
                json!({"key": "daemon.poll_ms", "scope": "user", "value": 100, "default": false})
            );
            // A project setting is logged like any file change; a
            // preference isn't in the project, so it isn't.
            call_live(
                &mut s,
                &mut live,
                None,
                "config.set",
                json!({"key": "default_unit", "value": "cm"}),
            )
            .unwrap();
            let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
            let e = log["entries"].as_array().unwrap();
            assert_eq!(e.len(), 1);
            assert_eq!(
                e[0]["files"],
                json!([{"path": "ryucad.json", "change": "modified"}])
            );
        });
    }

    #[test]
    fn setting_the_unit_keeps_the_project_id() {
        crate::config::tests::with_config(|_| {
            let (dir, mut s) = fresh();
            let id = crate::doc::id::ProjectId::from_bytes([9; 16]);
            let m = Manifest {
                project: Some(id.clone()),
                ..Manifest::new(Unit::Mm)
            };
            write_json(&dir.path().join(MANIFEST), &m.to_json()).unwrap();
            call(
                &mut s,
                "config.set",
                json!({"key": "default_unit", "value": "in"}),
            )
            .unwrap();
            let text = fs::read_to_string(dir.path().join(MANIFEST)).unwrap();
            assert_eq!(
                crate::doc::canonical::parse(&text).unwrap(),
                json!({"default_unit": "in", "format": 2, "kind": "ryucad.project",
                       "project": id})
            );
            assert_eq!(s.project().manifest.project, Some(id));
        });
    }

    #[test]
    fn editing_one_feature_meshes_one_body() {
        crate::config::tests::with_config(|_| {
            let (_dir, mut s) = with_part();
            for (id, r) in [("a", 1), ("b", 2)] {
                call(
                    &mut s,
                    "feature.add",
                    json!({"part": "p", "type": "sphere", "id": id, "params": {"radius": r}}),
                )
                .unwrap();
            }
            let m = call(
                &mut s,
                "mesh.get",
                json!({"part": "p", "quality": "coarse"}),
            )
            .unwrap();
            assert_eq!(m["quality"], "coarse");
            assert_eq!(m["bodies"][1]["feature"], "b");
            assert_eq!(s.meshes.len(), 2);
            call(
                &mut s,
                "feature.edit",
                json!({"part": "p", "id": "a", "params": {"radius": 3}}),
            )
            .unwrap();
            let again = call(
                &mut s,
                "mesh.get",
                json!({"part": "p", "quality": "coarse"}),
            )
            .unwrap();
            assert_eq!(s.meshes.len(), 3);
            assert_eq!(again["bodies"][1], m["bodies"][1]);
            assert_ne!(again["bodies"][0]["hash"], m["bodies"][0]["hash"]);
            // The user's display quality is the default.
            let v = call(&mut s, "mesh.get", json!({"part": "p", "feature": "b"})).unwrap();
            assert_eq!(
                (v["quality"].as_str(), v["bodies"].as_array().unwrap().len()),
                (Some("normal"), 1)
            );
            let e = call(&mut s, "mesh.get", json!({"part": "p", "feature": "z"})).unwrap_err();
            assert_eq!(e.code, Code::NotFound);
        });
    }

    #[test]
    fn meshes_are_made_before_the_lock_is_taken() {
        crate::config::tests::with_config(|_| {
            let (dir, s) = with_part();
            let mut slot = Some(s);
            for (id, r) in [("a", 1), ("b", 2)] {
                call(
                    slot.as_mut().unwrap(),
                    "feature.add",
                    json!({"part": "p", "type": "sphere", "id": id, "params": {"radius": r}}),
                )
                .unwrap();
            }
            let req = |rev: Option<&str>| {
                Request::from_rpc(
                    "mesh.get",
                    Some(
                        json!({"part": "p", "quality": "coarse", "transfer": "binary",
                                "rev": rev}),
                    ),
                )
                .unwrap()
            };
            // A revision's meshes are made under the lock, as before.
            assert!(prepare(&mut slot, &req(Some("HEAD"))).is_none());
            let job = prepare(&mut slot, &req(None)).expect("two meshes to make");
            let put = job();
            let s = slot.as_mut().unwrap();
            assert_eq!(s.meshes.len(), 0);
            put(s);
            assert_eq!(s.meshes.len(), 2);
            assert!(prepare(&mut slot, &req(Some("working"))).is_none());
            // The answer is the one a session that meshed itself gives.
            let mut a = handle_safely(&mut slot, None, None, dir.path(), req(None)).unwrap();
            assert_eq!(slot.as_ref().unwrap().meshes.len(), 2);
            slot.as_mut().unwrap().meshes = mesh::Cache::default();
            let mut b = handle_safely(&mut slot, None, None, dir.path(), req(None)).unwrap();
            assert_eq!(a.result["binary"]["encoding"], mesh::ENCODING);
            let bytes = a.binary.take().unwrap();
            assert_eq!(Some(&bytes), b.binary.take().as_ref());
            assert_eq!(a.result, b.result);
            // `compact` answers the same meshes as `ryucad.mesh.2`, smaller.
            let compact = Request::from_rpc(
                "mesh.get",
                Some(json!({"part": "p", "quality": "coarse", "transfer": "compact"})),
            )
            .unwrap();
            let c = handle_safely(&mut slot, None, None, dir.path(), compact).unwrap();
            assert_eq!(c.result["binary"]["encoding"], mesh::COMPACT);
            let small = c.binary.unwrap();
            assert!(small.len() < bytes.len());
            let (one, two) = (mesh::decode(&bytes).unwrap(), mesh::decode(&small).unwrap());
            assert_eq!(one.len(), two.len());
            for (x, y) in one.iter().zip(&two) {
                let (x, y) = (x.mesh.as_ref().unwrap(), y.mesh.as_ref().unwrap());
                assert_eq!((&x.positions, &x.triangles), (&y.positions, &y.triangles));
            }
        });
    }

    #[test]
    fn a_repair_is_logged_and_can_be_undone() {
        let (dir, mut s) = with_part();
        let mut live = Live::new();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "a", "params": {"radius": 1}}),
        )
        .unwrap();
        let text = fs::read_to_string(dir.path().join("parts/p/features/a.json")).unwrap();
        fs::write(dir.path().join("parts/p/features/b.json"), text).unwrap();
        let r = call_live(&mut s, &mut live, None, "doctor", json!({})).unwrap();
        let adopt = r["issues"][0]["repairs"][0]["id"].clone();
        assert!(adopt.as_str().unwrap().starts_with("orphan_feature:adopt:"));
        call_live(
            &mut s,
            &mut live,
            None,
            "doctor.repair",
            json!({"repairs": [adopt]}),
        )
        .unwrap();
        assert_eq!(
            s.project()
                .part(&part_id("p").unwrap())
                .unwrap()
                .order()
                .len(),
            2
        );
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let last = log["entries"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["method"], "doctor.repair");
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        assert_eq!(
            s.project()
                .part(&part_id("p").unwrap())
                .unwrap()
                .order()
                .len(),
            1
        );
    }

    #[test]
    fn a_label_made_official_is_news() {
        use crate::assembly::tests::{place_in, released};
        use crate::git::tests::git;
        let (dir, mut s) = released();
        let r = dir.path();
        call(&mut s, "release.propose", json!({"part": "bolt"})).unwrap();
        place_in(r, "frame", "b", "bolt", json!({"label": "B"}), [0.0; 3]);
        let mut live = Live::new();
        let mut slot = Some(s);
        settle(&mut slot, &mut live, r);
        let sub = live.hub.subscribe("p-x", &[Topic::Model]);
        let mark = |live: &mut Live| live.hub.send(Topic::Model, "marker", &json!({}));
        // The first watched tick keys assemblies by their results; the
        // next has nothing to say.
        tick(&mut slot, &mut live, r);
        mark(&mut live);
        drain(&sub);
        tick(&mut slot, &mut live, r);
        mark(&mut live);
        assert!(drain(&sub).is_empty());
        // No file the session reads changes; the release branch does.
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "bolt B"]);
        tick(&mut slot, &mut live, r);
        mark(&mut live);
        let got = drain(&sub);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "model.changed");
        let frame = got[0].1["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["part"] == "frame")
            .unwrap()
            .clone();
        let b = frame["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "b")
            .unwrap()
            .clone();
        assert_eq!(b["instance"]["pin"]["official"], true, "{b}");
    }

    #[test]
    fn subscribers_hear_of_model_changes_once() {
        let (dir, s) = with_part();
        let mut live = Live::new();
        let mut slot = Some(s);
        // Changes from before the subscription aren't news.
        settle(&mut slot, &mut live, dir.path());
        let sub = live.hub.subscribe("p-x", &[Topic::Model, Topic::Project]);
        let mark = |live: &mut Live| live.hub.send(Topic::Model, "marker", &json!({}));
        let mut s = slot.take().unwrap();
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        mark(&mut live);
        let got = drain(&sub);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "model.changed");
        let part = &got[0].1["parts"][0];
        assert_eq!(part["features"][0]["id"], "s");
        assert_eq!(part["features"][0]["ok"], true);
        // Nothing changed: nothing is sent.
        call_live(&mut s, &mut live, None, "part.list", json!({})).unwrap();
        let mut slot = Some(s);
        tick(&mut slot, &mut live, dir.path());
        mark(&mut live);
        assert!(drain(&sub).is_empty());
        // An edit behind the daemon's back is found by the tick.
        let mut other = Session::open(dir.path()).unwrap();
        call(
            &mut other,
            "feature.edit",
            json!({"part": "p", "id": "s", "params": {"radius": 2}}),
        )
        .unwrap();
        tick(&mut slot, &mut live, dir.path());
        mark(&mut live);
        let got = drain(&sub);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "model.changed");
        // A deleted part is reported as removed.
        call(&mut other, "part.delete", json!({"part": "p"})).unwrap();
        tick(&mut slot, &mut live, dir.path());
        mark(&mut live);
        let got = drain(&sub);
        assert_eq!(got[0].0, "project.changed");
        assert_eq!(got[0].1["parts"], json!([]));
        assert_eq!(got[1].1["removed"], json!(["p"]));
    }
}
