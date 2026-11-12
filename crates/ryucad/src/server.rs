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
use crate::proto::{
    Answer, FeatureFields, PlacementArg, Position, Raw, Request, Transfer, VersionInfo,
};
use crate::rebuild;
use crate::session::Session;
use crate::solid::Solids;
use crate::units::{Quantity, Unit};
use crate::view::{Camera, DEFAULT_PROJECTION, StandardView};
use ryu_session::job::{Job, Phase};
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
    // no session: there is no project to open until init has run
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
    answer_live(session, live, me, req)
        .and_then(Answer::into_json)
        .map(|a| a.result)
}

/// Why `me`, a human in a mode that doesn't allow it, may not send `req`
/// (design 374, question 198); `None` for agents, connections that said
/// no hello, and requests any mode may send.
pub fn mode_refusal(live: Option<&Live>, me: Option<&str>, req: &Request) -> Option<Error> {
    let method = req.method();
    match (live, me, req.needs()) {
        (Some(l), Some(me), Some(needs)) => match l.get(me) {
            Ok(p) if p.kind == live::Kind::Human && !needs.contains(&p.view.mode) => Some(
                Error::new(
                    Code::ParticipantMode,
                    format!(
                        "{} is in {} mode: {method} needs {}",
                        p.name,
                        p.view.mode.as_str(),
                        needs
                            .iter()
                            .map(|m| m.as_str())
                            .collect::<Vec<_>>()
                            .join(" or ")
                    ),
                )
                .with("participant", &p.id)
                .with("mode", p.view.mode)
                .with("needs", needs),
            ),
            _ => None,
        },
        _ => None,
    }
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
    // A branch switch changes files the way an outside change does: the
    // selections are pruned after it.
    let switches = matches!(req, Request::BranchSwitch { .. });
    // A daemon keeps what its writes change, for the log and for undo;
    // undo and redo log themselves.
    let (method, params) = (req.method(), req.params());
    // A human's window writes only in the mode that allows it (design
    // 374, question 198); agents, and connections that said no hello,
    // write as they always have.
    let refused = mode_refusal(live.as_deref(), me, &req);
    let logs_itself = matches!(req, Request::Undo { .. } | Request::Redo { .. });
    let files_before = match (&live, mutates && !logs_itself && refused.is_none()) {
        (Some(_), true) => Some(session.files()),
        _ => None,
    };
    let mut refused = refused;
    let result = match req {
        _ if refused.is_some() => Err(refused.take().expect("checked")),
        Request::MeshGet {
            part,
            feature,
            features,
            quality,
            transfer: Some(t @ (Transfer::Binary | Transfer::Compact)),
            rev,
            flat: Some(solid),
        } => flat_alone(feature.as_deref(), &features, quality.as_deref())
            .and_then(|()| {
                mesh_flat_binary(
                    session,
                    &part,
                    &solid,
                    rev.as_deref(),
                    t == Transfer::Compact,
                )
            })
            .and_then(|mut a| {
                a.result = versioned(Ok(a.result))?;
                Ok(a)
            }),
        Request::MeshGet {
            part,
            feature,
            features,
            quality,
            transfer: Some(t @ (Transfer::Binary | Transfer::Compact)),
            rev,
            flat: None,
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
            datums,
            rev,
            frame,
            compare,
            hidden,
            origin,
            appearance,
            section,
            explode,
            fraction,
            annotations,
            annotation,
            flat,
        } => {
            let annotation = match annotation {
                None => None,
                Some(_) if camera.is_some() || view.is_some() => {
                    return Err(Error::new(
                        Code::Usage,
                        "--annotation renders its own view: give no camera or view",
                    ));
                }
                Some(id) => {
                    let a = crate::annotation::one(session, &id, None)?;
                    if a.view.is_none() {
                        return Err(Error::new(
                            Code::InvalidParam,
                            format!("annotation {id} keeps no view to render"),
                        )
                        .with("annotation", &id));
                    }
                    Some(a)
                }
            };
            // A flat pattern's render draws one solid flat: no section or
            // explode applies, the request's own refused and the window's
            // left out.
            if flat.is_some() {
                for (param, given) in [
                    ("section", section.is_some()),
                    ("explode", explode.is_some() || fraction.is_some()),
                ] {
                    if given {
                        return Err(Error::new(
                            Code::InvalidParam,
                            format!("{param} doesn't apply to a flat pattern's render"),
                        )
                        .with("param", param));
                    }
                }
            }
            let (section, explode) = if flat.is_some() {
                (Some(Vec::new()), Some(String::new()))
            } else {
                (section, explode)
            };
            // A named view brings the planes and the explode it saved,
            // unless the request names its own.
            let named = match (&camera, view.as_deref()) {
                (None, Some(v)) if StandardView::named(v).is_none() => {
                    let focus = live
                        .as_deref()
                        .and_then(|l| l.get(participant.as_deref()?).ok()?.view.focus.clone());
                    let at = crate::views::part_for(session, part.as_deref(), focus.as_ref())?;
                    Some(crate::views::named_saved(session, at.as_ref(), v)?)
                }
                _ => None,
            };
            let saved_sections: Option<Vec<crate::section::Plane>> = match &annotation {
                Some(a) => Some(
                    a.view
                        .as_ref()
                        .expect("checked")
                        .sections
                        .iter()
                        .map(|s| crate::section::Plane {
                            origin_m: s.origin_m,
                            normal: s.normal,
                        })
                        .collect(),
                ),
                None => named.as_ref().map(|s| s.sections.clone()),
            };
            let explode = match &named {
                Some(saved) => explode.or_else(|| {
                    Some(
                        saved
                            .explode
                            .as_ref()
                            .map_or_else(String::new, |s| s.explode.to_string()),
                    )
                }),
                None => explode,
            };
            let fraction = fraction.or(named
                .as_ref()
                .and_then(|s| s.explode.as_ref())
                .map(|s| s.fraction));
            // The participant's shown explode, unless one is named.
            let shown = match live.as_deref_mut() {
                Some(l) if explode.is_none() => {
                    let who = l.view_target(participant.as_deref(), me)?;
                    l.get(&who)?.view.explode.clone()
                }
                _ => None,
            };
            if let Some(f) = fraction
                && !(0.0..=1.0).contains(&f)
            {
                return Err(Error::new(
                    Code::InvalidParam,
                    "an explode's fraction is from 0 (assembled) to 1 (exploded)",
                )
                .with("fraction", f));
            }
            let explode = match (explode, shown) {
                (Some(e), _) if e.is_empty() => None,
                (Some(e), _) => Some(DrawExplode {
                    shown: None,
                    id: e,
                    fraction: fraction.unwrap_or(1.0),
                }),
                (None, Some(s)) => Some(DrawExplode {
                    shown: Some(s.part),
                    id: s.explode.to_string(),
                    fraction: fraction.unwrap_or(s.fraction),
                }),
                (None, None) => None,
            };
            // What the participant's window cuts away, as it hides.
            let sections = match (section, saved_sections) {
                (Some(args), _) => crate::section::resolve_all(session, &args, "/section")?,
                (None, Some(saved)) => saved,
                (None, None) => match live.as_deref_mut() {
                    Some(l) => {
                        let who = l.view_target(participant.as_deref(), me)?;
                        l.get(&who)?.view.sections.clone()
                    }
                    None => Vec::new(),
                },
            };
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
            // An annotation's render shows the model its marks name, else
            // its first pin's part.
            let part = part.or_else(|| {
                let a = annotation.as_ref()?;
                let p = match &a.marks {
                    Some(m) => m.part.to_string(),
                    None => a.pins.first()?.part().to_owned(),
                };
                (!p.is_empty()).then_some(p)
            });
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
                datums: datums.unwrap_or(true),
                rev,
                frame,
                compare,
                hidden,
                origin,
                appearance: appearance.unwrap_or(true),
                sections,
                explode,
                annotations,
                flat,
                camera_m: annotation
                    .as_ref()
                    .and_then(|a| a.view.as_ref())
                    .map(crate::view::Camera::from),
                annotation,
                pick: false,
            };
            render_noting(session, target, shown, ask)
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
            brep,
            mesh,
            pipe_tolerance,
            skip_failing,
            appearance,
            solid,
            notes,
            fixed,
            bends,
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
                brep,
                mesh,
                pipe_tolerance,
                skip_failing,
                appearance,
                solid,
                notes,
                fixed,
                bends,
            },
        ),
        Request::DrawingRender {
            part,
            drawing,
            rev,
            sheet,
            format,
            scale,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            let mut a = crate::drawing::request::render(
                s,
                &part,
                &drawing,
                sheet.as_deref(),
                format.as_deref(),
                scale,
            )?;
            a.result = noted(Ok(a.result), commit)?;
            Ok(a)
        }),
        Request::DrawingExport {
            part,
            drawing,
            rev,
            sheet,
            format,
            out,
            unit,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            let mut a = crate::drawing::request::export(
                s,
                &part,
                &drawing,
                sheet.as_deref(),
                format.as_deref(),
                out.as_deref(),
                unit.as_deref(),
            )?;
            a.result = noted(Ok(a.result), commit)?;
            Ok(a)
        }),
        Request::ReleasePackage {
            part,
            label,
            out,
            into,
            formats,
        } => crate::package::package(
            session,
            crate::package::Ask {
                part,
                label,
                out,
                into,
                formats,
            },
        ),
        Request::Rebuild { part, rev } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            rebuild(s, part.as_deref(), commit.as_deref())
                .map_err(|e| noted(Err(e), commit).expect_err("an error"))
        }),
        req => dispatch(session, live.as_deref_mut(), me, req).map(Answer::json),
    };
    // A stale annotation's revision says so in the answer.
    let notes = std::mem::take(&mut session.rev_warnings);
    let result = match result {
        Ok(mut a) if !notes.is_empty() && a.raw.is_none() => {
            let mut list = a
                .result
                .get("warnings")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for n in notes {
                if !list.contains(&n) {
                    list.push(n);
                }
            }
            a.result["warnings"] = json!(list);
            Ok(a)
        }
        r => r,
    };
    if let (Some(l), Some(files_before), true) = (live.as_deref_mut(), files_before, result.is_ok())
    {
        let files = activity::diff(&files_before, &session.files());
        let who = l.caller(me);
        if !files.is_empty() {
            activity::record(l, EntryKind::Change, &who, &method, params, None, files);
        } else if method == "release.sync" {
            activity::record(l, EntryKind::Refs, &who, &method, params, None, files);
        }
    }
    if let (Some(l), Some(before)) = (live, before) {
        // Entities that no longer exist drop out of every selection.
        if changed || mutates || switches {
            l.prune(|r| r.resolve(session).is_ok());
        }
        l.announce(&before);
        publish(l, session);
        // What the request used may go now that it has answered.
        session.trim();
    }
    result
}

