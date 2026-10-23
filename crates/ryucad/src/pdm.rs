//! ryupdm through the protocol: the project's PDM settings and its parts'
//! properties. Writes are checked against the whole project first: one
//! that would give any part a fault it doesn't have now (an unknown or
//! ill-fitting property, a number off the pattern or already taken) is
//! refused with that fault, unless a settings change is forced.
//!
//! The first PDM write gives the project its ID, in the same logged
//! change.

use crate::activity::Files;
use crate::config::check_project_revision;
use crate::doc::canonical;
use crate::doc::files::Manifest;
use crate::doc::id::{Id, ProjectId};
use crate::doc::project::{MANIFEST, PDM_CONFIG, properties_path, remove_file, write_json};
use crate::error::{Code, Error, Result};
use crate::server::{check_revision, part_id};
use crate::session::Session;
use ryupdm::config::{CORE, Config, Decl, Pattern, Scheme, branch_name, property_name};
use ryupdm::props::{self, Pdm, Properties};
use serde_json::{Map, Value, json};
use std::path::Path;

/// The folder release records live in, beside the manifest.
pub const RELEASES: &str = "releases";

/// Whether any release record exists: from then on, the revision scheme
/// can't change.
pub(crate) fn released(root: &Path) -> bool {
    std::fs::read_dir(root.join(RELEASES))
        .map(|mut it| it.any(|e| e.is_ok_and(|e| has_files(&e.path()))))
        .unwrap_or(false)
}

/// Whether the folder `dir` holds a file, at any depth.
fn has_files(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut it| {
        it.any(|e| {
            e.is_ok_and(|e| {
                let p = e.path();
                p.is_file() || (p.is_dir() && has_files(&p))
            })
        })
    })
}

/// Refuses a part ID that a release record names: its stable names, and
/// its releases, are another part's.
pub fn check_not_retired(root: &Path, part: &Id) -> Result<()> {
    if has_files(&root.join(RELEASES).join(part.as_str())) {
        return Err(Error::new(
            Code::PartIdRetired,
            format!(
                "part ID {part} was released before ({RELEASES}/{part}/ names it), so it can't be used again"
            ),
        )
        .with("part", part));
    }
    Ok(())
}

/// The project's ID, written into the manifest first if it has none.
fn ensure_id(session: &Session) -> Result<ProjectId> {
    let manifest = &session.project().manifest;
    if let Some(id) = &manifest.project {
        return Ok(id.clone());
    }
    let id = ryupdm::new_project_id()?;
    let m = Manifest {
        project: Some(id.clone()),
        ..manifest.clone()
    };
    write_json(&session.root().join(MANIFEST), &m.to_json())?;
    Ok(id)
}

/// A fault's identity, to tell faults a write adds from those it finds.
fn key(e: &Error) -> (Code, Value, Value) {
    (
        e.code,
        e.details.get("part").cloned().unwrap_or_default(),
        e.details.get("property").cloned().unwrap_or_default(),
    )
}

/// The faults the project's properties would have with `files` that they
/// don't have with the session's files.
fn added_faults(session: &Session, files: &Files) -> Vec<Error> {
    let before: Vec<_> = Pdm::read(&session.snapshot(), session.project())
        .faults()
        .iter()
        .map(key)
        .collect();
    Pdm::read(files, session.project())
        .faults()
        .into_iter()
        .filter(|e| !before.contains(&key(e)))
        .collect()
}

/// `pdm.config`: the settings with their defaults filled in, whether the
/// project has `ryupdm.json` yet, its ID, and the revision writes guard.
pub fn config(session: &Session) -> Result<Value> {
    let files = session.snapshot();
    let c = Config::read(&files)?;
    let core: Map<String, Value> = CORE
        .iter()
        .map(|n| {
            (
                (*n).to_owned(),
                ryupdm::config::core(n).expect("core").to_json(),
            )
        })
        .collect();
    let custom: Map<String, Value> = c
        .properties
        .iter()
        .map(|(n, d)| (n.clone(), d.to_json()))
        .collect();
    Ok(json!({
        "configured": files.contains_key(PDM_CONFIG),
        "project": session.project().manifest.project,
        "revisions": c.revisions.as_str(),
        "release_branch": c.release_branch,
        "pattern": c.pattern.as_ref().map(Pattern::text),
        "core": core,
        "properties": custom,
        "project_revision": crate::config::project_revision(session.root())?,
    }))
}

/// Writes the settings (and the project's ID, if it has none) and reloads.
fn save_config(session: &mut Session, c: &Config) -> Result<()> {
    ensure_id(session)?;
    write_json(&session.root().join(PDM_CONFIG), &c.to_json())?;
    session.reload()
}

