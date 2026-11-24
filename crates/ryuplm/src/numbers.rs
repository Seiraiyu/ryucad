//! Part numbers against the catalogue's numbering schemes (design 734):
//! a number matching none that applies is a warning, never an error, so
//! legacy numbers are kept as written; `next` fills a scheme's template
//! from the highest number in use.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::load::Catalogue;
use crate::session::Session;
use crate::source::Source;
use ryupdm::config::{Config, NumberScheme, SchemeKind, Template};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Whether `scheme` applies to an item of `class`: never a document
/// scheme (design 937).
pub fn applies(scheme: &NumberScheme, class: Option<&str>) -> bool {
    scheme.kind == SchemeKind::Item && (scheme.class.is_none() || scheme.class.as_deref() == class)
}

/// `part_number_unschemed` for each item whose number matches no scheme
/// that applies to it; none when there are no item schemes.
pub fn unschemed(cat: &Catalogue, config: &Config) -> Vec<Error> {
    if !config.schemes.iter().any(|s| s.kind == SchemeKind::Item) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        let class = item.class.as_deref();
        let schemes: Vec<&NumberScheme> = config
            .schemes
            .iter()
            .filter(|s| applies(s, class))
            .collect();
        if !schemes.iter().any(|s| s.pattern.matches(&item.number)) {
            let names: Vec<&str> = schemes.iter().map(|s| s.name.as_str()).collect();
            out.push(
                Error::new(
                    Code::PartNumberUnschemed,
                    format!(
                        "item {id}: number {:?} matches no numbering scheme ({})",
                        item.number,
                        names.join(", ")
                    ),
                )
                .with("item", id)
                .with("pointer", "/number")
                .with("value", &item.number)
                .with("schemes", &names),
            );
        }
    }
    out
}

/// The item scheme `name` names, or the one `class` takes, or the only one.
pub fn scheme<'a>(
    config: &'a Config,
    name: Option<&str>,
    class: Option<&str>,
) -> Result<&'a NumberScheme> {
    scheme_of(config, name, class, SchemeKind::Item)
}

/// [`scheme`] among the schemes of `kind`: a named scheme of the other
/// kind is refused (design 937).
pub fn scheme_of<'a>(
    config: &'a Config,
    name: Option<&str>,
    class: Option<&str>,
    kind: SchemeKind,
) -> Result<&'a NumberScheme> {
    if !config.schemes.iter().any(|s| s.kind == kind) {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "ryupdm.json declares no {} numbering schemes: add part_number.schemes",
                kind.as_str()
            ),
        ));
    }
    if let Some(n) = name {
        let s = config.schemes.iter().find(|s| s.name == n).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("no numbering scheme {n:?}")).with("scheme", n)
        })?;
        if s.kind != kind {
            return Err(Error::new(
                Code::InvalidParam,
                format!("scheme {n:?} numbers {}s", s.kind.as_str()),
            )
            .with("scheme", n));
        }
        return Ok(s);
    }
    let with_next: Vec<&NumberScheme> = config
        .schemes
        .iter()
        .filter(|s| s.kind == kind && s.next.is_some())
        .collect();
    if let Some(c) = class
        && let Some(s) = with_next.iter().find(|s| s.class.as_deref() == Some(c))
    {
        return Ok(s);
    }
    match with_next.as_slice() {
        [one] => Ok(one),
        _ => Err(Error::new(
            Code::Usage,
            "several numbering schemes: name one with --next <scheme>",
        )
        .with(
            "schemes",
            config
                .schemes
                .iter()
                .filter(|s| s.kind == kind)
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
        )),
    }
}

/// The scheme [`scheme`] chooses and its template: `invalid_param` when
/// it has none.
pub fn template<'a>(
    config: &'a Config,
    name: Option<&str>,
    class: Option<&str>,
) -> Result<(&'a NumberScheme, &'a Template)> {
    let s = scheme(config, name, class)?;
    let t = s.next.as_ref().ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!("scheme {:?} has no next template", s.name),
        )
        .with("scheme", &s.name)
    })?;
    Ok((s, t))
}

