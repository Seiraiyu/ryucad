//! The material library's protocol: `materials.list`, `materials.set`
//! and `materials.delete`, each write one logged, undoable change guarded
//! by the project's revision. See [`crate::doc::materials`] for the file.

use crate::config;
use crate::doc::id::Id;
use crate::doc::materials::{self, MATERIALS, Material, Materials, check_colour, check_name};
use crate::doc::project::{remove_file, write_json};
use crate::error::{Code, Error, Result};
use crate::session::Session;
use crate::units::Quantity;
use serde_json::{Value, json};

/// The library as the session reads it now: an empty one when there is
/// no file.
pub fn library(session: &Session) -> Result<Materials> {
    Ok(materials::read(&session.files())?.unwrap_or_default())
}

/// A part's PDM `material` property, if set to a string.
pub fn material_of(session: &Session, part: &Id) -> Option<String> {
    ryupdm::props::read(&session.files(), part)
        .ok()?
        .values
        .get("material")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The parts whose `material` names `name`, in ID order.
fn users(session: &Session, name: &str) -> Vec<Id> {
    session
        .project()
        .parts
        .keys()
        .filter(|p| material_of(session, p).as_deref() == Some(name))
        .cloned()
        .collect()
}

/// A density as output gives it: as written, and in kg/m³.
pub fn density_json(q: &Quantity) -> Value {
    json!({"value": q.text(), "kg_m3": q.si()})
}

/// Every material, with the parts that name it, the default and the
/// project's revision.
pub fn list(session: &Session) -> Result<Value> {
    let lib = library(session)?;
    let materials: Vec<Value> = lib
        .materials
        .iter()
        .map(|(name, m)| {
            let mut v = json!({
                "name": name,
                "density": density_json(&m.density),
                "default": lib.default.as_deref() == Some(name.as_str()),
                "parts": users(session, name),
            });
            if let Some(c) = &m.colour {
                v["colour"] = json!(c);
            }
            v
        })
        .collect();
    Ok(json!({
        "materials": materials,
        "default": lib.default,
        "project_revision": config::project_revision(session.root())?,
    }))
}

fn save(session: &mut Session, lib: &Materials) -> Result<Value> {
    let path = session.root().join(MATERIALS);
    if lib.materials.is_empty() {
        remove_file(session.root(), &path)?;
    } else {
        write_json(&path, &lib.to_json())?;
    }
    session.reload()?;
    Ok(json!(config::project_revision(session.root())?))
}

/// Adds or replaces material `name`; `default` makes it the library's
/// default. A colour left out keeps the one it had.
pub fn set(
    session: &mut Session,
    name: &str,
    density: &str,
    colour: Option<&str>,
    default: bool,
    expected: Option<&str>,
) -> Result<Value> {
    config::check_project_revision(session.root(), expected)?;
    check_name(name)?;
    let density = Quantity::density(density)?;
    let colour = colour.map(check_colour).transpose()?;
    let mut lib = library(session)?;
    let colour = colour.or_else(|| lib.materials.get(name).and_then(|m| m.colour.clone()));
    let created = !lib.materials.contains_key(name);
    lib.materials
        .insert(name.to_owned(), Material { density, colour });
    if default {
        lib.default = Some(name.to_owned());
    }
    let rev = save(session, &lib)?;
    Ok(json!({"material": name, "created": created, "project_revision": rev}))
}

/// Removes material `name`, refused with `has_dependents` while a part
/// names it; the default goes with it, and the file with its last
/// material.
pub fn delete(session: &mut Session, name: &str, expected: Option<&str>) -> Result<Value> {
    config::check_project_revision(session.root(), expected)?;
    let mut lib = library(session)?;
    if !lib.materials.contains_key(name) {
        return Err(
            Error::new(Code::NotFound, format!("there is no material {name:?}"))
                .with("material", name),
        );
    }
    let parts = users(session, name);
    if !parts.is_empty() {
        let list: Vec<&str> = parts.iter().map(Id::as_str).collect();
        return Err(Error::new(
            Code::HasDependents,
            format!(
                "{name} is the material of {}; change theirs first",
                list.join(", ")
            ),
        )
        .with("material", name)
        .with("parts", &parts));
    }
    lib.materials.remove(name);
    if lib.default.as_deref() == Some(name) {
        lib.default = None;
    }
    let rev = save(session, &lib)?;
    Ok(json!({"material": name, "project_revision": rev}))
}

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::server::tests::{call, fresh};
    use serde_json::json;

    #[test]
    fn materials_are_set_listed_and_deleted() {
        let (dir, mut s) = fresh();
        assert_eq!(
            call(&mut s, "materials.list", json!({})).unwrap()["materials"],
            json!([])
        );
        let v = call(
            &mut s,
            "materials.set",
            json!({"name": "steel", "density": "7850 kg/m^3", "default": true}),
        )
        .unwrap();
        assert_eq!(v["created"], true);
        call(
            &mut s,
            "materials.set",
            json!({"name": "brass", "density": "8.5 g/cm^3", "colour": "#B5A642",
                   "expected_revision": v["project_revision"]}),
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("materials.json")).unwrap();
        assert!(text.contains("\"#b5a642\""), "{text}");
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"material": "brass"}}),
        )
        .unwrap();
        let l = call(&mut s, "materials.list", json!({})).unwrap();
        assert_eq!(l["default"], "steel");
        assert_eq!(
            l["materials"],
            json!([
                {"name": "brass", "colour": "#b5a642", "default": false, "parts": ["p"],
                 "density": {"value": "8.5 g/cm^3", "kg_m3": 8500.0}},
                {"name": "steel", "default": true, "parts": [],
                 "density": {"value": "7850 kg/m^3", "kg_m3": 7850.0}},
            ])
        );
        let e = call(&mut s, "materials.delete", json!({"name": "brass"})).unwrap_err();
        assert_eq!(
            (e.code, e.details["parts"].clone()),
            (Code::HasDependents, json!(["p"]))
        );
        let e = call(&mut s, "materials.delete", json!({"name": "gold"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = call(
            &mut s,
            "materials.set",
            json!({"name": "x", "density": "1 kg/m^3", "expected_revision": "0"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let e = call(
            &mut s,
            "materials.set",
            json!({"name": "x", "density": "1 kg"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidQuantity);
        call(&mut s, "materials.delete", json!({"name": "steel"})).unwrap();
        assert_eq!(
            call(&mut s, "materials.list", json!({})).unwrap()["default"],
            json!(null)
        );
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"material": null}}),
        )
        .unwrap();
        call(&mut s, "materials.delete", json!({"name": "brass"})).unwrap();
        assert!(!dir.path().join("materials.json").exists());
    }
}
