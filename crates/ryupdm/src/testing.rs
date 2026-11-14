//! A product of this crate's own for its core's tests, as `ryu-git` has
//! one: `toy.json` projects whose parts are files `things/<id>.json`,
//! their properties `properties/<id>.json`. Nothing here is ryucad's.

use crate::product::{Identity, PartFiles, Product};
use ryu_doc::canonical;
use ryu_doc::error::{Code, Error, Result};
use ryu_doc::id::{Id, ProjectId};
use ryu_doc::source::{Kind, Source};
use ryu_doc::units::Unit;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) struct Toy;

/// A toy project's ID.
pub(crate) const PROJECT: &str = "0123456789abcdef0123456789abcdef";

impl ryu_git::Layout for Toy {
    const NAME: &'static str = "toy";
    const MANIFEST: &'static str = "toy.json";
    const ROOT_FILES: &'static [&'static str] = &["toy.json", "ryupdm.json"];
    const FOLDERS: &'static [&'static str] = &["things", "properties", "releases"];
    fn project_file(rel: &str) -> bool {
        Self::ROOT_FILES.contains(&rel)
            || ["things/", "properties/"]
                .iter()
                .any(|d| rel.strip_prefix(d).is_some_and(|f| !f.contains('/')))
            || rel
                .strip_prefix("releases/")
                .is_some_and(|f| f.matches('/').count() == 1)
    }
}

/// `things/<id>.json` or `properties/<id>.json`'s ID.
fn file_id(path: &str) -> Option<Id> {
    let name = path
        .strip_prefix("things/")
        .or_else(|| path.strip_prefix("properties/"))?;
    Id::parse(name.strip_suffix(".json")?, "part").ok()
}

impl Product for Toy {
    const PART_FOLDERS: &'static [&'static str] = &["things", "properties"];

    fn parts(src: &dyn Source) -> Result<Vec<Id>> {
        Ok(src
            .list("things")?
            .into_iter()
            .filter(|(_, k)| *k == Kind::File)
            .filter_map(|(n, _)| file_id(&format!("things/{n}")))
            .collect())
    }

    fn properties_path(part: &Id) -> String {
        format!("properties/{part}.json")
    }

    fn part_files(part: &Id) -> PartFiles {
        PartFiles {
            files: vec![format!("things/{part}.json"), Self::properties_path(part)],
            folders: Vec::new(),
        }
    }

    fn part_of(path: &str) -> Option<Id> {
        file_id(path)
    }

    fn content(src: &dyn Source, part: &Id) -> Result<String> {
        let manifest = crate::read_json(src, "toy.json")?
            .ok_or_else(|| Error::new(Code::NotFound, "no toy.json").with("path", "toy.json"))?;
        let unit = manifest["default_unit"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let at = format!("things/{part}.json");
        let thing = src.read(&at)?.ok_or_else(|| {
            Error::new(Code::NotFound, format!("no {at}")).with("path", at.as_str())
        })?;
        let mut h = blake3::Hasher::new();
        for piece in [b"toy.content.1".as_slice(), unit.as_bytes(), &thing] {
            h.update(&(piece.len() as u64).to_le_bytes());
            h.update(piece);
        }
        Ok(h.finalize().to_hex().to_string())
    }

    fn identity(manifest: &Value) -> Option<Identity> {
        (manifest["kind"] == "toy.project").then_some(())?;
        Some(Identity {
            project: manifest["project"]
                .as_str()
                .and_then(|p| ProjectId::parse(p).ok()),
            forked_from: None,
            unit: Unit::parse(manifest["default_unit"].as_str()?)?,
        })
    }
}

/// A part ID.
pub(crate) fn id(s: &str) -> Id {
    Id::parse(s, "part").unwrap()
}

/// A toy project's files: its manifest and each `(id, json)` thing.
pub(crate) fn project(things: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    let mut f = BTreeMap::new();
    f.insert(
        "toy.json".to_owned(),
        canonical::to_string(&json!({"default_unit": "mm", "format": 1,
                                     "kind": "toy.project", "project": PROJECT}))
        .into_bytes(),
    );
    for (id, text) in things {
        f.insert(format!("things/{id}.json"), text.as_bytes().to_vec());
    }
    f
}

