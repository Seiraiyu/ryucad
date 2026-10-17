//! The request handlers. The daemon and the in-process CLI both call
//! [`handle`], so they give the same answers.

use crate::doc::files::{Feature, FeatureType, Manifest, PartFile, Placement, number};
use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, Part, feature_path, part_dir, part_path, write_json};
use crate::entity::{self, EntityRef};
use crate::error::{Code, Error, Result};
use crate::live::{self, Live, SelectMode};
use crate::proto::{PlacementArg, Position, Request, VersionInfo};
use crate::rebuild;
use crate::session::Session;
use crate::units::{Quantity, Unit};
use crate::view::Camera;
use serde_json::{Map, Value, json};
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
    let m = Manifest {
        default_unit: default_unit.unwrap_or(Unit::Mm),
    };
    write_json(&manifest, &m.to_json())?;
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
    mut live: Option<&mut Live>,
    me: Option<&str>,
    req: Request,
) -> Result<Value> {
    let changed = session.refresh()?;
    let before = live.as_deref().map(Live::snapshot);
    if let (Some(l), Some(me)) = (live.as_deref_mut(), me) {
        l.touch(me);
    }
    let mutates = req.mutates();
    let result = dispatch(session, live.as_deref_mut(), me, req);
    if let (Some(l), Some(before)) = (live, before) {
        // Entities that no longer exist drop out of every selection.
        if changed || mutates {
            l.prune(|r| r.resolve(session).is_ok());
        }
        l.announce(&before);
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
        Request::PartCreate { part } => part_create(session, &part),
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
            id,
            position,
            expected_revision,
        } => feature_add(
            session,
            &part,
            &ty,
            params,
            placement.unwrap_or_default(),
            id.as_deref(),
            &position,
            expected_revision.as_deref(),
        ),
        Request::FeatureEdit {
            part,
            id,
            params,
            placement,
            expected_revision,
        } => feature_edit(
            session,
            &part,
            &id,
            params,
            placement.unwrap_or_default(),
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
        Request::FeatureShow { part, id } => {
            let (part, fid) = (part_id(&part)?, feature_id(&id)?);
            let f = feature(session.project().part(&part)?, &fid)?;
            Ok(feature_report(&fid, f))
        }
        Request::DaemonStatus {} | Request::DaemonShutdown {} => Err(Error::new(
            Code::Usage,
            "only a running daemon answers daemon requests",
        )),
        Request::Rebuild { part } => rebuild(session, part.as_deref()),
        Request::Inspect {
            part,
            feature,
            entity,
        } => inspect(session, &part, feature.as_deref(), entity.as_deref()),
        Request::CheckFormat {} => check_format(session),
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
                Some(p) => json!({"participant": p.id, "camera": p.camera}),
                None => json!({"participant": null, "camera": null}),
            })
        }
        Request::CameraSet { camera } => {
            let live = live.ok_or_else(|| live::no_live_session("camera.set"))?;
            let camera: Camera = camera.resolve(session.project().manifest.default_unit)?;
            let id = live.caller(me);
            live.get_mut(&id)?.camera = Some(camera.clone());
            Ok(json!({"participant": id, "camera": camera}))
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
) -> Result<Value> {
    guarded(slot, root, |s| handle_live(s, live, me, req))
}