pub(crate) fn dispatch(
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
            expressions,
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
            &expressions,
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
        Request::PdmNewId { expected_revision } => {
            crate::pdm::new_id(session, expected_revision.as_deref())
        }
        Request::PdmConfig {} => crate::pdm::config(session),
        Request::PdmConfigure {
            revisions,
            release_branch,
            pattern,
            properties,
            approvals,
            bump,
            package_formats,
            force,
            expected_revision,
        } => crate::pdm::configure(
            session,
            crate::pdm::Change {
                revisions,
                release_branch,
                pattern,
                properties,
                approvals,
                bump,
                package_formats,
                force,
            },
            expected_revision.as_deref(),
        ),
        Request::PropsGet { part, rev } => {
            let (_, session) = session.at(rev.as_deref())?;
            crate::pdm::get(session, &part)
        }
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
            within,
            no_dependents,
        } => crate::bom::where_used(
            session,
            &crate::bom::WhereUsed {
                part: &part,
                label: label.as_deref(),
                working,
                direct,
                within: &within,
                no_dependents,
            },
        ),
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
        Request::ReleasePackageVerify { file, rebuild } => {
            crate::package::verify(session, &file, rebuild)
        }
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
        Request::ParamList { part, scope, rev } => {
            let (_, session) = session.at(rev.as_deref())?;
            match param::Target::of(part, scope)? {
                param::Target::Part(p) => param::list(session, &p),
                param::Target::Project => param::list_project(session),
            }
        }
        Request::ParamSet {
            part,
            scope,
            parameters,
            expected_revision,
        } => match param::Target::of(part, scope)? {
            param::Target::Part(p) => {
                param::set(session, &p, parameters, expected_revision.as_deref())
            }
            param::Target::Project => {
                param::set_project(session, parameters, expected_revision.as_deref())
            }
        },
        Request::ParamDelete {
            part,
            scope,
            name,
            expected_revision,
        } => match param::Target::of(part, scope)? {
            param::Target::Part(p) => {
                param::delete(session, &p, &name, expected_revision.as_deref())
            }
            param::Target::Project => {
                param::delete_project(session, &name, expected_revision.as_deref())
            }
        },
        Request::ParamRename {
            part,
            scope,
            from,
            to,
            expected_revision,
        } => match param::Target::of(part, scope)? {
            param::Target::Part(p) => {
                param::rename(session, &p, &from, &to, expected_revision.as_deref())
            }
            param::Target::Project => {
                param::rename_project(session, &from, &to, expected_revision.as_deref())
            }
        },
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
            rev,
        } => {
            let (_, session) = session.at(rev.as_deref())?;
            let quality = match quality {
                Some(q) => mesh::Quality::parse(&q)?,
                None => mesh::Quality::preferred(),
            };
            crate::sketch::get(session, &part, &sketch, quality)
        }
        Request::PathGet {
            part,
            path,
            quality,
            rev,
        } => {
            let (_, session) = session.at(rev.as_deref())?;
            let quality = match quality {
                Some(q) => mesh::Quality::parse(&q)?,
                None => mesh::Quality::preferred(),
            };
            crate::path::get(session, &part, &path, quality)
        }
        Request::SweepGet {
            part,
            feature,
            quality,
            rev,
        } => {
            let (_, session) = session.at(rev.as_deref())?;
            let quality = match quality {
                Some(q) => mesh::Quality::parse(&q)?,
                None => mesh::Quality::preferred(),
            };
            crate::sweep_view::get(session, &part, &feature, quality)
        }
        Request::AssemblyMove {
            part,
            instance,
            placement,
            translate,
            axis,
            angle,
            about,
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
                about,
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
        Request::SheetGet { part, solid, rev } => {
            crate::sheet_metal::report::get(session, &part, solid.as_deref(), rev.as_deref())
        }
        Request::FeatureFacesGet { face, rev } => {
            crate::tweak::feature_faces_get(session, &face, rev.as_deref())
        }
        Request::BlendsGet {
            part,
            solid,
            tolerance,
            below,
            rev,
        } => crate::blends::get(
            session,
            &part,
            solid.as_deref(),
            tolerance.as_deref(),
            below.as_deref(),
            rev.as_deref(),
        ),
        Request::FlatGet {
            part,
            solid,
            rev,
            fixed,
            bends,
        } => crate::sheet_metal::flat::get(
            session,
            &part,
            solid.as_deref(),
            rev.as_deref(),
            &crate::sheet_metal::flat::Options { fixed, bends },
        ),
        Request::Mass { of, tolerance, rev } => {
            crate::mass::run(session, &of, tolerance, rev.as_deref())
        }
        Request::DepsAdd {
            url,
            path,
            name,
            expected_revision,
        } => crate::deps::add(
            session,
            &url,
            &path,
            name.as_deref(),
            expected_revision.as_deref(),
        ),
        Request::DepsFetch { names } => crate::deps::fetch(session, &names),
        Request::DepsList {} => crate::deps::list(session),
        Request::DepsOutdated {} => crate::deps::outdated(session),
        Request::DepsUses { rev } => crate::deps::uses(session, rev.as_deref()),
        Request::DepsRemove {
            name,
            expected_revision,
        } => crate::deps::remove(session, &name, expected_revision.as_deref()),
        Request::DepsUpdate {
            names,
            part,
            to,
            offline,
            dry_run,
            expected_revision,
        } => crate::deps::update(
            session,
            crate::deps::Update {
                names: &names,
                part: part.as_deref(),
                to: to.as_deref(),
                offline,
                dry_run,
            },
            expected_revision.as_deref(),
        ),
        Request::DepsParts { name, part } => crate::deps::parts(session, &name, part.as_deref()),
        Request::InsertUpdate {
            part,
            feature,
            to,
            dry_run,
            expected_revision,
        } => crate::insert::update(
            session,
            crate::insert::Update {
                part: &part,
                feature: &feature,
                to: to.as_deref(),
                dry_run,
            },
            expected_revision.as_deref(),
        ),
        Request::MaterialsList {} => crate::materials::list(session),
        Request::EquationsList { part, all, rev } => crate::equations::list(
            session,
            &crate::equations::Ask { part, all },
            rev.as_deref(),
        ),
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
        Request::AppearanceGet {
            part,
            participant,
            rev,
        } => {
            // A participant's appearance is of what its window shows.
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
            let (commit, s) = session.at(rev.as_deref())?;
            noted(crate::appearance::get(s, part.as_deref()), commit)
        }
        Request::AppearanceSet {
            entities,
            colour,
            material,
            opacity,
            unset,
            file_styles,
            expected_revision,
        } => {
            let change = crate::appearance::Change {
                colour,
                material,
                opacity,
                unset,
                file_styles,
            };
            crate::appearance::set(session, &entities, &change, expected_revision.as_deref())
        }
        Request::AppearanceClear {
            entities,
            expected_revision,
        } => crate::appearance::clear(session, &entities, expected_revision.as_deref()),
        Request::AssemblySolve {
            part,
            moves,
            component,
            rev,
            mates,
        } => {
            let moves = moves
                .into_iter()
                .map(|m| {
                    (
                        m.instance,
                        crate::assembly::Move {
                            placement: m.placement,
                            translate: m.translate,
                            axis: m.axis,
                            angle: m.angle,
                            about: m.about,
                        },
                    )
                })
                .collect();
            solve_at(session, &part, moves, component, rev.as_deref(), mates)
        }
        Request::FeatureShow { part, id, rev } => {
            let (_, session) = session.at(rev.as_deref())?;
            let (part, fid) = (part_id(&part)?, feature_id(&id)?);
            let p = session.project().part(&part)?;
            let mut v = feature_report(p, &fid, feature(p, &fid)?);
            // The revision it was read at: an edit made from it says so.
            v["revision"] = json!(p.revision);
            Ok(v)
        }
        Request::MergeStatus {} => crate::validate::status(session),
        Request::MergePreview { rev } => crate::validate::preview(session, &rev),
        Request::WorktreeList {} => crate::worktree::list(session, live.as_deref()),
        Request::BranchList {} => crate::branch::list(session),
        Request::BranchSwitch {
            branch,
            expected_revision,
        } => crate::branch::switch(session, live, me, &branch, expected_revision.as_deref()),
        Request::MergeResolve {
            resolutions,
            expected_revisions,
        } => crate::validate::resolve(session, &resolutions, &expected_revisions),
        Request::DaemonStatus {} | Request::DaemonShutdown {} => Err(Error::new(
            Code::Usage,
            "only a running daemon answers daemon requests",
        )),
        // A daemon answers this itself, without the lock; in process
        // nothing else is running.
        Request::RequestCancel { token, .. } => Ok(json!({"cancelled": false, "token": token})),
        Request::Import {
            path,
            part,
            into,
            product,
            dry_run,
            expected_revision,
            update,
            solid,
            flatten,
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
                solid,
                flatten,
            },
        ),
        Request::ImportReport { file, detail } => {
            crate::import::transfer_report(session, &file, detail)
        }
        Request::ImportConvert {
            part,
            feature,
            reference,
            expected_revision,
        } => crate::import::convert::convert(
            session,
            crate::import::convert::Ask {
                part,
                feature,
                reference,
                expected: expected_revision,
            },
        ),
        // A rebuild's report is written as text: `answer_live` handles it.
        Request::Rebuild { .. } => Err(Error::new(Code::Internal, "rebuild answers as text")),
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
            // A named view restores the planes and the explode it saved,
            // as well as its camera.
            let mut restore = None;
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
                    if let Some(v) = view.as_deref().filter(|v| StandardView::named(v).is_none()) {
                        let at = crate::views::part_for(session, part.as_deref(), focus.as_ref())?;
                        restore = Some(crate::views::named_saved(session, at.as_ref(), v)?);
                    }
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
            if let Some(saved) = restore {
                live.set_sections(&id, saved.sections)?;
                live.set_explode(&id, saved.explode)?;
            }
            let v = &live.get(&id)?.view;
            Ok(
                json!({"participant": id, "camera": camera, "sections": v.sections, "explode": v.explode}),
            )
        }
        Request::ViewList { part } => crate::views::list(session, part.as_deref()),
        Request::ViewSave {
            part,
            view,
            camera,
            participant,
            expected_revision,
        } => {
            // A participant's camera comes with its planes and explode.
            let (camera, sections, explode) = match camera {
                Some(c) => (
                    c.resolve(session.project().manifest.default_unit)?,
                    Vec::new(),
                    None,
                ),
                None => {
                    let live = live.ok_or_else(|| live::no_live_session("view.save"))?;
                    let who = live.compare_target(participant.as_deref(), me)?;
                    let v = &live.get(&who)?.view;
                    let camera = v.camera.clone().ok_or_else(|| {
                        Error::new(
                            Code::InvalidParam,
                            format!("participant {who} has no camera to save"),
                        )
                        .with("participant", &who)
                    })?;
                    (camera, v.sections.clone(), v.explode.clone())
                }
            };
            crate::views::save(
                session,
                &part,
                &view,
                &camera,
                &sections,
                explode.as_ref(),
                expected_revision.as_deref(),
            )
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
                if crate::session::worktree_path(&rev).is_some() {
                    return json!({"rev": rev, "commit": null});
                }
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
            // Another worktree is named by its project folder, however the
            // caller named it.
            let rev = match rev.as_deref().map(|r| session.worktree_files(r)) {
                Some(found) => match found? {
                    Some((w, ..)) => {
                        Some(format!("{}{}", crate::session::WORKTREE, w.root.display()))
                    }
                    None => rev,
                },
                None => None,
            };
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
        Request::DrawingList { part } => crate::drawing::request::list(session, part.as_deref()),
        Request::DrawingCreate {
            part,
            drawing,
            name,
            template,
            size,
            views,
            expected_revision,
        } => crate::drawing::request::create(
            session,
            &part,
            &drawing,
            &crate::drawing::request::Create {
                name,
                template,
                size,
                views,
            },
            expected_revision.as_deref(),
        ),
        Request::DrawingEdit {
            part,
            drawing,
            ops,
            expected_revision,
        } => crate::drawing::request::edit(
            session,
            &part,
            &drawing,
            &ops,
            expected_revision.as_deref(),
        ),
        Request::DrawingDelete {
            part,
            drawing,
            expected_revision,
        } => {
            crate::drawing::request::delete(session, &part, &drawing, expected_revision.as_deref())
        }
        Request::DrawingRename {
            part,
            drawing,
            to,
            expected_revision,
        } => crate::drawing::request::rename(
            session,
            &part,
            &drawing,
            &to,
            expected_revision.as_deref(),
        ),
        Request::DrawingLayout {
            part,
            drawing,
            dry_run,
            expected_revision,
        } => crate::drawing::request::layout(
            session,
            &part,
            &drawing,
            dry_run,
            expected_revision.as_deref(),
        ),
        Request::DrawingGet {
            part,
            drawing,
            rev,
            sheet,
            items,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            noted(
                crate::drawing::request::get(s, &part, &drawing, sheet.as_deref(), items),
                commit,
            )
        }),
        Request::DrawingDescribe {
            part,
            drawing,
            rev,
            sheet,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            noted(
                crate::drawing::request::describe(s, &part, &drawing, sheet.as_deref()),
                commit,
            )
        }),
        Request::DrawingLint {
            part,
            drawing,
            rev,
            sheet,
            explain,
        } => session.at(rev.as_deref()).and_then(|(commit, s)| {
            noted(
                crate::drawing::request::lint(
                    s,
                    part.as_deref(),
                    drawing.as_deref(),
                    sheet.as_deref(),
                    explain.as_deref(),
                ),
                commit,
            )
        }),
        Request::DrawingLintConfigure {
            weights,
            severity,
            expected_revision,
        } => crate::drawing::request::lint_configure(
            session,
            &weights,
            &severity,
            expected_revision.as_deref(),
        ),
        Request::ExplodeList { part } => crate::explode::list(session, part.as_deref()),
        Request::ExplodeSave {
            part,
            explode,
            steps,
            expected_revision,
        } => crate::explode::save(
            session,
            &part,
            &explode,
            &steps,
            expected_revision.as_deref(),
        ),
        Request::ParticipantMode { mode } => {
            let live = live.ok_or_else(|| live::no_live_session("participant.mode"))?;
            let id = live.caller(me);
            live.set_mode(&id, mode)?;
            Ok(json!({"participant": id, "mode": mode}))
        }
        Request::AnnotationApply {
            id,
            keep_open,
            expect,
            author,
        } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            crate::annotation::propose::apply(session, &id, keep_open, expect.as_deref(), &by)
        }
        Request::AnnotationCreate {
            pins,
            comment,
            camera,
            with_view,
            marks,
            view: named,
            author,
        } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            let unit = session.project().manifest.default_unit;
            let view = match (camera, with_view, named) {
                (Some(c), false, None) => Some(c.resolve(unit)?.to_view_file()),
                (None, false, Some(v)) => {
                    let part = marks
                        .as_ref()
                        .map(|m| m.part.clone())
                        .or_else(|| {
                            pins.first().and_then(|p| {
                                let r = p.reference.clone().or(p.part.clone())?;
                                Some(r.split('/').next().unwrap_or_default().to_owned())
                            })
                        })
                        .ok_or_else(|| {
                            Error::new(
                                Code::Usage,
                                "a named view is of the marks' part: give marks or a pin",
                            )
                        })?;
                    Some(crate::views::view_file(session, &part_id(&part)?, &v)?)
                }
                (None, true, None) => {
                    let l = live
                        .as_deref()
                        .ok_or_else(|| live::no_live_session("annotation.create --view"))?;
                    let camera = me
                        .and_then(|m| l.get(m).ok())
                        .and_then(|p| p.view.camera.clone())
                        .ok_or_else(|| {
                            Error::new(
                                Code::InvalidParam,
                                "your participant has no camera to keep (camera.set first)",
                            )
                        })?;
                    Some(camera.to_view_file())
                }
                (None, false, None) => None,
                _ => {
                    return Err(Error::new(
                        Code::Usage,
                        "give one of camera, --view and --of",
                    ));
                }
            };
            let from = pick_from(live.as_deref(), me);
            crate::annotation::create(session, by, &pins, &comment, view, marks.as_ref(), &from)
        }
        Request::AnnotationList {
            status,
            part,
            by,
            open,
        } => crate::annotation::list(session, &status, part.as_deref(), by.as_deref(), open),
        Request::AnnotationShow { id } => crate::annotation::show(session, &id),
        Request::AnnotationReply {
            id,
            text,
            expect,
            author,
        } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            crate::annotation::reply(session, &id, &text, expect.as_deref(), by)
        }
        Request::AnnotationClaim { id, expect, author } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            crate::annotation::claim(session, &id, expect.as_deref(), by)
        }
        Request::AnnotationResolve {
            id,
            note,
            expect,
            author,
        } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            crate::annotation::close(session, &id, None, note.as_deref(), expect.as_deref(), by)
        }
        Request::AnnotationReject {
            id,
            reason,
            expect,
            author,
        } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            crate::annotation::close(session, &id, Some(&reason), None, expect.as_deref(), by)
        }
        Request::AnnotationReopen { id, expect, .. } => {
            crate::annotation::reopen(session, &id, expect.as_deref())
        }
        Request::AnnotationEdit {
            id,
            comment,
            pins,
            marks,
            no_marks,
            expect,
            author,
        } => {
            if no_marks && marks.is_some() {
                return Err(Error::new(
                    Code::Usage,
                    "--no-marks takes them away: give no marks with it",
                ));
            }
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            let from = pick_from(live.as_deref(), me);
            crate::annotation::edit(
                session,
                &id,
                comment.as_deref(),
                pins.as_deref(),
                marks.as_ref(),
                no_marks,
                &from,
                expect.as_deref(),
                by,
            )
        }
        Request::AnnotationDelete { id, expect, author } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            crate::annotation::delete(session, &id, expect.as_deref(), by)
        }
        Request::AnnotationPropose {
            request,
            comment,
            pins,
            rough,
            author,
        } => {
            let by = crate::annotation::author(live.as_deref(), me, author.as_deref());
            crate::annotation::propose::propose(
                session,
                live,
                me,
                crate::annotation::propose::Ask {
                    request,
                    comment,
                    pins,
                    rough,
                    by,
                },
            )
        }
        Request::ExplodeDelete {
            part,
            explode,
            expected_revision,
        } => {
            let v = crate::explode::delete(session, &part, &explode, expected_revision.as_deref())?;
            // No window goes on showing it.
            if let Some(l) = live {
                let showing: Vec<String> = l
                    .participants()
                    .filter(|p| {
                        p.view.explode.as_ref().is_some_and(|e| {
                            e.part.as_str() == v["part"] && e.explode.as_str() == v["explode"]
                        })
                    })
                    .map(|p| p.id.clone())
                    .collect();
                for id in showing {
                    l.set_explode(&id, None)?;
                }
            }
            Ok(v)
        }
        Request::ExplodeShow {
            part,
            explode,
            fraction,
            off,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("explode.show"))?;
            let id = live.compare_target(participant.as_deref(), me)?;
            let (now, focus) = {
                let v = &live.get(&id)?.view;
                (v.explode.clone(), v.focus.clone())
            };
            let ask = crate::explode::ShowAsk {
                part,
                explode,
                fraction,
                off,
            };
            let shown = crate::explode::show(session, now.as_ref(), focus.as_ref(), &ask)?;
            live.set_explode(&id, shown.clone())?;
            Ok(crate::explode::shown_json(session, &id, shown.as_ref()))
        }
        // Where `section.set` would cut: `participant`, else the one
        // human, else the caller.
        Request::SectionGet { participant } => match live {
            Some(l) => {
                let id = l.view_target(participant.as_deref(), me)?;
                Ok(json!({"participant": id, "sections": l.get(&id)?.view.sections}))
            }
            None => Ok(json!({"participant": null, "sections": []})),
        },
        Request::SectionSet {
            planes,
            add,
            off,
            participant,
        } => {
            let live = live.ok_or_else(|| live::no_live_session("section.set"))?;
            if off && (add || !planes.is_empty()) {
                return Err(Error::new(
                    Code::Usage,
                    "give planes to cut with, or off, not both",
                ));
            }
            if !off && planes.is_empty() {
                return Err(Error::new(
                    Code::Usage,
                    "give a plane to cut with, or off to cut with none",
                ));
            }
            let id = live.compare_target(participant.as_deref(), me)?;
            let new = crate::section::resolve_all(session, &planes, "/planes")?;
            let sections = crate::section::apply(&live.get(&id)?.view.sections, new, add)?;
            live.set_sections(&id, sections)?;
            Ok(json!({"participant": id, "sections": live.get(&id)?.view.sections}))
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
            flat: Some(solid),
        } => {
            flat_alone(feature.as_deref(), &features, quality.as_deref())?;
            versioned(mesh_flat_json(session, &part, &solid, rev.as_deref()))
        }
        Request::MeshGet {
            part,
            feature,
            features,
            quality,
            transfer: _,
            rev,
            flat: None,
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
        Request::ReleasePackage { .. } => Err(Error::new(
            Code::Internal,
            "release.package answers with bytes",
        )),
        Request::DrawingRender { .. } | Request::DrawingExport { .. } => Err(Error::new(
            Code::Internal,
            "drawings render and export with bytes",
        )),
        // The session is current: doctor reuses its rebuilds (#125).
        Request::Doctor {} => {
            let root = session.root().to_owned();
            crate::doctor::report_in(&mut crate::doctor::Builds::live(&root, session))
        }
        Request::DoctorRepair { repairs, params } => {
            let params = crate::doctor::Params::from_json(&params)?;
            let r = crate::doctor::repair_with(
                session.root(),
                &repairs,
                &params,
                session.deps_cache.clone(),
            )?;
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
            let mut v = config::get(&session.project().manifest, key.as_deref(), scope)?;
            v["project_revision"] = json!(session.project_revision()?);
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
                    session.check_project_revision(expected_revision.as_deref())?;
                    // The rest of the manifest (the project's ID) stays.
                    let m = config::set_project(&session.project().manifest, &key, &value);
                    session.write_json(&session.root().join(MANIFEST), &m.to_json())?;
                    session.reload()?;
                    let mut v = config::get(&m, Some(&key), None)?;
                    v["project_revision"] = json!(session.project_revision()?);
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
    handle_job(slot, live, me, root, req, &Job::default())
}

/// [`handle_safely`] for a request with a job: its progress goes where
/// the job sends it, and it stops at its next boundary once cancelled
/// (`cancelled`). The session holds the job while it answers, and no job
/// after.
pub fn handle_job(
    slot: &mut Option<Session>,
    live: Option<&mut Live>,
    me: Option<&str>,
    root: &Path,
    req: Request,
    job: &Job,
) -> Result<Answer> {
    handle_job_raw(slot, live, me, root, req, job).and_then(Answer::into_json)
}

/// [`handle_job`] for the daemon: an answer written as text (a
/// rebuild's report, #533) stays text, for the response line to carry.
pub fn handle_job_raw(
    slot: &mut Option<Session>,
    live: Option<&mut Live>,
    me: Option<&str>,
    root: &Path,
    req: Request,
    job: &Job,
) -> Result<Answer> {
    let watch = live.is_some();
    guarded(slot, root, watch, |s| {
        s.job = job.clone();
        // A `config set heal.budget` or `edit.budget` in another process
        // reaches this session at its next request.
        s.set_heal_budget(crate::session::heal_budget());
        s.set_edit_budget(crate::session::edit_budget());
        let a = answer_live(s, live, me, req);
        s.job = Job::default();
        // A write a cancel came too late for says so, as a daemon's does.
        // A failure written as text is a failure, which settling leaves.
        let a = match a {
            Ok(a) if a.fails() => return Ok(a),
            a => a,
        };
        job.settle(a).map(|mut a| {
            if job.ignored() && a.result.is_object() {
                a.result["cancel_ignored"] = json!(true);
            }
            a
        })
    })
}

/// The display meshes a `mesh.get` of the working files needs that the
/// cache doesn't hold yet, which the daemon makes without its lock, so a
/// window fetching a fine mesh of a large part doesn't hold up every other
/// request (#91). `handle` then finds them cached. The same for the
/// fetches of `deps.add`, `deps.fetch` and `deps.update`
/// ([`crate::deps::prepare`]), so a slow remote holds up no one else.
pub fn prepare(
    slot: &mut Option<Session>,
    req: &Request,
) -> Option<ryu_session::Prepared<Session>> {
    prepare_job(slot, req, &Job::default())
}

/// [`prepare`] for a request with a job: the meshes it makes without the
/// lock stop, body by body, when it is cancelled, and those it made are
/// kept.
pub fn prepare_job(
    slot: &mut Option<Session>,
    req: &Request,
    job: &Job,
) -> Option<ryu_session::Prepared<Session>> {
    if let Some(session) = slot.as_mut()
        && let Some(fetch) = crate::deps::prepare(session, req, job)
    {
        return Some(Box::new(move || {
            let got = fetch();
            Box::new(move |s: &mut Session| s.prefetched = got)
        }));
    }
    let Request::MeshGet {
        part,
        feature,
        features,
        quality,
        rev,
        flat: None,
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
    let (budget, job) = (mesh_budget(), job.clone());
    Some(Box::new(move || {
        let made = pending.make_for(&job);
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
    session.write_json(
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
    // So would a part inserting it at working (design 521).
    let users: Vec<(bool, String)> = session
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
                        Some((false, format!("{a}/{fid}")))
                    }
                    Some(Ok(Feature::Insert(i))) if i.part == *placed && i.rev == Rev::Working => {
                        Some((true, format!("{a}/{fid}")))
                    }
                    _ => None,
                })
        })
        .collect();
    if !users.is_empty() {
        let (inserts, instances): (Vec<_>, Vec<_>) = users.into_iter().partition(|(i, _)| *i);
        let instances: Vec<String> = instances.into_iter().map(|(_, u)| u).collect();
        let inserts: Vec<String> = inserts.into_iter().map(|(_, u)| u).collect();
        let mut said = Vec::new();
        if !instances.is_empty() {
            said.push(format!("assemblies place {id}: {}", instances.join(", ")));
        }
        if !inserts.is_empty() {
            said.push(format!("parts insert {id}: {}", inserts.join(", ")));
        }
        let mut e = Error::new(Code::HasDependents, said.join("; ")).with("part", &id);
        if !instances.is_empty() {
            e = e.with("instances", instances);
        }
        if !inserts.is_empty() {
            e = e.with("inserted_by", inserts);
        }
        return Err(e);
    }
    let dir = part_dir(session.root(), &id);
    let mine = format!("parts/{id}/");
    let files: Vec<String> = session
        .snapshot()
        .keys()
        .filter(|p| p.starts_with(&mine))
        .cloned()
        .collect();
    for rel in files {
        let at = rel
            .split('/')
            .fold(session.root().to_path_buf(), |p, s| p.join(s));
        session.remove_file(&at)?;
    }
    if dir.exists() {
        // no session: what the files leave on disk (empty folders, stray files) goes too
        fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, &e))?;
    }
    // A daemon sees its own removal whatever file events say (#263).
    crate::doc::source::note_written(&dir);
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
pub fn feature_report(p: &Part, id: &Id, f: &Feature) -> Value {
    let part = &p.id;
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
        Feature::MoveFace(_)
        | Feature::ReplaceFace(_)
        | Feature::Draft(_)
        | Feature::DeleteFace(_)
        | Feature::DeleteBlend(_)
        | Feature::ResizeBlend(_) => crate::tweak::report(part, f),
        Feature::Datum(d) => crate::datum::report(part, d),
        Feature::Path(x) => crate::path::report(part, x),
        Feature::Sweep(w) => crate::along::feature_report(part, w),
        Feature::Loft(l) => crate::loft::feature_report(part, l),
        Feature::Planar(x) => {
            let mut v = x.fields();
            if let crate::doc::sweep::Source::Face(g) = &x.profile.source {
                v["profile"]["face"] = json!(g.text(part));
            }
            v
        }
        Feature::Instance(i) => i.fields(),
        Feature::Insert(i) => i.fields(),
        Feature::DeleteBody(d) => d.fields(),
        Feature::Import(i) => i.fields(),
        Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_) => {
            crate::surface::report(part, f)
        }
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
        Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
            crate::body::report(part, f)
        }
        Feature::BaseFlange(_) | Feature::Flange(_) | Feature::Unfold(_) | Feature::Refold(_) => {
            crate::sheet_metal::report(p, f)
        }
        Feature::Hole(h) => hole_report(part, h),
        Feature::Pattern(x) => pattern_report(part, x),
        Feature::Revolve(r) => {
            let mut v = r.fields();
            if let Angle::Of(q) = &r.angle {
                v["angle"] = q.report();
            }
            if let RevolveAxis::Ref(e) = &r.axis {
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
    for (role, r) in x.geometry() {
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
    if let Plane::Datum(g) = &s.plane {
        v["plane"] = json!(g.text(part));
    }
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
        session.write_json(&feature_path(session.root(), part, id), &f.to_json())?;
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
    let file = PartFile {
        kind: part_kind(p),
        features: order,
        items,
    }
    .to_json();
    session.write_json(&part_path(session.root(), part), &file)?;
    session.reload()?;
    // What the working files place from other projects may have changed.
    if crate::deps::keep_lock(session)? {
        session.reload()?;
    }
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

/// Reads what a primitive is given: its sizes are parameters, but a
/// cylinder's, cone's or sphere's radius may come as the fillet's
/// `radius` field (the CLI's `--radius`), which is then that parameter
/// (issue #205). Any other field it doesn't take is refused, saying
/// where its sizes go.
fn primitive_fields(ty: PrimitiveType, given: &mut Given) -> Result<()> {
    if ty.required().contains(&"radius")
        && let Some(r) = given.fields.radius.take()
    {
        if given.params.contains_key("radius") {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "a {}'s radius is given twice: as radius and as a parameter",
                    ty.as_str()
                ),
            )
            .with("param", "radius"));
        }
        given.params.insert("radius".into(), r);
    }
    given.only(ty.as_str(), &PRIMITIVE).map_err(|mut e| {
        let sizes: Vec<&str> = ty.required().iter().chain(ty.optional()).copied().collect();
        e.message = format!(
            "{}: a {}'s sizes are parameters ({}), given as --param NAME=QUANTITY \
             (params in a request)",
            e.message,
            ty.as_str(),
            sizes.join(", ")
        );
        e
    })
}

/// The fields a sketch takes.
const SKETCH: [&str; 5] = ["plane", "placement", "face", "face_origin", "face_x"];

/// The fields a primitive, an extrude, a revolve and a boolean take.
const PRIMITIVE: [&str; 4] = ["params", "placement", "operation", "targets"];
const EXTRUDE: [&str; 7] = [
    "sketch",
    "loops",
    "extent",
    "reverse",
    "surface",
    "operation",
    "targets",
];
const REVOLVE: [&str; 8] = [
    "sketch",
    "loops",
    "axis",
    "angle",
    "symmetric",
    "surface",
    "operation",
    "targets",
];
const BOOLEAN: [&str; 4] = ["op", "target", "targets", "tools"];
const INSTANCE: [&str; 4] = ["of", "rev", "placement", "fixed"];

/// The fields an insert takes: its source (`of`), the revision it pins
/// and its placement.
const INSERT: [&str; 3] = ["of", "rev", "placement"];
const DELETE_BODY: [&str; 2] = ["solids", "solid"];
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

/// A connector read as an occurrence whose path's last step is a datum of
/// the part the rest of it reaches (`<asm>/<instance>/…/<datum>`): a step
/// names an instance while the part before it is an assembly, and a
/// feature once it is a part. Then it is at that datum.
fn datum_at(project: &crate::doc::project::Project, asm: &Id, c: &mut Value) {
    let Some(path) = c["instance"].as_array().cloned() else {
        return;
    };
    let Some((last, steps)) = path.split_last() else {
        return;
    };
    if steps.is_empty() {
        return;
    }
    let mut part = asm.clone();
    for step in steps {
        let Some(step) = step.as_str() else { return };
        // A copy's seed follows it, an instance of the same assembly.
        if step.contains(':') {
            continue;
        }
        let Ok(id) = Id::parse(step, "instance") else {
            return;
        };
        let Some(Ok(Feature::Instance(i))) =
            project.parts.get(&part).and_then(|p| p.features.get(&id))
        else {
            return;
        };
        part = i.part.clone();
    }
    let Some(p) = project.parts.get(&part) else {
        return;
    };
    let datum = last
        .as_str()
        .and_then(|l| Id::parse(l, "feature").ok())
        .filter(|d| matches!(p.features.get(d), Some(Ok(Feature::Datum(_)))));
    if part_kind(p) == PartKind::Part
        && let Some(d) = datum
    {
        c["instance"] = json!(steps);
        c["at"] = json!({"feature": d});
    }
}

/// A connector's text, as requests give it.
pub fn connector_text(asm: &Id, c: &crate::doc::mate::Connector) -> String {
    let path = c.path().join("/");
    match &c.at {
        crate::doc::mate::At::Origin => format!("{asm}/{path}"),
        crate::doc::mate::At::Entity(r) => format!("{asm}/{path}/{}/{}", r.feature, r.name),
        crate::doc::mate::At::Datum(d) => format!("{asm}/{path}/{d}"),
    }
}

