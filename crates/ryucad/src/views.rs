//! Named views' protocol: `view.list`, `view.save`, `view.delete` and
//! `view.rename`, each write one logged, undoable change guarded by the
//! part's revision; and finding a named view for `camera.set` and
//! `render`. See [`crate::doc::views`] for the file.

use crate::doc::id::Id;
use crate::doc::project::{part_dir, remove_file, write_json};
use crate::doc::views::{VIEWS, check_id};
use crate::error::{Code, Error, Result};
use crate::server::{check_revision, part_id};
use crate::session::Session;
use crate::view::Camera;
use serde_json::{Value, json};
use std::path::PathBuf;

fn path(session: &Session, part: &Id, view: &Id) -> PathBuf {
    part_dir(session.root(), part)
        .join(VIEWS)
        .join(format!("{view}.json"))
}

/// `view_not_found`, naming the part and the views it has.
fn not_found(session: &Session, part: &Id, view: &str) -> Error {
    let known: Vec<&Id> = session
        .project()
        .part(part)
        .map(|p| p.views.keys().collect())
        .unwrap_or_default();
    Error::new(
        Code::ViewNotFound,
        format!("{part} has no named view {view:?}; `ryucad view list {part}` lists them"),
    )
    .with("part", part)
    .with("view", view)
    .with("views", known)
}

/// The named view `view` of `part`, as a camera; one that doesn't read is
/// its file's error.
pub fn camera(session: &Session, part: &Id, view: &str) -> Result<Camera> {
    let p = session.project().part(part)?;
    let found = Id::parse(view, "view")
        .ok()
        .and_then(|id| p.views.get(&id))
        .ok_or_else(|| not_found(session, part, view))?;
    match found {
        Ok(v) => Ok(Camera::from(v)),
        Err(e) => Err(e.clone()),
    }
}

/// The part whose named views a request means: the one it names, else
/// `focus` (a participant's), else the project's model when that is one
/// part; `None` when it shows several.
pub fn part_for(session: &Session, part: Option<&str>, focus: Option<&Id>) -> Result<Option<Id>> {
    if let Some(p) = part {
        let id = part_id(p)?;
        session.project().part(&id)?;
        return Ok(Some(id));
    }
    if let Some(f) = focus {
        return Ok(Some(f.clone()));
    }
    Ok(
        match crate::assembly::shown_parts(session.project()).as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        },
    )
}

/// A camera from a view's name: a standard view's direction is framed
/// by the caller; a named view is `part`'s. `view_not_found` when no part
/// is known to look in.
pub fn named(session: &Session, part: Option<&Id>, view: &str) -> Result<Camera> {
    match part {
        Some(p) => camera(session, p, view),
        None => Err(Error::new(
            Code::ViewNotFound,
            format!(
                "{view:?} is no standard view, and the project shows several parts: name the part whose view it is"
            ),
        )
        .with("view", view)),
    }
}

/// One view as `view.list` gives it.
fn entry(part: &Id, id: &Id, view: &Result<crate::doc::views::ViewFile>) -> Value {
    match view {
        Ok(v) => json!({"part": part, "view": id, "camera": Camera::from(v)}),
        Err(e) => json!({"part": part, "view": id, "error": e}),
    }
}

/// The named views of `part`, or of every part, with each listed part's
/// revision.
pub fn list(session: &Session, part: Option<&str>) -> Result<Value> {
    let parts: Vec<Id> = match part {
        Some(p) => {
            let id = part_id(p)?;
            session.project().part(&id)?;
            vec![id]
        }
        None => session.project().parts.keys().cloned().collect(),
    };
    let mut views = Vec::new();
    let mut revisions = serde_json::Map::new();
    for id in &parts {
        let p = session.project().part(id)?;
        if part.is_some() || !p.views.is_empty() {
            revisions.insert(id.to_string(), json!(p.revision));
        }
        views.extend(p.views.iter().map(|(v, f)| entry(id, v, f)));
    }
    Ok(json!({"views": views, "revisions": revisions}))
}

/// Saves `camera` as `part`'s named view `view`, replacing one of that
/// ID.
pub fn save(
    session: &mut Session,
    part: &str,
    view: &str,
    camera: &Camera,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let vid = check_id(view)?;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let replaced = p.views.contains_key(&vid);
    camera.check()?;
    write_json(&path(session, &pid, &vid), &camera.to_view_file().to_json())?;
    session.reload()?;
    let p = session.project().part(&pid)?;
    let saved = p.views.get(&vid).expect("just written");
    Ok(json!({
        "part": pid,
        "view": vid,
        "replaced": replaced,
        "camera": saved.as_ref().map(Camera::from).map_err(Clone::clone)?,
        "revision": p.revision,
    }))
}