fn guarded(
    slot: &mut Option<Session>,
    root: &Path,
    f: impl FnOnce(&mut Session) -> Result<Value>,
) -> Result<Value> {
    let session = match slot {
        Some(s) => s,
        None => slot.insert(Session::open(root)?),
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

fn project_info(session: &Session) -> Value {
    let p = session.project();
    json!({
        "root": p.root,
        "default_unit": p.manifest.default_unit,
        "parts": p.parts.keys().collect::<Vec<_>>(),
        "version": VersionInfo::current(),
    })
}

fn part_id(s: &str) -> Result<Id> {
    Id::parse(s, "part")
}

/// Refuses to write when the agent's view of the part is out of date.
fn check_revision(part: &Part, expected: Option<&str>) -> Result<()> {
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

fn part_create(session: &mut Session, part: &str) -> Result<Value> {
    let id = part_id(part)?;
    if session.project().parts.contains_key(&id) {
        return Err(
            Error::new(Code::DuplicateId, format!("part {id} already exists")).with("part", &id),
        );
    }
    write_json(
        &part_path(session.root(), &id),
        &PartFile { features: vec![] }.to_json(),
    )?;
    session.reload()?;
    let revision = &session.project().part(&id)?.revision;
    Ok(json!({"part": id, "revision": revision}))
}

fn part_delete(session: &mut Session, part: &str, expected: Option<&str>) -> Result<Value> {
    let id = part_id(part)?;
    check_revision(session.project().part(&id)?, expected)?;
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
            let mut v = json!({"part": p.id, "features": p.order().len(), "revision": p.revision});
            if let Err(e) = &p.file {
                v["error"] = json!(e);
            }
            v
        })
        .collect();
    json!({ "parts": parts })
}

fn feature_id(s: &str) -> Result<Id> {
    Id::parse(s, "feature")
}

/// The part's feature list, or why it can't be used.
fn order(part: &Part) -> Result<Vec<Id>> {
    part.file
        .as_ref()
        .map(|f| f.features.clone())
        .map_err(Clone::clone)
}

/// A feature of `part`, or the error that keeps it from loading.
fn feature<'a>(part: &'a Part, id: &Id) -> Result<&'a Feature> {
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
/// meters.
pub fn feature_report(id: &Id, f: &Feature) -> Value {
    let params: Map<String, Value> = f
        .params
        .iter()
        .map(|(k, q)| (k.clone(), q.report()))
        .collect();
    json!({
        "id": id,
        "type": f.ty.as_str(),
        "params": params,
        "placement": {
            "origin": f.placement.origin.iter().map(Quantity::report).collect::<Vec<_>>(),
            "z": f.placement.z.map(number),
            "x": f.placement.x.map(number),
        },
    })
}

/// Applies the parts of `arg` that are given to `p`.
fn apply_placement(p: &mut Placement, arg: PlacementArg, unit: Unit) -> Result<()> {
    if let Some(origin) = arg.origin {
        for (i, v) in origin.iter().enumerate() {
            p.origin[i] = Quantity::from_json(v, unit)
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
fn save(
    session: &mut Session,
    part: &Id,
    writes: &[(&Id, &Feature)],
    order: Vec<Id>,
) -> Result<String> {
    for (id, f) in writes {
        write_json(&feature_path(session.root(), part, id), &f.to_json())?;
    }
    write_json(
        &part_path(session.root(), part),
        &PartFile { features: order }.to_json(),
    )?;
    session.reload()?;
    Ok(session.project().part(part)?.revision.clone())
}

#[allow(clippy::too_many_arguments)]
fn feature_add(
    session: &mut Session,
    part: &str,
    ty: &str,
    params: Map<String, Value>,
    placement: PlacementArg,
    id: Option<&str>,
    position: &Position,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut list = order(p)?;
    let ty = FeatureType::parse(ty)?;
    let fid = match id {
        Some(id) => feature_id(id)?,
        None => Id::generate(ty.as_str(), |s| p.features.keys().any(|f| f.as_str() == s)),
    };
    if p.features.contains_key(&fid) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("part {pid} already has a feature {fid}"),
        )
        .with("feature", &fid));
    }
    let params = Feature::params_from_json(ty, &Value::Object(params), unit)?;
    let mut placed = Placement::identity(unit);
    // With z given and x left out, x defaults to world X, or to world Y
    // when z lies along X; either way the file records it.
    let along_x = placement.x.is_none()
        && placement.z.is_some_and(|z| {
            let mut p = Placement::identity(unit);
            p.z = z;
            rebuild::placement(&p).is_err_and(|e| e.details["kernel"] == "Degenerate")
        });
    if along_x {
        placed.x = [0.0, 1.0, 0.0];
    }
    apply_placement(&mut placed, placement, unit)?;
    let f = Feature {
        ty,
        params,
        placement: placed,
    };
    list.insert(insert_at(&list, position)?, fid.clone());
    let revision = save(session, &pid, &[(&fid, &f)], list)?;
    Ok(json!({"feature": feature_report(&fid, &f), "revision": revision}))
}

fn feature_edit(
    session: &mut Session,
    part: &str,
    id: &str,
    params: Map<String, Value>,
    placement: PlacementArg,
    expected: Option<&str>,
) -> Result<Value> {
    let (pid, fid) = (part_id(part)?, feature_id(id)?);
    let unit = session.project().manifest.default_unit;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let list = order(p)?;
    let mut f = feature(p, &fid)?.clone();
    let mut merged: Map<String, Value> = f
        .params
        .iter()
        .map(|(k, q)| (k.clone(), Value::String(q.text())))
        .collect();
    for (k, v) in params {
        if v.is_null() {
            merged.remove(&k);
        } else {
            merged.insert(k, v);
        }
    }
    f.params = Feature::params_from_json(f.ty, &Value::Object(merged), unit)?;
    apply_placement(&mut f.placement, placement, unit)?;
    let revision = save(session, &pid, &[(&fid, &f)], list)?;
    Ok(json!({"feature": feature_report(&fid, &f), "revision": revision}))
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
    let mut list = order(p)?;
    list.retain(|f| *f != fid);
    // The list first: a crash then leaves an unlisted file, never a listed
    // feature with no file.
    let revision = save(session, &pid, &[], list)?;
    let path = feature_path(session.root(), &pid, &fid);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(&path, &e)),
    }
    session.reload()?;
    Ok(json!({"deleted": fid, "revision": revision}))
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
    list.insert(insert_at(&list, position)?, fid);
    let revision = save(session, &pid, &[], list.clone())?;
    Ok(json!({"features": list, "revision": revision}))
}