/// A mate from `current`'s fields (none for a new one) with what was
/// given laid over them: connectors as text, each keeping its offset
/// unless one is given (`{}` for none); `angle` or `distance` `""` lets it
/// go; a new form keeps only the driven values it takes.
fn mate_from(
    current: Option<&crate::doc::mate::Mate>,
    given: &Given,
    (project, part): (&crate::doc::project::Project, &Id),
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
            datum_at(project, part, &mut c);
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
    // A geometric reference given as text, kept as a file holds it.
    let entity = |t: &str, at: &str| geom_json(t, part, at);
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
            d["up_to"]["target"] = geom_json(t, part, "/depth/up_to/target")?;
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
/// given laid over them. A new instance needs `of`: a part, or another
/// project's as `<dependency>:<part>` (a dependency of `deps` by name or
/// project ID), which needs a release label. Its revision is `working`
/// and its pose the assembly's frame unless given. The origin takes
/// quantities and expressions, held as meters.
fn instance_from(
    current: Option<&Instance>,
    given: &Given,
    unit: Unit,
    env: &Env,
    deps: &crate::doc::deps::Dependencies,
) -> Result<Instance> {
    let f = &given.fields;
    // Another project's: by `of`, else as the current instance is.
    let mut project = current.and_then(|c| c.rev.project().cloned());
    let part = match (&f.of, current) {
        (Some(of), _) => match of.split_once(':') {
            Some((dep, part)) => {
                let (id, _) =
                    crate::doc::deps::find(deps, dep).map_err(|e| e.with("param", "of"))?;
                project = Some(id.clone());
                Id::parse(part, "part").map_err(|e| e.with("param", "of"))?
            }
            None => {
                project = None;
                Id::parse(of, "part").map_err(|e| e.with("param", "of"))?
            }
        },
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
    let rev = match (project, rev.label()) {
        (None, _) => match rev {
            Rev::Foreign { label, .. } => Rev::Label(label),
            r => r,
        },
        (Some(project), Some(label)) => Rev::Foreign {
            label: label.to_owned(),
            project,
        },
        (Some(project), None) => {
            return Err(Error::new(
                Code::InstanceForeignWorking,
                format!(
                    "{part} is another project's ({project}): place one of its official release labels (rev)"
                ),
            )
            .with("param", "rev")
            .with("project", &project)
            .with("part", &part));
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

/// An insert from a request: `of` (a part, or `<dependency>:<part>`),
/// `rev` (`working`, a label; left out: as `current` has it, or for a
/// new insert the source's newest official label, else `working`) and
/// `placement`. Answers the insert and, for a new one with no `rev`,
/// which it took and why (design 521, question 624).
fn insert_from(
    session: &mut Session,
    current: Option<&crate::doc::insert::Insert>,
    given: &Given,
    unit: Unit,
    env: &Env,
) -> Result<(crate::doc::insert::Insert, Option<Value>)> {
    let deps = session.project().manifest.dependencies.clone();
    let f = &given.fields;
    let (part, project) = match (f.of.as_deref(), current) {
        (Some(of), _) => match of.split_once(':') {
            Some((dep, part)) => {
                let (id, _) =
                    crate::doc::deps::find(&deps, dep).map_err(|e| e.with("param", "of"))?;
                let id = id.clone();
                (
                    Id::parse(part, "part").map_err(|e| e.with("param", "of"))?,
                    Some(id),
                )
            }
            None => (
                Id::parse(of, "part").map_err(|e| e.with("param", "of"))?,
                None,
            ),
        },
        (None, Some(c)) => (c.part.clone(), c.rev.project().cloned()),
        (None, None) => {
            return Err(Error::new(
                Code::InvalidParam,
                "an insert names the part it inserts (of)",
            )
            .with("param", "of"));
        }
    };
    let mut took = None;
    let rev = match (f.rev.as_deref(), current) {
        (Some("working"), _) => Rev::Working,
        (Some(l), _) => {
            Rev::Label(crate::doc::assembly::label_text(l).map_err(|e| e.with("param", "rev"))?)
        }
        (None, Some(c)) if c.part == part => match &c.rev {
            Rev::Foreign { label, .. } => Rev::Label(label.clone()),
            r => r.clone(),
        },
        (None, _) => match crate::release::newest_official(session, &part, project.as_ref())? {
            Some(l) => {
                took = Some(json!({"rev": {"label": l}, "why": "newest_official_label"}));
                Rev::Label(l)
            }
            None => {
                took = Some(json!({"rev": "working", "why": "no_official_label"}));
                Rev::Working
            }
        },
    };
    let rev = match (project, rev) {
        (Some(project), Rev::Label(label)) => Rev::Foreign { project, label },
        (Some(project), _) => {
            return Err(Error::new(
                Code::InstanceForeignWorking,
                format!(
                    "{part} is another project's ({project}): insert one of its official release labels (rev)"
                ),
            )
            .with("param", "rev")
            .with("project", &project)
            .with("part", &part));
        }
        (None, r) => r,
    };
    let placement = match (&given.placement, current) {
        (Some(arg), Some(c)) => {
            let mut p = c
                .placement
                .clone()
                .unwrap_or_else(|| Placement::identity(unit));
            apply_placement(&mut p, arg.clone(), unit, env)?;
            Some(p)
        }
        (Some(arg), None) => Some(new_placement(arg.clone(), unit, env)?),
        (None, Some(c)) => c.placement.clone(),
        (None, None) => None,
    };
    // Refuse a placement that could never build, before it is written.
    if let Some(p) = &placement {
        rebuild::placement(p)?;
    }
    let placement = placement.filter(|p| !p.is_identity());
    Ok((
        crate::doc::insert::Insert {
            part,
            rev,
            placement,
        },
        took,
    ))
}

/// Refuses insert `id` of part `pid`, listed in `list`, that can't build:
/// a part that isn't there (`not_found`), an assembly (`wrong_part_kind`,
/// question 625), a cycle through inserts and instances
/// (`insert_cycle`), a label that doesn't resolve now
/// (`label_not_found`, `label_not_official`), a source that shows no
/// solid (`insert_source_empty`).
fn check_insert(session: &mut Session, pid: &Id, list: &[Id], id: &Id, f: &Feature) -> Result<()> {
    let ins = f.insert().expect("an insert");
    if let Rev::Foreign { .. } = &ins.rev {
        // The lock the write would need, from what was fetched, as a
        // foreign instance's.
        let mut project = session.project().clone();
        let q = proposed(project.part(pid)?, list, &[(id, f)]);
        project.parts.insert(pid.clone(), q);
        crate::deps::relock_keeping(session, &project).map_err(|e| e.with("feature", id))?;
        return Ok(());
    }
    let Some(src) = session.project().parts.get(&ins.part) else {
        return Err(Error::new(
            Code::NotFound,
            format!(
                "{id} inserts {}, which isn't a part of the project",
                ins.part
            ),
        )
        .with("feature", id)
        .with("part", &ins.part)
        .with("param", "of"));
    };
    if part_kind(src) == PartKind::Assembly {
        return Err(crate::insert::assembly_error(id, &ins.part));
    }
    match &ins.rev {
        Rev::Working => {
            let mut project = session.project().clone();
            let q = proposed(project.part(pid)?, list, &[(id, f)]);
            project.parts.insert(pid.clone(), q);
            let cycles = crate::assembly::Cycles::of(&project, &crate::assembly::Pins::new());
            let (a, c) = (
                (pid.clone(), Rev::Working),
                (ins.part.clone(), Rev::Working),
            );
            if cycles.closes(&a, &c) {
                return Err(crate::assembly::cycle_error(&cycles, &a, &c).with("feature", id));
            }
        }
        Rev::Label(l) => {
            crate::release::pinned(session, &ins.part, l).map_err(|e| e.with("feature", id))?;
        }
        Rev::Foreign { .. } => unreachable!("judged above"),
    }
    let model = session.model(&ins.part, &ins.rev)?;
    if model
        .occurrences
        .first()
        .is_none_or(|o| o.solids.is_empty())
    {
        return Err(Error::new(
            Code::InsertSourceEmpty,
            format!("{id} inserts {}, which shows no solid", ins.part),
        )
        .with("feature", id)
        .with("part", &ins.part));
    }
    Ok(())
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
    if let Rev::Foreign { .. } = &i.rev {
        // The lock the write would need, from what was fetched: never the
        // network. Only what the write adds is judged; what the lock holds
        // now stands.
        let mut project = session.project().clone();
        let q = proposed(project.part(pid)?, list, &[(id, f)]);
        project.parts.insert(pid.clone(), q);
        crate::deps::relock_keeping(session, &project).map_err(|e| e.with("feature", id))?;
        return Ok(());
    }
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
        Rev::Foreign { .. } => unreachable!("judged above"),
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

/// Whether body `x` of part `pid`, as a feature at `at` of `list` would
/// see it, is a sheet in the current rebuild: the tip of its chain among
/// the features before `at`. A part no feature of which may make a
/// sheet is never rebuilt for this.
fn source_is_sheet(session: &mut Session, pid: &Id, list: &[Id], at: usize, x: &Id) -> bool {
    let Ok(p) = session.project().part(pid).cloned() else {
        return false;
    };
    let may = p
        .features
        .values()
        .any(|g| g.as_ref().is_ok_and(|g| g.may_make_sheet()));
    if !may {
        return false;
    }
    let solids = Solids::of_order(&p, &list[..at]);
    let Some(tip) = solids.tips().find(|(s, _)| *s == x).map(|(_, t)| t.clone()) else {
        return false;
    };
    let Ok(results) = session.rebuilt(pid) else {
        return false;
    };
    crate::rebuild::Results::result(results, &tip)
        .and_then(|r| r.as_ref().ok())
        .and_then(|o| o.solid(x))
        .is_some_and(|b| b.body.kind() == ryukei::topo::BodyKind::Sheet)
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
        if let Some(Ok(f)) = q.features.get(*w) {
            named.extend(f.deletes());
        }
    }
    for id in q.order() {
        let Some(Ok(f)) = q.features.get(id) else {
            continue;
        };
        let targets = f.targets();
        let concerned = written.contains(&id)
            || targets.iter().any(|t| {
                named.contains(t) || written.iter().any(|w| w.as_str() == t.founder_str())
            });
        if targets.is_empty() || !concerned {
            continue;
        }
        crate::graph::check_order(q, q.order(), id, f)?;
        for t in targets {
            if let Some(by) = solids.consumed_by(t, id) {
                return Err(rebuild::consumed(id, t, by));
            }
            if let Some(by) = solids.deleted_by(t, id) {
                return Err(rebuild::deleted(id, t, by));
            }
            if !solids.is_solid(t) {
                return Err(rebuild::not_a_solid(q, &solids, id, "targets", t));
            }
        }
    }
    Ok(())
}

/// A `delete_body` from what it is given: `solids` (or one `solid`, as
/// the CLI sends a lone `--solid`), read as its file reads them.
fn delete_from(given: &Given) -> Result<crate::doc::body::DeleteBody> {
    let solids: Vec<String> = match (&given.fields.solids, &given.fields.solid) {
        (Some(s), None) => s.clone(),
        (None, Some(s)) => vec![s.clone()],
        (Some(_), Some(_)) => {
            return Err(Error::new(
                Code::InvalidParam,
                "a delete names its solids once: solids, or one solid",
            )
            .with("param", "solid"));
        }
        (None, None) => Vec::new(),
    };
    if solids.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "a delete names the solids it deletes: solids",
        )
        .with("param", "solids"));
    }
    let v = json!({"solids": solids});
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let d =
        crate::doc::body::DeleteBody::from_fields(&mut o).map_err(|e| e.with("param", "solids"))?;
    o.finish()?;
    Ok(d)
}

/// Refuses a `delete_body` its rebuild would refuse, against the part as
/// the write would leave it: the solids before it as they are now.
fn check_delete(session: &mut Session, pid: &Id, list: &[Id], id: &Id, f: &Feature) -> Result<()> {
    let Feature::DeleteBody(d) = f else {
        return Ok(());
    };
    let q = proposed(session.project().part(pid)?, list, &[(id, f)]);
    let solids = Solids::of(&q);
    let r = session.rebuilt(pid)?;
    crate::body::delete_body(&q, &solids, id, d, r).map(|_| ())
}

/// The part as a write would leave it: `writes` in place, listed as
/// `order`.
pub(crate) fn proposed(p: &Part, order: &[Id], writes: &[(&Id, &Feature)]) -> Part {
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
                    let g = geom_json(t, part, "/extent/target")?;
                    e.insert("target".into(), g);
                }
                Value::Object(e)
            }
            // An axis given as `<part>/…` is any axis; a bare ID a line of
            // the swept sketch.
            ("axis", Value::String(t)) if t.contains('/') => geom_json(&t, part, "/axis")?,
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

/// A planar face from `current`'s fields (none for a new one) with what
/// was given laid over them: its profile's sketch or face, and loops.
fn planar_from(current: Option<Value>, given: &Given, part: &Id) -> Result<Feature> {
    let bad = |param: &str, msg: &str| Error::new(Code::InvalidParam, msg).with("param", param);
    let mut profile = current
        .and_then(|v| v.get("profile").cloned())
        .unwrap_or_else(|| json!({}));
    let f = &given.fields;
    match (&f.sketch, &f.face) {
        (Some(_), Some(_)) => {
            return Err(bad(
                "face",
                "a planar face's profile is a sketch or a planar face, not both",
            ));
        }
        (Some(sk), None) => profile = json!({"sketch": sk}),
        (None, Some(face)) => profile = json!({"face": geom_json(face, part, "/profile/face")?}),
        (None, None) => {}
    }
    if profile.as_object().is_none_or(|o| o.is_empty()) {
        return Err(bad(
            "sketch",
            "a planar face fills closed loops: name a sketch (sketch) or a planar face (face)",
        ));
    }
    match &f.loops {
        Some(l) if l.is_empty() => {
            if let Some(o) = profile.as_object_mut() {
                o.remove("loops");
            }
        }
        Some(l) => profile["loops"] = json!(l),
        None => {}
    }
    let v = json!({"profile": profile});
    let mut o = crate::doc::files::Obj::new(&v, "")?;
    let p = crate::doc::sweep::Planar::from_fields(&mut o)?;
    o.finish()?;
    Ok(Feature::Planar(Box::new(p)))
}

/// A geometric reference given as text in `part`, as a file holds it;
/// `at` is its pointer for errors.
pub(crate) fn geom_json(t: &str, part: &Id, at: &str) -> Result<Value> {
    Ok(crate::doc::geometry::GeomRef::parse(t, part)
        .map_err(|e| e.with("pointer", at))?
        .to_json())
}

/// The listed features that sweep `sketch` or drill at its points.
pub(crate) fn sweeps_of(part: &Part, sketch: &Id) -> Vec<Id> {
    part.order()
        .iter()
        .filter(|f| {
            part.features
                .get(*f)
                .and_then(|r| r.as_ref().ok())
                .is_some_and(|f| f.profiles().contains(&sketch))
        })
        .cloned()
        .collect()
}

/// Refuses a sweep or a hole that names what isn't there: its sketch must
/// be a sketch listed before it in `order`, with the entities it names,
/// its axis a line and a hole's points points. Whether the loops make a
/// profile only rebuild can tell.
fn check_sweep(part: &Part, order: &[Id], id: &Id, f: &Feature) -> Result<()> {
    // A loft's sections: each sketch listed before it, with the entities
    // its loops name. How they match only its rebuild check tells.
    if let Feature::Loft(l) = f {
        for (k, s) in l.sections.iter().enumerate() {
            let Some(sketch) = s.sketch() else { continue };
            check_sketch(part, order, id, sketch)?;
            let Some(Ok(Feature::Sketch(sk))) = part.features.get(sketch) else {
                continue;
            };
            for (i, e) in s.loops.iter().enumerate() {
                if !sk.entities.contains_key(e) {
                    return Err(
                        Error::new(Code::NotFound, format!("sketch {sketch} has no {e}"))
                            .with("sketch", sketch)
                            .with("entity", e)
                            .with("pointer", format!("/sections/{k}/loops/{i}")),
                    );
                }
            }
        }
        return Ok(());
    }
    let sketch = f.profiles()[0];
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
            RevolveAxis::Ref(_) => (&r.loops, None),
        },
        Feature::Sweep(w) => (&w.profile.loops, None),
        Feature::Planar(x) => (&x.profile.loops, None),
        Feature::BaseFlange(b) => (&b.loops, None),
        _ => unreachable!("a sweep"),
    };
    let at = if f.sweep().is_some() || matches!(f, Feature::Planar(_)) {
        "/profile"
    } else {
        ""
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
                        .with("pointer", format!("{at}/{field}")),
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
    use crate::doc::geometry::GeomRef;
    let reference = |s: &Option<String>| -> Result<Option<Option<GeomRef>>> {
        match s.as_deref() {
            None => Ok(None),
            Some("") => Ok(Some(None)),
            Some(t) => GeomRef::parse(t, part).map(|r| Some(Some(r))),
        }
    };
    let (origin, x) = (reference(&f.face_origin)?, reference(&f.face_x)?);
    let plane = match (&f.plane, &given.placement, &f.face) {
        // An origin plane; a datum plane or another sketch; a face.
        (Some(p), _, _) => {
            match GeomRef::parse(p, part).map_err(|e| e.with("pointer", "/plane"))? {
                GeomRef::Plane(o) => Plane::Origin(o),
                g @ GeomRef::Feature(_) => Plane::Datum(g),
                GeomRef::Entity(r) => Plane::Face(OnFace {
                    face: r,
                    origin: None,
                    x: None,
                }),
                _ => {
                    return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a sketch's plane is xy, yz, zx, a datum plane or sketch <part>/<feature>, or a planar face; not {p}"
                    ),
                )
                .with("pointer", "/plane"));
                }
            }
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
                Some(Plane::Face(c)) if f.face.is_none() && f.plane.is_none() => Some(c),
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
/// leaves it. Gives the `reference_followed` warnings of those that
/// follow a face or edge a later feature split or merged (design 447).
fn check_references(
    session: &mut Session,
    part: &Id,
    order: &[Id],
    id: &Id,
    f: &Feature,
) -> Result<Vec<Value>> {
    // Nothing may change a reference import's body, whether or not the
    // feature references anything (a cut, a boolean). Only an import in
    // reference mode makes one, so a part without one isn't rebuilt here.
    if (!f.targets().is_empty() || f.source().is_some())
        && session.project().part(part)?.features.values().any(|g| {
            g.as_ref()
                .ok()
                .and_then(|g| g.import())
                .is_some_and(|i| i.mode() == crate::doc::import::Mode::Reference)
        })
    {
        let p = session.project().part(part)?.clone();
        let results = session.rebuilt(part)?;
        rebuild::check_not_reference(&proposed(&p, order, &[(id, f)]), id, f, results)?;
    }
    // A loft's sections are matched against the rebuild whatever they
    // reference, and what a feature acts on must be a solid (ryucad 18).
    // Only a feature whose file says so makes a sheet, so a part holding
    // none needs no rebuild to judge a feature that wants solids; one that
    // wants a sheet there names a solid, which the rebuild must refuse.
    let may_hold_sheets = session
        .project()
        .part(part)?
        .features
        .values()
        .any(|g| g.as_ref().is_ok_and(|g| g.may_make_sheet()));
    let read = crate::kind::bodies_read(f);
    let reads_bodies = !read.is_empty()
        && (may_hold_sheets
            || read
                .iter()
                .any(|r| r.want == Some(crate::kind::Want::Sheet)));
    let planar = matches!(f, Feature::Planar(_));
    // An unfold's allowance and a refold's unfold are judged against the
    // rebuild whatever they reference.
    if f.loft().is_none()
        && f.unfold().is_none()
        && f.refold().is_none()
        && f.references().is_empty()
        && f.geometry().is_empty()
        && !reads_bodies
        && !planar
        && !matches!(f, Feature::Split(s) if s.plane().is_none())
    {
        return Ok(Vec::new());
    }
    let p = session.project().part(part)?.clone();
    let results = session.rebuilt(part)?;
    let q = proposed(&p, order, &[(id, f)]);
    let checked = crate::kind::check(&q, order, id, f, results)
        .and_then(|()| check_resolved(&p, order, id, f, results));
    let followed = match &checked {
        Ok(()) => crate::reference::followed_by(&q, &Solids::of_order(&q, order), id, f, results),
        Err(_) => Vec::new(),
    };
    checked.map_err(|e| session.explain_in(part, order, e))?;
    Ok(followed)
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
        // That it is an axis; whether it lies in the sketch's plane only
        // rebuild can tell.
        Feature::Revolve(r) => match &r.axis {
            RevolveAxis::Ref(g) => {
                let at = crate::geometry::At {
                    part: p,
                    order,
                    owner: id,
                    results,
                };
                crate::geometry::one(&at, "axis", g, crate::geometry::Want::Axis).map(|_| ())
            }
            RevolveAxis::Line(_) => Ok(()),
        },
        // That each reference is the kind its field wants; what they make
        // of each other (parallel, collinear) only rebuild can tell.
        Feature::Datum(d) => crate::datum::check(p, order, id, d, results),
        Feature::Path(x) => crate::path::check(p, order, id, x, results),
        Feature::Sweep(w) => crate::along::check(p, order, id, w, results),
        Feature::Loft(l) => crate::loft::check(p, order, id, l, results),
        // A planar face's profile is closed loops: its open curves are
        // refused when written (what else fails is rebuild's).
        Feature::Planar(x) => match crate::profile::planar(p, id, x, results) {
            Err(e) if e.code == Code::ProfileInvalid => Err(e),
            _ => Ok(()),
        },
        // Faces of its target, each once; what the kernel makes of them
        // only rebuild can tell.
        Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
            crate::body::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // Faces of its target, its direction, axis or surfaces their
        // kinds; what the kernel makes of them only rebuild can tell.
        Feature::MoveFace(_)
        | Feature::ReplaceFace(_)
        | Feature::Draft(_)
        | Feature::DeleteFace(_)
        | Feature::DeleteBlend(_)
        | Feature::ResizeBlend(_) => {
            crate::tweak::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        Feature::BaseFlange(_) | Feature::Flange(_) | Feature::Unfold(_) | Feature::Refold(_) => {
            crate::sheet_metal::check(&proposed(p, order, &[(id, f)]), id, f, results)
        }
        // Its sides edges and curves, its faces of one body; whether they
        // close up, or extract cleanly, only rebuild can tell.
        Feature::Fill(x) => {
            crate::surface::check_fill(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        Feature::Extract(x) => {
            crate::surface::check_extract(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        // Its sheet or faces; what the kernel makes of them only rebuild
        // can tell. A sew's sheets are judged by kind and order already.
        Feature::Thicken(x) => {
            crate::surface::check_thicken(&proposed(p, order, &[(id, f)]), id, x, results)
        }
        _ => Ok(()),
    }
}

/// Refuses an up-to target (`role` in feature `id`) that isn't a plane
/// (a planar face, a datum plane) or a point (a vertex, a datum point) now.
fn check_up_to(
    p: &Part,
    order: &[Id],
    id: &Id,
    role: &str,
    target: &crate::doc::geometry::GeomRef,
    results: &crate::rebuild::PartResult,
) -> Result<()> {
    use crate::geometry::{At, Want, resolve};
    let at = At {
        part: p,
        order,
        owner: id,
        results,
    };
    resolve(&at, role, target, &[Want::Point, Want::Plane]).map(|_| ())
}

/// Every feature type `feature.add` makes.
const FEATURE_TYPES: [&str; 28] = [
    "box",
    "cylinder",
    "cone",
    "sphere",
    "torus",
    "sketch",
    "extrude",
    "revolve",
    "sweep",
    "loft",
    "planar",
    "fill",
    "extract",
    "sew",
    "thicken",
    "boolean",
    "fillet",
    "chamfer",
    "hole",
    "pattern",
    "instance",
    "mate",
    "instance_pattern",
    "insert",
    "delete_body",
    "plane",
    "axis",
    "point",
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
    // A mirror whose operation is left out joins its own source (design
    // 327, question 20); one with no solid copies the part's one solid.
    // The file names both all the same.
    if ty == "mirror" && given.fields.solid.is_none() {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.solid = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a mirror names the solid it copies (solid): the part has {} before it",
                        before.len()
                    ),
                )
                .with("param", "solid")
                .with("solids", before));
            }
        }
    }
    // A sheet's copy can't be joined: left out, its operation is new
    // (design 388).
    if ty == "mirror" && given.fields.operation.is_none() {
        let sheet = given
            .fields
            .solid
            .as_deref()
            .and_then(|x| Id::parse(x, "feature").ok())
            .is_some_and(|x| source_is_sheet(session, &pid, &list, at, &x));
        if sheet {
            given.fields.operation = Some("new".into());
        } else {
            given.fields.operation = Some("join".into());
            if given.fields.targets.is_none() {
                given.fields.targets = given.fields.solid.clone().map(|s| vec![s]);
            }
        }
    }
    // An insert may look up its source's newest label: read it before
    // the part is borrowed for the rest.
    let mut took = None;
    let mut inserted = None;
    if ty == "insert" {
        given.only(ty, &INSERT)?;
        let (ins, t) = insert_from(session, None, &given, unit, &env)?;
        (inserted, took) = (Some(ins), t);
    }
    let p = session.project().part(&pid)?;
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
    // A tweak sorts what it was given; with no target it acts on the part's
    // one solid, else the one its faces name. The file names it all the same.
    if crate::tweak::TYPES.contains(&ty) {
        crate::tweak::take_given(ty, &mut given.fields)?;
        if given.fields.target.is_none() && given.fields.targets.is_none() {
            let before = solids_before(p, &list, at);
            given.fields.target = Some(crate::tweak::default_target(
                ty,
                &given.fields,
                &before,
                &pid,
            )?);
        }
    }
    // A shell or offset with no target acts on the part's one solid; the
    // file names it all the same.
    if matches!(ty, "shell" | "offset" | "split")
        && given.fields.target.is_none()
        && given.fields.targets.is_none()
    {
        let before = solids_before(p, &list, at);
        match before.as_slice() {
            [one] => given.fields.target = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!(
                        "a {ty} names the solid it acts on (target): the part has {} before it",
                        before.len()
                    ),
                )
                .with("param", "target")
                .with("solids", before));
            }
        }
    }
    if matches!(ty, "flange" | "unfold" | "refold")
        && given.fields.target.is_none()
        && given.fields.targets.is_none()
    {
        let before = solids_before(p, &list, at);
        let sheet: Vec<Id> = before
            .iter()
            .filter(|s| crate::sheet_metal::founder(p, s).is_some())
            .cloned()
            .collect();
        match (before.as_slice(), sheet.as_slice()) {
            ([one], _) | (_, [one]) => given.fields.target = Some(one.to_string()),
            _ => {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("a {ty} names the solid it acts on (target): the part has {} solids before it, {} of them founded by a base flange", before.len(), sheet.len()),
                )
                .with("param", "target")
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
        "sweep" => {
            given.only(ty, &crate::doc::sweep::SWEEP_FIELDS)?;
            crate::along::from_given(None, &given.fields, p, unit, &env)?
        }
        "loft" => {
            given.only(ty, &crate::doc::sweep::LOFT_FIELDS)?;
            crate::loft::from_given(None, &given.fields, p, unit, &env)?
        }
        "planar" => {
            given.only(ty, &crate::doc::sweep::PLANAR_FIELDS)?;
            planar_from(None, &given, &pid)?
        }
        "fill" | "extract" | "sew" | "thicken" => {
            given.only(ty, crate::doc::surface::request_fields(ty))?;
            crate::surface::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "boolean" => {
            given.only(ty, &BOOLEAN)?;
            Feature::Boolean(boolean_from(None, &given)?)
        }
        "instance" => {
            given.only(ty, &INSTANCE)?;
            let deps = session.project().manifest.dependencies.clone();
            Feature::Instance(instance_from(None, &given, unit, &env, &deps)?)
        }
        "insert" => Feature::Insert(Box::new(
            inserted.take().expect("read before the part was borrowed"),
        )),
        "delete_body" => {
            given.only(ty, &DELETE_BODY)?;
            Feature::DeleteBody(Box::new(delete_from(&given)?))
        }
        "mate" => {
            given.only(ty, &MATE)?;
            mate_from(None, &given, (session.project(), &pid), unit, &env)?
        }
        "instance_pattern" => {
            given.only(ty, &INSTANCE_PATTERN)?;
            instance_pattern_from(None, &given, &pid, unit, &env)?
        }
        "fillet" | "chamfer" => {
            given.only(ty, if ty == "fillet" { &FILLET } else { &CHAMFER })?;
            blend_from(ty, None, &given, &pid, unit, &env)?
        }
        "shell" | "offset" | "mirror" | "split" => {
            given.only(ty, crate::doc::body::request_fields(ty))?;
            crate::body::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "move_face" | "replace_face" | "draft" | "delete_face" | "delete_blend"
        | "resize_blend" => {
            given.only(ty, crate::tweak::request_fields(ty))?;
            crate::tweak::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "base_flange" | "flange" | "unfold" | "refold" => {
            crate::sheet_metal::take_given(ty, &mut given.fields);
            given.only(ty, crate::doc::sheet_metal::request_fields(ty))?;
            crate::sheet_metal::from_given(ty, None, &given.fields, &pid, unit, &env)?
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
        "plane" | "axis" | "point" => {
            given.only(ty, crate::datum::request_fields(ty))?;
            crate::datum::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        "path" | "helix" => {
            given.only(ty, crate::doc::path::request_fields(ty))?;
            crate::path::from_given(ty, None, &given.fields, &pid, unit, &env)?
        }
        _ => {
            let ty = PrimitiveType::parse(ty)?;
            primitive_fields(ty, &mut given)?;
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
    if f.insert().is_some() {
        check_insert(session, &pid, &list, &fid, &f)?;
    }
    check_delete(session, &pid, &list, &fid, &f)?;
    // What its references follow: a split face or edge (design 447).
    let mut followed = Vec::new();
    if !f.connectors().is_empty() || f.instance_pattern().is_some() {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
        let r = session.rebuilt(&pid)?;
        followed.extend(crate::mate::followed(&pid, &fid, &f, r));
    }
    let p = session.project().part(&pid)?;
    if !f.profiles().is_empty() {
        check_sweep(p, &list, &fid, &f)?;
    }
    check_solids(&proposed(p, &list, &[(&fid, &f)]), &[&fid])?;
    if let Feature::Pattern(x) = &f {
        crate::pattern::check_seeds(&proposed(p, &list, &[(&fid, &f)]), &fid, x)?;
    }
    followed.extend(check_references(session, &pid, &list, &fid, &f)?);
    if crate::assembly::assembles(&f) {
        let p = session.project().part(&pid)?;
        let q = proposed(p, &list, &[(&fid, &f)]);
        crate::mate::refuse_overconstrained(session, q)?;
    }
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &[(&fid, &f)], list)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let saved = session.project().part(&pid)?;
    let revision = &saved.revision;
    let mut v = json!({"feature": feature_report(saved, &fid, &f), "revision": revision});
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    if !followed.is_empty() {
        v["warnings"] = json!(followed);
    }
    if let Some(t) = took {
        v["took"] = t;
    }
    Ok(v)
}

/// A feature with `expressions` set by JSON pointer, each one of its
/// expression fields, given alone; a sketch's dimensions are solved, so
/// they are set through `sketch.edit`.
fn edited_at(
    old: &Feature,
    given: &Given,
    expressions: &Map<String, Value>,
    unit: crate::units::Unit,
    env: &crate::expr::Env,
) -> Result<Feature> {
    if let Some(name) = given.present().into_iter().next() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("give expressions by pointer alone, without {name}"),
        )
        .with("param", name));
    }
    if old.sketch().is_some() {
        return Err(Error::new(
            Code::InvalidParam,
            "a sketch's dimensions are solved: set them with sketch.edit",
        ));
    }
    if let Feature::Import(_) = old {
        return Err(import_not_written_here("edit"));
    }
    let mut v = old.to_json();
    let fields: Vec<String> = crate::doc::files::expression_fields(&v)
        .into_iter()
        .map(|(at, _)| at)
        .collect();
    for (at, x) in expressions {
        if !fields.contains(at) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{at} is no expression field of this feature"),
            )
            .with("pointer", at)
            .with("fields", &fields));
        }
        let e = crate::doc::files::expression(x).map_err(|e| e.with("pointer", at))?;
        *v.pointer_mut(at).expect("a field it has") = Value::String(e.text().to_owned());
    }
    Feature::from_json(&v, unit, env)
}

