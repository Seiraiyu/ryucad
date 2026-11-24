//! A catalogue's manifest, `ryuplm.json` (design 734): the project's ID,
//! the ID it was forked from, the length unit bare quantities take, and
//! the classes items may be filed under.

use crate::error::{Code, Error, Result};
use crate::id::ProjectId;
use crate::units::Unit;
use ryu_doc::files::{Obj, header_in, invalid};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const MANIFEST: &str = "ryuplm.json";
pub const KIND: &str = "ryuplm.catalogue";
pub const FORMAT: u64 = 1;
/// The product's name in messages.
pub const NAME: &str = "ryuplm";

/// A class: the declared properties its items may carry, every declared
/// one when `None`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Class {
    pub properties: Option<Vec<String>>,
}

/// The project a catalogue was forked from, and the commit it was forked
/// at: ryueda's shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkedFrom {
    pub project: ProjectId,
    pub commit: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub project: ProjectId,
    pub forked_from: Option<ForkedFrom>,
    pub default_unit: Unit,
    pub classes: BTreeMap<String, Class>,
    /// New documents' storage unless `--storage` says (design 937): Git by
    /// default, written only when `service`.
    pub document_storage: crate::document::Storage,
}

impl Manifest {
    pub fn new(project: ProjectId, default_unit: Unit) -> Manifest {
        Manifest {
            project,
            forked_from: None,
            default_unit,
            classes: BTreeMap::new(),
            document_storage: crate::document::Storage::Git,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "default_unit": self.default_unit.as_str(),
            "format": FORMAT,
            "kind": KIND,
            "project": self.project.as_str(),
        });
        if let Some(f) = &self.forked_from {
            v["forked_from"] = json!({"commit": f.commit, "project": f.project.as_str()});
        }
        if !self.classes.is_empty() {
            let c: Map<String, Value> = self
                .classes
                .iter()
                .map(|(n, c)| {
                    let mut o = json!({});
                    if let Some(p) = &c.properties {
                        o["properties"] = json!(p);
                    }
                    (n.clone(), o)
                })
                .collect();
            v["classes"] = Value::Object(c);
        }
        if self.document_storage != crate::document::Storage::Git {
            v["documents"] = json!({"storage": self.document_storage.as_str()});
        }
        v
    }

    pub fn to_text(&self) -> String {
        crate::canonical::to_string(&self.to_json())
    }

    pub fn from_json(v: &Value) -> Result<Manifest> {
        let mut o = header_in(v, KIND, FORMAT, NAME)?;
        let p = o.take("project")?;
        let project = p
            .as_str()
            .and_then(|s| ProjectId::parse(s).ok())
            .ok_or_else(|| invalid("/project", "project must be 32 lowercase hex digits"))?;
        let forked_from = match o.take_opt("forked_from") {
            None => None,
            Some(f) => {
                let mut fo = Obj::new(&f, "/forked_from")?;
                let p = fo.take("project")?;
                let project = p
                    .as_str()
                    .and_then(|s| ProjectId::parse(s).ok())
                    .ok_or_else(|| invalid("/forked_from/project", "a project ID"))?;
                let c = fo.take("commit")?;
                let commit = c
                    .as_str()
                    .ok_or_else(|| invalid("/forked_from/commit", "commit must be a string"))?
                    .to_owned();
                fo.finish()?;
                Some(ForkedFrom { project, commit })
            }
        };
        let u = o.take("default_unit")?;
        let default_unit = u
            .as_str()
            .and_then(Unit::parse)
            .ok_or_else(|| invalid("/default_unit", "default_unit is m, cm, mm, um, in or ft"))?;
        let mut classes = BTreeMap::new();
        if let Some(c) = o.take_opt("classes") {
            let map = c
                .as_object()
                .ok_or_else(|| invalid("/classes", "classes must be an object"))?;
            for (name, body) in map {
                let at = format!("/classes/{name}");
                if !ryupdm::config::scheme_name(name) {
                    return Err(invalid(
                        &at,
                        "a class's name is 1 to 63 of a-z, 0-9, _ and -",
                    ));
                }
                let mut co = Obj::new(body, &at)?;
                let properties = match co.take_opt("properties") {
                    None => None,
                    Some(p) => Some(
                        p.as_array()
                            .and_then(|a| {
                                a.iter()
                                    .map(|x| x.as_str().map(str::to_owned))
                                    .collect::<Option<Vec<_>>>()
                            })
                            .ok_or_else(|| {
                                invalid(&format!("{at}/properties"), "a list of property names")
                            })?,
                    ),
                };
                co.finish()?;
                classes.insert(name.clone(), Class { properties });
            }
        }
        let mut document_storage = crate::document::Storage::Git;
        if let Some(d) = o.take_opt("documents") {
            let mut d_o = Obj::new(&d, "/documents")?;
            if let Some(s) = d_o.take_opt("storage") {
                document_storage = s
                    .as_str()
                    .and_then(crate::document::Storage::parse)
                    .ok_or_else(|| invalid("/documents/storage", "storage is git or service"))?;
            }
            d_o.finish()?;
        }
        o.finish()?;
        Ok(Manifest {
            document_storage,
            project,
            forked_from,
            default_unit,
            classes,
        })
    }

    pub fn read(bytes: &[u8]) -> Result<Manifest> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Error::new(Code::InvalidJson, format!("{MANIFEST} is not UTF-8")))?;
        let v = crate::canonical::parse(text)?;
        Manifest::from_json(&v).map_err(|e| e.with("path", MANIFEST))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn pid(n: u8) -> ProjectId {
        ProjectId::from_bytes([n; 16])
    }

    #[test]
    fn a_manifest_reads_and_writes_canonically() {
        let mut m = Manifest::new(pid(1), Unit::Mm);
        m.classes.insert(
            "fastener".into(),
            Class {
                properties: Some(vec!["thread".into()]),
            },
        );
        m.classes.insert("label".into(), Class { properties: None });
        let text = m.to_text();
        assert!(crate::canonical::is_canonical(&text));
        assert_eq!(Manifest::read(text.as_bytes()).unwrap(), m);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["kind"], "ryuplm.catalogue");
        assert_eq!(v["format"], 1);
        assert_eq!(v["classes"]["fastener"]["properties"], json!(["thread"]));
        assert_eq!(v["classes"]["label"], json!({}));
    }

    #[test]
    fn bad_manifests_say_where() {
        for (v, at) in [
            (
                json!({"format": 1, "kind": "ryuplm.catalogue", "project": "x", "default_unit": "mm"}),
                "/project",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.catalogue", "project": pid(1), "default_unit": "kg"}),
                "/default_unit",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.catalogue", "project": pid(1), "default_unit": "mm",
                "classes": {"Bad Name": {}}}),
                "/classes/Bad Name",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.catalogue", "project": pid(1), "default_unit": "mm",
                "classes": {"a": {"x": 1}}}),
                "/classes/a/x",
            ),
            (
                json!({"format": 1, "kind": "ryuplm.catalogue", "project": pid(1), "default_unit": "mm", "y": 1}),
                "/y",
            ),
        ] {
            let e = Manifest::read(crate::canonical::to_string(&v).as_bytes()).unwrap_err();
            assert_eq!(e.details["pointer"], at, "{e:?}");
        }
        let newer = json!({"format": 2, "kind": "ryuplm.catalogue"});
        let e = Manifest::read(crate::canonical::to_string(&newer).as_bytes()).unwrap_err();
        assert_eq!(e.code, crate::error::Code::FormatTooNew);
        assert!(e.message.contains("Upgrade ryuplm"), "{}", e.message);
    }

    #[test]
    fn the_default_storage_is_git_unless_written() {
        let p = ProjectId::parse(&"01".repeat(16)).unwrap();
        let mut m = Manifest::new(p, Unit::Mm);
        assert_eq!(m.document_storage, crate::document::Storage::Git);
        assert!(m.to_json().get("documents").is_none(), "git is unwritten");
        m.document_storage = crate::document::Storage::Service;
        let v = m.to_json();
        assert_eq!(v["documents"], json!({"storage": "service"}));
        assert_eq!(v["format"], 1, "widened before any release, as items were");
        assert_eq!(Manifest::read(m.to_text().as_bytes()).unwrap(), m);
        let mut bad = v.clone();
        bad["documents"]["storage"] = json!("lfs");
        assert_eq!(
            Manifest::from_json(&bad).unwrap_err().details["pointer"],
            "/documents/storage"
        );
    }
}
