//! ryuplm as ryupdm and Git read it (design 734): items are ryupdm parts
//! of kind `item`, whose properties are the item file's own fields, read
//! through `Product::properties_from`.

use crate::Plm;
use crate::catalogue::MANIFEST;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::ids::{ITEMS, item_of, item_path};
use crate::item::Item;
use crate::source::Source;
use ryupdm::product::{Identity, PartFiles, PartKind, Product};
use ryupdm::props::Properties;
use ryupdm::{PDM_CONFIG, RELEASES};
use serde_json::Value;

pub const ROOT_FILES: [&str; 3] = [MANIFEST, PDM_CONFIG, ".gitattributes"];
pub const FOLDERS: [&str; 2] = [ITEMS, RELEASES];

impl ryu_git::Layout for Plm {
    const NAME: &'static str = crate::catalogue::NAME;
    const MANIFEST: &'static str = MANIFEST;
    const ROOT_FILES: &'static [&'static str] = &ROOT_FILES;
    const FOLDERS: &'static [&'static str] = &FOLDERS;
    fn project_file(rel: &str) -> bool {
        ROOT_FILES.contains(&rel)
            || item_of(rel).is_some()
            || rel
                .strip_prefix("releases/")
                .is_some_and(|f| f.matches('/').count() == 1)
    }
}

/// The fields of an item ryupdm reads as properties: the core ones by
/// their ryupdm names (`unit` is `uom`), then the declared ones.
pub fn properties(item: &Item) -> Properties {
    let mut values = item.properties.clone();
    values.insert("number".into(), Value::from(item.number.clone()));
    for (k, v) in [
        ("name", &item.name),
        ("description", &item.description),
        ("make_buy", &item.make_buy),
    ] {
        if let Some(v) = v {
            values.insert(k.into(), Value::from(v.clone()));
        }
    }
    if let Some(u) = &item.unit {
        values.insert("uom".into(), Value::from(u.clone()));
    }
    Properties { values }
}

/// The content an item's release names: its file without provenance,
/// unmapped fields or old numbers.
pub fn content_of(item: &Item) -> String {
    let mut v = item.to_json();
    let o = v.as_object_mut().expect("an object");
    for k in ["source", "extra", "previous_numbers"] {
        o.remove(k);
    }
    let body = crate::canonical::to_string(&v);
    let mut h = blake3::Hasher::new();
    for piece in [b"ryuplm.content.1".as_slice(), body.as_bytes()] {
        h.update(&(piece.len() as u64).to_le_bytes());
        h.update(piece);
    }
    h.finalize().to_hex().to_string()
}