fn feature_edit(
    session: &mut Session,
    part: &str,
    id: &str,
    mut given: Given,
    expressions: &Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let unit = session.project().manifest.default_unit;
    // An insert's edit may look up its source's newest label: read it
    // before the part is borrowed for the rest.
    let inserted = {
        let p = session.project().part(&pid)?;
        match feature(p, &fid)?.insert() {
            Some(i) if expressions.is_empty() => {
                let (i, env) = (i.clone(), p.env.clone());
                given.only("insert", &INSERT)?;
                Some(insert_from(session, Some(&i), &given, unit, &env)?.0)
            }
            _ => None,
        }
    };
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let list = order(p)?;
    let old = feature(p, &fid)?.clone();
    let f = if !expressions.is_empty() {
        edited_at(&old, &given, expressions, unit, &p.env).map_err(|e| e.with("feature", &fid))?
    } else {
        match old.clone() {
            Feature::Primitive(mut f) => {
                primitive_fields(f.ty, &mut given)?;
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
                let deps = session.project().manifest.dependencies.clone();
                Feature::Instance(instance_from(Some(&i), &given, unit, &p.env, &deps)?)
            }
            Feature::Insert(_) => Feature::Insert(Box::new(
                inserted.clone().expect("read before the part was borrowed"),
            )),
            Feature::DeleteBody(d) => {
                given.only("delete_body", &DELETE_BODY)?;
                if given.fields.solids.is_none() && given.fields.solid.is_none() {
                    Feature::DeleteBody(d)
                } else {
                    Feature::DeleteBody(Box::new(delete_from(&given)?))
                }
            }
            Feature::Mate(m) => {
                given.only("mate", &MATE)?;
                mate_from(Some(&m), &given, (session.project(), &pid), unit, &p.env)?
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
            f @ (Feature::Shell(_)
            | Feature::Offset(_)
            | Feature::Mirror(_)
            | Feature::Split(_)) => {
                let ty = f.type_name();
                given.only(ty, crate::doc::body::request_fields(ty))?;
                crate::body::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            f @ (Feature::BaseFlange(_)
            | Feature::Flange(_)
            | Feature::Unfold(_)
            | Feature::Refold(_)) => {
                let ty = f.type_name();
                crate::sheet_metal::take_given(ty, &mut given.fields);
                given.only(ty, crate::doc::sheet_metal::request_fields(ty))?;
                crate::sheet_metal::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
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
            f
            @ (Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_)) => {
                let ty = f.type_name();
                given.only(ty, crate::doc::surface::request_fields(ty))?;
                crate::surface::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            f @ (Feature::MoveFace(_)
            | Feature::ReplaceFace(_)
            | Feature::Draft(_)
            | Feature::DeleteFace(_)
            | Feature::DeleteBlend(_)
            | Feature::ResizeBlend(_)) => {
                let ty = f.type_name();
                crate::tweak::take_given(ty, &mut given.fields)?;
                given.only(ty, crate::tweak::request_fields(ty))?;
                crate::tweak::from_given(ty, Some(&f), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Datum(d) => {
                let ty = d.type_name();
                given.only(ty, crate::datum::request_fields(ty))?;
                crate::datum::from_given(ty, Some(&d), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Path(x) => {
                let ty = x.type_name();
                given.only(ty, crate::doc::path::request_fields(ty))?;
                crate::path::from_given(ty, Some(&x), &given.fields, &pid, unit, &p.env)?
            }
            Feature::Sweep(w) => {
                given.only("sweep", &crate::doc::sweep::SWEEP_FIELDS)?;
                let f = crate::along::from_given(Some(&w), &given.fields, p, unit, &p.env)?;
                if !f.profiles().is_empty() {
                    check_sweep(p, &list, &fid, &f)?;
                }
                f
            }
            Feature::Loft(l) => {
                given.only("loft", &crate::doc::sweep::LOFT_FIELDS)?;
                let f = crate::loft::from_given(Some(&l), &given.fields, p, unit, &p.env)?;
                check_sweep(p, &list, &fid, &f)?;
                f
            }
            Feature::Planar(x) => {
                given.only("planar", &crate::doc::sweep::PLANAR_FIELDS)?;
                let f = planar_from(Some(x.fields()), &given, &pid)?;
                if !f.profiles().is_empty() {
                    check_sweep(p, &list, &fid, &f)?;
                }
                f
            }
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
        }
    };
    // A solid's founder that stops founding it leaves what acts on it, or
    // refers to it, with nothing.
    let founded = |f: &Feature| f.founds();
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
    if f.insert().is_some() && f != old {
        check_insert(session, &pid, &list, &fid, &f)?;
    }
    check_delete(session, &pid, &list, &fid, &f)?;
    // What its references follow: a split face or edge (design 447).
    let mut followed = Vec::new();
    if (!f.connectors().is_empty() || f.instance_pattern().is_some()) && f != old {
        crate::mate::check(session, &pid, &list, &fid, &f)?;
        let r = session.rebuilt(&pid)?;
        followed.extend(crate::mate::followed(&pid, &fid, &f, r));
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
    // A sweep's twist and scale are judged against its path's closing, a
    // loft's sections against each other.
    let sweep_changed = (f.sweep().is_some() || f.loft().is_some()) && f != old;
    if f.references() != old.references()
        || f.geometry() != old.geometry()
        || propagates(&f) != propagates(&old)
        || sweep_changed
        || (matches!(
            f,
            Feature::Shell(_)
                | Feature::Offset(_)
                | Feature::Mirror(_)
                | Feature::Split(_)
                | Feature::MoveFace(_)
                | Feature::ReplaceFace(_)
                | Feature::Draft(_)
                | Feature::DeleteFace(_)
                | Feature::DeleteBlend(_)
                | Feature::ResizeBlend(_)
        ) && f.targets() != old.targets())
        || ((f.base_flange().is_some()
            || f.flange().is_some()
            || f.unfold().is_some()
            || f.refold().is_some())
            && f != old)
        || (matches!(
            f,
            Feature::Fill(_) | Feature::Extract(_) | Feature::Sew(_) | Feature::Thicken(_)
        ) && f != old)
        || f.source() != old.source()
    {
        followed.extend(check_references(session, &pid, &list, &fid, &f)?);
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
    // Each assembly the edit reaches whose mates fail now, whether the
    // edit broke them or drives an assembly they already left half
    // solved (issue #155).
    let reached = crate::mate::reached(session.project(), &pid);
    let mut warnings = crate::mate::failing_warnings(session, &reached, "");
    warnings.extend(followed);
    let saved = session.project().part(&pid)?;
    let revision = &saved.revision;
    let mut v = json!({"feature": feature_report(saved, &fid, &f), "revision": revision});
    if !sketches.is_empty() {
        v["sketches"] = json!(sketches);
    }
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
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
    session.remove_file(&path)?;
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
            && (*f == fid
                || crate::graph::inputs(feature)
                    .iter()
                    .any(|i| i.founder_str() == fid.as_str()))
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
                if let Some(i) = f.insert() {
                    let r = crate::insert::report(i, None);
                    v["of"] = r["of"].clone();
                    v["rev"] = r["rev"].clone();
                }
                if let Some(Err(e)) = built.get(id) {
                    v["ok"] = json!(false);
                    v["error"] = json!(e);
                    if e.code == Code::DependencyFailed {
                        v["blocked"] = json!(true);
                    }
                }
                // A solid key below an insert is used through the insert.
                let mut uses: Vec<Id> = Vec::new();
                for i in crate::graph::inputs(f) {
                    let i = i.founder();
                    if !uses.contains(&i) {
                        uses.push(i);
                    }
                }
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

/// A body's kind in a report: `kind`, and for a sheet how many
/// `boundaries` it has and whether it is `closed`.
fn kinds(v: &mut Value, b: &ryukei::topo::Body) {
    v["kind"] = json!(crate::kind::name(b));
    if b.kind() == ryukei::topo::BodyKind::Sheet {
        let n = crate::kind::boundaries(b);
        v["boundaries"] = json!(n);
        v["closed"] = json!(n == 0);
    }
}

/// A part's rebuild as agents and subscribers see it: each feature's
/// status, hash and counts. With `occurrences`, an assembly's occurrences
/// come apart, as the JSON text of the report's `occurrences` (#533: the
/// root's 100,000 took about 450 MiB as a `Value` tree), for
/// [`object_text`] to write into the report's text.
pub(crate) fn part_report(
    session: &mut Session,
    id: &Id,
    occurrences: bool,
) -> Result<(Value, Option<String>)> {
    let unit = session.project().manifest.default_unit;
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
            let mut ins: Vec<Id> = Vec::new();
            for i in crate::graph::inputs(f.as_ref().ok()?) {
                let i = i.founder();
                if !ins.contains(&i) {
                    ins.push(i);
                }
            }
            (!ins.is_empty()).then(|| (fid.clone(), ins))
        })
        .collect();
    // What each feature does to solids, and the bodies it consumes (a
    // boolean's tools, a sew's sheets, a thicken's sheet): for showing the
    // chains.
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
                let consumes: Vec<Id> = f.consumes().into_iter().cloned().collect();
                match f.combine() {
                    Some(c) if !c.is_new() => Some((fid.clone(), (c.operation.as_str(), consumes))),
                    _ if !consumes.is_empty() => Some((fid.clone(), ("new", consumes))),
                    _ => None,
                }
            }
        })
        .collect();
    // A pattern's form (`linear`, `circular` or `mirror`), for the
    // window's icons, whatever it rebuilt to.
    let forms: BTreeMap<Id, &'static str> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            use crate::doc::mate::PatternForm;
            use crate::doc::pattern::Kind;
            let form = match f.as_ref().ok()? {
                Feature::Pattern(p) => match p.kind {
                    Kind::Linear { .. } => "linear",
                    Kind::Circular { .. } => "circular",
                    Kind::Mirror { .. } => "mirror",
                },
                Feature::InstancePattern(p) => match p.form {
                    PatternForm::Linear { .. } => "linear",
                    PatternForm::Circular { .. } => "circular",
                },
                _ => return None,
            };
            Some((fid.clone(), form))
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
    // A shell's or offset's fields as resolved, shown whether it built or
    // not.
    let bodied: BTreeMap<Id, (&'static str, Value)> = part
        .features
        .iter()
        .filter_map(|(fid, f)| {
            let f = f.as_ref().ok()?;
            let report = match f {
                Feature::Shell(_) | Feature::Offset(_) | Feature::Mirror(_) | Feature::Split(_) => {
                    crate::body::report(id, f)
                }
                Feature::BaseFlange(_)
                | Feature::Flange(_)
                | Feature::Unfold(_)
                | Feature::Refold(_) => crate::sheet_metal::report(part, f),
                // A fill's rebuild report is what it measured (see
                // `Made::report`), an extract's its faces.
                Feature::Extract(_) => crate::surface::report(id, f),
                Feature::MoveFace(_)
                | Feature::ReplaceFace(_)
                | Feature::Draft(_)
                | Feature::DeleteFace(_)
                | Feature::DeleteBlend(_)
                | Feature::ResizeBlend(_) => crate::tweak::report(id, f),
                _ => return None,
            };
            Some((fid.clone(), (f.type_name(), report)))
        })
        .collect();
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
    // An insert's source, pin and solids (design 521): shown whether it
    // built or not.
    let inserts: BTreeMap<Id, crate::doc::insert::Insert> = part
        .features
        .iter()
        .filter_map(|(fid, f)| Some((fid.clone(), f.as_ref().ok()?.insert()?.clone())))
        .collect();
    let deletes: BTreeMap<Id, crate::doc::body::DeleteBody> = part
        .features
        .iter()
        .filter_map(|(fid, f)| match f {
            Ok(Feature::DeleteBody(d)) => Some((fid.clone(), (**d).clone())),
            _ => None,
        })
        .collect();
    // A sweep's profile and path, a loft's sections, a mirror's or
    // split's plane, and a tweak's motion, are resolved again from the rebuild for its report: the part, held apart
    // from the session while it rebuilds.
    let held = part
        .features
        .values()
        .any(|f| {
            f.as_ref().is_ok_and(|f| {
                f.sweep().is_some()
                    || f.loft().is_some()
                    || matches!(
                        f,
                        Feature::Mirror(_)
                            | Feature::Split(_)
                            | Feature::MoveFace(_)
                            | Feature::ReplaceFace(_)
                            | Feature::Draft(_)
                            | Feature::DeleteFace(_)
                            | Feature::DeleteBlend(_)
                            | Feature::ResizeBlend(_)
                    )
            })
        })
        .then(|| part.clone());
    // Instances an update kept though their usage left the file.
    let kept = if assembly {
        crate::import::usage_warnings(session.project(), id)
    } else {
        Vec::new()
    };
    // References that follow a face or edge a later feature split or
    // merged (design 447): under their features and the part's warnings.
    session.rebuilt(id)?;
    let followed = match (session.project().parts.get(id), session.cached(id)) {
        (Some(p), Some(r)) => crate::reference::followed(p, r),
        _ => Vec::new(),
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
                        && let Some(i) = &m.report
                    {
                        // A fill's report under its own type, else an
                        // import's; a tweak's report comes with its own
                        // fields (`tweak::resolved`).
                        let key = match types.get(fid).and_then(Value::as_str) {
                            Some("fill") => Some("fill"),
                            Some(
                                "draft" | "move_face" | "replace_face" | "delete_face"
                                | "delete_blend" | "resize_blend",
                            ) => None,
                            _ => Some("import"),
                        };
                        if let Some(key) = key {
                            v[key] = i.as_ref().clone();
                        }
                    }
                    if let crate::rebuild::Output::Solids(m) = o
                        && let Some(f) = &m.flat
                    {
                        v["flat"] = crate::sheet_metal::unfold::summary(f);
                    }
                    if let Some(p) = &held
                        && let Some(Ok(Feature::Sweep(w))) = p.features.get(fid)
                        && let Some(d) = crate::along::described(p, fid, w, r)
                    {
                        v["sweep"] = d;
                    }
                    if let Some(p) = &held
                        && let Some(Ok(Feature::Loft(l))) = p.features.get(fid)
                        && let Some(d) = crate::loft::described(p, fid, l, r)
                    {
                        v["loft"] = d;
                    }
                    match o.body() {
                        Some(b) => v["counts"] = counts(b),
                        None => {
                            v["bodies"] = o
                                .solids()
                                .iter()
                                .map(|(s, b)| {
                                    let mut e = json!({"solid": s, "hash": b.hash,
                                                       "counts": counts(b)});
                                    kinds(&mut e, &b.body);
                                    e
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
                Ok(crate::rebuild::Output::Datum(d)) => {
                    json!({"id": fid, "type": types.get(fid), "ok": true, "hash": d.hash,
                           "datum": d.report()})
                }
                Ok(crate::rebuild::Output::Path(m)) => {
                    let mut v = json!({"id": fid, "type": types.get(fid), "ok": true,
                                       "hash": m.hash});
                    v[m.kind()] = m.report(id);
                    v
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
            if let Some(form) = forms.get(fid) {
                v["form"] = json!(form);
            }
            if let Some((k, x)) = bodied.get(fid) {
                v[*k] = x.clone();
                if let Some(Value::Object(extra)) =
                    held.as_ref().filter(|_| v["ok"] == true).and_then(|q| {
                        let f = q.features.get(fid)?.as_ref().ok()?;
                        crate::body::resolved(q, r, fid, f)
                            .or_else(|| crate::tweak::resolved(q, r, fid, f, false, unit))
                    })
                {
                    for (key, x) in extra {
                        v[*k][key] = x;
                    }
                }
            }
            if let Some(d) = deletes.get(fid) {
                v["delete_body"] = crate::body::delete_report(d, r);
            }
            if let Some(i) = inserts.get(fid) {
                let made = match b {
                    Ok(crate::rebuild::Output::Solids(m)) => Some(&**m),
                    _ => None,
                };
                v["insert"] = crate::insert::report(i, made);
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
            // An insert's own (`insert_source_failed`, `insert_obsolete`)
            // go under it too, as a followed reference's do.
            let inserted = match b {
                Ok(o) => o.inserted().map(|i| i.warnings.as_slice()),
                Err(_) => None,
            };
            let own: Vec<&Value> = inserted
                .unwrap_or_default()
                .iter()
                .chain(followed.iter().filter(|w| w["feature"] == fid.as_str()))
                .collect();
            if !own.is_empty() {
                v["warnings"] = json!(own);
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
                kinds(&mut v, &b.body);
                if b.reference {
                    v["reference"] = json!(true);
                }
            }
            if let Some(e) = s.error {
                v["error"] = json!(e);
            }
            v
        })
        .collect();
    let mut v = json!({"part": id, "revision": revision, "hash": r.hash,
                       "failed": r.failed(), "features": features, "solids": shown});
    let mut all = None;
    let mut warnings = r.warnings(id);
    warnings.extend(followed);
    // An assembly's model is what its instances place.
    warnings.extend(kept);
    if assembly {
        v["kind"] = json!("assembly");
        v["occurrence_count"] = json!(crate::assembly::Model::count_of_assembly(r));
        if occurrences {
            let model = crate::assembly::Model::of_assembly(r);
            let mut text = String::from("[");
            for (i, o) in model.occurrences.iter().enumerate() {
                if i > 0 {
                    text.push(',');
                }
                text.push_str(&o.to_json().to_string());
            }
            text.push(']');
            all = Some(text);
        }
        if let Some(solve) = &r.solve {
            v["solve"] = solve.to_json();
        }
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok((v, all))
}

/// An object's JSON text with `extra`, a member already written as text,
/// in its sorted place: the bytes the object would give holding it.
fn object_text(v: &Value, extra: Option<(&str, &str)>) -> String {
    let Some((key, raw)) = extra else {
        return v.to_string();
    };
    let mut text = String::from("{");
    let mut extra = Some(raw);
    let mut first = true;
    let mut member = |text: &mut String, k: &str, raw: &str| {
        if !std::mem::take(&mut first) {
            text.push(',');
        }
        text.push_str(&json!(k).to_string());
        text.push(':');
        text.push_str(raw);
    };
    for (k, value) in v.as_object().into_iter().flatten() {
        if k.as_str() > key
            && let Some(raw) = extra.take()
        {
            member(&mut text, key, raw);
        }
        if k != key {
            member(&mut text, k, &value.to_string());
        }
    }
    if let Some(raw) = extra {
        member(&mut text, key, raw);
    }
    text.push('}');
    text
}

/// Tells subscribers what changed since they were last told: the project
/// (manifest, parts and, when it has any, its parameters as written) and
/// every part whose revision moved, rebuilt. While nobody watches, it
/// only notes the current state, so a new subscriber hears of changes
/// made after it subscribed.
pub fn publish(live: &mut Live, session: &mut Session) {
    let watched = live.hub.is_watched();
    let p = session.project();
    let mut project = json!({
        "default_unit": p.manifest.default_unit,
        "parts": p.parts.keys().collect::<Vec<_>>(),
    });
    match &p.parameters {
        Some(Ok(f)) => {
            project["parameters"] = json!(
                f.parameters
                    .iter()
                    .map(|(k, e)| (k.clone(), e.text().to_owned()))
                    .collect::<BTreeMap<_, _>>()
            );
        }
        Some(Err(e)) => project["parameters"] = json!({"error": e}),
        None => {}
    }
    if watched && live.seen.project != project {
        live.hub.send(Topic::Project, "project.changed", &project);
    }
    live.seen.project = project;
    // A merge: when it starts, ends, or what it leaves to settle changes.
    // A status that failed is said again once what it read moves, though
    // its key reads the same: else one read while Git was still writing
    // would stand until the merge's state next changed.
    let merge = crate::validate::key(session);
    let retry = live
        .seen
        .merge_failed
        .is_some()
        .then(|| crate::validate::retry_stamp(session));
    let failed_moved = retry.is_some() && retry != live.seen.merge_failed;
    if watched && (live.seen.merge != merge || failed_moved) {
        let stamp = merge
            .is_some()
            .then(|| retry.unwrap_or_else(|| crate::validate::retry_stamp(session)));
        let mut failed = None;
        let params = match merge {
            None => json!({"merging": false}),
            Some(_) => match crate::validate::status(session) {
                Ok(s) => json!({"merging": true, "status": s}),
                Err(e) => {
                    failed = stamp;
                    json!({"merging": true, "error": e})
                }
            },
        };
        live.hub.send(Topic::Merge, "merge.changed", &params);
        live.seen.merge_failed = failed;
    } else if !watched {
        live.seen.merge_failed = None;
    }
    live.seen.merge = merge;
    compared(live, session, watched);
    if !watched {
        // Noted only: a subscriber hears of what changes from now on.
        live.seen.pdm = None;
        live.seen.annotations = None;
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
    // Annotations: each added, changed or removed, and each whose derived
    // states moved with the model. Worked out only while anyone watches
    // and the files moved, and only over annotations there are.
    let all = crate::annotation::read_all(&session.snapshot());
    let mut now = BTreeMap::new();
    for (id, a) in &all {
        let v = match a {
            Ok(a) => crate::annotation::entry(session, &all, a),
            Err(e) => json!({"id": id, "error": e}),
        };
        now.insert(id.to_string(), v);
    }
    if let Some(seen) = &live.seen.annotations {
        let changed: Vec<&Value> = now
            .iter()
            .filter(|(k, v)| seen.get(*k) != Some(*v))
            .map(|(_, v)| v)
            .collect();
        let removed: Vec<&String> = seen.keys().filter(|k| !now.contains_key(*k)).collect();
        if !changed.is_empty() || !removed.is_empty() {
            live.hub.send(
                Topic::Annotations,
                "annotation.changed",
                &json!({"annotations": changed, "removed": removed}),
            );
        }
    }
    live.seen.annotations = Some(now);
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
        let mut report = part_report(session, id, false)
            .map_or_else(|e| json!({"part": id, "error": e}), |r| r.0);
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

/// Tells each participant comparing with another worktree when that
/// worktree's files change (`compare.changed`, to the participant's
/// connections): its window reads the diff again. Checked only while
/// anyone watches, as the project's own files are.
fn compared(live: &mut Live, session: &mut Session, watched: bool) {
    let revs: std::collections::BTreeSet<String> = live
        .participants()
        .filter_map(|p| p.view.compare.clone())
        .filter(|r| crate::session::worktree_path(r).is_some())
        .collect();
    let mut now = BTreeMap::new();
    if watched {
        for rev in &revs {
            if let Ok(Some((_, _, digest))) = session.worktree_files(rev) {
                now.insert(rev.clone(), digest);
            }
        }
        for (rev, digest) in &now {
            if live.seen.compared.get(rev).is_some_and(|d| d != digest) {
                let who: Vec<String> = live
                    .participants()
                    .filter(|p| p.view.compare.as_ref() == Some(rev))
                    .map(|p| p.id.clone())
                    .collect();
                for id in who {
                    live.hub.send_to(
                        &id,
                        "compare.changed",
                        &json!({"participant": id, "rev": rev, "state": &digest[..16]}),
                    );
                }
            }
        }
    }
    live.seen.compared = now;
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

/// A rebuild's report, `{commit?, parts, placed?, warnings?}`, written as
/// JSON text part by part: a whole project's (89 MB for the 10,000-part
/// benchmark) held as a `Value` tree took about 18 times that in the
/// daemon (#533). The text is the bytes the `Value` would give. A report
/// with failures is `rebuild_failed`, the report its details.
fn rebuild(session: &mut Session, part: Option<&str>, commit: Option<&str>) -> Result<Answer> {
    let ids: Vec<Id> = match part {
        Some(p) => vec![part_id(p)?],
        None => session.project().parts.keys().cloned().collect(),
    };
    // Members in their sorted order, as a `Value`'s map writes them.
    let mut text = String::from("{");
    if let Some(c) = commit {
        text.push_str(r#""commit":"#);
        text.push_str(&json!(c).to_string());
        text.push(',');
    }
    text.push_str(r#""parts":["#);
    let mut warnings = Vec::new();
    let mut failed = 0;
    let job = session.job.clone();
    let stage = job.begin(Phase::Rebuild, "parts", ids.len());
    for (i, id) in ids.into_iter().enumerate() {
        stage.step(|| id.to_string())?;
        // A part whose list can't be read can't be rebuilt.
        order(session.project().part(&id)?)?;
        let (mut report, occurrences) = part_report(session, &id, true)?;
        failed += report["failed"].as_u64().unwrap_or(0) as usize;
        if let Some(Value::Array(w)) = report.as_object_mut().and_then(|o| o.remove("warnings")) {
            warnings.extend(w);
        }
        if i > 0 {
            text.push(',');
        }
        let occurrences = occurrences.as_deref().map(|o| ("occurrences", o));
        text.push_str(&object_text(&report, occurrences));
    }
    text.push(']');
    // One assembly's rebuild takes in what fails in every assembly it
    // places, at any depth (issue #158), as `placed`: each such assembly
    // with its failing features. The whole project's lists them already.
    if let Some(top) = part {
        let top = part_id(top)?;
        let mut placed = Vec::new();
        for a in crate::assembly::placed_assemblies(session.project(), &top) {
            let p = session.project().part(&a)?.clone();
            let r = match session.rebuilt(&a) {
                Ok(r) => r,
                Err(e) if e.code == Code::Cancelled => return Err(e),
                Err(_) => continue,
            };
            let features: Vec<Value> = r
                .features
                .iter()
                .filter_map(|(fid, res)| {
                    let e = res.as_ref().err()?;
                    let ty = p.features.get(fid)?.as_ref().ok().map(Feature::type_name);
                    Some(json!({"id": fid, "type": ty, "ok": false, "error": e}))
                })
                .collect();
            if !features.is_empty() {
                failed += features.len();
                placed.push(json!({"part": a, "revision": p.revision,
                                   "failed": features.len(), "features": features}));
            }
        }
        if !placed.is_empty() {
            text.push_str(r#","placed":"#);
            text.push_str(&json!(placed).to_string());
        }
    }
    if !warnings.is_empty() {
        text.push_str(r#","warnings":"#);
        text.push_str(&json!(warnings).to_string());
    }
    text.push('}');
    if failed > 0 {
        // Every feature's status still reaches the agent, in the details.
        return Ok(Answer::raw(Raw::Error {
            code: Code::RebuildFailed,
            message: format!("{failed} feature(s) failed to rebuild"),
            details: text,
        }));
    }
    Ok(Answer::raw(Raw::Result(text)))
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
    let job = session.job.clone();
    let mut made = session
        .meshes
        .get_all_for(&ok, quality, budget, &job)?
        .into_iter();
    Ok(built
        .into_iter()
        .map(|(id, b)| {
            let r = b.map(|(_, b)| {
                let mesh = made.next().expect("one mesh per built body");
                let warning = mesh::unmeshed_warning(part, &id, &b, &mesh);
                mesh::Meshed {
                    body: b.body,
                    hash: b.hash,
                    mesh,
                    warning,
                }
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
                    .zip(occurrence_meshes(session, &solids, coarse)?)
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
) -> Result<Vec<std::sync::Arc<mesh::DisplayMesh>>> {
    let budget = mesh_budget();
    let items: Vec<(&Feature, &crate::rebuild::Built)> = solids
        .iter()
        .map(|s| (s.feature.as_ref(), s.built.as_ref()))
        .collect();
    let job = session.job.clone();
    session.meshes.get_all_for(&items, quality, budget, &job)
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
pub(crate) fn normal_to_face(
    session: &mut Session,
    reference: &str,
) -> Result<([f64; 3], [f64; 3])> {
    normal_to_face_at(session, reference, &crate::doc::assembly::Rev::Working)
}

/// [`normal_to_face`] with the face read in the model `rev` gives.
pub(crate) fn normal_to_face_at(
    session: &mut Session,
    reference: &str,
    rev: &crate::doc::assembly::Rev,
) -> Result<([f64; 3], [f64; 3])> {
    use ryukei::geom::{Curve, Surface};
    use ryukei::measure::Entity;
    // A datum plane: along its normal, its x level; a datum axis: along it.
    if let Some(d) = crate::geometry::locate_at(session, reference, rev)? {
        let a = |v: ryukei::geom::Vector3| [v.x, v.y, v.z];
        return match d.geom {
            crate::geometry::Geom::Plane(p) => Ok(crate::view::normal_to(
                a(p.frame.z().get()),
                Some(a(p.frame.x().get())),
            )),
            crate::geometry::Geom::Axis(l) => Ok(crate::view::normal_to(a(l.dir.get()), None)),
            _ => Err(Error::new(
                Code::InvalidParam,
                format!("{reference} is a datum point: look normal to a datum plane or along a datum axis"),
            )
            .with("entity", reference)),
        };
    }
    let located = crate::measure::locate_at(session, reference, rev)?;
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

/// What a pick pass for `me`'s marks leaves out: their participant's
/// hidden set, when the request came from one.
fn pick_from(live: Option<&Live>, me: Option<&str>) -> crate::annotation::PickFrom {
    crate::annotation::PickFrom {
        hidden: live
            .zip(me)
            .and_then(|(l, m)| l.get(m).ok())
            .map(|p| p.view.hidden.clone())
            .unwrap_or_default(),
    }
}

/// A render request, unpacked.
pub(crate) struct RenderAsk {
    pub(crate) part: Option<String>,
    pub(crate) camera: Option<crate::view::CameraArg>,
    pub(crate) view: Option<String>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) highlight: Vec<String>,
    pub(crate) edges: bool,
    pub(crate) labels: bool,
    pub(crate) sketches: bool,
    /// Draw the shown parts' datums.
    pub(crate) datums: bool,
    pub(crate) rev: Option<String>,
    pub(crate) frame: Vec<String>,
    pub(crate) compare: Option<String>,
    /// What to leave out: occurrences and solids, at the leaves.
    pub(crate) hidden: std::collections::BTreeSet<String>,
    /// Draw the origin's axes and planes.
    pub(crate) origin: bool,
    /// Draw each face in its appearance, else the fixed theme.
    pub(crate) appearance: bool,
    /// Cut away what lies beyond these planes.
    pub(crate) sections: Vec<crate::section::Plane>,
    /// An explode to draw.
    pub(crate) explode: Option<DrawExplode>,
    /// Draw open annotations' pins, numbered.
    pub(crate) annotations: bool,
    /// A sheet metal solid whose flat pattern to draw instead (`""`: the
    /// part's one).
    pub(crate) flat: Option<String>,
    /// A camera in metres that wins over `camera`, a participant's and
    /// `view` (a pick pass, `render --annotation`).
    pub(crate) camera_m: Option<crate::view::Camera>,
    /// Tag every entity drawn, for a pick pass.
    pub(crate) pick: bool,
    /// An annotation to draw, its pins and marks.
    pub(crate) annotation: Option<crate::annotation::Annotation>,
}

impl RenderAsk {
    /// What a pick pass draws of `part` seen through `camera`: the
    /// working model (or the session's draft), every edge and sketch,
    /// no datums, labels, appearance, compare or explode, cut by
    /// `sections` and leaving out `hidden`.
    pub(crate) fn pick(
        part: &str,
        camera: crate::view::Camera,
        sections: Vec<crate::section::Plane>,
        hidden: std::collections::BTreeSet<String>,
    ) -> RenderAsk {
        RenderAsk {
            part: Some(part.to_owned()),
            camera: None,
            view: None,
            width: 800,
            height: 600,
            highlight: Vec::new(),
            edges: true,
            labels: false,
            sketches: true,
            datums: false,
            rev: None,
            frame: Vec::new(),
            compare: None,
            hidden,
            origin: false,
            appearance: false,
            sections,
            explode: None,
            annotations: false,
            flat: None,
            camera_m: Some(camera),
            pick: true,
            annotation: None,
        }
    }
}

/// What a render draws and from where: its geometry, camera, each
/// assembly's occurrences' poses by instance path, the
/// commit read, the compare's revision and commit, and its warnings.
pub(crate) struct Scene {
    pub(crate) g: crate::render::Geometry,
    pub(crate) camera: crate::view::Camera,
    pub(crate) poses: BTreeMap<String, BTreeMap<String, ryukei::geom::Rigid>>,
    pub(crate) commit: Option<String>,
    pub(crate) compare: Option<Value>,
    pub(crate) warnings: Vec<Value>,
}

/// An explode a render draws.
pub(crate) struct DrawExplode {
    /// The assembly of the explode a participant's window shows, which the
    /// render draws only if it draws that assembly; `None` for one the
    /// request (or a named view) names, of the one part rendered.
    shown: Option<Id>,
    id: String,
    /// How far through it.
    fraction: f64,
}

/// The explode a render draws: its assembly, where it moves each
/// occurrence, its resolved steps and how far through them.
type Exploded = (
    Id,
    BTreeMap<String, crate::explode::Offset>,
    Vec<crate::explode::Step>,
    f64,
);

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
        let made = occurrence_meshes(s, &solids, mesh::Quality::Normal)?;
        for (sol, m) in solids.iter().zip(made) {
            if m.mesh.is_err() {
                continue;
            }
            let m = mesh::Meshed {
                body: sol.built.body.clone(),
                hash: sol.built.hash.clone(),
                mesh: m,
                warning: None,
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

/// A render's share of the session's memory budget: a quarter of
/// `session.max_memory`, at least 64 MiB (#400). A render of a large
/// assembly copies each occurrence's placed mesh into its geometry, so it
/// must stop before it holds more than the session may.
pub(crate) fn render_budget() -> usize {
    #[cfg(test)]
    if let Some(b) = tests::RENDER_BUDGET.with(|b| b.get()) {
        return b;
    }
    let max = config::user("session.max_memory")
        .as_u64()
        .unwrap_or_else(crate::budget::default_max);
    usize::try_from(max / 4).unwrap_or(usize::MAX).max(64 << 20)
}

/// `render_too_large` for a render of `part` whose geometry passed
/// `budget` after `drawn` of its `total` occurrences (#400).
fn render_past_budget(part: &Id, drawn: usize, total: usize, bytes: usize, budget: usize) -> Error {
    Error::new(
        Code::RenderTooLarge,
        format!(
            "{part}'s render would hold more than {} MiB (a quarter of session.max_memory) \
             after {drawn} of its {total} occurrences: hide some (`--hidden`), render a \
             subassembly, or cut it with a section",
            budget >> 20,
        ),
    )
    .with("part", part)
    .with("occurrences", total)
    .with("drawn", drawn)
    .with("bytes", bytes)
    .with("budget", budget)
}

/// A release package's render (design 436): `render <part> --rev <commit>
/// --view iso --size 1600x1200` with the CLI's other defaults, no
/// participant's view, highlights, hidden set, sections or explode.
pub(crate) fn render_for_package(session: &mut Session, part: &Id, commit: &str) -> Result<Answer> {
    let ask = RenderAsk {
        part: Some(part.to_string()),
        camera: None,
        view: Some("iso".into()),
        width: 1600,
        height: 1200,
        highlight: Vec::new(),
        edges: true,
        labels: false,
        sketches: true,
        datums: true,
        rev: Some(commit.to_owned()),
        frame: Vec::new(),
        compare: None,
        hidden: Default::default(),
        origin: false,
        appearance: true,
        sections: Vec::new(),
        explode: None,
        annotations: false,
        flat: None,
        camera_m: None,
        pick: false,
        annotation: None,
    };
    render_noting(session, None, Vec::new(), ask)
}

/// [`render`], counting the meshes it makes against its budget, and
/// taking them back out of the mesh cache when its budget refuses it, so
/// a refused render leaves nothing behind (#407).
fn render_noting(
    session: &mut Session,
    target: Option<(&str, Option<&Live>)>,
    shown: Vec<live::Highlight>,
    ask: RenderAsk,
) -> Result<Answer> {
    session.meshes.note();
    let r = render(session, target, shown, ask);
    let refused = matches!(&r, Err(e) if e.code == Code::RenderTooLarge);
    session.meshes.end_note(refused);
    r
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
    crate::render::check_size(ask.width, ask.height)?;
    if let Some(solid) = ask.flat.clone() {
        return render_flat(session, ask, solid);
    }
    let (width, height, edges, part) = (ask.width, ask.height, ask.edges, ask.part.clone());
    let scene = render_scene(session, target, shown, ask)?;
    let options = crate::render::Options {
        silhouettes: edges,
        triad: true,
    };
    let img = crate::render::render(&scene.g, &scene.camera, width, height, options)?;
    let mut result =
        json!({"part": part, "width": width, "height": height, "camera": scene.camera});
    if let Some(c) = scene.commit {
        result["commit"] = json!(c);
    }
    if let Some(c) = scene.compare {
        result["compare"] = c;
    }
    if !scene.warnings.is_empty() {
        result["warnings"] = json!(scene.warnings);
    }
    Ok(Answer::bytes(result, "image/png", img.png()))
}

/// The geometry and camera a render draws, without its image: what
/// `render` rasterizes, and what a pick pass ([`crate::render::pick`])
/// reads with `ask.pick`.
pub(crate) fn render_scene(
    session: &mut Session,
    target: Option<(&str, Option<&Live>)>,
    shown: Vec<live::Highlight>,
    ask: RenderAsk,
) -> Result<Scene> {
    use crate::render::{self as r, Geometry};
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
    if ask.pick {
        g.tags = Some(Default::default());
    }
    // Each assembly's occurrences' poses, by instance path, for the pins
    // `annotations` draws on them.
    let mut poses: BTreeMap<String, BTreeMap<String, ryukei::geom::Rigid>> = BTreeMap::new();
    // What the geometry holds, judged against the render's budget (#400).
    let budget = render_budget();
    let mut tally = r::Tally::default();
    // How each part's solids look, by key, unless asked for the theme.
    let mut looks: BTreeMap<Id, BTreeMap<String, crate::appearance::SolidLook>> = BTreeMap::new();
    if ask.appearance {
        for part in &parts {
            looks.insert(part.clone(), crate::appearance::looks(session, part)?);
        }
    }
    // The explode drawn, by its assembly: where it moves each occurrence,
    // and the lines it leaves.
    let mut warnings: Vec<Value> = Vec::new();
    let exploded: Option<Exploded> = match &ask.explode {
        None => None,
        // A window's explode of an assembly not drawn draws nothing.
        Some(DrawExplode { shown: Some(p), .. }) if !parts.contains(p) => None,
        Some(DrawExplode {
            shown: Some(part),
            id,
            fraction,
        }) => {
            // A window's explode that is gone (deleted, undone, checked out
            // away) draws nothing, with a warning (issue #262).
            match crate::explode::resolve(session, part, id) {
                Ok(r) => {
                    let offsets = crate::explode::offsets(&r.steps, *fraction);
                    Some((part.clone(), offsets, r.steps, *fraction))
                }
                Err(e) => {
                    warnings.push(json!({
                        "code": e.code,
                        "message": format!("the window's exploded view isn't drawn: {}", e.message),
                        "part": part,
                        "explode": id,
                    }));
                    None
                }
            }
        }
        Some(DrawExplode {
            shown: None,
            id,
            fraction,
        }) => {
            let [part] = parts.as_slice() else {
                return Err(Error::new(
                    Code::InvalidParam,
                    "the render shows several parts: render the assembly whose explode it is",
                )
                .with("explode", id.as_str()));
            };
            let r = crate::explode::resolve(session, part, id)?;
            let offsets = crate::explode::offsets(&r.steps, *fraction);
            Some((part.clone(), offsets, r.steps, *fraction))
        }
    };
    // Renders don't follow anyone's display quality, so they compare
    // across machines.
    for part in &parts {
        let look = |key: &str| looks.get(part).and_then(|l| l.get(key));
        let moved = exploded.as_ref().filter(|(p, ..)| p == part);
        // An assembly draws its occurrences where its instances put them.
        if part_kind(session.project().part(part)?) == PartKind::Assembly {
            let model = session.model(part, &crate::doc::assembly::Rev::Working)?;
            if let Some((_, _, steps, fraction)) = moved {
                for (path, start) in crate::explode::middles(&model) {
                    let trail = crate::explode::trail(steps, &path, start, *fraction);
                    if trail.len() < 2 || ask.hidden.contains(&format!("{part}/{path}")) {
                        continue;
                    }
                    let base = g.positions.len() as u32;
                    g.positions.extend(trail.iter().copied());
                    g.lines.push(crate::render::Line {
                        points: (base..base + trail.len() as u32).collect(),
                        color: crate::render::TRAIL,
                        width: 1.0,
                        dash: true,
                    });
                }
            }
            let total = model.occurrences.len();
            for (drawn, o) in model.occurrences.iter().enumerate() {
                let bytes = g.bytes_with(&mut tally) + session.meshes.noted_bytes();
                if bytes > budget {
                    return Err(render_past_budget(part, drawn, total, bytes, budget));
                }
                let path = crate::assembly::path_text(&o.path);
                if ask.hidden.contains(&format!("{part}/{path}")) {
                    continue;
                }
                let pose = match moved.and_then(|(_, offsets, ..)| offsets.get(&path)) {
                    Some(off) => off.0.then_after(&o.pose),
                    None => o.pose,
                };
                poses
                    .entry(part.to_string())
                    .or_default()
                    .insert(path.clone(), pose);
                let solids: Vec<&crate::assembly::Solid> = o.solids.iter().collect();
                let made = occurrence_meshes(session, &solids, mesh::Quality::Normal)?;
                for (s, m) in solids.iter().zip(made) {
                    let m = mesh::Meshed {
                        body: s.built.body.clone(),
                        hash: s.built.hash.clone(),
                        mesh: m,
                        warning: None,
                    };
                    let key = (path.clone(), s.founder.clone());
                    let (whole, faces) = match theirs_occurrences.get(part) {
                        Some(theirs) => compare_paint(theirs.get(&key).map(|(_, t)| t), &m),
                        None => (None, BTreeMap::new()),
                    };
                    draw_body(
                        &mut g,
                        &m,
                        &pose,
                        &format!("{part}/{path}/{}", s.founder),
                        &marks,
                        (whole, &faces, look(&format!("{path}/{}", s.founder))),
                        (ask.labels, ask.edges),
                    );
                    seen_occurrences.insert((part.clone(), key.0, key.1), (o.pose, m));
                }
            }
            let bytes = g.bytes_with(&mut tally) + session.meshes.noted_bytes();
            if bytes > budget {
                return Err(render_past_budget(part, total, total, bytes, budget));
            }
            continue;
        }
        let first = g.positions.len();
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
                (whole, &faces, look(fid.as_str())),
                (ask.labels, ask.edges),
            );
            let bytes = g.bytes_with(&mut tally) + session.meshes.noted_bytes();
            if bytes > budget {
                return Err(render_past_budget(part, 1, 1, bytes, budget));
            }
        }
        if ask.sketches {
            sketches(session, part, &marks, ask.labels, &mut g)?;
        }
        if ask.datums {
            let drawn = (
                bounds(&g.positions[first..]),
                session.rebuilt(part)?.features.clone(),
            );
            for (fid, r) in &drawn.1 {
                let Ok(crate::rebuild::Output::Datum(d)) = r else {
                    continue;
                };
                let key = format!("{part}/{fid}");
                if ask.hidden.contains(&key) {
                    continue;
                }
                g.add_datum(&key, &d.geom, drawn.0, marks.get(&key).map(|m| m.0));
            }
            // Paths and helices too, as polylines.
            let (lo, hi) = drawn.0;
            let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            let span = ryukei::math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            let span = if span.is_finite() { span } else { 0.0 };
            for (fid, r) in &drawn.1 {
                let Ok(crate::rebuild::Output::Path(m)) = r else {
                    continue;
                };
                let key = format!("{part}/{fid}");
                if ask.hidden.contains(&key) {
                    continue;
                }
                g.add_path(&key, &m.chain(), span, marks.get(&key).map(|m| m.0));
            }
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
    let files = session.snapshot();
    let none = BTreeMap::new();
    // The annotation `render --annotation` names is drawn once, by its
    // own block below; the others `--annotations` asks for keep their
    // numbers.
    let asked = ask.annotation.as_ref().map(|a| &a.id);
    let numbers = crate::annotation::numbers(&files);
    if ask.annotations {
        for part in &parts {
            let on = poses.get(part.as_str()).unwrap_or(&none);
            for (at, n) in crate::annotation::markers(&files, part.as_str(), on, asked) {
                g.add_marker(at, &n);
            }
        }
        let all = crate::annotation::readable(&crate::annotation::read_all(&files));
        for a in all
            .values()
            .filter(|a| a.status.is_open() && Some(&a.id) != asked)
        {
            if let (Some(m), Some(v)) = (&a.marks, &a.view)
                && parts.contains(&m.part)
            {
                let plane = crate::redline::Plane::of(&crate::view::Camera::from(v));
                crate::redline::draw(
                    &mut g,
                    &plane,
                    m,
                    &poses,
                    r::participant(&a.by.participant),
                    1.0,
                );
            }
        }
    }
    if let Some(a) = &ask.annotation {
        // Beside the other open annotations, an open one's pins carry its
        // number, as theirs do; alone, or closed, their place in it.
        let number = numbers.get(&a.id).filter(|_| ask.annotations);
        for (k, pin) in a.pins.iter().enumerate() {
            let label = number.map_or_else(|| (k + 1).to_string(), |n| n.to_string());
            for part in &parts {
                let on = poses.get(part.as_str()).unwrap_or(&none);
                if let Some(at) = crate::annotation::pin_on(pin, part.as_str(), on) {
                    g.add_marker(at, &label);
                }
            }
        }
        if let (Some(m), Some(v)) = (&a.marks, &a.view) {
            let plane = crate::redline::Plane::of(&crate::view::Camera::from(v));
            let alpha = if a.status.is_open() {
                1.0
            } else {
                crate::redline::CLOSED_ALPHA
            };
            crate::redline::draw(
                &mut g,
                &plane,
                m,
                &poses,
                r::participant(&a.by.participant),
                alpha,
            );
        }
    }

    g.sections = ask.sections.clone();
    let fixed = ask.camera_m.clone();
    let camera = match (ask.camera, target, ask.view.as_deref()) {
        _ if fixed.is_some() => fixed.expect("checked"),
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
    Ok(Scene {
        g,
        camera,
        poses,
        commit,
        compare: other.map(|(rev, (commit, _))| json!({"rev": rev, "commit": commit})),
        warnings,
    })
}

/// `render --flat`: `solid`'s flat pattern (`""`: the part's one sheet
/// metal solid) as `flat.get` gives it, its flat body drawn as any body
/// and seen along the pattern's `z` with its `y` up, its bend lines dashed
/// over it in [`crate::render::BEND_UP`] or `BEND_DOWN`, labelled with
/// their notes when `labels`.
fn render_flat(session: &mut Session, ask: RenderAsk, solid: String) -> Result<Answer> {
    use crate::render::{self as r, Geometry, Label, Line, Options};
    for (param, given) in [
        ("compare", ask.compare.is_some()),
        ("frame", !ask.frame.is_empty()),
        ("view", ask.view.is_some()),
    ] {
        if given {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{param} doesn't apply to a flat pattern's render"),
            )
            .with("param", param));
        }
    }
    let part = match render_parts(session.at(ask.rev.as_deref())?.1, ask.part.as_deref())?
        .as_slice()
    {
        [one] => one.to_string(),
        _ => {
            return Err(
                Error::new(Code::InvalidParam, "a flat pattern is one part's: name it")
                    .with("param", "part"),
            );
        }
    };
    let named = (!solid.is_empty()).then_some(solid.as_str());
    let (commit, f) = crate::sheet_metal::flat::of(
        session,
        &part,
        named,
        ask.rev.as_deref(),
        &Default::default(),
    )?;
    let p = &f.pattern;
    let meshed = flat_meshed(&f);
    let mut g = Geometry::default();
    draw_body(
        &mut g,
        &meshed,
        &ryukei::geom::Rigid::IDENTITY,
        &format!("{}/{}", f.part, f.solid),
        &BTreeMap::new(),
        (None, &BTreeMap::new(), None),
        (false, ask.edges),
    );
    let fr = &p.frame;
    let lift = |q: ryukei::geom::Point2| {
        let w = fr.origin() + fr.x().get() * q.x + fr.y().get() * q.y;
        [w.x, w.y, w.z]
    };
    for l in &p.bend_lines {
        let at = g.positions.len() as u32;
        g.positions.push(lift(l.start));
        g.positions.push(lift(l.end));
        let color = match l.direction {
            ryukei::sheet_metal::BendDirection::Up => r::BEND_UP,
            ryukei::sheet_metal::BendDirection::Down => r::BEND_DOWN,
        };
        g.lines.push(Line {
            points: vec![at, at + 1],
            color,
            width: 2.0,
            dash: true,
        });
        if ask.labels {
            let mid =
                ryukei::geom::Point2::new(0.5 * (l.start.x + l.end.x), 0.5 * (l.start.y + l.end.y));
            g.labels.push(Label {
                at: lift(mid),
                text: ryuformats::dxf::note(l, ryuformats::dxf::DxfUnit::Mm, "°"),
            });
        }
    }
    let camera = match ask.camera {
        Some(c) => c.resolve(session.project().manifest.default_unit)?,
        None => {
            let z = fr.z().get();
            let y = fr.y().get();
            let aspect = f64::from(ask.width) / f64::from(ask.height);
            crate::view::frame_points_along(
                &g.positions,
                [z.x, z.y, z.z],
                [y.x, y.y, y.z],
                DEFAULT_PROJECTION,
                aspect,
            )
        }
    };
    let img = r::render(
        &g,
        &camera,
        ask.width,
        ask.height,
        Options {
            silhouettes: ask.edges,
            triad: true,
        },
    )?;
    let mut result = json!({
        "part": f.part, "flat": f.solid, "width": ask.width, "height": ask.height, "camera": camera,
        "from": match &f.from { crate::sheet_metal::flat::From::Whole => "whole", crate::sheet_metal::flat::From::Unfold(_) => "unfold" },
    });
    if let Some(c) = commit {
        result["commit"] = json!(c);
    }
    Ok(Answer::bytes(result, "image/png", img.png()))
}

/// A flat's display mesh, at normal quality for the pattern's own size,
/// made the first time anything asks and kept with the flat: every name
/// the shown body's, so a pick on it is a pick on the solid.
fn flat_meshed(f: &crate::sheet_metal::flat::FlatOf) -> mesh::Meshed {
    let p = &f.pattern;
    let size = (p.bounds.max() - p.bounds.min()).length().max(1e-3);
    let mesh = f
        .mesh
        .get_or_init(|| {
            let params = crate::mesh::Quality::Normal.params(size);
            std::sync::Arc::new(crate::mesh::DisplayMesh {
                mesh: ryukei::mesh::tessellate(&f.body.body, &params)
                    .map(std::sync::Arc::new)
                    .map_err(|e| Error::new(Code::Internal, format!("meshing the flat: {e:?}"))),
                params,
            })
        })
        .clone();
    mesh::Meshed {
        body: f.body.body.clone(),
        hash: f.body.hash.clone(),
        mesh,
        warning: None,
    }
}

/// The flat `mesh.get` answers of: `solid` (`""`: the part's one sheet
/// metal solid) laid flat as `flat.get` lays it, the commit read.
fn mesh_flat(
    session: &mut Session,
    part: &str,
    solid: &str,
    rev: Option<&str>,
) -> Result<(Value, mesh::Meshed)> {
    let named = (!solid.is_empty()).then_some(solid);
    let (commit, f) = crate::sheet_metal::flat::of(session, part, named, rev, &Default::default())?;
    let mut v = json!({
        "part": f.part, "quality": crate::mesh::Quality::Normal.as_str(), "flat": f.solid,
        "from": match &f.from { crate::sheet_metal::flat::From::Whole => "whole", crate::sheet_metal::flat::From::Unfold(_) => "unfold" },
    });
    if let Some(c) = commit {
        v["commit"] = json!(c);
    }
    let m = flat_meshed(&f);
    Ok((v, m))
}

/// `mesh.get --flat` with `transfer`: the flat's one body as bytes,
/// labelled by its solid.
fn mesh_flat_binary(
    session: &mut Session,
    part: &str,
    solid: &str,
    rev: Option<&str>,
    compact: bool,
) -> Result<Answer> {
    let (v, m) = mesh_flat(session, part, solid, rev)?;
    let label = v["flat"].as_str().unwrap_or_default().to_owned();
    let bodies = [(label, Ok(m))];
    Ok(match compact {
        true => Answer::bytes(v, mesh::COMPACT, mesh::encode_compact(&bodies)),
        false => Answer::bytes(v, mesh::ENCODING, mesh::encode(&bodies)),
    })
}

/// `mesh.get --flat` meshes one flat body at normal quality, so it takes
/// no feature, occurrences or quality: refused, naming the field, rather
/// than answered as if they were honoured.
fn flat_alone(feature: Option<&str>, features: &[String], quality: Option<&str>) -> Result<()> {
    let given = [
        ("feature", feature.is_some()),
        ("features", !features.is_empty()),
        ("quality", quality.is_some()),
    ];
    match given.iter().find(|(_, g)| *g) {
        Some((field, _)) => Err(Error::new(
            Code::InvalidParam,
            format!(
                "a flat body is the solid's one flat, at normal quality: give flat without {field}"
            ),
        )
        .with("param", *field)),
        None => Ok(()),
    }
}

/// `mesh.get --flat` as JSON: the flat's one body, as `mesh_get` gives a
/// part's.
fn mesh_flat_json(
    session: &mut Session,
    part: &str,
    solid: &str,
    rev: Option<&str>,
) -> Result<Value> {
    let (mut v, m) = mesh_flat(session, part, solid, rev)?;
    let mut b = mesh::to_json(&m.body, &m.mesh);
    b["feature"] = v["flat"].clone();
    b["hash"] = json!(m.hash);
    v["bodies"] = json!([b]);
    Ok(v)
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
    (whole, faces, look): (
        Option<crate::render::Rgb>,
        &BTreeMap<String, crate::render::Rgb>,
        Option<&crate::appearance::SolidLook>,
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
    // Translucent faces by ID, with their opacities: their edges fade
    // with them.
    let mut glass: BTreeMap<ryukei::topo::FaceId, f32> = BTreeMap::new();
    // A sheet's faces are drawn from both sides (ryucad 18).
    let sheet = m.body.kind() == ryukei::topo::BodyKind::Sheet;
    for f in &mesh.faces {
        let name = m.body.face(f.face).expect("live").name.to_string();
        let mark = marks.get(&key(name.clone()));
        // Highlights over the compare's paint, as in the app, and both
        // over the appearance, opaque.
        let painted = mark
            .map(|(c, _)| *c)
            .or(whole)
            .or_else(|| faces.get(&name).copied());
        let seen = look.map(|l| l.face(&name));
        let color = painted.unwrap_or_else(|| seen.map_or(r::SURFACE, |e| e.rgb(r::SURFACE)));
        let alpha = match (painted, seen) {
            (None, Some(e)) if e.opacity < 1.0 => Some(e.opacity as f32),
            _ => None,
        };
        if let Some(a) = alpha {
            glass.insert(f.face, a);
        }
        let (start, sheet_start) = (g.triangles.len(), g.sheets.len());
        for t in &mesh.triangles[f.triangles.start as usize..f.triangles.end as usize] {
            let c = t.map(|i| mesh.corners[i as usize]);
            let t = Triangle {
                p: c.map(|c| base + c.position),
                n: c.map(|c| {
                    let n = c.normal.get();
                    let n = if moved { pose.apply_vector(n) } else { n };
                    [n.x as f32, n.y as f32, n.z as f32]
                }),
                color,
            };
            match alpha {
                Some(a) => g.translucent.push((t, a)),
                None if sheet => g.sheets.push(t),
                None => g.triangles.push(t),
            }
        }
        if let Some(t) = g.tags.as_mut() {
            let id = t.name(&key(name.clone()));
            if g.triangles.len() > start {
                t.triangles.push((start..g.triangles.len(), id));
            }
            if g.sheets.len() > sheet_start {
                t.sheets.push((sheet_start..g.sheets.len(), id));
            }
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
                if let Some(t) = g.tags.as_mut() {
                    let id = t.name(&key(name.clone()));
                    t.lines.push((g.lines.len() - 1, id));
                }
            }
            None if edges => {
                let line = Line {
                    points,
                    color: r::EDGE,
                    width: 1.0,
                    dash: false,
                };
                // As opaque as the most opaque face it bounds.
                let alpha = m
                    .body
                    .edge_fins(e.edge)
                    .filter_map(|fin| m.body.loop_(m.body.fin(fin)?.loop_))
                    .map(|l| glass.get(&l.face).copied().unwrap_or(1.0))
                    .fold(0.0f32, f32::max);
                if alpha > 0.0 && alpha < 1.0 {
                    g.translucent_lines.push((line, alpha));
                } else {
                    g.lines.push(line);
                    if let Some(t) = g.tags.as_mut() {
                        let id = t.name(&key(name.clone()));
                        t.lines.push((g.lines.len() - 1, id));
                    }
                }
            }
            None => {}
        }
    }
    for (i, v) in m.body.vertices().enumerate() {
        let name = m.body.vertex(v).expect("live").name.to_string();
        if let Some(t) = g.tags.as_mut() {
            let id = t.name(&key(name.clone()));
            t.points.push((base + i as u32, id));
        }
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

/// The box of `positions`; an empty one at the origin when there are none.
fn bounds(positions: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if positions.is_empty() {
        ([0.0; 3], [0.0; 3])
    } else {
        (lo, hi)
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
            if let Some(t) = g.tags.as_mut() {
                let id = t.name(&format!("{part}/{fid}/{}", c.entity));
                t.lines.push((g.lines.len() - 1, id));
            }
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
            if let Some(t) = g.tags.as_mut() {
                let id = t.name(&format!("{part}/{fid}/{}", p.entity));
                t.points.push((g.positions.len() as u32 - 1, id));
            }
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
    let made = occurrence_meshes(session, &refs, quality)?;
    let bodies = solids
        .into_iter()
        .zip(made)
        .map(|((label, s), m)| {
            let m = mesh::Meshed {
                body: s.built.body.clone(),
                hash: s.built.hash.clone(),
                mesh: m,
                warning: None,
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
    let mut warnings = Vec::new();
    let bodies: Vec<Value> = meshes(session, &pid, only.as_ref(), quality)?
        .into_iter()
        .map(|(id, r)| match r {
            Ok(m) => {
                warnings.extend(m.warning.clone());
                let mut v = mesh::to_json(&m.body, &m.mesh);
                v["feature"] = json!(id);
                v["hash"] = json!(m.hash);
                v
            }
            Err(e) => json!({"feature": id, "ok": false, "error": e}),
        })
        .collect();
    // Each body is a solid, by its founder (`feature`, as entity
    // references name it); a reference's holes are warned of.
    let mut v = json!({"part": pid, "quality": quality.as_str(), "bodies": bodies});
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    Ok(v)
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
    let unit = session.project().manifest.default_unit;
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
    // A sweep, loft, mirror, split or tweak asked for says what it built from:
    // its part, held apart from the session while it rebuilds.
    let held = match &only {
        Some(f)
            if features.iter().any(|(i, x)| {
                i == f
                    && (x.sweep().is_some()
                        || x.loft().is_some()
                        || matches!(
                            x,
                            Feature::Mirror(_)
                                | Feature::Split(_)
                                | Feature::MoveFace(_)
                                | Feature::ReplaceFace(_)
                                | Feature::Draft(_)
                                | Feature::DeleteFace(_)
                                | Feature::DeleteBlend(_)
                                | Feature::ResizeBlend(_)
                        ))
            }) =>
        {
            Some(session.project().part(&pid)?.clone())
        }
        _ => None,
    };
    let result = session.rebuilt(&pid)?;
    let of = |id: &Id| {
        &features
            .iter()
            .find(|(i, _)| i == id)
            .expect("built features loaded")
            .1
    };
    // A datum's geometry, or why it didn't build; the model lists every
    // datum (only when it has any).
    let datum_line = |id: &Id| -> Option<Value> {
        let (_, r) = result.features.iter().find(|(f, _)| f == id)?;
        let (_, f) = features.iter().find(|(i, _)| i == id)?;
        let ty = f.type_name();
        if !matches!(f, Feature::Datum(_)) {
            return None;
        }
        Some(match r {
            Ok(o) => json!({"feature": id, "type": ty, "datum": o.datum()?.report()}),
            Err(e) => json!({"feature": id, "type": ty, "error": e}),
        })
    };
    if let Some(f) = &only
        && let Some(d) = datum_line(f)
    {
        return Ok(json!({"part": pid, "datum": d, "bodies": []}));
    }
    // A path's or helix's geometry, or why it didn't build; the model
    // lists every one (only when it has any).
    let path_line = |id: &Id| -> Option<Value> {
        let (_, r) = result.features.iter().find(|(f, _)| f == id)?;
        let (_, f) = features.iter().find(|(i, _)| i == id)?;
        let ty = f.type_name();
        if !matches!(f, Feature::Path(_)) {
            return None;
        }
        Some(match r {
            Ok(o) => json!({"feature": id, "type": ty, ty: o.path()?.report(&pid)}),
            Err(e) => json!({"feature": id, "type": ty, "error": e}),
        })
    };
    if let Some(f) = &only
        && let Some(d) = path_line(f)
    {
        let ty = d["type"].as_str().unwrap_or("path").to_owned();
        return Ok(json!({"part": pid, ty: d, "bodies": []}));
    }
    let (datums, paths): (Vec<Value>, Vec<Value>) = if only.is_none() {
        (
            result
                .features
                .iter()
                .filter_map(|(id, _)| datum_line(id))
                .collect(),
            result
                .features
                .iter()
                .filter_map(|(id, _)| path_line(id))
                .collect(),
        )
    } else {
        (Vec::new(), Vec::new())
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
        // The model: each solid as it shows. An inserted solid nothing
        // acted on is counted under its insert instead (`inserts`), so a
        // board of hundreds is one line; asked for an entity, every
        // solid counts.
        None => {
            for shown in result.shown(&solids) {
                let untouched =
                    shown.solid.is_path() && shown.tip.as_str() == shown.solid.founder_str();
                if untouched && entity.is_none() {
                    continue;
                }
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
    let mut v = json!({"part": pid, "bodies": bodies});
    // Each insert: what it inserted, by count for the model, whole for
    // the insert asked for (design 521).
    let inserts: Vec<Value> = features
        .iter()
        .filter_map(|(id, f)| Some((id, f.insert()?)))
        .filter(|(id, _)| only.as_ref().is_none_or(|o| o == *id))
        .map(|(id, i)| {
            let made = match result.output(id) {
                Some(crate::rebuild::Output::Solids(m)) => Some(&**m),
                _ => None,
            };
            if only.is_some() {
                return crate::insert::report(i, made);
            }
            let r = crate::insert::report(i, None);
            json!({"feature": id, "of": r["of"], "rev": r["rev"],
                   "solids": made.map_or(0, |m| m.solids.len())})
        })
        .collect();
    match (&only, inserts.as_slice()) {
        (Some(_), [one]) => v["insert"] = one.clone(),
        (None, [_, ..]) => v["inserts"] = json!(inserts),
        _ => {}
    }
    if let (Some(p), Some(f)) = (&held, &only)
        && let Some(Ok(Feature::Sweep(w))) = p.features.get(f)
        && let Some(d) = crate::along::described(p, f, w, result)
    {
        v["sweep"] = d;
    }
    if let (Some(p), Some(f)) = (&held, &only)
        && let Some(Ok(Feature::Loft(l))) = p.features.get(f)
        && let Some(d) = crate::loft::described(p, f, l, result)
    {
        v["loft"] = d;
    }
    if !datums.is_empty() {
        v["datums"] = json!(datums);
    }
    if !paths.is_empty() {
        v["paths"] = json!(paths);
    }
    // A shell, offset, mirror or split says what it was given, as
    // `feature.show` does; a mirror or split that built, its plane as
    // resolved too.
    if let Some(f) = &only
        && let Some((
            _,
            x @ (Feature::Shell(_)
            | Feature::Offset(_)
            | Feature::Mirror(_)
            | Feature::Split(_)
            | Feature::MoveFace(_)
            | Feature::ReplaceFace(_)
            | Feature::Draft(_)
            | Feature::DeleteFace(_)
            | Feature::DeleteBlend(_)
            | Feature::ResizeBlend(_)),
        )) = features.iter().find(|(i, _)| i == f)
    {
        let k = x.type_name();
        let tweak = crate::tweak::takes_sheets(x);
        v[k] = if tweak {
            crate::tweak::report(&pid, x)
        } else {
            crate::body::report(&pid, x)
        };
        let built = matches!(crate::rebuild::Results::result(result, f), Some(Ok(_)));
        if let Some(Value::Object(extra)) = held.as_ref().filter(|_| built).and_then(|q| {
            if tweak {
                crate::tweak::resolved(q, result, f, x, true, unit)
            } else {
                crate::body::resolved(q, result, f, x)
            }
        }) {
            for (key, e) in extra {
                v[k][key] = e;
            }
        }
    }
    if let Some(f) = &only
        && let Some((_, x @ (Feature::Fill(_) | Feature::Extract(_)))) =
            features.iter().find(|(i, _)| i == f)
    {
        v[x.type_name()] = crate::surface::report(&pid, x);
    }
    Ok(v)
}

/// `assembly.solve` answered for `rev` (the working files when `None`):
/// any revision a read takes, a person's draft included, named in the
/// answer's `rev`. `mates` are each run as `feature.add` of a mate in a
/// capture session over those files and solved with the rest, never
/// written; a refusal is `feature.add`'s own, at `/mates/<i>`.
fn solve_at(
    session: &mut Session,
    part: &str,
    moves: Vec<(String, crate::assembly::Move)>,
    component: bool,
    rev: Option<&str>,
    mates: Vec<serde_json::Map<String, Value>>,
) -> Result<Value> {
    let rev = rev.filter(|r| *r != "working");
    let (named, files) = match rev {
        None => (None, None),
        Some(r) => {
            let (n, f) = files_at(session, r)?;
            (Some(n), Some(f))
        }
    };
    let mut v = if mates.is_empty() {
        let (_, s) = session.at(rev)?;
        crate::mate::solve_request(s, part, moves, component)?
    } else {
        let files = files.unwrap_or_else(|| session.snapshot());
        let mut capture = session.capturing(files)?;
        for (i, m) in mates.into_iter().enumerate() {
            let at = |mut e: Error| {
                if let Some(p) = e.details.get("pointer").cloned() {
                    e = e.with("field", p);
                }
                e.with("pointer", format!("/mates/{i}"))
            };
            let mut params = json!({"part": part, "type": "mate"});
            for (k, v) in m {
                params[k] = v;
            }
            let req = Request::from_rpc("feature.add", Some(params)).map_err(at)?;
            dispatch(&mut capture, None, None, req).map_err(at)?;
        }
        crate::mate::solve_request(&mut capture, part, moves, component)?
    };
    if let Some(n) = named {
        v["rev"] = n;
    }
    Ok(v)
}

/// A revision's files, and how the answer names it: the working files
/// (`working`), another worktree's (`worktree:<path>`, with its branch and
/// head) or a commit.
fn files_at(session: &mut Session, rev: &str) -> Result<(Value, activity::Files)> {
    if rev == "working" {
        return Ok((json!({"rev": rev}), session.snapshot()));
    }
    if let Some((named, files, stale)) = session.annotation_files(rev)? {
        if !stale.is_empty() {
            session
                .rev_warnings
                .push(crate::session::stale_warning(rev, &stale));
        }
        return Ok((named, files));
    }
    if let Some((named, files)) = session.draft_files(rev)? {
        return Ok((named, files));
    }
    if let Some((w, files, _)) = session.worktree_files(rev)? {
        let named = json!({"rev": rev, "worktree": w.root, "branch": w.branch, "head": w.head});
        return Ok((named, files));
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

/// Each drawing of every part at `rev`, built: by part and drawing, each
/// view's fingerprint by key, or why it didn't build.
fn drawn_at(session: &mut Session, rev: &str) -> Result<crate::diff::Drawn> {
    let (_, s) = session.at(Some(rev))?;
    let mut out = crate::diff::Drawn::new();
    let ids: Vec<(Id, Vec<Id>)> = s
        .project()
        .parts
        .iter()
        .map(|(id, p)| (id.clone(), p.drawings.keys().cloned().collect()))
        .collect();
    for (pid, drawings) in ids {
        for d in drawings {
            let views = match crate::drawing::build(s, &pid, &d) {
                Ok(b) => Ok(b
                    .sheets
                    .iter()
                    .flat_map(|sh| {
                        sh.views
                            .iter()
                            .map(move |v| (v.key.to_string(), v.fingerprint(sh)))
                    })
                    .collect()),
                Err(e) => Err(e.code),
            };
            out.insert((pid.to_string(), d.to_string()), views);
        }
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
    let (a_rev, b_rev) = (from.to_owned(), to.to_owned());
    let releases = crate::release::history(session)
        .map(|(_, r)| r)
        .unwrap_or_default();
    let mut v = crate::diff::structural(&fa, &fb, opts, &releases);
    v["from"] = a;
    v["to"] = b;
    if model {
        let job = session.job.clone();
        let stage = job.begin(Phase::Rebuild, "revisions", 2);
        stage.step(|| from.to_owned())?;
        let from = rebuilt_at(session, from)?;
        stage.step(|| to.to_owned())?;
        let to = rebuilt_at(session, to)?;
        drop(stage);
        crate::diff::model(&mut v, &from, &to, opts);
        // Each drawing whose built views differ (design 417).
        let (dfrom, dto) = (drawn_at(session, &a_rev)?, drawn_at(session, &b_rev)?);
        crate::diff::drawings_model(&mut v, &dfrom, &dto, opts);
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

    thread_local! {
        /// A render budget for this thread's renders in place of
        /// `session.max_memory`'s quarter (#400's test).
        pub(crate) static RENDER_BUDGET: std::cell::Cell<Option<usize>> =
            const { std::cell::Cell::new(None) };
    }

    /// #400: a render stops once its geometry passes its budget, naming
    /// how many occurrences it drew; under a budget that holds it, it
    /// draws as before.
    #[test]
    fn a_render_past_its_budget_is_refused_and_one_within_it_draws() {
        let (dir, mut s) = crate::assembly::tests::project();
        let add = |s: &mut Session, v: Value| call(s, "feature.add", v).unwrap();
        add(
            &mut s,
            json!({"part": "frame", "type": "instance", "id": "i", "of": "plate"}),
        );
        add(
            &mut s,
            json!({"part": "frame", "type": "instance_pattern", "id": "row",
                   "instances": ["i"], "direction": "x", "spacing": "200 mm", "count": 200}),
        );
        let ask = |slot: &mut Option<Session>, budget: usize| {
            RENDER_BUDGET.with(|b| b.set(Some(budget)));
            let req = Request::from_rpc(
                "render",
                Some(json!({"part": "frame", "view": "iso", "width": 64, "height": 48})),
            )
            .unwrap();
            let a = handle_safely(slot, None, None, dir.path(), req);
            RENDER_BUDGET.with(|b| b.set(None));
            a
        };
        let mut slot = Some(Session::open(dir.path()).unwrap());
        let e = ask(&mut slot, 64 * 1024).unwrap_err();
        // The meshes it made count against its budget, and a refused
        // render leaves none of them in the session's mesh cache (#407).
        let held = &slot.as_ref().unwrap().meshes;
        assert_eq!((held.len(), held.bytes()), (0, 0), "{e}");
        assert_eq!(e.code, Code::RenderTooLarge, "{e}");
        assert_eq!(e.details["occurrences"], 200);
        assert!(e.details["drawn"].as_u64().unwrap() < 200, "{e}");
        assert_eq!(e.details["budget"], 64 * 1024);
        assert!(e.message.contains("--hidden"), "{e}");
        let a = ask(&mut slot, 1 << 30).unwrap();
        assert!(a.binary.is_some());
        // One that draws keeps what it meshed (the plate, once).
        assert_eq!(slot.as_ref().unwrap().meshes.len(), 1);
        drop(s);
    }

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

    #[test]
    fn a_render_of_an_annotation_shows_its_view_and_marks() {
        let (_dir, mut s, view) = crate::annotation::tests::drilled();
        let boss = crate::annotation::tests::on(&view, [0.045, 0.02, 0.011]);
        let marks = crate::proto::MarksArg {
            part: "plate".into(),
            short: Vec::new(),
            list: vec![json!({"kind": "arrow", "in": "view", "xy_m": [[0.0, 0.0], boss]})],
        };
        let a = crate::annotation::create(
            &mut s,
            crate::annotation::By::agent("agent"),
            &[],
            "move it",
            Some(view.clone()),
            Some(&marks),
            &Default::default(),
        )
        .unwrap();
        let id = a["annotation"]["id"].as_str().unwrap().to_owned();
        let png = |s: &mut Session, p: Value| {
            let a =
                answer_live(s, None, None, Request::from_rpc("render", Some(p)).unwrap()).unwrap();
            (a.result.clone(), a.binary.unwrap())
        };
        let (r, marked) = png(
            &mut s,
            json!({"annotation": id, "width": 400, "height": 300}),
        );
        assert_eq!(r["camera"], json!(crate::view::Camera::from(&view)));
        // The same view without the annotation: the arrow is the difference.
        let camera = json!({"eye": ["120 mm", "-100 mm", "90 mm"], "target": ["30 mm", "20 mm", "2.5 mm"],
                            "up": [0, 0, 1], "projection": {"orthographic": {"height": "80 mm"}}});
        let (_, bare) = png(
            &mut s,
            json!({"part": "plate", "camera": camera, "width": 400, "height": 300}),
        );
        assert_ne!(marked, bare);
        // Asked with a camera too, it is a usage error.
        let e = answer_live(
            &mut s,
            None,
            None,
            Request::from_rpc("render", Some(json!({"annotation": id, "camera": camera}))).unwrap(),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // An annotation with no view can't be rendered so.
        let plain = crate::annotation::create(
            &mut s,
            crate::annotation::By::agent("agent"),
            &[],
            "words only",
            None,
            None,
            &Default::default(),
        )
        .unwrap();
        let e = answer_live(
            &mut s,
            None,
            None,
            Request::from_rpc(
                "render",
                Some(json!({"annotation": plain["annotation"]["id"]})),
            )
            .unwrap(),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
    }

    #[test]
    fn renders_without_marks_asked_for_are_unchanged() {
        let (_dir, mut s, view) = crate::annotation::tests::drilled();
        let render = |s: &mut Session, annotations: bool| {
            let p = json!({"part": "plate", "view": "iso", "width": 200, "height": 150, "annotations": annotations});
            answer_live(s, None, None, Request::from_rpc("render", Some(p)).unwrap())
                .unwrap()
                .binary
                .unwrap()
        };
        let before = render(&mut s, false);
        let marks = crate::proto::MarksArg {
            part: "plate".into(),
            short: Vec::new(),
            list: vec![json!({"kind": "line", "in": "view", "xy_m": [[0.0, 0.0], [0.01, 0.0]]})],
        };
        crate::annotation::create(
            &mut s,
            crate::annotation::By::agent("agent"),
            &[],
            "a line",
            Some(view),
            Some(&marks),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(render(&mut s, false), before);
        assert_ne!(
            render(&mut s, true),
            before,
            "--annotations draws open marks"
        );
    }

    #[test]
    fn an_annotation_rendered_beside_the_open_ones_is_drawn_once() {
        let (_dir, mut s, view) = crate::annotation::tests::drilled();
        let boss = crate::annotation::tests::on(&view, [0.045, 0.02, 0.011]);
        let pin = |x: f64| {
            vec![crate::proto::PinArg {
                reference: None,
                part: Some("plate".into()),
                point: Some([x, 0.02, 0.005].map(|v| json!(format!("{v} m")))),
            }]
        };
        crate::annotation::create(
            &mut s,
            crate::annotation::By::agent("agent"),
            &pin(0.01),
            "older",
            Some(view.clone()),
            None,
            &Default::default(),
        )
        .unwrap();
        let marks = crate::proto::MarksArg {
            part: "plate".into(),
            short: Vec::new(),
            list: vec![json!({"kind": "arrow", "in": "view", "xy_m": [[0.0, 0.0], boss]})],
        };
        let asked = crate::annotation::create(
            &mut s,
            crate::annotation::By::agent("agent"),
            &pin(0.02),
            "move it",
            Some(view.clone()),
            Some(&marks),
            &Default::default(),
        )
        .unwrap();
        let id = asked["annotation"]["id"].as_str().unwrap().to_owned();
        let a = crate::annotation::one(&s, &id, None).unwrap();
        let scene = |s: &mut Session, all: bool| {
            let mut ask = RenderAsk::pick(
                "plate",
                crate::view::Camera::from(&view),
                Vec::new(),
                Default::default(),
            );
            ask.pick = false;
            ask.annotations = all;
            ask.annotation = Some(a.clone());
            render_scene(s, None, Vec::new(), ask).unwrap().g
        };
        // Alone: its arrow (a line and a head) and its pin by its place.
        let g = scene(&mut s, false);
        assert_eq!(g.overlay.len(), 2);
        let labels: Vec<&str> = g.labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(labels, ["1"]);
        // Beside the open ones: still drawn once, its pin under its number
        // (the second open), the older one's under its own.
        let g = scene(&mut s, true);
        assert_eq!(g.overlay.len(), 2, "its marks once");
        let mut labels: Vec<&str> = g.labels.iter().map(|l| l.text.as_str()).collect();
        labels.sort_unstable();
        assert_eq!(labels, ["1", "2"], "each pin once, one numbering");
    }

    #[test]
    fn a_scene_for_a_fixed_camera_holds_the_tagged_model() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
        )
        .unwrap();
        let camera = crate::view::Camera {
            eye: [0.1, -0.1, 0.1],
            target: [0.005, 0.005, 0.005],
            up: [0.0, 0.0, 1.0],
            projection: crate::view::Projection::Orthographic { height: 0.05 },
        };
        let ask = RenderAsk::pick("p", camera.clone(), Vec::new(), Default::default());
        let scene = render_scene(&mut s, None, Vec::new(), ask).unwrap();
        assert_eq!(scene.camera, camera);
        assert_eq!(scene.g.tags.as_ref().unwrap().triangles.len(), 6);
        assert!(scene.g.labels.is_empty());
    }

    #[test]
    fn a_tagged_scene_names_every_face_edge_and_vertex() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 10, "y": 10, "z": 10}}),
        )
        .unwrap();
        let part = Id::parse("p", "part").unwrap();
        let (_, m) = meshes(&mut s, &part, None, mesh::Quality::Normal)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let m = m.unwrap();
        let mut g = crate::render::Geometry {
            tags: Some(Default::default()),
            ..Default::default()
        };
        draw_body(
            &mut g,
            &m,
            &ryukei::geom::Rigid::IDENTITY,
            "p/b",
            &BTreeMap::new(),
            (None, &BTreeMap::new(), None),
            (false, true),
        );
        let t = g.tags.unwrap();
        assert_eq!(t.triangles.len(), 6, "a run of triangles per face");
        assert_eq!(
            t.triangles.iter().map(|(r, _)| r.len()).sum::<usize>(),
            g.triangles.len()
        );
        assert_eq!(t.lines.len(), 12, "every edge");
        assert_eq!(t.points.len(), 8, "every vertex");
        assert!(
            t.names
                .iter()
                .all(|n| n.starts_with("p/b/") && n.len() == 4 + 32)
        );
    }

    #[test]
    fn sheet_metal_through_the_protocol() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
        )
        .unwrap();
        let mut ops = Vec::new();
        for (i, at) in [[0, 0], [60, 0], [60, 40], [0, 40]].into_iter().enumerate() {
            ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
            ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
        }
        for i in 1..=4 {
            ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                            "start": format!("p{i}"), "end": format!("p{}", i % 4 + 1)}));
        }
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": ops}),
        )
        .unwrap();
        // A plate; a bare K-factor is a number.
        let r = call(&mut s, "feature.add", json!({"part": "p", "type": "base_flange", "id": "plate",
            "sketch": "s", "thickness": "1.5 mm", "radius": "1 mm", "allowance": {"k_factor": 0.44}})).unwrap();
        assert_eq!(r["feature"]["thickness"]["m"], 0.0015);
        assert_eq!(r["feature"]["allowance"]["k_factor"]["value"], "0.44");
        // The right top edge, by its vertices.
        let body =
            call(&mut s, "inspect", json!({"part": "p", "feature": "plate"})).unwrap()["bodies"][0]
                .clone();
        let at = |p: [f64; 3]| -> String {
            body["vertices"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| {
                    (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k] * 1e-3).abs() < 1e-9)
                })
                .map(|v| v["name"].as_str().unwrap().to_owned())
                .unwrap()
        };
        let (a, b) = (at([60.0, 0.0, 1.5]), at([60.0, 40.0, 1.5]));
        let edge = body["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
            .map(|e| format!("p/plate/{}", e["name"].as_str().unwrap()))
            .unwrap();
        // A flange: its target filled in; its length through an extent, as
        // the CLI's --length sends it; its radius its founder's.
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "flange", "id": "f",
            "edges": [edge], "angle": "90 deg", "extent": {"length": "20 mm"}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["target"], "plate", "{r}");
        assert_eq!(r["feature"]["length"]["m"], 0.02, "{r}");
        assert_eq!(r["feature"]["radius"]["m"], 0.001, "{r}");
        assert_eq!(
            r["feature"]["inherited"],
            json!(["radius", "relief"]),
            "{r}"
        );
        assert_eq!(r["feature"]["edges"], json!([edge]), "{r}");
        // An end set back with a relief, by vertex.
        let corner = format!("p/plate/{b}");
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "f",
            "ends": [{"vertex": corner, "setback": "5 mm",
                      "relief": {"kind": "rectangular", "width": "1 mm", "depth": "0.5 mm"}}]}),
        )
        .unwrap();
        assert_eq!(r["feature"]["ends"][0]["vertex"], corner, "{r}");
        let rb = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        assert_eq!(rb["parts"][0]["failed"], 0, "{rb}");
        // Refusals made when written: a face where an edge goes, and an end
        // no edge of it has.
        let face = format!("p/plate/{}", body["faces"][0]["name"].as_str().unwrap());
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "f", "edges": [face]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        let far = format!("p/plate/{}", at([0.0, 0.0, 0.0]));
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "f",
            "ends": [{"vertex": far, "setback": "1 mm"}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e}");
        // What the kernel refuses only rebuild tells: set back with no relief.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "f", "ends": [],
                                            "setback": "5 mm"}),
        )
        .unwrap();
        let rb = call(&mut s, "rebuild", json!({"part": "p"}));
        let e = rb.unwrap_err();
        let f = &e.details["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == "f")
            .unwrap()["error"];
        assert_eq!(
            (f["code"].as_str(), f["details"]["reason"].as_str()),
            (Some("sheet_metal_failed"), Some("relief_needed")),
            "{f}"
        );
        // Deleting the base flange while the flange reads it.
        let e = call(
            &mut s,
            "feature.delete",
            json!({"part": "p", "id": "plate"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::HasDependents, "{e}");
    }

    #[test]
    fn unfolds_and_refolds_through_the_protocol() {
        let (_dir, mut s) = fresh();
        crate::sheet_metal::tests::tray(&mut s, "p");
        let sheet = call(&mut s, "sheet.get", json!({"part": "p"})).unwrap();
        let bend = sheet["bends"][0]["bend"].as_str().unwrap().to_owned();
        let side = sheet["flanges"][0]["sides"][0][0]
            .as_str()
            .unwrap()
            .to_owned();
        // An unfold: its target filled in, its fixed face written `fixed`,
        // its allowance its founder's until it has its own.
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "unfold", "id": "u",
            "fixed_face": side, "bends": [{"bend": bend, "deduction": "2.2 mm"}]}),
        )
        .unwrap();
        assert_eq!(r["feature"]["target"], "plate", "{r}");
        assert_eq!(r["feature"]["fixed"], side, "{r}");
        assert_eq!(r["feature"]["bends"][0]["bend"], bend, "{r}");
        assert_eq!(r["feature"]["bends"][0]["deduction"]["m"], 0.0022, "{r}");
        assert_eq!(
            r["feature"]["allowance"]["k_factor"]["value"], "0.44",
            "{r}"
        );
        assert_eq!(r["feature"]["inherited"], json!(["allowance"]), "{r}");
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "u",
            "allowance": {"k_factor": "0.5"}}),
        )
        .unwrap();
        assert_eq!(r["feature"]["inherited"], json!([]), "{r}");
        let r = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "u", "allowance": ""}),
        )
        .unwrap();
        assert_eq!(r["feature"]["inherited"], json!(["allowance"]), "{r}");
        // The rebuild report gives the flat.
        let rb = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let u = rb["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == "u")
            .unwrap()
            .clone();
        assert_eq!(u["flat"]["bends"], 4, "{u}");
        assert_eq!(u["flat"]["partial"], false, "{u}");
        assert!(u["flat"]["box_m"][1][0].as_f64().unwrap() > 0.06, "{u}");
        // A refold, its unfold named.
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "refold", "id": "rf", "unfold": "u", "only": [bend]}),
        )
        .unwrap();
        assert_eq!(r["feature"]["unfold"], "u", "{r}");
        assert_eq!(r["feature"]["only"], json!([bend]), "{r}");
        let rb = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        assert_eq!(rb["parts"][0]["failed"], 0, "{rb}");
        // Refused when written: a side face where a bend goes; an unfold
        // the refold doesn't fold by; the unfold deleted while the refold
        // reads it.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "u", "only": [side]}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/only/0")),
            "{e}"
        );
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "rf", "unfold": "f"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/unfold")),
            "{e}"
        );
        let e = call(&mut s, "feature.delete", json!({"part": "p", "id": "u"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents, "{e}");
        // A field another type takes is refused.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "rf", "angle": "90 deg"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e}");
    }

    #[test]
    fn a_split_by_a_tool_is_checked_when_written_and_reports_its_sides() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let add = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.add", f)
        };
        add(
            &mut s,
            json!({"type": "box", "id": "h", "params": {"x": 10, "y": 10, "z": 10}}),
        )
        .unwrap();
        add(
            &mut s,
            json!({"type": "box", "id": "t", "params": {"x": 12, "y": 12, "z": 10},
                   "placement": {"origin": [-1, -1, 5]}}),
        )
        .unwrap();
        // Its own target, and a body the part hasn't, are refused.
        let e = add(
            &mut s,
            json!({"type": "split", "id": "x", "target": "h", "tool": "h"}),
        )
        .unwrap_err();
        assert_eq!(e.details["pointer"], "/tool/body", "{e}");
        let e = add(
            &mut s,
            json!({"type": "split", "id": "x", "target": "h", "tool": "zz"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound, "{e}");
        // Faces of two bodies.
        let face = |s: &mut Session, f: &str| {
            let v = call(s, "inspect", json!({"part": "p", "feature": f})).unwrap();
            format!(
                "p/{f}/{}",
                v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
            )
        };
        let (fh, ft) = (face(&mut s, "h"), face(&mut s, "t"));
        let e = add(
            &mut s,
            json!({"type": "split", "id": "x", "target": "h", "tool_faces": [fh, ft]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::TargetsMixed, "{e}");
        add(
            &mut s,
            json!({"type": "split", "id": "sp", "target": "h", "tool": "t"}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let line = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "sp")
            .unwrap()
            .clone();
        assert_eq!(line["ok"], true, "{line}");
        assert_eq!(line["split"]["tool"], json!({"body": "t"}), "{line}");
        assert_eq!(line["split"]["tool_kind"], "solid", "{line}");
        let sides = line["split"]["sides"].as_array().unwrap();
        assert_eq!(sides.len(), 2, "{line}");
        assert_eq!(
            sides[0],
            json!({"side": "behind", "solid": "h", "lumps": 1, "kind": "solid"})
        );
    }

    #[test]
    fn a_mirror_of_a_sheet_is_new_and_an_offset_of_one_a_sheet() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
                {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
                {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}]}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "w", "sketch": "s", "surface": true,
                   "extent": {"type": "blind", "length": "20 mm"}}),
        )
        .unwrap();
        // Left out, the operation is new for a sheet.
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "mirror", "id": "m", "solid": "w", "plane": "zx"}),
        )
        .unwrap();
        let shown = call(&mut s, "feature.show", json!({"part": "p", "id": "m"})).unwrap();
        assert_eq!(shown["operation"], "new", "{shown}");
        assert!(shown.get("targets").is_none(), "{shown}");
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "offset", "id": "o", "target": "w", "distance": "2 mm"}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let solids = r["parts"][0]["solids"].as_array().unwrap();
        for id in ["m", "w"] {
            let b = solids
                .iter()
                .find(|x| x["solid"] == id)
                .unwrap_or_else(|| panic!("{r}"));
            assert_eq!(b["kind"], "sheet", "{b}");
            assert_eq!(b["ok"], true, "{b}");
        }
    }

    #[test]
    fn mirrors_and_splits_through_the_protocol() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let add = |s: &mut Session, f: Value| {
            let mut f = f;
            f["part"] = json!("p");
            call(s, "feature.add", f)
        };
        add(
            &mut s,
            json!({"type": "box", "id": "half", "params": {"x": 10, "y": 4, "z": 2}}),
        )
        .unwrap();
        // Left out, a mirror joins its source.
        let v = add(
            &mut s,
            json!({"type": "mirror", "id": "m", "solid": "half", "plane": "yz"}),
        )
        .unwrap();
        assert_eq!(
            (&v["feature"]["operation"], &v["feature"]["targets"]),
            (&json!("join"), &json!(["half"]))
        );
        let f = call(&mut s, "feature.show", json!({"part": "p", "id": "m"})).unwrap();
        assert_eq!(f["plane"], "yz");
        // Its rebuild report says where the plane is and what the copy did.
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let m = &r["parts"][0]["features"][1];
        assert_eq!(m["ok"], true, "{m}");
        assert_eq!(m["mirror"]["plane"]["normal"], json!([1.0, 0.0, 0.0]));
        // A new copy about zx, as a solid of its own.
        add(
            &mut s,
            json!({"type": "mirror", "id": "c", "solid": "half", "plane": "zx",
                           "operation": "new"}),
        )
        .unwrap();
        let v = call(&mut s, "feature.show", json!({"part": "p", "id": "c"})).unwrap();
        assert_eq!(v["operation"], "new");
        assert!(v.get("targets").is_none());
        // A split by a datum: both sides, each with its lumps.
        add(
            &mut s,
            json!({"type": "plane", "id": "cut", "by": "offset", "from": "yz",
                           "distance": "-2 mm"}),
        )
        .unwrap();
        add(
            &mut s,
            json!({"type": "split", "id": "sp", "target": "half", "plane": "p/cut"}),
        )
        .unwrap();
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let sp = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "sp")
            .unwrap()
            .clone();
        assert_eq!(sp["ok"], true, "{sp}");
        assert_eq!(sp["solids"], json!(["half", "sp"]));
        assert_eq!(
            sp["split"]["sides"],
            json!([{"side": "behind", "solid": "half", "lumps": 1, "kind": "solid"},
                   {"side": "front", "solid": "sp", "lumps": 1, "kind": "solid"}])
        );
        // Refusals at write time: a plane that isn't one, a solid that isn't.
        let e = add(
            &mut s,
            json!({"type": "split", "id": "bad", "target": "half", "plane": "z"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["role"].as_str()),
            (Code::ReferenceWrongKind, Some("plane"))
        );
        let e = add(
            &mut s,
            json!({"type": "mirror", "id": "bad", "solid": "cut", "plane": "yz"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
        // Keeping one side while something reads the side in front.
        add(
            &mut s,
            json!({"type": "box", "id": "on", "operation": "join", "targets": ["sp"],
                           "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "sp", "keep": "behind"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::HasDependents, "{e}");
        // Edited: flipped; and a mirror cut from another solid.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "sp", "flip": true}),
        )
        .unwrap();
        let v = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "c", "operation": "cut",
                                                    "targets": ["half"]}),
        )
        .unwrap();
        assert_eq!(v["feature"]["targets"], json!(["half"]));
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

    /// The box `b` of part `p` with a datum plane `d1` 3 mm above its
    /// top, an axis `a1` along y and a point `q1` at (1, 2, 3) mm, and the
    /// top's reference.
    pub(crate) fn with_datums() -> (tempfile::TempDir, Session, String) {
        let (dir, mut s, body) = crate::sketch::tests::with_box();
        let top = format!(
            "p/b/{}",
            crate::sketch::tests::face_along(&body, [0.0, 0.0, 1.0])
        );
        for f in [
            json!({"type": "plane", "id": "d1", "by": "offset", "from": top, "distance": 3}),
            json!({"type": "axis", "id": "a1", "by": "of", "axis": "y"}),
            json!({"type": "point", "id": "q1", "by": "coordinates", "coordinates": [1, 2, 3]}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        (dir, s, top)
    }

    /// Each face of `feature`'s body, as `p/<feature>/<name>`.
    fn inspect_faces(s: &mut Session, feature: &str) -> Vec<Value> {
        let v = call(s, "inspect", json!({"part": "p", "feature": feature})).unwrap();
        v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| json!(format!("p/{feature}/{}", f["name"].as_str().unwrap())))
            .collect()
    }

    #[test]
    fn sews_and_thickens_of_solids_are_refused_in_a_part_of_solids() {
        let (_dir, mut s) = fresh();
        let must = |s: &mut Session, m: &str, v: Value| call(s, m, v).unwrap();
        must(&mut s, "part.create", json!({"part": "p"}));
        for (id, at) in [("a", 0), ("b", 20)] {
            must(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "box", "id": id,
                "params": {"x": 10, "y": 10, "z": 10}, "placement": {"origin": [at, 0, 0]}}),
            );
        }
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sew", "sheets": ["a", "b"]}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["wanted"].as_str()),
            (Code::WrongBodyKind, Some("sheet")),
            "{e}"
        );
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "thicken", "sheet": "a", "front": "1 mm"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["role"].as_str()),
            (Code::WrongBodyKind, Some("from")),
            "{e}"
        );
        // Nothing was written.
        let v = call(&mut s, "feature.list", json!({"part": "p"})).unwrap();
        assert_eq!(v["features"].as_array().map(Vec::len), Some(2), "{v}");
    }

    #[test]
    fn sews_and_thickens_are_refused_when_written() {
        let (_dir, mut s) = fresh();
        let must = |s: &mut Session, m: &str, v: Value| call(s, m, v).unwrap();
        must(&mut s, "part.create", json!({"part": "p"}));
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b",
            "params": {"x": 10, "y": 10, "z": 10}}),
        );
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "c",
            "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [20, 0, 0]}}),
        );
        let faces = inspect_faces(&mut s, "b");
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extract", "id": "x",
            "faces": [faces[0].clone()]}),
        );
        // A sew of a solid: wrong_body_kind, naming it.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sew",
            "sheets": ["x", "c"]}),
        )
        .unwrap_err();
        assert_eq!(
            (
                e.code,
                e.details["solid"].as_str(),
                e.details["wanted"].as_str()
            ),
            (Code::WrongBodyKind, Some("c"), Some("sheet")),
            "{e}"
        );
        // A thicken of a solid by name: wrong_body_kind too.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "thicken",
            "sheet": "c", "front": "1 mm"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["role"].as_str()),
            (Code::WrongBodyKind, Some("from")),
            "{e}"
        );
        // A thicken whose group names a face it doesn't thicken.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "thicken",
            "from_faces": [faces[0].clone()], "front": "1 mm",
            "face_sides": [{"front": "2 mm", "back": "0", "faces": [faces[1].clone()]}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e}");
        // After a thicken consumes x, nothing acts on it.
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extract", "id": "y",
            "faces": [faces[1].clone()]}),
        );
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "thicken", "id": "t",
            "sheet": "x", "front": "1 mm"}),
        );
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sew",
            "sheets": ["x", "y"]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::BodyConsumed, "{e}");
    }

    #[test]
    fn fills_and_extracts_through_the_protocol() {
        let (_dir, mut s) = fresh();
        let must = |s: &mut Session, m: &str, v: Value| call(s, m, v).unwrap();
        must(&mut s, "part.create", json!({"part": "p"}));
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b",
            "params": {"x": 10, "y": 10, "z": 10}}),
        );
        let b = inspect_faces(&mut s, "b");
        let x = must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extract", "id": "x",
            "faces": [b[0].clone()]}),
        );
        assert_eq!(x["feature"]["type"], "extract", "{x}");
        // A face of another body beside it is targets_mixed.
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "c",
            "params": {"x": 1, "y": 1, "z": 1}, "placement": {"origin": [20, 0, 0]}}),
        );
        let c = inspect_faces(&mut s, "c");
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extract",
            "faces": [b[0].clone(), c[0].clone()]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::TargetsMixed, "{e}");
        // A fill's sides as text; its tolerance and angle written with defaults.
        must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
        );
        must(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [0, 0]},
            {"op": "add", "id": "p2", "type": "point", "at": [10, 0]},
            {"op": "add", "id": "p3", "type": "point", "at": [0, 10]},
            {"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p1"}]}),
        );
        let f = must(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "fill", "id": "f",
            "sides": [{"curve": "p/s/l1"}, {"curve": "p/s/l2"}, {"curve": "p/s/l3"}]}),
        );
        assert_eq!(f["feature"]["tolerance"]["value"], "0.001 mm", "{f}");
        // An edge side that names a face is refused when written.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "fill",
            "sides": [{"edge": b[0].clone()}, {"curve": "p/s/l2"}, {"curve": "p/s/l3"}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ReferenceWrongKind, "{e}");
    }

    #[test]
    fn surface_sweeps_and_planars_are_written_by_feature_add() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s", "plane": "xy"}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
                {"op": "add", "id": "a", "type": "point", "at": [0, 0]},
                {"op": "add", "id": "b", "type": "point", "at": [10, 0]},
                {"op": "add", "id": "l1", "type": "line", "start": "a", "end": "b"}]}),
        )
        .unwrap();
        let v = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "id": "w", "sketch": "s", "surface": true,
                   "extent": {"type": "blind", "length": "5 mm"}}),
        )
        .unwrap();
        assert_eq!(v["feature"]["surface"], true, "{v}");
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "extrude", "sketch": "s", "surface": true,
                   "operation": "join", "targets": ["w"],
                   "extent": {"type": "blind", "length": "5 mm"}}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidParam, Some("/operation")),
            "{e}"
        );
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "planar", "sketch": "s"}),
        )
        .unwrap_err();
        assert_eq!(e.details["reason"], "open_profile", "{e}");
        let v = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "w", "surface": false}),
        );
        // An open line makes no solid: back from a surface, the extrude
        // is refused or fails, never written as a surface.
        if let Ok(v) = v {
            assert!(v["feature"].get("surface").is_none(), "{v}");
        }
    }

    #[test]
    fn reports_and_inspect_say_what_kind_each_body_is() {
        let (_dir, mut s, _) = crate::sketch::tests::with_box();
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let solid = &r["parts"][0]["solids"][0];
        assert_eq!(solid["kind"], "solid", "{r}");
        assert!(solid.get("boundaries").is_none(), "{r}");
        let i = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        let body = &i["bodies"][0];
        assert_eq!(body["kind"], "solid", "{i}");
        let edges = body["edges"].as_array().unwrap();
        assert!(!edges.is_empty(), "{i}");
        assert!(edges.iter().all(|e| e.get("laminar").is_none()), "{i}");
    }

    #[test]
    fn datums_are_cut_along_looked_at_hidden_and_inspected() {
        let (_dir, mut s, _) = with_datums();
        // A section on the datum plane, 8 mm up.
        let arg = crate::section::PlaneArg {
            plane: Some("p/d1".into()),
            ..Default::default()
        };
        let cut = arg.resolve(&mut s, "/planes/0").unwrap();
        assert_eq!((cut.origin_m[2], cut.normal), (0.008, [0.0, 0.0, 1.0]));
        let axis = crate::section::PlaneArg {
            plane: Some("p/a1".into()),
            ..Default::default()
        };
        assert_eq!(
            axis.resolve(&mut s, "/planes/0").unwrap_err().code,
            Code::InvalidParam
        );
        // Looking normal to it looks down; along the axis, along y.
        let (dir_d, _) = normal_to_face(&mut s, "p/d1").unwrap();
        assert_eq!(dir_d.map(f64::abs), [0.0, 0.0, 1.0]);
        let (dir_a, _) = normal_to_face(&mut s, "p/a1").unwrap();
        assert_eq!(dir_a.map(f64::abs), [0.0, 1.0, 0.0]);
        assert!(normal_to_face(&mut s, "p/q1").is_err());
        // Datums are leaves of the hidden set, as solids are, and a
        // highlight resolves one.
        assert_eq!(crate::visibility::reach(&mut s, "p/d1").unwrap(), ["p/d1"]);
        let all = crate::visibility::reach(&mut s, "p").unwrap();
        assert_eq!(all, ["p/b", "p/d1", "p/a1", "p/q1"]);
        let r = crate::entity::EntityRef::parse("p/d1").unwrap();
        assert_eq!(
            r.resolve(&mut s).unwrap(),
            crate::entity::Target::Datum("plane")
        );
        // Inspect gives a datum's geometry, and the model lists them.
        let v = call(&mut s, "inspect", json!({"part": "p", "feature": "a1"})).unwrap();
        assert_eq!(v["datum"]["datum"]["direction"], json!([0.0, 1.0, 0.0]));
        let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        assert_eq!(v["datums"].as_array().unwrap().len(), 3, "{v}");
    }

    #[test]
    fn datums_are_measured_in_closed_form() {
        let (_dir, mut s, top) = with_datums();
        // One datum measures as itself; two, and a datum and a face, in
        // closed form.
        let m = call(&mut s, "measure", json!({"entities": ["p/q1"]})).unwrap();
        assert_eq!(m["kind"], "datum point");
        let m = call(&mut s, "measure", json!({"entities": ["p/q1", "p/d1"]})).unwrap();
        assert_eq!(m["distance"]["m"], 0.005);
        assert_eq!(m["distance"]["exact"], true);
        let m = call(&mut s, "measure", json!({"entities": ["p/a1", top]})).unwrap();
        assert_eq!(m["distance"]["m"], 0.005);
        assert_eq!(m["angle"]["value"], "0 deg", "{m}");
        let e = call(&mut s, "measure", json!({"entities": ["p/d1", "p/b"]})).unwrap_err();
        assert_eq!(e.details["reason"], "datum_against_body");
    }

    /// Part `p` with its box, and a helix `h` about z (radius 8 mm, 10
    /// turns of 2 mm) and a path `w` along it.
    pub(crate) fn with_helix() -> (tempfile::TempDir, Session) {
        let (dir, mut s, _) = crate::sketch::tests::with_box();
        for f in [
            json!({"type": "helix", "id": "h", "axis": "z", "radius": 8, "pitch": 2,
                   "turns": 10}),
            json!({"type": "path", "id": "w", "segments": ["h"]}),
        ] {
            let mut f = f;
            f["part"] = json!("p");
            call(&mut s, "feature.add", f).unwrap();
        }
        (dir, s)
    }

    #[test]
    fn paths_are_hidden_highlighted_inspected_and_measured() {
        let (_dir, mut s) = with_helix();
        // Leaves of the hidden set, as datums are, and highlighted alike.
        assert_eq!(crate::visibility::reach(&mut s, "p/h").unwrap(), ["p/h"]);
        let all = crate::visibility::reach(&mut s, "p").unwrap();
        assert_eq!(all, ["p/b", "p/h", "p/w"]);
        let r = crate::entity::EntityRef::parse("p/w").unwrap();
        assert_eq!(
            r.resolve(&mut s).unwrap(),
            crate::entity::Target::Datum("path")
        );
        // Inspect gives a helix's numbers, and the model lists both.
        let v = call(&mut s, "inspect", json!({"part": "p", "feature": "h"})).unwrap();
        assert_eq!(v["helix"]["helix"]["helix"]["turns"], 10.0, "{v}");
        let v = call(&mut s, "inspect", json!({"part": "p"})).unwrap();
        assert_eq!(v["paths"].as_array().unwrap().len(), 2, "{v}");
        assert!(v.get("datums").is_none());
        // Measured alone, a path is its length, certified.
        let m = call(&mut s, "measure", json!({"entities": ["p/w"]})).unwrap();
        assert_eq!(m["kind"], "path");
        let want = 10.0
            * (0.002f64 * 0.002 + 0.016 * 0.016 * core::f64::consts::PI * core::f64::consts::PI)
                .sqrt();
        let got = m["length"]["m"].as_f64().unwrap();
        let bound = m["length"]["bound_m"].as_f64().unwrap();
        assert!((got - want).abs() < 1e-6 + bound, "{m}");
        assert_eq!(m["segments"], 1);
        let m = call(&mut s, "measure", json!({"entities": ["p/h"]})).unwrap();
        assert_eq!(m["kind"], "helix");
        assert_eq!(m["turns"], 10.0);
        let e = call(&mut s, "measure", json!({"entities": ["p/w", "p/b"]})).unwrap_err();
        assert_eq!(e.details["reason"], "path_against_entity");
    }

    #[test]
    fn renders_draw_annotations_only_when_asked() {
        use crate::render::ANNOTATION;
        let (dir, mut s, face) = crate::annotation::tests::plate();
        crate::annotation::tests::put(
            &mut s,
            &crate::annotation::tests::comment(
                "aaaaaaaaaa",
                vec![crate::annotation::Pin::Entity {
                    reference: face,
                    point_m: Some([0.0, 0.0, 0.005]),
                }],
            ),
        );
        drop(s);
        let count = |annotations: bool| {
            let mut p = json!({"part": "plate", "view": "iso", "width": 200, "height": 150});
            if annotations {
                p["annotations"] = json!(true);
            }
            let px = ANNOTATION.map(|x| (x * 255.0 + 0.5) as u8);
            let s = Session::open(dir.path()).unwrap();
            render_pixels(dir.path(), s, p)
                .into_iter()
                .filter(|p| *p == px)
                .count()
        };
        assert_eq!(count(false), 0, "only when asked");
        assert!(count(true) > 5, "the pin's marker is drawn");
    }

    #[test]
    fn renders_draw_datums_unless_told_not_to() {
        use crate::render::{DATUM, NOTE};
        let (dir, s, _) = with_datums();
        let count = |params: Value, c: crate::render::Rgb| {
            let px = c.map(|x| (x * 255.0 + 0.5) as u8);
            let s = Session::open(dir.path()).unwrap();
            render_pixels(dir.path(), s, params)
                .into_iter()
                .filter(|p| *p == px)
                .count()
        };
        let base = json!({"part": "p", "view": "front", "width": 160, "height": 120});
        assert!(count(base.clone(), DATUM) > 20);
        let mut off = base.clone();
        off["datums"] = json!(false);
        assert_eq!(count(off, DATUM), 0);
        // A highlighted datum takes its highlight's colour.
        let mut lit = base;
        lit["highlight"] = json!(["p/a1"]);
        assert!(count(lit, NOTE) > 5);
        drop(s);
    }

    #[test]
    fn renders_draw_paths_and_helices_unless_told_not_to() {
        use crate::render::{NOTE, PATH};
        let (dir, s) = with_helix();
        let count = |params: Value, c: crate::render::Rgb| {
            let px = c.map(|x| (x * 255.0 + 0.5) as u8);
            let s = Session::open(dir.path()).unwrap();
            render_pixels(dir.path(), s, params)
                .into_iter()
                .filter(|p| *p == px)
                .count()
        };
        let base = json!({"part": "p", "view": "front", "width": 160, "height": 120});
        assert!(count(base.clone(), PATH) > 50);
        let mut off = base.clone();
        off["datums"] = json!(false);
        assert_eq!(count(off, PATH), 0);
        let mut hidden = base.clone();
        hidden["hidden"] = json!(["p/h", "p/w"]);
        assert_eq!(count(hidden, PATH), 0);
        let mut lit = base;
        lit["highlight"] = json!(["p/w"]);
        assert!(count(lit, NOTE) > 50);
        drop(s);
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

    /// A part deleted is gone from the next answer though the watch keeps
    /// no event, as when FSEvents hands the marker's event over before the
    /// folder's (issue #263).
    #[test]
    fn a_deleted_part_is_gone_without_file_events() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path(), None).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let marks = tempfile::tempdir().unwrap();
        let watch = ryu_session::fresh::Watch::start(&root, &marks.path().join("m"), |_| false);
        let mut s = Session::open(&root).unwrap().watched(watch);
        let listed = |s: &mut Session| -> Vec<Value> {
            let list = call(s, "part.list", json!({})).unwrap();
            list["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["part"].clone())
                .collect()
        };
        assert!(listed(&mut s).is_empty());
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        assert_eq!(listed(&mut s), [json!("p")]);
        call(&mut s, "part.delete", json!({"part": "p"})).unwrap();
        assert!(listed(&mut s).is_empty());
        let w = s.watch_status().unwrap();
        assert_eq!(w["watching"], true, "{w}");
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
    fn a_primitives_radius_may_come_as_the_radius_field() {
        let (_dir, mut s) = with_part();
        // `ryucad feature add p cylinder --radius 2 --param height=3`.
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "cylinder", "radius": "2", "params": {"height": 3}}),
        )
        .unwrap();
        assert_eq!(
            r["feature"]["params"]["radius"],
            json!({"value": "2 mm", "m": 0.002})
        );
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "cylinder-1", "radius": "4 mm"}),
        )
        .unwrap();
        let show = call(
            &mut s,
            "feature.show",
            json!({"part": "p", "id": "cylinder-1"}),
        )
        .unwrap();
        assert_eq!(show["params"]["radius"]["value"], "4 mm");
        // Both at once is one too many.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "radius": 1, "params": {"radius": 1}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["param"], "radius");
        // A box has no radius, and the refusal says where its sizes go.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "radius": 1, "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert_eq!(e.details["param"], "radius");
        assert!(
            e.message.contains("(x, y, z)") && e.message.contains("--param NAME=QUANTITY"),
            "{}",
            e.message
        );
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
        // Rebuild reports give a pattern's form, for the window's icons.
        fn line<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
            match v {
                Value::Object(o) if o.get("id") == Some(&json!(id)) && o.contains_key("ok") => {
                    Some(v)
                }
                Value::Object(o) => o.values().find_map(|x| line(x, id)),
                Value::Array(a) => a.iter().find_map(|x| line(x, id)),
                _ => None,
            }
        }
        // (The mirror cuts nothing, so the rebuild fails: its report is in
        // the refusal.)
        let r = match call(&mut s, "rebuild", json!({"part": "p"})) {
            Ok(r) => r,
            Err(e) => e.details.clone(),
        };
        assert_eq!(line(&r, "pat").unwrap()["form"], "mirror", "{r}");
        assert!(line(&r, "h").unwrap().get("form").is_none());
        // What can be judged now is refused before anything is written.
        let vertex = format!("p/b/{}", vertex_at(&body, [0.0, 0.0, 0.0]));
        for (f, code) in [
            (json!({"features": ["b"]}), Code::ReferenceWrongKind),
            (json!({"plane": vertex}), Code::ReferenceWrongKind),
            (json!({"features": ["nope"]}), Code::NotFound),
            (json!({"radius": 1}), Code::InvalidParam),
            // A bare word that isn't an origin axis is a feature of the
            // part: here, none.
            (
                json!({"form": "linear", "direction": "w", "spacing": 1, "count": 2}),
                Code::NotFound,
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
            // Not an origin plane: a feature of the part, which it lacks.
            (
                "feature.add",
                json!({"type": "sketch", "plane": "xz"}),
                Code::NotFound,
            ),
            (
                "feature.add",
                json!({"type": "sketch", "plane": "origin"}),
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

    /// A rebuild's report is written as text (#533): the bytes a `Value`
    /// of it gives, passing and failing, an assembly's occurrences in
    /// their sorted place.
    #[test]
    fn a_rebuilds_text_is_its_values() {
        let (_dir, mut s) = with_part();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "params": {"x": 1, "y": 2, "z": 3}, "id": "b"}),
        )
        .unwrap();
        call(
            &mut s,
            "part.create",
            json!({"part": "frame", "kind": "assembly"}),
        )
        .unwrap();
        for id in ["i1", "i2"] {
            call(
                &mut s,
                "feature.add",
                json!({"part": "frame", "type": "instance", "of": "p", "id": id}),
            )
            .unwrap();
        }
        let raw = |s: &mut Session, params: Value| {
            let req = Request::from_rpc("rebuild", Some(params)).unwrap();
            answer_live(s, None, None, req).unwrap().raw.expect("text")
        };
        let Raw::Result(text) = raw(&mut s, json!({})) else {
            panic!("the project rebuilds")
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(text, v.to_string());
        assert_eq!(
            v["parts"][0]["occurrences"].as_array().unwrap().len(),
            2,
            "{v}"
        );
        assert_eq!(v, call(&mut s, "rebuild", json!({})).unwrap());
        call(&mut s, "feature.add", json!({"part": "p", "type": "torus", "params": {"major_radius": 1, "minor_radius": 1}, "id": "bad"})).unwrap();
        for params in [json!({}), json!({"part": "p"})] {
            let Raw::Error { code, details, .. } = raw(&mut s, params.clone()) else {
                panic!("a feature fails")
            };
            assert_eq!(code, Code::RebuildFailed);
            let v: Value = serde_json::from_str(&details).unwrap();
            assert_eq!(details, v.to_string());
            assert_eq!(v, call(&mut s, "rebuild", params).unwrap_err().details);
        }
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
    #[test]
    fn annotations_are_announced_with_their_derived_states() {
        let (_dir, mut s, face) = crate::annotation::tests::plate();
        let mut live = Live::new();
        let sub = live.hub.subscribe("p-x", &[Topic::Annotations]);
        // The first reading is what the subscription starts from.
        crate::server::publish(&mut live, &mut s);
        let id = call_live(
            &mut s,
            &mut live,
            None,
            "annotation.create",
            json!({"pins": [{"ref": face}], "comment": "here"}),
        )
        .unwrap()["annotation"]["id"]
            .clone();
        let notes = drained(&sub);
        let n = notes
            .iter()
            .find(|v| v["method"] == "annotation.changed")
            .expect("told");
        assert_eq!(n["params"]["annotations"][0]["id"], id);
        assert_eq!(n["params"]["annotations"][0]["outdated"], false);
        // The model moving the pin away is news too.
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.delete",
            json!({"part": "plate", "id": "slab"}),
        )
        .unwrap();
        let notes = drained(&sub);
        let n = notes
            .iter()
            .find(|v| v["method"] == "annotation.changed")
            .expect("told");
        assert_eq!(n["params"]["annotations"][0]["outdated"], true);
        call_live(
            &mut s,
            &mut live,
            None,
            "annotation.delete",
            json!({"id": id}),
        )
        .unwrap();
        let notes = drained(&sub);
        let n = notes
            .iter()
            .find(|v| v["method"] == "annotation.changed")
            .expect("told");
        assert_eq!(n["params"]["removed"], json!([id]));
    }

    #[test]
    fn a_participant_sets_its_own_mode() {
        let (_dir, mut s) = fresh();
        let mut live = Live::new();
        let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
        assert_eq!(live.get(&h).unwrap().view.mode, live::Mode::View);
        let v = call_live(
            &mut s,
            &mut live,
            Some(&h),
            "participant.mode",
            json!({"mode": "annotate"}),
        )
        .unwrap();
        assert_eq!(v, json!({"participant": h, "mode": "annotate"}));
        assert_eq!(live.get(&h).unwrap().to_json()["mode"], "annotate");
        // No request sets another's: an agent sets only its own.
        call_live(
            &mut s,
            &mut live,
            None,
            "participant.mode",
            json!({"mode": "edit"}),
        )
        .unwrap();
        assert_eq!(live.get(&h).unwrap().view.mode, live::Mode::Annotate);
        let e = call(&mut s, "participant.mode", json!({"mode": "edit"})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
        let e = call_live(
            &mut s,
            &mut live,
            Some(&h),
            "participant.mode",
            json!({"mode": "write"}),
        )
        .unwrap_err();
        // An unknown mode is refused as any unknown variant is.
        assert_eq!(e.code, Code::Usage);
    }

    #[test]
    fn a_humans_writes_are_refused_outside_their_mode_and_an_agents_never() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let mut live = Live::new();
        let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
        let add =
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}});
        let note = json!({"pins": [], "comment": "here"});
        let e = call_live(&mut s, &mut live, Some(&h), "feature.add", add.clone()).unwrap_err();
        assert_eq!(e.code, Code::ParticipantMode);
        assert_eq!(e.details["mode"], "view");
        assert_eq!(e.details["needs"], json!(["edit"]));
        let e = call_live(
            &mut s,
            &mut live,
            Some(&h),
            "annotation.create",
            note.clone(),
        )
        .unwrap_err();
        assert_eq!(e.details["needs"], json!(["annotate", "edit"]));
        // A mark is an annotation write like any other.
        let marked = json!({"pins": [], "comment": "here",
                            "camera": {"eye": [1, -1, 1], "target": [0, 0, 0], "up": [0, 0, 1],
                                       "projection": {"perspective": {"fov_deg": 40}}},
                            "marks": {"part": "p", "list": [{"kind": "line", "in": "view",
                                                             "xy_m": [[0, 0], [0.01, 0]]}]}});
        let e = call_live(&mut s, &mut live, Some(&h), "annotation.create", marked).unwrap_err();
        assert_eq!(e.code, Code::ParticipantMode);
        // Reading is never refused.
        call_live(&mut s, &mut live, Some(&h), "part.list", json!({})).unwrap();
        call_live(
            &mut s,
            &mut live,
            Some(&h),
            "participant.mode",
            json!({"mode": "annotate"}),
        )
        .unwrap();
        call_live(
            &mut s,
            &mut live,
            Some(&h),
            "annotation.create",
            note.clone(),
        )
        .unwrap();
        assert_eq!(
            call_live(&mut s, &mut live, Some(&h), "feature.add", add.clone())
                .unwrap_err()
                .code,
            Code::ParticipantMode
        );
        call_live(
            &mut s,
            &mut live,
            Some(&h),
            "participant.mode",
            json!({"mode": "edit"}),
        )
        .unwrap();
        call_live(&mut s, &mut live, Some(&h), "feature.add", add.clone()).unwrap();
        call_live(
            &mut s,
            &mut live,
            Some(&h),
            "annotation.create",
            note.clone(),
        )
        .unwrap();
        // An agent writes in any mode, and with no hello.
        let a = live.hello(live::Kind::Agent, Some("bot"), None).unwrap();
        call_live(
            &mut s,
            &mut live,
            Some(&a),
            "participant.mode",
            json!({"mode": "view"}),
        )
        .unwrap();
        call_live(
            &mut s,
            &mut live,
            Some(&a),
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 2}}),
        )
        .unwrap();
        call_live(
            &mut s,
            &mut live,
            None,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 3}}),
        )
        .unwrap();
        // Undo is a write.
        call_live(
            &mut s,
            &mut live,
            Some(&h),
            "participant.mode",
            json!({"mode": "annotate"}),
        )
        .unwrap();
        assert_eq!(
            call_live(&mut s, &mut live, Some(&h), "undo", json!({}))
                .unwrap_err()
                .code,
            Code::ParticipantMode
        );
    }

    #[test]
    fn a_view_mode_humans_dependency_fetch_is_refused_before_it_fetches() {
        use ryu_session::Serve;
        let (_dir, s) = fresh();
        let mut slot = Some(s);
        let mut live = Live::new();
        let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
        let a = live.hello(live::Kind::Agent, Some("bot"), None).unwrap();
        let add =
            Request::from_rpc("deps.add", Some(json!({"url": "file:///nowhere/lib.git"}))).unwrap();
        let job = Job::default();
        // An agent's fetch is prepared without the lock; the human's isn't
        // prepared at all, since `handle` refuses it.
        assert!(crate::Cad::prepare(&mut slot, &live, Some(&a), &add, &job).is_some());
        assert!(crate::Cad::prepare(&mut slot, &live, Some(&h), &add, &job).is_none());
        let update = Request::from_rpc("deps.update", Some(json!({}))).unwrap();
        assert!(crate::Cad::prepare(&mut slot, &live, Some(&h), &update, &job).is_none());
        call_live(
            slot.as_mut().unwrap(),
            &mut live,
            Some(&h),
            "participant.mode",
            json!({"mode": "edit"}),
        )
        .unwrap();
        assert!(crate::Cad::prepare(&mut slot, &live, Some(&h), &add, &job).is_some());
    }

    #[test]
    fn an_export_never_writes_over_a_project_file_whoever_asks() {
        // A human in View mode may export (the window does), so `out` may
        // never be one of the project's files: export is never logged.
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 1, "y": 1, "z": 1}}),
        )
        .unwrap();
        for d in [
            "releases/p",
            "imports",
            "annotations",
            ".git",
            "out",
            "Parts/p",
            "Imports",
        ] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        // Windows can't make a folder whose name ends in a space or a dot,
        // so `parts. /p` is made and judged only elsewhere.
        if cfg!(not(windows)) {
            std::fs::create_dir_all(dir.path().join("parts. /p")).unwrap();
        }
        let mut live = Live::new();
        let h = live.hello(live::Kind::Human, Some("Dana"), None).unwrap();
        let a = live.hello(live::Kind::Agent, Some("bot"), None).unwrap();
        let export = |out: &std::path::Path| json!({"part": "p", "format": "stl", "out": out.to_str().unwrap()});
        for rel in [
            "ryucad.json",
            ".gitattributes",
            "parts/p/part.json",
            "parts/p/features/b.json",
            "parts/p/new.json",
            "releases/p/A.json",
            "imports/x.step",
            "annotations/x.json",
            ".git/config",
            // Windows and macOS fold case, and Windows drops trailing dots
            // and spaces: refused on every platform.
            "RYUCAD.JSON",
            "Parts/p/part.json",
            "Imports/x.step",
            "ryucad.json.",
            "ryucad.json ",
            "parts. /p/part.json",
        ]
        .into_iter()
        .filter(|rel| cfg!(not(windows)) || !rel.starts_with("parts. "))
        {
            let out = dir.path().join(rel);
            let before = std::fs::read(&out).ok();
            for who in [Some(h.as_str()), Some(a.as_str()), None] {
                let e = call_live(&mut s, &mut live, who, "export", export(&out)).expect_err(rel);
                assert_eq!(e.code, Code::InvalidParam, "{rel}: {e:?}");
                assert_eq!(e.details["param"], "out", "{rel}");
            }
            assert_eq!(std::fs::read(&out).ok(), before, "{rel} unchanged");
        }
        // Elsewhere in the project folder, or outside it, View may export.
        let beside = tempfile::tempdir().unwrap();
        for out in [
            dir.path().join("p.stl"),
            dir.path().join("out/p.stl"),
            beside.path().join("p.stl"),
        ] {
            call_live(&mut s, &mut live, Some(&h), "export", export(&out)).unwrap();
            assert!(out.exists(), "{}", out.display());
        }
    }

    /// Every queued notification of `sub`, as JSON.
    fn drained(sub: &crate::notify::Subscriber) -> Vec<Value> {
        sub.drain()
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
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
                    "camera": null, "highlights": [], "compare": null, "focus": null, "hidden": [],
                    "sections": [], "explode": null, "mode": "view"}])
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
        // A person's window writes the model in Direct edit.
        call_live(
            &mut s,
            &mut live,
            human.as_str(),
            "participant.mode",
            json!({"mode": "edit"}),
        )
        .unwrap();
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
        // A person's window writes the model in Direct edit.
        call_live(
            &mut s,
            &mut live,
            h,
            "participant.mode",
            json!({"mode": "edit"}),
        )
        .unwrap();
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
    fn subscribers_hear_of_project_parameters_nothing_reads() {
        let (dir, s) = with_part();
        let mut live = Live::new();
        let mut slot = Some(s);
        settle(&mut slot, &mut live, dir.path());
        let sub = live.hub.subscribe("p-x", &[Topic::Model, Topic::Project]);
        let mark = |live: &mut Live| live.hub.send(Topic::Model, "marker", &json!({}));
        let mut s = slot.take().unwrap();
        call_live(
            &mut s,
            &mut live,
            None,
            "param.set",
            json!({"scope": "project", "parameters": {"spare": "2 mm"}}),
        )
        .unwrap();
        mark(&mut live);
        let got = drain(&sub);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "project.changed");
        assert_eq!(got[0].1["parameters"], json!({"spare": "2 mm"}));
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
