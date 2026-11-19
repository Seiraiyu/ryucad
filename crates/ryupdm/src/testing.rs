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
    const ROOT_FILES: &'static [&'static str] = &["toy.json", "ryupdm.json", "common.json"];
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

    fn parts(src: &dyn Source) -> Result<Vec<(Id, crate::product::PartKind)>> {
        Ok(src
            .list("things")?
            .into_iter()
            .filter(|(_, k)| *k == Kind::File)
            .filter_map(|(n, _)| file_id(&format!("things/{n}")))
            .map(|id| (id, crate::product::PartKind::Component))
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
        // A project's shared file, when it has one, after the thing.
        let common = src.read("common.json")?;
        let mut pieces: Vec<&[u8]> = vec![b"toy.content.1".as_slice(), unit.as_bytes(), &thing];
        if let Some(c) = &common {
            pieces.push(c);
        }
        let mut h = blake3::Hasher::new();
        for piece in pieces {
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
    fn shared(path: &str) -> bool {
        path == "common.json"
    }

    fn defaults(src: &dyn Source, part: &Id) -> Result<BTreeMap<String, Value>> {
        let Some(bytes) = src.read(&format!("things/{part}.json"))? else {
            return Ok(BTreeMap::new());
        };
        let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        Ok(v.get("name")
            .and_then(Value::as_str)
            .map(|n| BTreeMap::from([("name".to_owned(), json!(n))]))
            .unwrap_or_default())
    }

    /// A thing's `carried` label is continued, as ryuplm continues an
    /// item's legacy revision (design 935).
    fn first_label(src: &dyn Source, part: &Id, scheme: crate::config::Scheme) -> String {
        crate::read_json(src, &format!("things/{part}.json"))
            .ok()
            .flatten()
            .and_then(|v| {
                v["carried"]
                    .as_str()
                    .and_then(|c| crate::label::next(scheme, c))
            })
            .unwrap_or_else(|| crate::label::first(scheme))
    }
}

/// A toy whose parts hold their own properties, as ryuplm's items do:
/// `things/<id>.json` is `{"format": 1, "kind": "toy.thing", "number": …,
/// "props": {…}}`, read through [`Product::properties_from`].
pub(crate) struct Inline;

impl ryu_git::Layout for Inline {
    const NAME: &'static str = "inline";
    const MANIFEST: &'static str = "toy.json";
    const ROOT_FILES: &'static [&'static str] = Toy::ROOT_FILES;
    const FOLDERS: &'static [&'static str] = Toy::FOLDERS;
    fn project_file(rel: &str) -> bool {
        Toy::project_file(rel)
    }
}

impl Product for Inline {
    const PART_FOLDERS: &'static [&'static str] = &["things"];
    fn parts(src: &dyn Source) -> Result<Vec<(Id, crate::product::PartKind)>> {
        Ok(Toy::parts(src)?
            .into_iter()
            .map(|(id, _)| (id, crate::product::PartKind::Item))
            .collect())
    }
    fn properties_path(part: &Id) -> String {
        format!("things/{part}.json")
    }
    fn part_files(part: &Id) -> PartFiles {
        PartFiles {
            files: vec![Self::properties_path(part)],
            folders: Vec::new(),
        }
    }
    fn part_of(path: &str) -> Option<Id> {
        file_id(path)
    }
    fn content(src: &dyn Source, part: &Id) -> Result<String> {
        Toy::content(src, part)
    }
    fn identity(manifest: &Value) -> Option<Identity> {
        Toy::identity(manifest)
    }
    fn properties_from(v: &Value) -> Option<Result<crate::props::Properties>> {
        if v["kind"] != "toy.thing" {
            return None;
        }
        let mut values = BTreeMap::new();
        if let Some(n) = v.get("number") {
            values.insert("number".to_owned(), n.clone());
        }
        if let Some(Value::Object(p)) = v.get("props") {
            values.extend(p.iter().map(|(k, x)| (k.clone(), x.clone())));
        }
        Some(Ok(crate::props::Properties { values }))
    }
}