/// `pdm.init`: gives the project its ID and the default settings, keeping
/// any it already has.
pub fn init(session: &mut Session) -> Result<Value> {
    let files = session.snapshot();
    if session.project().manifest.project.is_none() || !files.contains_key(PDM_CONFIG) {
        let c = Config::read(&files)?;
        save_config(session, &c)?;
    }
    config(session)
}

/// What `pdm.configure` changes; what it leaves out stays.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub revisions: Option<String>,
    pub release_branch: Option<String>,
    /// A part number pattern, or `""` for none.
    pub pattern: Option<String>,
    /// Declarations by name; `null` removes one.
    pub properties: Map<String, Value>,
    /// Write even if parts' properties stop fitting.
    pub force: bool,
}

/// `pdm.configure`: changes the settings, guarded by the project revision.
pub fn configure(session: &mut Session, change: Change, expected: Option<&str>) -> Result<Value> {
    check_project_revision(session.root(), expected)?;
    if change.revisions.is_none()
        && change.release_branch.is_none()
        && change.pattern.is_none()
        && change.properties.is_empty()
    {
        return Err(Error::new(
            Code::Usage,
            "give at least one change: revisions, release_branch, pattern or a property",
        ));
    }
    let mut files = session.snapshot();
    let mut c = Config::read(&files)?;
    if let Some(r) = &change.revisions {
        let scheme = Scheme::parse(r).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!("revisions {r:?}: use letters, numeric or numeric2"),
            )
            .with("param", "revisions")
        })?;
        if scheme != c.revisions && released(session.root()) {
            return Err(Error::new(
                Code::RevisionSchemeLocked,
                format!(
                    "parts have been released with {} labels, so the scheme can't change",
                    c.revisions.as_str()
                ),
            )
            .with("revisions", c.revisions.as_str()));
        }
        c.revisions = scheme;
    }
    if let Some(b) = &change.release_branch {
        if !branch_name(b) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("release branch {b:?} isn't a branch name Git accepts"),
            )
            .with("param", "release_branch"));
        }
        c.release_branch = b.clone();
    }
    if let Some(p) = &change.pattern {
        c.pattern = match p.as_str() {
            "" => None,
            p => Some(Pattern::new(p)?),
        };
    }
    for (name, v) in &change.properties {
        let at = format!("/properties/{name}");
        if v.is_null() {
            if c.properties.remove(name).is_none() {
                return Err(Error::new(
                    Code::NotFound,
                    format!("no custom property {name} is declared"),
                )
                .with("property", name));
            }
            continue;
        }
        property_name(name, &at)?;
        c.properties.insert(name.clone(), Decl::from_json(v, &at)?);
    }
    files.insert(
        PDM_CONFIG.to_owned(),
        canonical::to_string(&c.to_json()).into_bytes(),
    );
    let added = added_faults(session, &files);
    if let (false, Some(first)) = (change.force, added.first()) {
        let mut e = first.clone();
        e.message = format!(
            "{}; change those parts first, or pass force to write it anyway",
            e.message
        );
        return Err(e.with("faults", &added));
    }
    save_config(session, &c)?;
    config(session)
}

/// `props.get`: every core and declared property, set or not, with its
/// declaration, value as written and faults; then any set but unknown.
pub fn get(session: &Session, part: &str) -> Result<Value> {
    let id = part_id(part)?;
    let p = session.project().part(&id)?;
    let pdm = Pdm::read(&session.snapshot(), session.project());
    let c = pdm.config?;
    let pp = &pdm.parts[&id];
    let file = pp.file.clone()?;
    let faults_of = |name: &str| -> Vec<&Error> {
        pp.faults
            .iter()
            .filter(|e| e.details["property"] == name)
            .collect()
    };
    let mut list: Vec<Value> = c
        .decls()
        .into_iter()
        .map(|(name, decl)| {
            let mut v = json!({"name": name, "core": CORE.contains(&name.as_str()),
                               "declaration": decl.to_json(), "faults": faults_of(&name)});
            if let Some(x) = file.values.get(&name) {
                v["value"] = x.clone();
            }
            v
        })
        .collect();
    list.extend(
        file.values
            .iter()
            .filter(|(n, _)| c.decl(n).is_none())
            .map(|(name, x)| {
                json!({"name": name, "core": false, "declaration": null, "value": x,
                       "faults": faults_of(name)})
            }),
    );
    Ok(json!({"part": id, "revision": p.revision, "properties": list, "faults": pp.faults}))
}