impl Product for Plm {
    const PART_FOLDERS: &'static [&'static str] = &[ITEMS];

    fn parts(src: &dyn Source) -> Result<Vec<(Id, PartKind)>> {
        let mut out = Vec::new();
        for (shard, _) in src.list(ITEMS)? {
            for (name, kind) in src.list(&format!("{ITEMS}/{shard}"))? {
                if kind == crate::source::Kind::File
                    && let Some(id) = item_of(&format!("{ITEMS}/{shard}/{name}"))
                {
                    out.push((id, PartKind::Item));
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn properties_path(part: &Id) -> String {
        item_path(part)
    }

    fn part_files(part: &Id) -> PartFiles {
        PartFiles {
            files: vec![item_path(part)],
            folders: Vec::new(),
        }
    }

    fn part_of(path: &str) -> Option<Id> {
        item_of(path)
    }

    /// The manifest (classes, the default unit) may change how any item
    /// is judged.
    fn shared(path: &str) -> bool {
        path == MANIFEST
    }

    fn content(src: &dyn Source, part: &Id) -> Result<String> {
        let at = item_path(part);
        let bytes = src
            .read(&at)?
            .ok_or_else(|| Error::new(Code::ItemNotFound, format!("no {at}")).with("path", &at))?;
        Ok(content_of(&Item::read(part, &bytes)?))
    }

    fn identity(manifest: &Value) -> Option<Identity> {
        let m = crate::catalogue::Manifest::from_json(manifest).ok()?;
        Some(Identity {
            project: Some(m.project),
            forked_from: m.forked_from.map(|f| f.project),
            unit: m.default_unit,
        })
    }

    fn properties_from(v: &Value) -> Option<Result<Properties>> {
        (v["kind"] == crate::item::KIND).then(|| Item::from_json(v).map(|i| properties(&i)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::item::Item;
    use ryu_git::Layout;
    use ryupdm::product::{PartFiles, PartKind, Product};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn id(s: &str) -> Id {
        Id::parse(s, "item").unwrap()
    }

    fn files() -> BTreeMap<String, Vec<u8>> {
        let mut f = BTreeMap::new();
        let m = crate::catalogue::Manifest::new(
            crate::catalogue::tests::pid(1),
            crate::units::Unit::Mm,
        );
        f.insert("ryuplm.json".to_owned(), m.to_text().into_bytes());
        let mut a = Item::new("A-1");
        a.name = Some("Bracket".into());
        a.unit = Some("kg".into());
        a.properties.insert("finish".into(), json!("anodized"));
        f.insert(
            "items/k3/k3v9q2m8x4c1p.json".into(),
            a.to_text().into_bytes(),
        );
        f.insert(
            "items/00/0000000000000.json".into(),
            Item::new("A-1").to_text().into_bytes(),
        );
        f
    }

    #[test]
    fn items_are_ryupdm_parts() {
        let f = files();
        assert_eq!(Plm::NAME, "ryuplm");
        assert_eq!(Plm::MANIFEST, "ryuplm.json");
        assert!(Plm::project_file("items/k3/k3v9q2m8x4c1p.json"));
        assert!(Plm::project_file("ryupdm.json"));
        assert!(Plm::project_file("releases/k3v9q2m8x4c1p/A.json"));
        assert!(!Plm::project_file("items/k3/deep/x.json"));
        assert!(!Plm::project_file("notes.txt"));
        assert_eq!(
            <Plm as Product>::parts(&f).unwrap(),
            [
                (id("0000000000000"), PartKind::Item),
                (id("k3v9q2m8x4c1p"), PartKind::Item)
            ]
        );
        assert_eq!(
            <Plm as Product>::properties_path(&id("k3v9q2m8x4c1p")),
            "items/k3/k3v9q2m8x4c1p.json"
        );
        assert_eq!(
            <Plm as Product>::part_of("items/k3/k3v9q2m8x4c1p.json"),
            Some(id("k3v9q2m8x4c1p"))
        );
        assert_eq!(<Plm as Product>::part_of("ryuplm.json"), None);
        assert!(<Plm as Product>::shared("ryuplm.json"));
        assert_eq!(
            <Plm as Product>::part_files(&id("k3v9q2m8x4c1p")),
            PartFiles {
                files: vec!["items/k3/k3v9q2m8x4c1p.json".into()],
                folders: vec![]
            }
        );
        let p = ryupdm::props::read_for::<Plm>(&f, &id("k3v9q2m8x4c1p")).unwrap();
        assert_eq!(p.number(), Some("A-1"));
        assert_eq!(p.values["name"], "Bracket");
        assert_eq!(p.values["uom"], "kg");
        assert_eq!(p.values["finish"], "anodized");
        // Two live items with one number: ryupdm's own fault.
        let pdm = ryupdm::props::Pdm::of_for::<Plm>(
            &f,
            ryupdm::product::part_ids::<Plm>(&f).unwrap(),
            crate::units::Unit::Mm,
        );
        assert!(
            pdm.faults()
                .iter()
                .any(|e| e.code == crate::error::Code::PartNumberDuplicate)
        );
        let i = ryupdm::product::identity::<Plm>(&f).unwrap();
        assert_eq!(i.project, Some(crate::catalogue::tests::pid(1)));
    }

    #[test]
    fn provenance_never_changes_an_items_content() {
        let mut f = files();
        let at = "items/k3/k3v9q2m8x4c1p.json";
        let before = <Plm as Product>::content(&f, &id("k3v9q2m8x4c1p")).unwrap();
        let mut item = Item::read(&id("k3v9q2m8x4c1p"), &f[at]).unwrap();
        item.source
            .insert("erp".into(), json!({"key": "1", "digest": "ff"}));
        item.extra.insert("erp".into(), json!({"Col": "x"}));
        item.previous_numbers.push("OLD-1".into());
        f.insert(at.into(), item.to_text().into_bytes());
        assert_eq!(
            <Plm as Product>::content(&f, &id("k3v9q2m8x4c1p")).unwrap(),
            before
        );
        item.description = Some("changed".into());
        f.insert(at.into(), item.to_text().into_bytes());
        assert_ne!(
            <Plm as Product>::content(&f, &id("k3v9q2m8x4c1p")).unwrap(),
            before
        );
        assert_eq!(before.len(), 64);
    }
}
