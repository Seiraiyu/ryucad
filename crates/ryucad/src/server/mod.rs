//! The request handlers. The daemon and the in-process CLI both call
//! [`handle`], so they give the same answers.

use crate::activity::{self, EntryKind};
use crate::config;
use crate::doc::files::Manifest;
use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, write_json};
use crate::entity::{self, EntityRef};
use crate::error::{Code, Error, Result};
use crate::live::{self, CadLive, Live, SelectMode};
use crate::mesh;
use crate::notify::Topic;
use crate::param;
use crate::proto::{Answer, Request, Transfer};
use crate::session::Session;
use crate::units::Unit;
use crate::view::{Camera, StandardView};
use ryu_session::job::Job;
use serde_json::{Value, json};
use std::collections::BTreeMap;
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

mod assembly;
mod cache;
mod feature;
mod part;
mod session;
mod view;

use assembly::solve_at;
use cache::{cache_login, cache_stats, cache_warm};
use feature::{Given, feature_add, feature_delete, feature_edit, feature_list, feature_move};
use part::rebuild;
use part::{check_format, part_create, part_delete, part_list, project_info};
use session::participant_name;
use view::{Look, framed, inspect, mesh_flat_json, mesh_get, pick_from};
use view::{flat_alone, mesh_binary, mesh_flat_binary, mesh_pending, wanted};
// Old paths (design 541, "Paths kept"); 27.F removes the ones nothing uses.
#[allow(unused_imports)]
pub use crate::scene::{meshes, model_points, occurrence_meshes};
#[allow(unused_imports)]
pub use feature::feature_report;

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

#[cfg(test)]
pub(crate) mod tests;