/// A proposed release record of a toy part at `label`, format 1, with
/// `content` as its hash.
pub(crate) fn record(part: &str, label: &str, content: &str) -> Vec<u8> {
    canonical::to_string(&json!({
        "built_with": {"commit": "c0ffee", "ryucad": "0", "ryukei": "0"},
        "content": content, "format": 1, "kind": "ryupdm.release", "label": label,
        "part": part, "project": PROJECT, "properties": {},
        "released_by": {"participant": "agent"}, "results": {}, "supersedes": null,
    }))
    .into_bytes()
}

/// A properties file with only a number.
pub(crate) fn numbered(n: &str) -> Vec<u8> {
    canonical::to_string(&json!({"format": 1, "kind": "ryupdm.properties", "number": n}))
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::Product;
    use ryu_git::Layout;

    #[test]
    fn a_toys_parts_are_its_things() {
        let f = project(&[("b", "{}"), ("a", "{}")]);
        assert_eq!(Toy::parts(&f).unwrap(), [id("a"), id("b")]);
        assert_eq!(Toy::properties_path(&id("a")), "properties/a.json");
        assert_eq!(Toy::part_of("things/a.json"), Some(id("a")));
        assert_eq!(Toy::part_of("properties/a.json"), Some(id("a")));
        assert_eq!(Toy::part_of("toy.json"), None);
        assert!(Toy::project_file("things/a.json") && !Toy::project_file("things/x/a.json"));
        let i = crate::product::identity::<Toy>(&f).unwrap();
        assert_eq!(i.project.unwrap().as_str(), PROJECT);
    }

    #[test]
    fn a_toys_content_is_its_thing_and_unit() {
        let f = project(&[("a", "{\"x\": 1}")]);
        let h = Toy::content(&f, &id("a")).unwrap();
        assert_eq!(h.len(), 64);
        let mut g = f.clone();
        g.insert(
            "properties/a.json".into(),
            b"{\"format\": 1, \"kind\": \"ryupdm.properties\"}".to_vec(),
        );
        assert_eq!(
            Toy::content(&g, &id("a")).unwrap(),
            h,
            "properties aren't content"
        );
        g.insert("things/a.json".into(), b"{\"x\": 2}".to_vec());
        assert_ne!(Toy::content(&g, &id("a")).unwrap(), h);
        assert!(Toy::content(&f, &id("b")).is_err());
    }

    #[test]
    fn a_toys_properties_are_judged() {
        use crate::props::{Pdm, read_for};
        let mut f = project(&[("a", "{}"), ("b", "{}")]);
        f.insert("properties/a.json".into(), numbered("1"));
        f.insert("properties/b.json".into(), numbered("1"));
        assert_eq!(read_for::<Toy>(&f, &id("a")).unwrap().number(), Some("1"));
        let pdm = Pdm::of_for::<Toy>(&f, Toy::parts(&f).unwrap(), Unit::Mm);
        for p in ["a", "b"] {
            assert_eq!(pdm.parts[&id(p)].faults[0].code, Code::PartNumberDuplicate);
        }
    }

    #[test]
    fn a_toys_files_at_a_commit() {
        use crate::history::tests::{commit, put, repo};
        let d = repo();
        let root = d.path();
        let manifest = project(&[]);
        for (rel, text) in [
            (
                "t/toy.json",
                std::str::from_utf8(&manifest["toy.json"]).unwrap(),
            ),
            ("t/things/a.json", "{}"),
            ("t/things/b.json", "{}"),
            ("t/properties/a.json", "{}"),
            ("t/releases/a/A.json", "{}"),
            ("t/README", "not the project's"),
        ] {
            put(root, rel, text);
        }
        let c = commit(root, "toy");
        let git = gix::open(root).unwrap();
        let keys = |m: BTreeMap<String, Vec<u8>>| m.into_keys().collect::<Vec<_>>();
        assert_eq!(
            keys(crate::history::files_for::<Toy>(&git, &c, "t/").unwrap()),
            [
                "properties/a.json",
                "releases/a/A.json",
                "things/a.json",
                "things/b.json",
                "toy.json"
            ]
        );
        assert_eq!(
            keys(crate::history::part_files_for::<Toy>(&git, &c, "t/", &id("a")).unwrap()),
            ["properties/a.json", "things/a.json", "toy.json"]
        );
    }
}
