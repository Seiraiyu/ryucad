//! The request handlers. The daemon and the in-process CLI both call
//! [`handle`], so they give the same answers.

use crate::doc::files::{Feature, FeatureType, Manifest, PartFile, Placement, number};
use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, Part, feature_path, part_dir, part_path, write_json};
use crate::error::{Code, Error, Result};
use crate::proto::{PlacementArg, Position, Request, VersionInfo};
use crate::rebuild;
use crate::session::Session;
use crate::units::{Quantity, Unit};
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
/// with the files.
pub fn handle(session: &mut Session, req: Request) -> Result<Value> {
    session.refresh()?;
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
        other => Err(Error::new(
            Code::Usage,
            format!("{} is not implemented yet", other.method()),
        )),
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
    fn daemon_requests_need_a_daemon() {
        let (_dir, mut s) = fresh();
        let e = call(&mut s, "daemon.status", json!({})).unwrap_err();
        assert_eq!(e.code, Code::Usage);
    }
}