/// `props.set`: sets properties, or clears those given as `null`, as one
/// change. Text is read as each property's type (`"3"` as a whole number).
pub fn set(
    session: &mut Session,
    part: &str,
    values: Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let id = part_id(part)?;
    check_revision(session.project().part(&id)?, expected)?;
    if values.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "give at least one property: NAME=VALUE, or NAME= to clear it",
        ));
    }
    let mut files = session.snapshot();
    let c = Config::read(&files)?;
    let mut file: Properties = props::read(&files, &id)?;
    for (name, v) in values {
        if v.is_null() {
            file.values.remove(&name);
            continue;
        }
        let v = match c.decl(&name) {
            Some(d) => props::coerce(&d, v),
            None => v,
        };
        file.values.insert(name, v);
    }
    let at = props::path(&id);
    match file.values.is_empty() {
        true => files.remove(&at),
        false => files.insert(at, canonical::to_string(&file.to_json()).into_bytes()),
    };
    // A duplicate number is reported on both parts: say it of this one.
    let added = added_faults(session, &files);
    if let Some(e) = added
        .iter()
        .find(|e| e.details["part"] == id.as_str())
        .or(added.first())
    {
        return Err(e.clone());
    }
    ensure_id(session)?;
    let path = properties_path(session.root(), &id);
    if file.values.is_empty() {
        if path.is_file() {
            remove_file(session.root(), &path)?;
        }
    } else {
        write_json(&path, &file.to_json())?;
    }
    session.reload()?;
    get(session, part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};

    fn with_parts() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        for p in ["a", "b"] {
            call(&mut s, "part.create", json!({"part": p})).unwrap();
        }
        (dir, s)
    }

    fn value(v: &Value, name: &str) -> Value {
        v["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .map(|p| p.get("value").cloned().unwrap_or_default())
            .unwrap()
    }

    #[test]
    fn init_gives_the_project_an_id_and_settings_once() {
        let (dir, mut s) = fresh();
        let c = call(&mut s, "pdm.config", json!({})).unwrap();
        assert_eq!(
            (&c["configured"], &c["project"]),
            (&json!(false), &Value::Null)
        );
        assert_eq!(c["revisions"], "letters");
        assert_eq!(c["core"]["number"]["required"], true);
        let c = call(&mut s, "pdm.init", json!({})).unwrap();
        assert_eq!(c["configured"], true);
        let id = c["project"].as_str().unwrap().to_owned();
        assert_eq!(id.len(), 32);
        let manifest = std::fs::read_to_string(dir.path().join(MANIFEST)).unwrap();
        assert!(manifest.contains(&id) && manifest.contains("\"format\": 2"));
        assert!(dir.path().join(PDM_CONFIG).is_file());
        // Again: nothing changes.
        assert_eq!(call(&mut s, "pdm.init", json!({})).unwrap()["project"], id);
    }

    #[test]
    fn configure_changes_what_it_is_given() {
        let (_dir, mut s) = fresh();
        let rev = call(&mut s, "pdm.config", json!({})).unwrap()["project_revision"].clone();
        let c = call(
            &mut s,
            "pdm.configure",
            json!({"revisions": "numeric", "pattern": "\\d{3}-\\d{4}",
                   "properties": {"finish": {"type": "enum", "values": ["bare", "red"]}},
                   "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(
            (&c["revisions"], &c["pattern"]),
            (&json!("numeric"), &json!("\\d{3}-\\d{4}"))
        );
        assert_eq!(c["properties"]["finish"]["revising"], true);
        assert_eq!(c["project"].as_str().unwrap().len(), 32);
        let stale = call(
            &mut s,
            "pdm.configure",
            json!({"pattern": "", "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(stale.code, Code::Conflict);
        let c = call(
            &mut s,
            "pdm.configure",
            json!({"pattern": "", "properties": {"finish": null}}),
        )
        .unwrap();
        assert_eq!(
            (&c["pattern"], &c["properties"]),
            (&Value::Null, &json!({}))
        );
        for (change, code) in [
            (json!({}), Code::Usage),
            (json!({"revisions": "roman"}), Code::InvalidParam),
            (json!({"release_branch": "a b"}), Code::InvalidParam),
            (json!({"pattern": "("}), Code::InvalidParam),
            (
                json!({"properties": {"number": {"type": "string"}}}),
                Code::PropertyInvalid,
            ),
            (
                json!({"properties": {"f": {"type": "colour"}}}),
                Code::InvalidDocument,
            ),
            (json!({"properties": {"ghost": null}}), Code::NotFound),
        ] {
            assert_eq!(
                call(&mut s, "pdm.configure", change.clone())
                    .unwrap_err()
                    .code,
                code,
                "{change}"
            );
        }
    }

    #[test]
    fn the_scheme_is_locked_once_anything_is_released() {
        let (dir, mut s) = fresh();
        std::fs::create_dir_all(dir.path().join("releases/a")).unwrap();
        call(&mut s, "pdm.configure", json!({"revisions": "numeric"})).unwrap();
        std::fs::write(dir.path().join("releases/a/1.json"), "{}").unwrap();
        let e = call(&mut s, "pdm.configure", json!({"revisions": "letters"})).unwrap_err();
        assert_eq!(e.code, Code::RevisionSchemeLocked);
        // Setting it to what it is changes nothing, so it is allowed.
        call(&mut s, "pdm.configure", json!({"revisions": "numeric"})).unwrap();
    }

    #[test]
    fn properties_are_set_typed_and_cleared() {
        let (dir, mut s) = with_parts();
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"count": {"type": "integer"}, "coated": {"type": "boolean"}}}),
        )
        .unwrap();
        let rev = call(&mut s, "part.list", json!({})).unwrap()["parts"][0]["revision"].clone();
        let v = call(
            &mut s,
            "props.set",
            json!({"part": "a", "properties": {"number": "100", "count": "3", "coated": "true",
                                                 "name": "Bracket"},
                   "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(value(&v, "number"), "100");
        assert_eq!(value(&v, "count"), 3);
        assert_eq!(value(&v, "coated"), true);
        assert_eq!(value(&v, "material"), Value::Null);
        assert_ne!(v["revision"], rev);
        let names: Vec<&str> = v["properties"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "number",
                "name",
                "description",
                "material",
                "uom",
                "make_buy",
                "coated",
                "count"
            ]
        );
        let e = call(
            &mut s,
            "props.set",
            json!({"part": "a", "properties": {"x": 1}, "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        let path = dir.path().join("parts/a/properties.json");
        assert!(canonical::is_canonical(
            &std::fs::read_to_string(&path).unwrap()
        ));
        call(
            &mut s,
            "props.set",
            json!({"part": "a", "properties": {"number": null, "count": null, "coated": null,
                                                 "name": null}}),
        )
        .unwrap();
        assert!(!path.exists() && dir.path().join("parts/a/part.json").is_file());
    }

    #[test]
    fn writes_that_add_faults_are_refused() {
        let (_dir, mut s) = with_parts();
        call(
            &mut s,
            "props.set",
            json!({"part": "a", "properties": {"number": "100-0001"}}),
        )
        .unwrap();
        for (props, code) in [
            (json!({"colour": "red"}), Code::PropertyUnknown),
            (json!({"uom": "litre"}), Code::PropertyInvalid),
            (json!({"number": "100-0001"}), Code::PartNumberDuplicate),
        ] {
            let e = call(
                &mut s,
                "props.set",
                json!({"part": "b", "properties": props}),
            )
            .unwrap_err();
            assert_eq!((e.code, &e.details["part"]), (code, &json!("b")), "{props}");
        }
        assert_eq!(
            call(&mut s, "props.set", json!({"part": "b", "properties": {}}))
                .unwrap_err()
                .code,
            Code::Usage
        );
        // A pattern that a number already breaks needs force.
        let e = call(&mut s, "pdm.configure", json!({"pattern": "[A-Z]+"})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["part"]),
            (Code::PartNumberInvalid, &json!("a"))
        );
        assert_eq!(e.details["faults"].as_array().unwrap().len(), 1);
        call(
            &mut s,
            "pdm.configure",
            json!({"pattern": "[A-Z]+", "force": true}),
        )
        .unwrap();
        let v = call(&mut s, "props.get", json!({"part": "a"})).unwrap();
        assert_eq!(v["faults"][0]["code"], "part_number_invalid");
        // A part with a fault can still be written, as long as nothing new
        // breaks.
        call(
            &mut s,
            "props.set",
            json!({"part": "a", "properties": {"name": "A"}}),
        )
        .unwrap();
        // Removing a declaration a part uses needs force too.
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"coat": {"type": "boolean"}}}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "b", "properties": {"coat": "false"}}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"coat": null}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::PropertyUnknown);
        let e = call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"coat": {"type": "integer"}}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::PropertyInvalid);
        let v = call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"coat": null}, "force": true}),
        )
        .unwrap();
        assert_eq!(v["properties"], json!({}));
        let b = call(&mut s, "props.get", json!({"part": "b"})).unwrap();
        let coat = b["properties"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(
            (
                &coat["name"],
                &coat["declaration"],
                &coat["faults"][0]["code"]
            ),
            (&json!("coat"), &Value::Null, &json!("property_unknown"))
        );
    }

    #[test]
    fn a_retired_part_id_stays_retired() {
        let (dir, mut s) = fresh();
        std::fs::create_dir_all(dir.path().join("releases/old")).unwrap();
        call(&mut s, "part.create", json!({"part": "old"})).unwrap();
        call(&mut s, "part.delete", json!({"part": "old"})).unwrap();
        std::fs::write(dir.path().join("releases/old/A.json"), "{}").unwrap();
        let e = call(&mut s, "part.create", json!({"part": "old"})).unwrap_err();
        assert_eq!(
            (e.code, &e.details["part"]),
            (Code::PartIdRetired, &json!("old"))
        );
    }
}
