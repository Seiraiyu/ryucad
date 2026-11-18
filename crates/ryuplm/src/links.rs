//! Links (design 734): an item names its representations,
//! `links: [{product, project, part}]`, at most one per product, and a
//! representation belongs to at most one item. The link lives on the
//! item; a representation needs no knowledge of ryuplm. What a write
//! refuses ([`judge`]) reads only the catalogue; what reads other folders
//! ([`resolve`]) is `check`'s.

use crate::error::{Code, Error};
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
}