/// The highest sequence `t` reads back from the numbers in use: any
/// item's number or previous number, and any number a release record in
/// `src` holds (an item released, then deleted, keeps its number taken);
/// 0 when none. The ledger's `floor`.
pub fn highest(src: &dyn Source, cat: &Catalogue, t: &Template) -> u64 {
    cat.items
        .values()
        .flat_map(|i| std::iter::once(&i.number).chain(&i.previous_numbers))
        .chain(held_by(src, cat).0.keys())
        .filter_map(|n| t.read(n))
        .max()
        .unwrap_or(0)
}

/// The next number of the scheme chosen as [`scheme`] chooses: one more
/// than [`highest`].
pub fn next(
    src: &dyn Source,
    cat: &Catalogue,
    config: &Config,
    name: Option<&str>,
    class: Option<&str>,
) -> Result<String> {
    let (_, t) = template(config, name, class)?;
    Ok(t.fill(highest(src, cat, t) + 1))
}

/// `number.local`: what the CLI reads before it asks the ledger: the
/// catalogue's project ID, whether its ledger is on, and its revision;
/// with `next` (a scheme, or "" for the one that applies), the scheme
/// chosen as `--next` chooses it (by `item`'s class when renumbering, else
/// `class`), its template, the highest sequence in use (`floor`) and the
/// local next number.
pub fn local(
    session: &Session,
    next: Option<&str>,
    class: Option<&str>,
    item: Option<&str>,
) -> Result<Value> {
    let src = session.snapshot();
    let config = Config::read_in(&src, crate::catalogue::NAME)?;
    let cat = session.catalogue();
    let mut v = json!({"catalogue": cat.manifest.project, "ledger": config.ledger,
                       "revision": session.revision()});
    if let Some(name) = next {
        let class = match item {
            Some(i) => {
                let id = cat.find(i)?;
                cat.items[&id].class.clone()
            }
            None => class.map(str::to_owned),
        };
        let (s, t) = template(
            &config,
            (!name.is_empty()).then_some(name),
            class.as_deref(),
        )?;
        let floor = highest(&src, cat, t);
        v["scheme"] = json!(s.name);
        v["template"] = json!(t.text());
        v["floor"] = json!(floor);
        v["next"] = json!(t.fill(floor + 1));
    }
    Ok(v)
}

/// `part_number_duplicate` for live items sharing a number, then for a
/// live item whose number a release record of another item names.
pub fn duplicates(src: &dyn Source, cat: &Catalogue) -> Vec<Error> {
    let mut out: Vec<Error> =
        ryupdm::props::duplicates(cat.items.iter().map(|(id, i)| (id, i.number.as_str())))
            .into_iter()
            .map(|(id, e)| e.with("item", &id))
            .collect();
    let held = held_by(src, cat).0;
    for (id, item) in &cat.items {
        let others: Vec<&Id> = held
            .get(&item.number)
            .map(|v| v.iter().filter(|p| *p != id).collect())
            .unwrap_or_default();
        if !others.is_empty() {
            out.push(
                Error::new(
                    Code::PartNumberDuplicate,
                    format!(
                        "item {id}: number {:?} is held by a release of {}",
                        item.number, others[0]
                    ),
                )
                .with("item", id)
                .with("pointer", "/number")
                .with("value", &item.number)
                .with("records", &others),
            );
        }
    }
    out.sort_by(|a, b| a.details["item"].as_str().cmp(&b.details["item"].as_str()));
    out
}