/// Deletes `part`'s named view `view`; a file that doesn't read can be
/// deleted too.
pub fn delete(
    session: &mut Session,
    part: &str,
    view: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let vid = Id::parse(view, "view")
        .ok()
        .filter(|v| p.views.contains_key(v))
        .ok_or_else(|| not_found(session, &pid, view))?;
    remove_file(session.root(), &path(session, &pid, &vid))?;
    session.reload()?;
    let revision = session.project().part(&pid)?.revision.clone();
    Ok(json!({"part": pid, "view": vid, "revision": revision}))
}

/// Renames `part`'s named view `from` to `to`, which must be free.
pub fn rename(
    session: &mut Session,
    part: &str,
    from: &str,
    to: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let to_id = check_id(to)?;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let from_id = Id::parse(from, "view")
        .ok()
        .filter(|v| p.views.contains_key(v))
        .ok_or_else(|| not_found(session, &pid, from))?;
    if p.views.contains_key(&to_id) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("{pid} already has a view {to}; delete it or choose another name"),
        )
        .with("part", &pid)
        .with("view", to));
    }
    let (old, new) = (path(session, &pid, &from_id), path(session, &pid, &to_id));
    let bytes = std::fs::read(&old).map_err(|e| Error::io(&old, &e))?;
    crate::doc::project::write_atomic(&new, bytes)?;
    remove_file(session.root(), &old)?;
    session.reload()?;
    let revision = session.project().part(&pid)?.revision.clone();
    Ok(json!({"part": pid, "from": from_id, "view": to_id, "revision": revision}))
}

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    fn camera() -> serde_json::Value {
        json!({"eye": ["100 mm", "-100 mm", "100 mm"], "target": [0, 0, 0], "up": [0, 0, 1],
               "projection": {"perspective": {"fov_deg": 30}}})
    }

    #[test]
    fn views_are_saved_listed_renamed_and_deleted() {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let rev =
            call(&mut s, "view.list", json!({"part": "p"})).unwrap()["revisions"]["p"].clone();
        let v = call(
            &mut s,
            "view.save",
            json!({"part": "p", "view": "detail", "camera": camera(),
                   "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(v["replaced"], false);
        assert_eq!(v["camera"]["eye"], json!([0.1, -0.1, 0.1]));
        assert_ne!(v["revision"], rev);
        let file = dir.path().join("parts/p/views/detail.json");
        assert!(file.is_file());
        let l = call(&mut s, "view.list", json!({})).unwrap();
        assert_eq!(l["views"][0]["view"], "detail");
        assert_eq!(l["views"][0]["camera"], v["camera"]);
        assert_eq!(l["revisions"]["p"], v["revision"]);
        // Saving again replaces it; a stale revision is refused.
        let again = call(
            &mut s,
            "view.save",
            json!({"part": "p", "view": "detail", "camera": camera()}),
        )
        .unwrap();
        assert_eq!(again["replaced"], true);
        let e = call(
            &mut s,
            "view.save",
            json!({"part": "p", "view": "x", "camera": camera(), "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let e = call(
            &mut s,
            "view.save",
            json!({"part": "p", "view": "front", "camera": camera()}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidId);
        call(
            &mut s,
            "view.save",
            json!({"part": "p", "view": "side", "camera": camera()}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "view.rename",
            json!({"part": "p", "from": "detail", "to": "side"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
        let r = call(
            &mut s,
            "view.rename",
            json!({"part": "p", "from": "detail", "to": "close"}),
        )
        .unwrap();
        assert_eq!(
            (r["from"].clone(), r["view"].clone()),
            (json!("detail"), json!("close"))
        );
        assert!(!file.exists());
        let e = call(
            &mut s,
            "view.delete",
            json!({"part": "p", "view": "detail"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.details["views"].clone()),
            (Code::ViewNotFound, json!(["close", "side"]))
        );
        call(&mut s, "view.delete", json!({"part": "p", "view": "close"})).unwrap();
        call(&mut s, "view.delete", json!({"part": "p", "view": "side"})).unwrap();
        assert!(!dir.path().join("parts/p/views").exists());
        assert_eq!(
            call(&mut s, "view.list", json!({})).unwrap()["views"],
            json!([])
        );
    }

    #[test]
    fn a_view_saves_a_participants_camera() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        // Without a live session there is no one's camera to save.
        let e = call(&mut s, "view.save", json!({"part": "p", "view": "mine"})).unwrap_err();
        assert_eq!(e.code, Code::NoLiveSession);
    }
}
