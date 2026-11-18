//! Links (design 734): an item names its representations,
//! `links: [{product, project, part}]`, at most one per product, and a
//! representation belongs to at most one item. The link lives on the
//! item; a representation needs no knowledge of ryuplm. What a write
//! refuses ([`judge`]) reads only the catalogue; what reads other folders
//! ([`resolve`]) is `check`'s.

use crate::error::{Code, Error, Result};
use crate::ids::item_path;
use crate::item::Link;
use crate::session::Session;
use crate::write::{Writes, commit};
use serde_json::Value;
use std::path::Path;
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

/// The project `project` names for `product`: a project ID, or the folder
/// of one (relative to `root`), whose manifest gives its ID.
fn project_of(root: &Path, product: &str, project: &str) -> Result<ProjectId> {
    if let Ok(p) = ProjectId::parse(project) {
        return Ok(p);
    }
    let manifest = reps::manifest_of(product).expect("product checked");
    let dir = root.join(project);
    reps::project_id(&crate::source::Folder(&dir), manifest).ok_or_else(|| {
        Error::new(
            Code::InvalidParam,
            format!(
                "{project:?} is neither a project ID nor the folder of a {product} project with one ({manifest} with a project)"
            ),
        )
        .with("param", "project")
    })
}

/// `item.link`: the item's link of `product` set to `project` and `part`
/// (replacing any it had), links in product order.
pub fn link(
    session: &mut Session,
    item: &str,
    product: &str,
    project: &str,
    part: &str,
    expected: Option<&str>,
) -> Result<Value> {
    if reps::manifest_of(product).is_none() {
        return Err(Error::new(
            Code::InvalidParam,
            format!("product is ryucad or ryueda, not {product:?}"),
        )
        .with("param", "product"));
    }
    let project = project_of(session.root(), product, project)?;
    let part = Id::parse(part, "part")?;
    let (id, mut it) = crate::items::current(session, item, expected)?;
    it.links.retain(|l| l.product != product);
    it.links.push(Link {
        product: product.to_owned(),
        project: project.clone(),
        part: part.clone(),
    });
    it.links.sort_by(|a, b| a.product.cmp(&b.product));
    let text = crate::items::text_of(&id, &it)?;
    commit(
        session,
        &format!("linking item {id} to {product} {project}:{part}"),
        Writes(vec![(item_path(&id), Some(text))]),
    )?;
    Ok(crate::items::shown(session, &id))
}

/// `item.unlink`: the item's link of `product` removed.
pub fn unlink(
    session: &mut Session,
    item: &str,
    product: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let (id, mut it) = crate::items::current(session, item, expected)?;
    let before = it.links.len();
    it.links.retain(|l| l.product != product);
    if it.links.len() == before {
        return Err(Error::new(
            Code::InvalidParam,
            format!("item {id} has no {product} link"),
        )
        .with("param", "product"));
    }
    let text = crate::items::text_of(&id, &it)?;
    commit(
        session,
        &format!("unlinking item {id} from {product}"),
        Writes(vec![(item_path(&id), Some(text))]),
    )?;
    Ok(crate::items::shown(session, &id))
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

    #[test]
    fn linking_replaces_the_products_link_and_unlinking_removes_it() {
        use crate::proto::Request;
        use crate::server::tests::{ask, catalogue};
        let (_d, root) = catalogue();
        crate::reps::tests::cad_project(&root.join("mech"), &pid(3), &["frame"], &[]);
        let made = ask(
            &root,
            Request::ItemCreate {
                fields: Default::default(),
                number: Some("A-1".into()),
                next: None,
                expected_revision: None,
            },
        )
        .unwrap();
        let id = made["item"]["id"].as_str().unwrap().to_owned();
        let link = |project: &str, part: &str| Request::ItemLink {
            item: "A-1".into(),
            product: "ryucad".into(),
            project: project.into(),
            part: part.into(),
            expected_revision: None,
        };
        // By the project's folder: its manifest gives the ID.
        let v = ask(&root, link(root.join("mech").to_str().unwrap(), "frame")).unwrap();
        assert_eq!(
            v["item"]["links"],
            serde_json::json!([{"product": "ryucad", "project": pid(3), "part": "frame"}])
        );
        // By ID, replacing the ryucad link.
        let v = ask(&root, link(pid(3).as_str(), "other")).unwrap();
        assert_eq!(v["item"]["links"][0]["part"], "other");
        assert_eq!(v["item"]["links"].as_array().unwrap().len(), 1);
        let bad = ask(
            &root,
            Request::ItemLink {
                item: id.clone(),
                product: "ryudfm".into(),
                project: pid(3).to_string(),
                part: "p".into(),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(bad.code, crate::error::Code::InvalidParam);
        let nowhere = ask(&root, link("no/such/folder", "frame")).unwrap_err();
        assert_eq!(nowhere.code, crate::error::Code::InvalidParam);
        let v = ask(
            &root,
            Request::ItemUnlink {
                item: id.clone(),
                product: "ryucad".into(),
                expected_revision: None,
            },
        )
        .unwrap();
        assert!(v["item"].get("links").is_none(), "{v}");
        let again = ask(
            &root,
            Request::ItemUnlink {
                item: id,
                product: "ryucad".into(),
                expected_revision: None,
            },
        )
        .unwrap_err();
        assert_eq!(again.code, crate::error::Code::InvalidParam);
    }
}