/// Every number a release record names, with the items whose records
/// name it.
pub fn held(src: &dyn Source) -> BTreeMap<String, Vec<Id>> {
    let mut out: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    let Ok(records) = ryupdm::record::Records::read_in(src, crate::catalogue::NAME) else {
        return out;
    };
    for e in &records.entries {
        if let Ok(r) = &e.record
            && let Some(rel) = r.release()
            && let Some(n) = rel.properties.get("number").and_then(|v| v.as_str())
        {
            let v = out.entry(n.to_owned()).or_default();
            if !v.contains(&rel.part) {
                v.push(rel.part.clone());
            }
        }
    }
    out
}

/// [`held`] split by number space (design 937, Decision 3): a record's
/// number is held in the document space when its part is a live document,
/// else in the item space.
pub fn held_by(
    src: &dyn Source,
    cat: &Catalogue,
) -> (BTreeMap<String, Vec<Id>>, BTreeMap<String, Vec<Id>>) {
    let (mut items, mut docs): (BTreeMap<String, Vec<Id>>, BTreeMap<String, Vec<Id>>) =
        Default::default();
    for (n, parts) in held(src) {
        for p in parts {
            let to = if cat.documents.contains_key(&p) {
                &mut docs
            } else {
                &mut items
            };
            to.entry(n.clone()).or_default().push(p);
        }
    }
    (items, docs)
}

/// `part_number_duplicate` for each live document sharing its number with
/// another live document, or whose number another document's release
/// holds: documents' own number space.
pub fn document_duplicates(src: &dyn Source, cat: &Catalogue) -> Vec<Error> {
    let at = |id: &Id, e: Error| {
        e.with("document", id)
            .with("kind", "document")
            .with("path", crate::ids::document_path(id))
            .with("pointer", "/number")
    };
    let mut out: Vec<Error> =
        ryupdm::props::duplicates(cat.documents.iter().map(|(id, d)| (id, d.number.as_str())))
            .into_iter()
            .map(|(id, e)| at(&id, e))
            .collect();
    let held = held_by(src, cat).1;
    for (id, d) in &cat.documents {
        let others: Vec<&Id> = held
            .get(&d.number)
            .map(|v| v.iter().filter(|p| *p != id).collect())
            .unwrap_or_default();
        if !others.is_empty() {
            out.push(at(
                id,
                Error::new(
                    Code::PartNumberDuplicate,
                    format!(
                        "document {id}: number {:?} is held by a release of {}",
                        d.number, others[0]
                    ),
                )
                .with("value", &d.number)
                .with("records", &others),
            ));
        }
    }
    out.sort_by(|a, b| {
        a.details["document"]
            .as_str()
            .cmp(&b.details["document"].as_str())
    });
    out
}

/// `part_number_unschemed` for each document whose number matches no
/// document scheme; none when there are no document schemes.
pub fn document_unschemed(cat: &Catalogue, config: &Config) -> Vec<Error> {
    let schemes: Vec<&NumberScheme> = config
        .schemes
        .iter()
        .filter(|s| s.kind == SchemeKind::Document)
        .collect();
    if schemes.is_empty() {
        return Vec::new();
    }
    let names: Vec<&str> = schemes.iter().map(|s| s.name.as_str()).collect();
    cat.documents
        .iter()
        .filter(|(_, d)| !schemes.iter().any(|s| s.pattern.matches(&d.number)))
        .map(|(id, d)| {
            Error::new(
                Code::PartNumberUnschemed,
                format!(
                    "document {id}: number {:?} matches no document scheme ({})",
                    d.number,
                    names.join(", ")
                ),
            )
            .with("document", id)
            .with("kind", "document")
            .with("path", crate::ids::document_path(id))
            .with("pointer", "/number")
            .with("value", &d.number)
            .with("schemes", &names)
        })
        .collect()
}

/// The next number of document scheme `name` (or the one there is), from
/// the highest documents' number in use or held.
pub fn next_document(
    src: &dyn Source,
    cat: &Catalogue,
    config: &Config,
    name: Option<&str>,
) -> Result<String> {
    let s = scheme_of(config, name, None, SchemeKind::Document)?;
    let t = s.next.as_ref().ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!("scheme {:?} has no next template", s.name),
        )
        .with("scheme", &s.name)
    })?;
    let held = held_by(src, cat).1;
    let highest = cat
        .documents
        .values()
        .flat_map(|d| std::iter::once(&d.number).chain(&d.previous_numbers))
        .chain(held.keys())
        .filter_map(|n| t.read(n))
        .max()
        .unwrap_or(0);
    Ok(t.fill(highest + 1))
}

