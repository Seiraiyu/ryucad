//! Part numbers against the catalogue's numbering schemes (design 734):
//! a number matching none that applies is a warning, never an error, so
//! legacy numbers are kept as written; `next` fills a scheme's template
//! from the highest number in use.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::load::Catalogue;
use crate::source::Source;
use ryupdm::config::{Config, NumberScheme};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Whether `scheme` applies to an item of `class`.
pub fn applies(scheme: &NumberScheme, class: Option<&str>) -> bool {
    scheme.class.is_none() || scheme.class.as_deref() == class
}

/// `part_number_unschemed` for each item whose number matches no scheme
/// that applies to it; none when there are no schemes.
pub fn unschemed(cat: &Catalogue, config: &Config) -> Vec<Error> {
    if config.schemes.is_empty() {
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

/// The scheme `name` names, or the one `class` takes, or the only one.
pub fn scheme<'a>(
    config: &'a Config,
    name: Option<&str>,
    class: Option<&str>,
) -> Result<&'a NumberScheme> {
    if config.schemes.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            "ryupdm.json declares no numbering schemes: add part_number.schemes",
        ));
    }
    if let Some(n) = name {
        return config.schemes.iter().find(|s| s.name == n).ok_or_else(|| {
            Error::new(Code::InvalidParam, format!("no numbering scheme {n:?}")).with("scheme", n)
        });
    }
    let with_next: Vec<&NumberScheme> =
        config.schemes.iter().filter(|s| s.next.is_some()).collect();
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
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
        )),
    }
}

/// The next number of the scheme chosen as [`scheme`] chooses: one more
/// than the highest sequence in use read back through its template: any
/// item's number or previous number, and any number a release record in
/// `src` holds (an item released, then deleted, keeps its number taken).
pub fn next(
    src: &dyn Source,
    cat: &Catalogue,
    config: &Config,
    name: Option<&str>,
    class: Option<&str>,
) -> Result<String> {
    let s = scheme(config, name, class)?;
    let t = s.next.as_ref().ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!("scheme {:?} has no next template", s.name),
        )
        .with("scheme", &s.name)
    })?;
    let highest = cat
        .items
        .values()
        .flat_map(|i| std::iter::once(&i.number).chain(&i.previous_numbers))
        .chain(held(src).keys())
        .filter_map(|n| t.read(n))
        .max()
        .unwrap_or(0);
    Ok(t.fill(highest + 1))
}

/// `part_number_duplicate` for live items sharing a number, then for a
/// live item whose number a release record of another item names.
pub fn duplicates(src: &dyn Source, cat: &Catalogue) -> Vec<Error> {
    let mut out: Vec<Error> =
        ryupdm::props::duplicates(cat.items.iter().map(|(id, i)| (id, i.number.as_str())))
            .into_iter()
            .map(|(id, e)| e.with("item", &id))
            .collect();
    let held = held(src);
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

/// The catalogues under `folders` and in `here`'s work tree, but `here`.
pub fn others(here: &Path, folders: &[PathBuf]) -> Vec<PathBuf> {
    let me = here.canonicalize().unwrap_or_else(|_| here.to_path_buf());
    let mut tops: Vec<PathBuf> = folders.to_vec();
    if let Ok(repo) = gix::discover(here)
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
