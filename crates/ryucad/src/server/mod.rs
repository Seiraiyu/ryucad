//! The request handlers. The daemon and the in-process CLI both call
//! [`handle`], so they give the same answers.

use crate::activity::{self, EntryKind};
use crate::config;
use crate::doc::files::Manifest;
use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, write_json};
use crate::error::{Code, Error, Result};
use crate::live::{self, Live};
use crate::notify::Topic;
use crate::proto::{Answer, Request, Transfer};
use crate::session::Session;
use crate::units::Unit;
use crate::view::StandardView;
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

mod annotation;
mod assembly;
mod cache;
mod feature;
mod git;
mod part;
mod pdm;
mod route;
mod session;
mod view;

use part::rebuild;
pub(crate) use route::dispatch;
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