/// The project in the folder `dir`, as a source.
pub(crate) fn folder(dir: &std::path::Path) -> ryu_doc::source::Folder<'_> {
    ryu_doc::source::Folder(dir)
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
    fn a_product_may_keep_properties_in_its_part_files() {
        let mut f: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (id, n) in [("a", "100-0001"), ("b", "100-0001"), ("c", "100-0002")] {
            f.insert(
                format!("things/{id}.json"),
                canonical::to_string(&json!({
                    "format": 1, "kind": "toy.thing", "number": n,
                    "props": {"name": format!("thing {id}")}
                }))
                .into_bytes(),
            );
        }
        let p = crate::props::read_for::<Inline>(&f, &Id::parse("a", "part").unwrap()).unwrap();
        assert_eq!(p.number(), Some("100-0001"));
        assert_eq!(p.values["name"], "thing a");
        let pdm = crate::props::Pdm::of_for::<Inline>(
            &f,
            crate::product::part_ids::<Inline>(&f).unwrap(),
            Unit::Mm,
        );
        let dup: Vec<_> = pdm
            .faults()
            .into_iter()
            .map(|e| (e.code, e.details["part"].clone()))
            .collect();
        assert_eq!(
            dup,
            [
                (Code::PartNumberDuplicate, json!("a")),
                (Code::PartNumberDuplicate, json!("b"))
            ]
        );
        // Toy keeps reading `ryupdm.properties` files: the default is None.
        assert!(Toy::properties_from(&json!({"kind": "toy.thing"})).is_none());
    }

    #[test]
    fn a_toys_parts_are_its_things() {
        let f = project(&[("b", "{}"), ("a", "{}")]);
        use crate::product::PartKind;
        assert_eq!(
            Toy::parts(&f).unwrap(),
            [
                (id("a"), PartKind::Component),
                (id("b"), PartKind::Component)
            ]
        );
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
        let pdm = Pdm::of_for::<Toy>(&f, crate::product::part_ids::<Toy>(&f).unwrap(), Unit::Mm);
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

    #[test]
    fn a_toys_states() {
        use crate::history::Releases;
        use crate::state::{Status, derive_all_for, derive_for};
        let mut f = project(&[("a", "{}"), ("b", "{}")]);
        f.insert(
            "releases/c/A.json".into(),
            record("c", "A", &"0".repeat(64)),
        );
        let all = derive_all_for::<Toy>(&f, &Releases::default());
        assert_eq!(
            all.keys().cloned().collect::<Vec<_>>(),
            [id("a"), id("b"), id("c")]
        );
        let a = derive_for::<Toy>(&f, &id("a"), &Releases::default());
        assert_eq!((a.status, a.next.as_deref()), (Status::InWork, Some("A")));
        assert_eq!(
            all[&id("c")].proposed.as_ref().map(|(l, _)| l.as_str()),
            Some("A")
        );
    }

    #[test]
    fn a_toys_proposal_goes_stale_when_its_thing_changes() {
        use crate::check::run_for;
        let mut f = project(&[("a", "{\"x\": 1}")]);
        let c = Toy::content(&f, &id("a")).unwrap();
        f.insert("releases/a/A.json".into(), record("a", "A", &c));
        let codes = |f: &BTreeMap<String, Vec<u8>>| -> Vec<&'static str> {
            run_for::<Toy>(f, None, None)
                .unwrap()
                .into_iter()
                .map(|x| x.code)
                .collect()
        };
        assert!(!codes(&f).contains(&"release_stale"), "{:?}", codes(&f));
        f.insert("things/a.json".into(), b"{\"x\": 2}".to_vec());
        assert!(codes(&f).contains(&"release_stale"), "{:?}", codes(&f));
        let mut other = f.clone();
        other.insert(
            "toy.json".into(),
            canonical::to_string(
                &json!({"default_unit": "mm", "format": 1, "kind": "toy.project",
                                         "project": "fedcba9876543210fedcba9876543210"}),
            )
            .into_bytes(),
        );
        assert!(codes(&other).contains(&"release_project_mismatch"));
    }

    #[test]
    fn a_shared_file_staged_touches_every_part_with_records() {
        use crate::check::staged_for;
        use crate::history::tests::{commit, git, put};
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        // A work branch: the record stays proposed (the release branch is main).
        git(dir, &["init", "-q", "-b", "work"]);
        for (p, b) in &project(&[("a", "{}")]) {
            put(dir, p, std::str::from_utf8(b).unwrap());
        }
        put(dir, "common.json", "{\"v\": 1}\n");
        let content = Toy::content(&folder(dir), &id("a")).unwrap();
        put(
            dir,
            "releases/a/A.json",
            std::str::from_utf8(&record("a", "A", &content)).unwrap(),
        );
        commit(dir, "a proposed");
        // Nothing staged: nothing found, the root files read as unchanged.
        let repo = gix::open(dir).unwrap();
        assert_eq!(staged_for::<Toy>(&repo, "").unwrap().len(), 0);
        put(dir, "common.json", "{\"v\": 2}\n");
        git(dir, &["add", "common.json"]);
        let repo = gix::open(dir).unwrap();
        let found = staged_for::<Toy>(&repo, "").unwrap();
        assert_eq!(
            found.iter().map(|f| f.code).collect::<Vec<_>>(),
            ["release_stale"],
            "{found:?}"
        );
    }

    #[test]
    fn a_toy_merge_that_shares_a_number_blames_both() {
        use crate::merge::{Cause, validate_for};
        let base = project(&[("a", "{}"), ("b", "{}")]);
        let mut ours = base.clone();
        ours.insert("properties/a.json".into(), numbered("1"));
        let mut theirs = base.clone();
        theirs.insert("properties/b.json".into(), numbered("1"));
        let mut merged = ours.clone();
        merged.insert("properties/b.json".into(), numbered("1"));
        let broke = validate_for::<Toy>(&base, &ours, &theirs, &merged, &Default::default());
        let got: Vec<(&str, &str, Cause)> = broke
            .iter()
            .map(|b| (b.code.as_str(), b.path.as_str(), b.cause))
            .collect();
        assert_eq!(
            got,
            [
                ("part_number_duplicate", "properties/a.json", Cause::Theirs),
                ("part_number_duplicate", "properties/b.json", Cause::Ours),
            ]
        );
    }

    #[test]
    fn a_toy_continues_its_things_carried_label() {
        use crate::history::Releases;
        use crate::state::derive_for;
        let mut f = project(&[
            ("a", "{\"carried\": \"C\"}"),
            ("b", "{\"carried\": \"X3\"}"),
        ]);
        let next = |f: &BTreeMap<String, Vec<u8>>, p: &str| {
            derive_for::<Toy>(f, &id(p), &Releases::default()).next
        };
        assert_eq!(next(&f, "a").as_deref(), Some("D"));
        assert_eq!(
            next(&f, "b").as_deref(),
            Some("A"),
            "a label the scheme can't read"
        );
        let c = Toy::content(&f, &id("a")).unwrap();
        f.insert("releases/a/D.json".into(), record("a", "D", &c));
        let sequence = |f: &BTreeMap<String, Vec<u8>>| {
            crate::check::run_for::<Toy>(f, None, None)
                .unwrap()
                .into_iter()
                .filter(|x| x.code == "release_sequence")
                .count()
        };
        assert_eq!(sequence(&f), 0, "D is a's first label");
        assert_eq!(next(&f, "a").as_deref(), Some("E"));
        f.insert("things/a.json".into(), b"{}".to_vec());
        assert_eq!(
            sequence(&f),
            1,
            "without the carried label, a first release is A"
        );
    }
}
