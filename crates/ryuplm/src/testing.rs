//! Seeded catalogues for tests, the golden and the fuzz target.

use crate::catalogue::{Class, MANIFEST, Manifest};
use crate::id::ProjectId;
use crate::item::Item;
use crate::units::Unit;
use serde_json::json;
use std::collections::BTreeMap;

/// A catalogue of `n` items from `seed`: three classes, numbers in two
/// schemes and a legacy form (some duplicated, some unschemed), AMLs,
/// declared properties, own lines naming other items.
pub fn catalogue(seed: &str, n: usize) -> BTreeMap<String, Vec<u8>> {
    let mut f = BTreeMap::new();
    let mut m = Manifest::new(ProjectId::from_bytes([7; 16]), Unit::Mm);
    m.classes.insert(
        "fastener".into(),
        Class {
            properties: Some(vec!["thread".into()]),
        },
    );
    m.classes
        .insert("machined".into(), Class { properties: None });
    m.classes.insert("label".into(), Class::default());
    f.insert(MANIFEST.to_owned(), m.to_text().into_bytes());
    let config = json!({"format": 5, "kind": "ryupdm.config",
        "part_number": {"schemes": [
            {"name": "mech", "class": "machined", "pattern": "MECH-\\d{5}", "next": "MECH-{seq:5}"},
            {"name": "fast", "class": "fastener", "pattern": "F\\d{4}", "next": "F{seq:4}"},
            {"name": "legacy", "pattern": "\\d{3}-\\d{4}"}]},
        "properties": {"thread": {"type": "string"}, "finish": {"type": "string"}}});
    f.insert(
        "ryupdm.json".to_owned(),
        crate::canonical::to_string(&config).into_bytes(),
    );
    let mut ids = Vec::new();
    for k in 0..n {
        let id = crate::ids::random_from(Some(seed), |s| {
            ids.iter().any(|i: &crate::id::Id| i.as_str() == s)
        })
        .unwrap();
        ids.push(id.clone());
        let class = ["fastener", "machined", "label"][k % 3];
        let number = match k % 7 {
            0 => format!("MECH-{:05}", k),
            1 => format!("F{:04}", k),
            2 => format!("{:03}-{:04}", k % 1000, k),
            3 => format!("Legacy {k}/b"),
            // Thirteen numbers shared out: duplicates.
            _ => format!("{:03}-{:04}", k % 13, k % 13),
        };
        let mut item = Item::new(&number);
        item.class = Some(class.into());
        item.name = Some(format!("Item {k}"));
        if class == "fastener" {
            item.properties
                .insert("thread".into(), json!(format!("M{}", 2 + k % 4)));
        }
        if k % 5 == 0 {
            item.aml.push(json!({"manufacturer": format!("Maker {}", k % 4), "mpn": format!("P{k}"), "status": "active"}));
        }
        f.insert(crate::ids::item_path(&id), item.to_text().into_bytes());
    }
    f
}
