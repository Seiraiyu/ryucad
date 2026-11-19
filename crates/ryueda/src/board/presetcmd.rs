//! `preset list | show | create | edit | delete` (design 924): the built-in
//! fab presets and a project's own, as documents in `presets/<id>.json`.

use super::preset::{self, KIND, PRESETS, Preset};
use crate::coord;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::project::Project;
use crate::server::check_revision;
use crate::session::Session;
use crate::source::join;
use crate::write::{Writes, commit};
use serde_json::{Value, json};
use std::borrow::Cow;

fn path_of(id: &Id) -> String {
    join(PRESETS, &format!("{id}.json"))
}

fn invalid(msg: impl Into<String>, param: &str) -> Error {
    Error::new(Code::InvalidParam, msg.into()).with("param", param)
}

fn not_found(p: &Project, name: &str) -> Error {
    Error::new(
        Code::BoardPresetNotFound,
        format!(
            "there is no preset {name}; presets are {}",
            preset::names_in(p)
        ),
    )
    .with("preset", name)
}

/// The boards naming a preset.
fn users(p: &Project, name: &str) -> Vec<String> {
    p.boards
        .iter()
        .filter(|(_, b)| b.board.item.as_ref().is_ok_and(|x| x.preset == name))
        .map(|(id, _)| format!("board/{id}"))
        .collect()
}

/// `preset.list`: the built-ins, then the project's, each with whether it
/// is built in and the boards using it.
pub fn list(p: &Project) -> Value {
    let mut out: Vec<Value> = preset::ALL
        .iter()
        .map(|x| json!({"name": x.name, "built_in": true, "boards": users(p, &x.name)}))
        .collect();
    for (id, l) in &p.presets {
        let faults: Vec<Value> = l.faults.iter().map(|f| json!(f.error)).collect();
        out.push(json!({"name": id, "built_in": false, "path": path_of(id), "boards": users(p, id.as_str()), "faults": faults}));
    }
    json!({ "presets": out })
}

/// `preset.show`: a preset's document.
pub fn show(p: &Project, name: &str) -> Result<Value> {
    if let Some(b) = preset::by_name(name) {
        return Ok(
            json!({"name": name, "built_in": true, "preset": b.document(), "boards": users(p, name)}),
        );
    }
    let id = Id::parse(name, "preset").map_err(|_| not_found(p, name))?;
    let l = p.presets.get(&id).ok_or_else(|| not_found(p, name))?;
    let doc = l.item.as_ref().map_err(Clone::clone)?.document();
    Ok(
        json!({"name": name, "built_in": false, "path": path_of(&id), "preset": doc, "boards": users(p, name)}),
    )
}

/// `preset.create`: a project preset copied from another.
pub fn create(
    session: &mut Session,
    name: &str,
    from: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    let id = Id::parse(name, "preset")?;
    if preset::by_name(name).is_some() || p.presets.contains_key(&id) {
        return Err(invalid(format!("there is already a preset {name}"), "name"));
    }
    let base = preset::resolve(p, from).ok_or_else(|| not_found(p, from))?;
    let mut new = Preset {
        name: Cow::Owned(name.to_owned()),
        ..(*base).clone()
    };
    // A copy of a built-in carries its stackup, so the copy reads as it did.
    new.stackup = Some(base.stackup());
    let mut w = Writes::default();
    w.put(path_of(&id), &new, KIND);
    commit(session, &format!("preset {name}"), w)?;
    Ok(
        json!({"name": name, "path": path_of(&id), "preset": new.document(), "revision": session.project().revision}),
    )
}

/// `preset.edit`: `key=value` pairs set on a project preset's document;
/// `*_nm` values are lengths (`coord`), `blind_vias` a boolean, the rest
/// JSON. The edited document must read.
pub fn edit(
    session: &mut Session,
    name: &str,
    set: &[String],
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    if preset::by_name(name).is_some() {
        return Err(invalid(
            format!(
                "{name} is built in; `preset create NAME --from {name}` makes an editable copy"
            ),
            "name",
        ));
    }
    let id = Id::parse(name, "preset").map_err(|_| not_found(p, name))?;
    let l = p.presets.get(&id).ok_or_else(|| not_found(p, name))?;
    let mut doc = l.item.as_ref().map_err(Clone::clone)?.document();
    let unit = p.manifest.default_unit;
    if set.is_empty() {
        return Err(invalid("give key=value pairs to set", "set"));
    }
    for kv in set {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| invalid(format!("{kv} isn't key=value"), "set"))?;
        if matches!(k, "kind" | "format" | "name") {
            return Err(invalid(format!("{k} can't be set"), "set"));
        }
        let value = if k.ends_with("_nm") {
            json!(coord::nm(&json!(v), unit)?)
        } else if k == "blind_vias" {
            json!(
                v.parse::<bool>()
                    .map_err(|_| invalid(format!("{k} is true or false"), "set"))?
            )
        } else {
            serde_json::from_str(v)
                .map_err(|e| invalid(format!("{k}'s value isn't JSON: {e}"), "set"))?
        };
        doc[k] = value;
    }
    let edited = Preset::from_document(&doc, &path_of(&id)).map_err(|e| {
        invalid(
            format!("the edited preset doesn't read: {}", e.message),
            "set",
        )
    })?;
    let mut w = Writes::default();
    w.put(path_of(&id), &edited, KIND);
    commit(session, &format!("preset {name}"), w)?;
    Ok(
        json!({"name": name, "path": path_of(&id), "preset": edited.document(), "revision": session.project().revision}),
    )
}

/// `preset.delete`: a project preset no board names.
pub fn delete(session: &mut Session, name: &str, expected: Option<&str>) -> Result<Value> {
    let p = session.project();
    check_revision("the project", &p.revision, expected)?;
    if preset::by_name(name).is_some() {
        return Err(invalid(format!("{name} is built in"), "name"));
    }
    let id = Id::parse(name, "preset").map_err(|_| not_found(p, name))?;
    if !p.presets.contains_key(&id) {
        return Err(not_found(p, name));
    }
    let used = users(p, name);
    if !used.is_empty() {
        return Err(Error::new(
            Code::PresetInUse,
            format!("preset {name} is used by {}", used.join(", ")),
        )
        .with("boards", used));
    }
    let mut w = Writes::default();
    w.delete(path_of(&id));
    commit(session, &format!("preset {name} deleted"), w)?;
    Ok(json!({"name": name, "deleted": true, "revision": session.project().revision}))
}
