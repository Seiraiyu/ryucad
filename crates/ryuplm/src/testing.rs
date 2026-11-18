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

/// A catalogue with structure from `seed`: [`catalogue`]'s items, with
/// own lines from item k to the next `k % 3` items (never back, so no
/// loop); every tenth item linked to ryucad assembly `a<k>`, whose lines
/// place screw `screw<k>` (no item links it: `bom_unlinked`) and
/// assembly `a<k+10>` (item k+10's), every thirtieth also naming item
/// k+10 by an own line (`bom_line_twice`); every fifteenth linked to
/// ryueda design `d<k>`, whose one line places component `c<k>`, which
/// item k+5 links. The representations' lines are held in memory.
pub fn structure(seed: &str, n: usize) -> (BTreeMap<String, Vec<u8>>, crate::reps::Fixed) {
    use crate::id::Id;
    use crate::item::{ItemRef, Line, Link};
    use crate::reps::RepLine;
    let mut f = catalogue(seed, n);
    let cat = crate::load::Catalogue::load(&f).expect("the seeded catalogue reads");
    let ids: Vec<Id> = cat.items.keys().cloned().collect();
    let mut items: Vec<Item> = ids.iter().map(|id| cat.items[id].clone()).collect();
    let (cad, eda) = (ProjectId::from_bytes([3; 16]), ProjectId::from_bytes([4; 16]));
    let part = |s: String| Id::parse(&s, "part").expect("a part ID");
    let mut reps = crate::reps::Fixed::default();
    let mut made = 0u64;
    for k in 0..ids.len() {
        let mut add: Vec<(usize, String)> = (1..=(k % 3))
            .filter(|d| k + d < ids.len())
            .map(|d| (k + d, format!("{}", 1 + k % 4)))
            .collect();
        if k % 30 == 0 && k + 10 < ids.len() {
            add.push((k + 10, "2".to_owned()));
        }
        for (child, quantity) in add {
            made += 1;
            let find = format!("{}", 10 * (items[k].bom.len() + 1));
            items[k].bom.push(Line {
                line: Id::parse(&crate::ids::text(made), "line").expect("a line ID"),
                item: ItemRef {
                    catalogue: None,
                    item: ids[child].clone(),
                },
                quantity,
                unit: None,
                find: Some(find),
                refdes: None,
                notes: None,
            });
        }
        if k % 10 == 0 {
            items[k].links.push(Link {
                product: "ryucad".into(),
                project: cad.clone(),
                part: part(format!("a{k}")),
            });
            let mut lines = vec![RepLine {
                key: format!("screw{k}"),
                project: Some(cad.clone()),
                part: part(format!("screw{k}")),
                quantity: 4,
                find: Some("1".into()),
                refdes: None,
            }];
            if k + 10 < ids.len() {
                lines.push(RepLine {
                    key: format!("a{}", k + 10),
                    project: Some(cad.clone()),
                    part: part(format!("a{}", k + 10)),
                    quantity: 2,
                    find: Some("2".into()),
                    refdes: None,
                });
            }
            reps.0
                .insert(("ryucad".into(), cad.clone(), part(format!("a{k}"))), lines);
        }
        if k % 15 == 0 {
            items[k].links.push(Link {
                product: "ryueda".into(),
                project: eda.clone(),
                part: part(format!("d{k}")),
            });
            reps.0.insert(
                ("ryueda".into(), eda.clone(), part(format!("d{k}"))),
                vec![RepLine {
                    key: format!("self/c{k}\u{0}10k\u{0}R0603"),
                    project: Some(eda.clone()),
                    part: part(format!("c{k}")),
                    quantity: 3,
                    find: None,
                    refdes: Some("R1,R2,R3".into()),
                }],
            );
            if k + 5 < ids.len() {
                items[k + 5].links.push(Link {
                    product: "ryueda".into(),
                    project: eda.clone(),
                    part: part(format!("c{k}")),
                });
                reps.0
                    .insert(("ryueda".into(), eda.clone(), part(format!("c{k}"))), Vec::new());
            }
        }
    }
    for (id, item) in ids.iter().zip(&items) {
        f.insert(crate::ids::item_path(id), item.to_text().into_bytes());
    }
    (f, reps)
}
