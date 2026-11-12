//! The request handlers. The daemon and the in-process CLI both call
//! [`handle`], so they give the same answers.

use crate::activity::{self, EntryKind};
use crate::config;
use crate::diff::files_at;
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
use crate::scene::flat_meshed;
use crate::session::Session;
use crate::solid::Solids;
use crate::units::{Quantity, Unit};
use crate::view::{Camera, DEFAULT_PROJECTION, StandardView};
use ryu_session::job::{Job, Phase};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

// Old paths, kept while ryucad 27 runs beside other stages (design 541,
// "Paths kept"); 27.F removes the ones nothing uses.
#[allow(unused_imports)]
pub(crate) use crate::budget::{mesh_budget, render_budget};
#[allow(unused_imports)]
pub(crate) use crate::diff::diff;
#[allow(unused_imports)]
pub(crate) use crate::geometry::{geom_json, normal_to_face, normal_to_face_at};
#[allow(unused_imports)]
pub use crate::part::connector_text;
#[allow(unused_imports)]
pub(crate) use crate::part::{
    check_revision, feature, feature_id, insert_at, no_feature, order, part_id, part_kind,
    part_report, proposed, reference_texts, sweeps_of,
};
#[allow(unused_imports)]
pub(crate) use crate::save::save;
#[allow(unused_imports)]
pub(crate) use crate::scene::{
    DrawExplode, RenderAsk, Scene, draw_body, ghost, render, render_flat, render_for_package,
    render_noting, render_scene, sketches,
};
#[allow(unused_imports)]
pub use crate::scene::{meshes, model_points, occurrence_meshes};

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
const FEATURE_TYPES: [&str; 26] = [
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
    let mut solids: Vec<(String, &Id, &crate::assembly::Solid)> = Vec::new();
    for o in &chosen {
        for s in &o.solids {
            if seen.insert(s.built.hash.clone()) {
                let path = crate::assembly::path_text(&o.path);
                solids.push((format!("{path}/{}", s.founder), &o.part, s));
            }
        }
    }
    let refs: Vec<&crate::assembly::Solid> = solids.iter().map(|(_, _, s)| *s).collect();
    let made = occurrence_meshes(session, &refs, quality)?;
    let bodies = solids
        .into_iter()
        .zip(made)
        .map(|((label, part, s), mesh)| {
            // A reference's holes are warned of, as a part's are (#676).
            let warning = mesh::unmeshed_warning(part, &s.founder, &s.built, &mesh);
            let m = mesh::Meshed {
                body: s.built.body.clone(),
                hash: s.built.hash.clone(),
                mesh,
                warning,
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

/// The warnings bodies' meshes carry (a reference's unmeshed faces), in
/// body order.
fn mesh_warnings<K>(bodies: &[(K, Result<mesh::Meshed>)]) -> Vec<Value> {
    bodies
        .iter()
        .filter_map(|(_, m)| m.as_ref().ok()?.warning.clone())
        .collect()
}

/// Puts `warnings` into `result` when there are any.
fn with_warnings(result: &mut Value, warnings: Vec<Value>) {
    if !warnings.is_empty() {
        result["warnings"] = json!(warnings);
    }
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
        let mut result =
            json!({"part": pid, "quality": quality.as_str(), "occurrences": occurrences});
        with_warnings(&mut result, mesh_warnings(&bodies));
        let (encoding, b) = bytes(&bodies, compact);
        return Ok(Answer::bytes(result, encoding, b));
    }
    let (pid, only, quality) = mesh_args(part, one_of(only)?, quality)?;
    let bodies = meshes(session, &pid, only.as_ref(), quality)?;
    let mut result = json!({"part": pid, "quality": quality.as_str()});
    with_warnings(&mut result, mesh_warnings(&bodies));
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
        let warnings = mesh_warnings(&bodies);
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
        let mut v = json!({"part": pid, "quality": quality.as_str(), "bodies": bodies,
                           "occurrences": occurrences});
        with_warnings(&mut v, warnings);
        return Ok(v);
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
    with_warnings(&mut v, warnings);
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
    let mut v = json!({"part": pid, "bodies": bodies});
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
pub(crate) mod tests;