fn feature_list(session: &Session, part: &str) -> Result<Value> {
    let p = session.project().part(&part_id(part)?)?;
    let features: Vec<Value> = order(p)?
        .iter()
        .map(|id| match feature(p, id) {
            Ok(f) => json!({"id": id, "type": f.ty.as_str(), "ok": true}),
            Err(e) => json!({"id": id, "ok": false, "error": e}),
        })
        .collect();
    Ok(json!({"part": p.id, "features": features, "orphans": p.orphans(), "revision": p.revision}))
}

fn rebuild(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let ids: Vec<Id> = match part {
        Some(p) => vec![part_id(p)?],
        None => session.project().parts.keys().cloned().collect(),
    };
    let mut parts = Vec::new();
    let mut failed = 0;
    for id in ids {
        let revision = session.project().part(&id)?.revision.clone();
        // A part whose list can't be read can't be rebuilt.
        order(session.project().part(&id)?)?;
        let r = session.rebuilt(&id)?;
        failed += r.failed();
        let features: Vec<Value> = r
            .features
            .iter()
            .map(|(fid, b)| match b {
                Ok(b) => {
                    let c = b.body.counts();
                    json!({"id": fid, "ok": true, "hash": b.hash,
                           "counts": {"faces": c.faces, "edges": c.edges, "vertices": c.vertices}})
                }
                Err(e) => json!({"id": fid, "ok": false, "error": e}),
            })
            .collect();
        parts.push(json!({"part": id, "revision": revision, "hash": r.hash,
                          "failed": r.failed(), "features": features}));
    }
    let report = json!({ "parts": parts });
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

fn inspect(
    session: &mut Session,
    part: &str,
    only: Option<&str>,
    entity: Option<&str>,
) -> Result<Value> {
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
    let result = session.rebuilt(&pid)?;
    let mut bodies = Vec::new();
    for (id, built) in &result.features {
        if only.as_ref().is_some_and(|o| o != id) {
            continue;
        }
        let Ok(built) = built else { continue };
        let f = &features
            .iter()
            .find(|(i, _)| i == id)
            .expect("built features loaded")
            .1;
        let mut v = crate::inspect::body(f, built);
        v["feature"] = json!(id);
        bodies.push(v);
    }
    if let Some(name) = entity {
        // Just the one entity, wherever it is.
        for b in &bodies {
            for kind in ["faces", "edges", "vertices"] {
                if let Some(e) = b[kind]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["name"] == name)
                {
                    return Ok(
                        json!({"feature": b["feature"], "kind": &kind[..kind.len() - 1], "entity": e}),
                    );
                }
            }
        }
        return Err(
            Error::new(Code::NotFound, format!("no entity {name} in part {pid}"))
                .with("entity", name),
        );
    }
    Ok(json!({"part": pid, "bodies": bodies}))
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
    for part in p.parts.values() {
        check(&part_path(root, &part.id));
        if let Err(e) = &part.file {
            errors.push(json!(e));
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
            json!([{"part": "bracket", "features": 0, "revision": rev}])
        );
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
        let r = call(&mut s, "check_format", json!({})).unwrap();
        assert_eq!(r["clean"], false);
        assert!(
            r["non_canonical"]
                .as_array()
                .unwrap()
                .contains(&json!("parts/p/features/a.json"))
        );
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
        let r = handle_safely(&mut slot, None, None, dir.path(), Request::PartList {}).unwrap();
        assert_eq!(r["parts"], json!([]));
        assert!(slot.is_some());
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let e = guarded(&mut slot, dir.path(), |_| panic!("boom")).unwrap_err();
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
                    "camera": null, "highlights": []}])
        );
        let list = call_live(&mut s, &mut live, Some(&id), "participant.list", json!({})).unwrap();
        assert_eq!(list, r["session"]);
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
}
