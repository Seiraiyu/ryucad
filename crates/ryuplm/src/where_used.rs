//! Where-used (design 734): every item whose own lines, or whose
//! representations' lines, name an item; in this catalogue, in other
//! catalogues' checkouts (`--in`, the `pdm.dependents` preference, the
//! work tree) and in their official releases.

use crate::id::{Id, ProjectId};
use crate::item::Item;
use crate::links::Key;
use crate::load::Catalogue;
use crate::reps::{At, Reps};
use serde_json::{Value, json};

/// The item asked about: its catalogue, ID and representations.
pub struct Target {
    pub catalogue: ProjectId,
    pub item: Id,
    pub links: Vec<Key>,
}

/// Item `id` of `cat` as a target.
pub fn target(cat: &Catalogue, id: &Id) -> Target {
    Target {
        catalogue: cat.manifest.project.clone(),
        item: id.clone(),
        links: cat.items[id]
            .links
            .iter()
            .map(|l| (l.product.clone(), l.project.clone(), l.part.clone()))
            .collect(),
    }
}

/// The rows of `items` (of the catalogue `own`) naming `t`: by an own
/// line (`{item, number, via: "own", line, quantity}`) or by a line of
/// one of their representations read at `at` (`{item, number, via:
/// <product>, key, quantity, rev}`). Representations are read only when
/// `t` has any; one that can't be read is a problem, not a failure.
pub fn rows<'a>(
    own: &ProjectId,
    items: impl Iterator<Item = (&'a Id, &'a Item)>,
    t: &Target,
    reps: &mut dyn Reps,
    at: &At,
    problems: &mut Vec<Value>,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (id, item) in items {
        for l in &item.bom {
            let p = l.item.catalogue.as_ref().unwrap_or(own);
            if *p == t.catalogue && l.item.item == t.item {
                out.push(json!({"item": id, "number": item.number, "via": "own",
                    "line": l.line, "quantity": l.quantity}));
            }
        }
        if t.links.is_empty() {
            continue;
        }
        for link in &item.links {
            match reps.lines(link, at) {
                Err(e) => problems.push(json!(e.with("item", id))),
                Ok(read) => {
                    for rl in &read.lines {
                        let names_t = rl.project.as_ref().is_some_and(|p| {
                            t.links
                                .iter()
                                .any(|(pr, pj, pt)| *pr == link.product && pj == p && *pt == rl.part)
                        });
                        if names_t {
                            out.push(json!({"item": id, "number": item.number, "via": link.product,
                                "key": rl.key, "quantity": rl.quantity.to_string(), "rev": read.rev}));
                        }
                    }
                }
            }
        }
    }
    out
}

/// [`rows`] over every item of `cat`.
pub fn rows_in(
    cat: &Catalogue,
    t: &Target,
    reps: &mut dyn Reps,
    at: &At,
    problems: &mut Vec<Value>,
) -> Vec<Value> {
    rows(&cat.manifest.project, cat.items.iter(), t, reps, at, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bom::tests::{id, pcba};

    #[test]
    fn an_item_is_used_by_own_lines_and_by_representations() {
        let (f, mut reps) = pcba();
        let cat = Catalogue::load(&f).unwrap();
        let screw = target(&cat, &id("0000000000002"));
        let mut problems = Vec::new();
        let rows = rows_in(&cat, &screw, &mut reps, &At::Working, &mut problems);
        let got: Vec<(String, String, String)> = rows
            .iter()
            .map(|r| {
                (
                    r["item"].as_str().unwrap().to_owned(),
                    r["via"].as_str().unwrap().to_owned(),
                    r["quantity"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("0000000000001".into(), "own".into(), "4".into()),
                ("0000000000001".into(), "ryucad".into(), "4".into()),
            ]
        );
        assert_eq!(problems, Vec::<Value>::new());
        // The glue has no links: only own lines are searched.
        let glue = target(&cat, &id("0000000000004"));
        assert_eq!(rows_in(&cat, &glue, &mut reps, &At::Working, &mut problems).len(), 1);
        // Another catalogue names this one's items by project and ID.
        let mut other = cat.clone();
        other.manifest.project = crate::id::ProjectId::from_bytes([8; 16]);
        for it in other.items.values_mut() {
            for l in &mut it.bom {
                l.item.catalogue = Some(cat.manifest.project.clone());
            }
        }
        let rows = rows_in(&other, &screw, &mut reps, &At::Working, &mut problems);
        assert_eq!(rows.iter().filter(|r| r["via"] == "own").count(), 1);
    }
}