/// The catalogues under `folders` and in `here`'s work tree, but `here`.
pub fn others(here: &Path, folders: &[PathBuf]) -> Vec<PathBuf> {
    let me = here.canonicalize().unwrap_or_else(|_| here.to_path_buf());
    let mut tops: Vec<PathBuf> = folders.to_vec();
    if let Some(repo) = ryu_git::discover_upwards(here, false)
        && let Some(w) = repo.workdir()
    {
        tops.push(w.to_path_buf());
    }
    let mut out = Vec::new();
    for t in tops {
        for p in
            ryupdm::dependents::projects_under(&t, crate::catalogue::MANIFEST).unwrap_or_default()
        {
            let p = p.canonicalize().unwrap_or(p);
            if p != me && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// `part_number_duplicate_elsewhere` for each item whose number an item
/// of a catalogue in `others` has, naming the first such.
pub fn elsewhere(cat: &Catalogue, others: &[PathBuf]) -> Vec<Error> {
    let mut by: BTreeMap<&str, (String, String, Id)> = BTreeMap::new();
    let mut loaded = Vec::new();
    for p in others {
        if let Ok(c) = Catalogue::load(&ryu_doc::source::Folder(p))
            && c.manifest.project != cat.manifest.project
        {
            loaded.push((p.clone(), c));
        }
    }
    for (p, c) in &loaded {
        for (id, i) in &c.items {
            by.entry(i.number.as_str()).or_insert_with(|| {
                (
                    p.display().to_string(),
                    c.manifest.project.to_string(),
                    id.clone(),
                )
            });
        }
    }
    cat.items
        .iter()
        .filter_map(|(id, i)| {
            let (path, project, other) = by.get(i.number.as_str())?;
            Some(
                Error::new(
                    Code::PartNumberDuplicateElsewhere,
                    format!(
                        "item {id}: number {:?} is also item {other} of catalogue {project}",
                        i.number
                    ),
                )
                .with("item", id)
                .with("pointer", "/number")
                .with("value", &i.number)
                .with(
                    "elsewhere",
                    json!({"catalogue": project, "item": other, "path": path}),
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn documents_have_their_own_number_space() {
        let mut f = crate::load::tests::files(&[("0000000000001", "100-200")]);
        f.insert(
            "ryupdm.json".into(),
            crate::canonical::to_string(&serde_json::json!({"format": 7, "kind": "ryupdm.config",
                "release_branch": "main", "revisions": "letters",
                "part_number": {"schemes": [
                    {"name": "legacy", "pattern": "\\d{3}-\\d{3}"},
                    {"name": "doc", "kind": "document", "pattern": "DOC-\\d{4}", "next": "DOC-{seq:4}"}]}}))
            .into_bytes(),
        );
        let file = crate::document::File::of("a.pdf", b"a", None).unwrap();
        for (id, n) in [
            ("d1000000000000", "100-200"),
            ("d2000000000000", "DOC-0007"),
            ("d3000000000000", "DOC-0007"),
        ] {
            let d = crate::document::Document::new(n, "T", file.clone());
            let id = Id::parse(id, "document").unwrap();
            f.insert(crate::ids::document_path(&id), d.to_text().into_bytes());
            f.insert(crate::ids::bytes_path(&id, "a.pdf"), b"a".to_vec());
        }
        let cat = Catalogue::load(&f).unwrap();
        let config = Config::read_in(&f, "ryuplm").unwrap();
        // The drawing shares its part's number: no duplicate across spaces.
        let dups: Vec<(String, String)> = document_duplicates(&f, &cat)
            .iter()
            .map(|e| {
                (
                    e.details["document"].as_str().unwrap().to_owned(),
                    e.details["kind"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            dups,
            [
                ("d2000000000000".into(), "document".into()),
                ("d3000000000000".into(), "document".into())
            ]
        );
        assert!(
            duplicates(&f, &cat).is_empty(),
            "the item's number is its own"
        );
        // 100-200 matches no document scheme; DOC-0007 does.
        let un: Vec<String> = document_unschemed(&cat, &config)
            .iter()
            .map(|e| e.details["document"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(un, ["d1000000000000"]);
        assert!(
            unschemed(&cat, &config).is_empty(),
            "the item scheme is the item's"
        );
        assert_eq!(next_document(&f, &cat, &config, None).unwrap(), "DOC-0008");
        assert_eq!(
            next(&f, &cat, &config, Some("doc"), None).unwrap_err().code,
            Code::InvalidParam,
            "a document scheme never numbers an item"
        );
    }

    use super::*;
    use crate::item::Item;
    use crate::judge::tests::{codes, files};
    use serde_json::json;

    fn config() -> serde_json::Value {
        json!({"format": 5, "kind": "ryupdm.config", "part_number": {"schemes": [
            {"name": "mech", "class": "machined", "pattern": "MECH-\\d{5}", "next": "MECH-{seq:5}"},
            {"name": "elec", "pattern": "E\\d{4}", "next": "E{seq:4}"}
        ]}})
    }

    fn item(n: &str, class: Option<&str>) -> Item {
        let mut i = Item::new(n);
        i.class = class.map(str::to_owned);
        i
    }

    #[test]
    fn unschemed_numbers_are_warnings() {
        let f = files(
            &[("machined", None), ("label", None)],
            Some(config()),
            &[
                ("0000000000001", item("MECH-00001", Some("machined"))),
                ("0000000000002", item("MECH-00002", Some("label"))),
                ("0000000000003", item("E0007", Some("label"))),
                ("0000000000004", item("100-0042 rev", None)),
            ],
        );
        assert_eq!(
            codes(&f),
            [
                (
                    "part_number_unschemed".into(),
                    "warning",
                    "0000000000002".into()
                ),
                (
                    "part_number_unschemed".into(),
                    "warning",
                    "0000000000004".into()
                ),
            ]
        );
        // No schemes, no warnings.
        let plain = files(&[], None, &[("0000000000004", item("100-0042 rev", None))]);
        assert!(codes(&plain).is_empty());
    }

    #[test]
    fn the_next_number_follows_the_highest_in_use() {
        let f = files(
            &[("machined", None)],
            Some(config()),
            &[
                ("0000000000001", item("MECH-00007", Some("machined"))),
                ("0000000000002", {
                    let mut i = item("MECH-00009", Some("machined"));
                    i.previous_numbers.push("MECH-00041".into());
                    i
                }),
                ("0000000000003", item("E0002", None)),
            ],
        );
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let c = ryupdm::config::Config::read_in(&f, "ryuplm").unwrap();
        assert_eq!(
            next(&f, &cat, &c, Some("mech"), None).unwrap(),
            "MECH-00042"
        );
        assert_eq!(
            next(&f, &cat, &c, None, Some("machined")).unwrap(),
            "MECH-00042"
        );
        assert_eq!(next(&f, &cat, &c, Some("elec"), None).unwrap(), "E0003");
        assert_eq!(
            next(&f, &cat, &c, None, None).unwrap_err().code,
            Code::Usage
        );
        assert_eq!(
            next(&f, &cat, &c, Some("none"), None).unwrap_err().code,
            Code::InvalidParam
        );
        let empty = files(
            &[],
            Some(json!({"format": 1, "kind": "ryupdm.config"})),
            &[],
        );
        let cat = crate::load::Catalogue::load(&empty).unwrap();
        let c = ryupdm::config::Config::read_in(&empty, "ryuplm").unwrap();
        assert_eq!(
            next(&empty, &cat, &c, None, None).unwrap_err().code,
            Code::InvalidParam
        );
    }
    #[test]
    fn the_next_number_skips_numbers_records_hold() {
        use ryupdm::record::{BuiltWith, Record, Release, ReleasedBy};
        let mut f = files(
            &[("machined", None)],
            Some(config()),
            &[("0000000000001", item("MECH-00007", Some("machined")))],
        );
        // MECH-00050's item was released, then deleted.
        let r = Record::Release(Release {
            project: crate::catalogue::tests::pid(1),
            part: Id::parse("0000000000009", "item").unwrap(),
            label: "A".into(),
            content: "0".repeat(64),
            properties: [("number".to_owned(), json!("MECH-00050"))].into(),
            results: Default::default(),
            built_with: BuiltWith::of_product("ryuplm", "0.0.0", "c"),
            released_by: ReleasedBy {
                name: None,
                email: None,
                participant: "agent".into(),
            },
            supersedes: None,
            note: None,
        });
        f.insert(
            r.path(),
            crate::canonical::to_string(&r.to_json()).into_bytes(),
        );
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let c = ryupdm::config::Config::read_in(&f, "ryuplm").unwrap();
        assert_eq!(
            next(&f, &cat, &c, Some("mech"), None).unwrap(),
            "MECH-00051"
        );
    }

    #[test]
    fn numbers_are_unique_live_and_released() {
        use ryupdm::record::{BuiltWith, Record, Release, ReleasedBy};
        let mut f = files(
            &[],
            None,
            &[
                ("0000000000001", item("A-1", None)),
                ("0000000000002", item("A-1", None)),
                ("0000000000003", item("A-9", None)),
            ],
        );
        // A record of a deleted item still holds A-9.
        let r = Record::Release(Release {
            project: crate::catalogue::tests::pid(1),
            part: Id::parse("0000000000009", "item").unwrap(),
            label: "A".into(),
            content: "0".repeat(64),
            properties: [("number".to_owned(), json!("A-9"))].into(),
            results: Default::default(),
            built_with: BuiltWith::of_product("ryuplm", "0.0.0", "c"),
            released_by: ReleasedBy {
                name: None,
                email: None,
                participant: "agent".into(),
            },
            supersedes: None,
            note: None,
        });
        f.insert(
            r.path(),
            crate::canonical::to_string(&r.to_json()).into_bytes(),
        );
        assert_eq!(
            codes(&f),
            [
                (
                    "part_number_duplicate".into(),
                    "error",
                    "0000000000001".into()
                ),
                (
                    "part_number_duplicate".into(),
                    "error",
                    "0000000000002".into()
                ),
                (
                    "part_number_duplicate".into(),
                    "error",
                    "0000000000003".into()
                ),
            ]
        );
    }

    #[test]
    fn numbers_elsewhere_are_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let other = files(&[], None, &[("000000000000a", item("A-1", None))]);
        // Another catalogue, with its own project ID.
        let mut m = crate::catalogue::Manifest::new(
            crate::catalogue::tests::pid(2),
            crate::units::Unit::Mm,
        );
        m.classes.clear();
        for (p, b) in other {
            let path = dir.path().join("other").join(&p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = if p == "ryuplm.json" {
                m.to_text().into_bytes()
            } else {
                b
            };
            std::fs::write(path, bytes).unwrap();
        }
        let f = files(&[], None, &[("0000000000001", item("A-1", None))]);
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let cx = crate::judge::Context {
            elsewhere: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let found: Vec<_> = crate::judge::all(&f, &cat, &cx)
            .into_iter()
            .map(|x| {
                (
                    x.error.code,
                    x.is_error(),
                    x.error.details["elsewhere"]["item"].clone(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [(
                Code::PartNumberDuplicateElsewhere,
                false,
                json!("000000000000a")
            )]
        );
    }
}
