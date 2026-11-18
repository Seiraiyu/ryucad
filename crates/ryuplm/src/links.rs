//! Links (design 734): an item names its representations,
//! `links: [{product, project, part}]`, at most one per product, and a
//! representation belongs to at most one item. The link lives on the
//! item; a representation needs no knowledge of ryuplm. What a write
//! refuses ([`judge`]) reads only the catalogue; what reads other folders
//! ([`resolve`]) is `check`'s.

use crate::error::{Code, Error};
use crate::fault::{Fault, Severity};
use crate::reps::{self, Index};
use crate::id::{Id, ProjectId};
use crate::load::Catalogue;
use std::collections::BTreeMap;

/// What a link names: product, project and part.
pub type Key = (String, ProjectId, Id);

/// Every representation the catalogue's items link, with the item
/// linking it (the first in ID order, when two do).
pub fn linked(cat: &Catalogue) -> BTreeMap<Key, Id> {
    let mut out = BTreeMap::new();
    for (id, item) in &cat.items {
        for l in &item.links {
            out.entry((l.product.clone(), l.project.clone(), l.part.clone()))
                .or_insert_with(|| id.clone());
        }
    }
    out
}

/// `item_link_duplicate`, errors: a second link of one product on an
/// item, and a link to a representation an item earlier in ID order
/// links, each at its link's pointer.
pub fn judge(cat: &Catalogue) -> Vec<Error> {
    let mut out = Vec::new();
    let mut first: BTreeMap<Key, Id> = BTreeMap::new();
    for (id, item) in &cat.items {
        let mut products: Vec<&str> = Vec::new();
        for (i, l) in item.links.iter().enumerate() {
            let at = format!("/links/{i}");
            if products.contains(&l.product.as_str()) {
                out.push(
                    Error::new(
                        Code::ItemLinkDuplicate,
                        format!(
                            "item {id} links two {} representations: an item has at most one per product",
                            l.product
                        ),
                    )
                    .with("item", id)
                    .with("pointer", &at)
                    .with("product", &l.product),
                );
                continue;
            }
            products.push(&l.product);
            let k = (l.product.clone(), l.project.clone(), l.part.clone());
            match first.get(&k) {
                Some(other) => out.push(
                    Error::new(
                        Code::ItemLinkDuplicate,
                        format!(
                            "item {id} links {} {}:{}, which item {other} links: a representation's BOM belongs to one item",
                            l.product, l.project, l.part
                        ),
                    )
                    .with("item", id)
                    .with("pointer", &at)
                    .with("product", &l.product)
                    .with("other", other),
                ),
                None => {
                    first.insert(k, id.clone());
                }
            }
        }
    }
    out
}

/// Links followed on disk, all warnings: `item_link_unresolved` for a
/// project no folder searched holds or a part it hasn't, and
/// `item_number_mismatch` for a representation whose `number` property
/// isn't its item's. A link that can't be followed never fails anything
/// (design 734): a catalogue cloned without its CAD repos still checks.
pub fn resolve(cat: &Catalogue, index: &Index) -> Vec<Fault> {
    let mut out = Vec::new();
    for (id, item) in &cat.items {
        for (i, l) in item.links.iter().enumerate() {
            let at = format!("/links/{i}");
            let warn = |e: Error| {
                Fault::of(
                    Severity::Warning,
                    e.with("item", id)
                        .with("pointer", &at)
                        .with("product", &l.product)
                        .with("project", &l.project)
                        .with("part", &l.part),
                )
            };
            let Some(dir) = index.dir(&l.product, &l.project) else {
                out.push(warn(
                    Error::new(
                        Code::ItemLinkUnresolved,
                        format!(
                            "item {id}: no folder searched holds {} project {}",
                            l.product, l.project
                        ),
                    )
                    .with("searched", &index.searched),
                ));
                continue;
            };
            let src = crate::source::Folder(dir);
            if !reps::part_exists(&src, &l.product, &l.part) {
                out.push(warn(
                    Error::new(
                        Code::ItemLinkUnresolved,
                        format!(
                            "item {id}: {} project {} in {} has no part {}",
                            l.product,
                            l.project,
                            dir.display(),
                            l.part
                        ),
                    )
                    .with("folder", dir.display().to_string()),
                ));
                continue;
            }
            if let Some(n) = reps::number_of(&src, &l.product, &l.part)
                && n != item.number
            {
                out.push(warn(
                    Error::new(
                        Code::ItemNumberMismatch,
                        format!(
                            "item {id} is {:?}, but its {} representation {} says {n:?}",
                            item.number, l.product, l.part
                        ),
                    )
                    .with("number", &n),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::catalogue::tests::pid;
    use crate::id::Id;
    use crate::item::{Item, Link};
    use crate::judge::tests::{codes, files};

    pub(crate) fn link(product: &str, project: u8, part: &str) -> Link {
        Link {
            product: product.into(),
            project: pid(project),
            part: Id::parse(part, "part").unwrap(),
        }
    }

    #[test]
    fn one_link_per_product_and_one_item_per_representation() {
        let mut a = Item::new("A-1");
        a.links = vec![link("ryucad", 3, "frame"), link("ryueda", 4, "main")];
        let mut b = Item::new("A-2");
        // Two ryucad links on one item.
        b.links = vec![link("ryucad", 3, "plate"), link("ryucad", 3, "bolt")];
        let mut c = Item::new("A-3");
        // The representation item 1 links.
        c.links = vec![link("ryucad", 3, "frame")];
        let f = files(
            &[],
            None,
            &[("0000000000001", a), ("0000000000002", b), ("0000000000003", c)],
        );
        assert_eq!(
            codes(&f),
            [
                ("item_link_duplicate".into(), "error", "0000000000002".into()),
                ("item_link_duplicate".into(), "error", "0000000000003".into()),
            ]
        );
    }

    #[test]
    fn links_are_followed_on_disk_and_never_fail() {
        use crate::reps::tests::cad_project;
        let d = tempfile::tempdir().unwrap();
        let top = d.path().canonicalize().unwrap();
        cad_project(&top.join("mech"), &pid(3), &["frame", "bolt"], &[]);
        std::fs::write(
            top.join("mech/parts/frame/properties.json"),
            r#"{"format":1,"kind":"ryupdm.properties","number":"OLD-1"}"#,
        )
        .unwrap();
        let mut a = Item::new("A-1");
        a.links = vec![link("ryucad", 3, "frame")]; // number differs
        let mut b = Item::new("A-2");
        b.links = vec![link("ryucad", 3, "nut")]; // no such part
        let mut c = Item::new("A-3");
        c.links = vec![link("ryueda", 9, "main")]; // no such project
        let mut e = Item::new("A-4");
        e.links = vec![link("ryucad", 3, "bolt")]; // fine: no number set
        let f = files(
            &[],
            None,
            &[
                ("0000000000001", a),
                ("0000000000002", b),
                ("0000000000003", c),
                ("0000000000004", e),
            ],
        );
        let cat = crate::load::Catalogue::load(&f).unwrap();
        let ix = crate::reps::Index::scan(&[top.clone()]);
        let got: Vec<(String, bool, String)> = super::resolve(&cat, &ix)
            .iter()
            .map(|x| {
                (
                    serde_json::to_value(x.error.code).unwrap().as_str().unwrap().to_owned(),
                    x.is_error(),
                    x.error.details["item"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("item_number_mismatch".into(), false, "0000000000001".into()),
                ("item_link_unresolved".into(), false, "0000000000002".into()),
                ("item_link_unresolved".into(), false, "0000000000003".into()),
            ]
        );
    }
}
