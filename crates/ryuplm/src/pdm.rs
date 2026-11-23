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
pub const FOLDERS: [&str; 4] = [
    ITEMS,
    crate::ids::DOCUMENTS,
    RELEASES,
    crate::import::IMPORTS,
];

impl ryu_git::Layout for Plm {
    const NAME: &'static str = crate::catalogue::NAME;
    const MANIFEST: &'static str = MANIFEST;
    const ROOT_FILES: &'static [&'static str] = &ROOT_FILES;
    const FOLDERS: &'static [&'static str] = &FOLDERS;
    fn project_file(rel: &str) -> bool {
        ROOT_FILES.contains(&rel)
            || item_of(rel).is_some()
            || crate::ids::document_of(rel).is_some()
            || crate::ids::bytes_of(rel).is_some()
            || rel
                .strip_prefix("releases/")
                .is_some_and(|f| f.matches('/').count() == 1)
            || crate::import::name_of(rel).is_some()
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

/// A document's fields ryupdm reads as properties: `number`, its title as
/// `name`, and `description`.
pub fn document_properties(d: &crate::document::Document) -> Properties {
    let mut values = std::collections::BTreeMap::new();
    values.insert("number".into(), Value::from(d.number.clone()));
    values.insert("name".into(), Value::from(d.title.clone()));
    if let Some(x) = &d.description {
        values.insert("description".into(), Value::from(x.clone()));
    }
    Properties { values }
}

/// The content a document's release names (`ryuplm.document.1`): its
/// record without `storage`, `pin` and `previous_numbers`. The bytes are in
/// it through `file.blake3`, so it never reads them.
pub fn document_content(d: &crate::document::Document) -> String {
    let mut v = d.to_json();
    let o = v.as_object_mut().expect("an object");
    for k in ["storage", "pin", "previous_numbers"] {
        o.remove(k);
    }
    let body = crate::canonical::to_string(&v);
    let mut h = blake3::Hasher::new();
    for piece in [b"ryuplm.document.1".as_slice(), body.as_bytes()] {
        h.update(&(piece.len() as u64).to_le_bytes());
        h.update(piece);
    }
    h.finalize().to_hex().to_string()
}

impl Product for Plm {
    const PART_FOLDERS: &'static [&'static str] = &[ITEMS, crate::ids::DOCUMENTS];

    fn parts(src: &dyn Source) -> Result<Vec<(Id, PartKind)>> {
        let mut out = Vec::new();
        for (top, kind, of) in [
            (ITEMS, PartKind::Item, item_of as fn(&str) -> Option<Id>),
            (
                crate::ids::DOCUMENTS,
                PartKind::Document,
                crate::ids::document_of,
            ),
        ] {
            for (shard, k) in src.list(top)? {
                if k != crate::source::Kind::Dir {
                    continue;
                }
                for (name, kind_of) in src.list(&format!("{top}/{shard}"))? {
                    if kind_of == crate::source::Kind::File
                        && let Some(id) = of(&format!("{top}/{shard}/{name}"))
                    {
                        out.push((id, kind));
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Where `part`'s properties are when the snapshot has no file of it:
    /// the item's file (as before documents).
    fn properties_path(part: &Id) -> String {
        item_path(part)
    }

    /// Items and documents share one ID space: the file that is there says
    /// which kind `part` is.
    fn properties_path_in(src: &dyn Source, part: &Id) -> String {
        let doc = crate::ids::document_path(part);
        match (src.read_shared(&item_path(part)), src.read_shared(&doc)) {
            (Ok(None), Ok(Some(_))) => doc,
            _ => item_path(part),
        }
    }

    fn part_files(part: &Id) -> PartFiles {
        PartFiles {
            files: vec![item_path(part), crate::ids::document_path(part)],
            folders: vec![crate::ids::bytes_dir(part)],
        }
    }

    fn part_of(path: &str) -> Option<Id> {
        item_of(path)
            .or_else(|| crate::ids::document_of(path))
            .or_else(|| crate::ids::bytes_of(path).map(|(id, _)| id))
    }

    fn content(src: &dyn Source, part: &Id) -> Result<String> {
        let at = item_path(part);
        if let Some(bytes) = src.read(&at)? {
            return Ok(content_of(&Item::read(part, &bytes)?));
        }
        let doc = crate::ids::document_path(part);
        match src.read(&doc)? {
            Some(bytes) => Ok(document_content(&crate::document::Document::read(
                part, &bytes,
            )?)),
            None => Err(Error::new(Code::ItemNotFound, format!("no {at} or {doc}"))
                .with("path", &at)
                .with("document_path", &doc)),
        }
    }

    fn properties_from(v: &Value) -> Option<Result<Properties>> {
        if v["kind"] == crate::item::KIND {
            return Some(Item::from_json(v).map(|i| properties(&i)));
        }
        (v["kind"] == crate::document::KIND)
            .then(|| crate::document::Document::from_json(v).map(|d| document_properties(&d)))
    }

    /// The manifest (classes, the default unit) may change how any item
    /// is judged.
    fn shared(path: &str) -> bool {
        path == MANIFEST
    }

    fn identity(manifest: &Value) -> Option<Identity> {
        let m = crate::catalogue::Manifest::from_json(manifest).ok()?;
        Some(Identity {
            project: Some(m.project),
            forked_from: m.forked_from.map(|f| f.project),
            unit: m.default_unit,
        })
    }

    /// An item whose legacy history carried a label of the scheme
    /// continues it (design 935); any other starts at the scheme's first.
    fn first_label(src: &dyn Source, part: &Id, scheme: ryupdm::config::Scheme) -> String {
        src.read(&item_path(part))
            .ok()
            .flatten()
            .and_then(|b| Item::read(part, &b).ok())
            .and_then(|i| {
                crate::import::history::carried(&i).and_then(|c| ryupdm::label::next(scheme, c))
            })
            .unwrap_or_else(|| ryupdm::label::first(scheme))
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
            // One ID space (design 937): either kind's file, and a
            // document's bytes folder.
            PartFiles {
                files: vec![
                    "items/k3/k3v9q2m8x4c1p.json".into(),
                    "documents/k3/k3v9q2m8x4c1p.json".into()
                ],
                folders: vec!["documents/k3/k3v9q2m8x4c1p".into()]
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
    #[test]
    fn documents_are_parts_beside_items() {
        let mut f = files();
        let d = id("d1000000000000");
        let mut doc = crate::document::Document::new(
            "DOC-1",
            "Spec",
            crate::document::File::of("Spec.pdf", b"%PDF", None).unwrap(),
        );
        doc.description = Some("Finish".into());
        f.insert(crate::ids::document_path(&d), doc.to_text().into_bytes());
        f.insert(crate::ids::bytes_path(&d, "Spec.pdf"), b"%PDF".to_vec());
        let parts = <Plm as Product>::parts(&f).unwrap();
        assert!(parts.contains(&(d.clone(), PartKind::Document)));
        assert!(parts.iter().any(|(_, k)| *k == PartKind::Item));
        assert_eq!(
            <Plm as Product>::properties_path_in(&f, &d),
            "documents/d1/d1000000000000.json"
        );
        let props = ryupdm::props::read_for::<Plm>(&f, &d).unwrap();
        assert_eq!(props.values["number"], "DOC-1");
        assert_eq!(props.values["name"], "Spec");
        assert_eq!(props.values["description"], "Finish");
        // Moving tier, re-pinning and renumbering history keep the content.
        let c = <Plm as Product>::content(&f, &d).unwrap();
        doc.storage = crate::document::Storage::Service;
        doc.pin = Some(crate::document::Pin {
            drive_id: "x".into(),
            revision: "y".into(),
        });
        doc.previous_numbers.push("OLD".into());
        assert_eq!(document_content(&doc), c);
        doc.file.blake3 = "b".repeat(64);
        assert_ne!(document_content(&doc), c, "new bytes are a new revision");
        assert_eq!(
            <Plm as Product>::part_of("documents/d1/d1000000000000/Spec.pdf"),
            Some(d.clone())
        );
        assert_eq!(
            <Plm as Product>::part_of("documents/d1/d1000000000000.json"),
            Some(d.clone())
        );
        assert!(<Plm as ryu_git::Layout>::project_file(
            "documents/d1/d1000000000000/Spec.pdf"
        ));
        let own = <Plm as Product>::part_files(&d);
        assert!(
            own.files
                .contains(&"documents/d1/d1000000000000.json".to_owned())
        );
        assert_eq!(own.folders, ["documents/d1/d1000000000000"]);
    }
}
