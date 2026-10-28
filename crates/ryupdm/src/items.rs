//! Item numbers: the find numbers an assembly's balloons cite, kept in
//! its `part.json` (`items`, by placed part). A number labels a row of
//! one assembly's BOM and is never an identity: a part takes the next
//! number when it first enters the assembly ([`assigned`]), loses it when
//! its last instance leaves, and keeps it otherwise, so a reorder or
//! another part's removal moves no number. Only a request changes one
//! ([`renumbered`], [`compacted`]).

use ryucad_doc::doc::files::{Feature, Items, MAX_ITEM, PartKind};
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::Part;
use ryucad_doc::doc::source::{Kind, Source};
use ryucad_doc::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The parts the features `order` lists place, each once, in the order
/// their instances first place them; `feature` gives a listed feature, or
/// `None` when it can't be read. Instance patterns copy instances, so they
/// add none.
pub fn placed_by<'a>(order: &[Id], feature: impl Fn(&Id) -> Option<&'a Feature>) -> Vec<Id> {
    let mut out: Vec<Id> = Vec::new();
    for fid in order {
        if let Some(Feature::Instance(i)) = feature(fid)
            && !out.contains(&i.part)
        {
            out.push(i.part.clone());
        }
    }
    out
}

/// The parts `part` places ([`placed_by`] over its listed features).
pub fn placed(part: &Part) -> Vec<Id> {
    placed_by(part.order(), |f| part.features.get(f)?.as_ref().ok())
}

/// The number the next part to enter takes: one past the largest, else
/// (past [`MAX_ITEM`]) the smallest free one.
pub fn next(items: &Items) -> u32 {
    let top = items.values().copied().max().unwrap_or(0);
    if top < MAX_ITEM {
        return top + 1;
    }
    let used: BTreeSet<u32> = items.values().copied().collect();
    (1..=MAX_ITEM)
        .find(|n| !used.contains(n))
        .unwrap_or(MAX_ITEM)
}

/// `items` after a write that changed what an assembly places from
/// `before` to `after`: each part only `after` places takes the next
/// number, in `after`'s order (unless it still has one), counted before
/// anything leaves, so a part replacing another in one write never takes
/// its number; then, when `whole` (every feature listed after the write
/// reads), each part only `before` placed loses its entry: a listed
/// feature that doesn't read may still place it, as [`faults`] judges
/// unused numbers. Nothing else changes.
pub fn assigned(items: &Items, before: &[Id], after: &[Id], whole: bool) -> Items {
    let mut out = items.clone();
    for new in after.iter().filter(|p| !before.contains(p)) {
        if !out.contains_key(new) {
            let n = next(&out);
            out.insert(new.clone(), n);
        }
    }
    if whole {
        for gone in before.iter().filter(|p| !after.contains(p)) {
            out.remove(gone);
        }
    }
    out
}

fn names(parts: &[&Id]) -> String {
    parts
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What is wrong with assembly `id`'s item numbers (`part` its files):
/// placed parts with none (`bom_item_missing`), a number shared by parts
/// (`bom_item_duplicate`, one error per number, its parts in list order),
/// and numbers of parts it no longer places (`bom_item_unused`, judged
/// only when every listed feature reads, since one that doesn't may place
/// anything). Each error's details name the assembly (`part`), the parts
/// (`parts`) and, for a shared number, the number (`item`). Nothing for a
/// part that isn't an assembly or whose list can't be read.
pub fn faults(id: &Id, part: &Part) -> Vec<Error> {
    let Ok(file) = &part.file else {
        return Vec::new();
    };
    if file.kind != PartKind::Assembly {
        return Vec::new();
    }
    let placed = placed(part);
    let mut out = Vec::new();
    let missing: Vec<&Id> = placed
        .iter()
        .filter(|p| !file.items.contains_key(*p))
        .collect();
    if !missing.is_empty() {
        out.push(
            Error::new(
                Code::BomItemMissing,
                format!(
                    "{id} places {} with no item number: give them numbers (`ryucad bom renumber`, or doctor's assign_items)",
                    names(&missing)
                ),
            )
            .with("part", id)
            .with("parts", &missing),
        );
    }
    let place = |p: &Id| placed.iter().position(|q| q == p).unwrap_or(usize::MAX);
    let mut by: BTreeMap<u32, Vec<&Id>> = BTreeMap::new();
    for (p, n) in &file.items {
        by.entry(*n).or_default().push(p);
    }
    for (n, mut parts) in by.into_iter().filter(|(_, ps)| ps.len() > 1) {
        parts.sort_by_key(|p| place(p));
        out.push(
            Error::new(
                Code::BomItemDuplicate,
                format!(
                    "{id} gives item {n} to {}: an item number names one part",
                    names(&parts)
                ),
            )
            .with("part", id)
            .with("item", n)
            .with("parts", &parts),
        );
    }
    let whole = part
        .order()
        .iter()
        .all(|f| matches!(part.features.get(f), Some(Ok(_))));
    let unused: Vec<&Id> = file.items.keys().filter(|p| !placed.contains(p)).collect();
    if whole && !unused.is_empty() {
        out.push(
            Error::new(
                Code::BomItemUnused,
                format!("{id} numbers {}, which it no longer places", names(&unused)),
            )
            .with("part", id)
            .with("parts", &unused),
        );
    }
    out
}

/// `items` with every part of `placed` that has none given the next
/// number, in list order: doctor's `assign_items`.
pub fn with_missing(items: &Items, placed: &[Id]) -> Items {
    let mut out = items.clone();
    for p in placed {
        if !out.contains_key(p) {
            let n = next(&out);
            out.insert(p.clone(), n);
        }
    }
    out
}

/// `items` with each shared number kept by the first of its parts in list
/// order (`placed`, then parts not placed by ID) and every later one given
/// the next number: doctor's `renumber_items`.
pub fn deduplicated(items: &Items, placed: &[Id]) -> Items {
    let mut out = items.clone();
    let mut seen = BTreeSet::new();
    let order = placed
        .iter()
        .chain(items.keys().filter(|p| !placed.contains(p)));
    for p in order {
        let Some(n) = items.get(p) else {
            continue;
        };
        if !seen.insert(*n) {
            let m = next(&out);
            out.insert(p.clone(), m);
        }
    }
    out
}

/// `items` without the numbers of parts `placed` doesn't hold: doctor's
/// `remove`.
pub fn without_unused(items: &Items, placed: &[Id]) -> Items {
    items
        .iter()
        .filter(|(p, _)| placed.contains(p))
        .map(|(p, n)| (p.clone(), *n))
        .collect()
}

/// `items` with each part of `sets` given its number: `not_found` for a
/// part the assembly doesn't place (`placed`), `invalid_param` for a
/// number outside 1 to [`MAX_ITEM`], and `bom_item_duplicate` when a part
/// set would share its number with another. Numbers are set together, so
/// two parts may swap theirs in one request.
pub fn renumbered(items: &Items, placed: &[Id], sets: &[(Id, u64)]) -> Result<Items> {
    let mut out = items.clone();
    for (p, n) in sets {
        if !placed.contains(p) {
            return Err(Error::new(
                Code::NotFound,
                format!("the assembly doesn't place {p}, so it has no item to number"),
            )
            .with("part", p));
        }
        let n = u32::try_from(*n)
            .ok()
            .filter(|n| (1..=MAX_ITEM).contains(n))
            .ok_or_else(|| {
                Error::new(
                    Code::InvalidParam,
                    format!(
                        "item {n} for {p}: an item number is a whole number from 1 to {MAX_ITEM}"
                    ),
                )
                .with("param", "items")
                .with("part", p)
            })?;
        out.insert(p.clone(), n);
    }
    for (p, _) in sets {
        let n = out[p];
        let mut parts: Vec<&Id> = out
            .iter()
            .filter(|(_, m)| **m == n)
            .map(|(q, _)| q)
            .collect();
        if parts.len() > 1 {
            parts.sort_by_key(|q| placed.iter().position(|x| x == *q));
            return Err(Error::new(
                Code::BomItemDuplicate,
                format!(
                    "item {n} would name {}: an item number names one part",
                    names(&parts)
                ),
            )
            .with("item", n)
            .with("parts", &parts));
        }
    }
    Ok(out)
}

/// The parts `placed` holds numbered 1 to n in their current order: by
/// number, then those without one in list order. Numbers of parts no
/// longer placed go.
pub fn compacted(items: &Items, placed: &[Id]) -> Items {
    let mut order: Vec<(usize, &Id)> = placed.iter().enumerate().collect();
    order.sort_by_key(|(i, p)| (items.get(*p).is_none(), items.get(*p).copied(), *i));
    order
        .into_iter()
        .zip(1..)
        .map(|((_, p), n)| (p.clone(), n))
        .collect()
}

/// An item number a merge made shared: each side gave it to a different
/// part of one assembly, and the merge, taking items per key, kept both.
#[derive(Clone, Debug, PartialEq)]
pub struct Collision {
    pub assembly: Id,
    pub item: u32,
    /// The parts sharing it, by ID.
    pub parts: Vec<Id>,
    /// Theirs' part: the one the repair renumbers.
    pub theirs: Id,
    /// The number the repair gives it.
    pub to: u32,
    /// The assembly's `part.json` repaired: every collision in it
    /// settled, so its repairs agree whichever is taken.
    pub repaired: Value,
}

/// An assembly's `part.json` as JSON, and its items, read as they are
/// (a merge's file may hold a conflict record).
fn read_items(src: &dyn Source, path: &str) -> Option<(Value, BTreeMap<String, u64>)> {
    let bytes = src.read(path).ok()??;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let items = v["items"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, n)| Some((k.clone(), n.as_u64()?)))
                .collect()
        })
        .unwrap_or_default();
    Some((v, items))
}

/// The item numbers the merged files `merged` share between parts that
/// neither `ours` nor `theirs` shared: in each, theirs' part (the one
/// theirs numbered so and ours didn't) takes the next free number.
pub fn collisions(ours: &dyn Source, theirs: &dyn Source, merged: &dyn Source) -> Vec<Collision> {
    let mut out = Vec::new();
    for (name, kind) in merged.list("parts").unwrap_or_default() {
        let Ok(assembly) = Id::parse(&name, "part") else {
            continue;
        };
        if kind != Kind::Dir {
            continue;
        }
        let path = format!("parts/{name}/part.json");
        let Some((mut file, items)) = read_items(merged, &path) else {
            continue;
        };
        let side = |src: &dyn Source| read_items(src, &path).map(|(_, i)| i).unwrap_or_default();
        let (o, t) = (side(ours), side(theirs));
        let mut by: BTreeMap<u64, Vec<&String>> = BTreeMap::new();
        for (p, n) in &items {
            by.entry(*n).or_default().push(p);
        }
        let shared =
            |side: &BTreeMap<String, u64>, n: u64| side.values().filter(|m| **m == n).count() > 1;
        let mut found = Vec::new();
        let mut top = items.values().copied().max().unwrap_or(0);
        for (n, parts) in by {
            if parts.len() < 2 || shared(&o, n) || shared(&t, n) {
                continue;
            }
            let Some(their) = parts
                .iter()
                .rev()
                .find(|p| t.get(**p) == Some(&n) && o.get(**p) != Some(&n))
            else {
                continue;
            };
            top += 1;
            file["items"][their.as_str()] = json!(top);
            let ids: Vec<Id> = parts
                .iter()
                .filter_map(|p| Id::parse(p, "part").ok())
                .collect();
            let (Ok(their), Ok(n), Ok(to)) = (
                Id::parse(their, "part"),
                u32::try_from(n),
                u32::try_from(top),
            ) else {
                continue;
            };
            found.push((n, ids, their, to));
        }
        for (item, parts, their, to) in found {
            out.push(Collision {
                assembly: assembly.clone(),
                item,
                parts,
                theirs: their,
                to,
                repaired: file.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::id;
    use crate::structure::tests::{block, files, instance};
    use ryucad_doc::doc::canonical;
    use ryucad_doc::doc::project::Project;
    use serde_json::json;
    use std::path::Path;

    fn items(pairs: &[(&str, u32)]) -> Items {
        pairs.iter().map(|(p, n)| (id(p), *n)).collect()
    }

    fn ids(list: &[&str]) -> Vec<Id> {
        list.iter().map(|p| id(p)).collect()
    }

    #[test]
    fn a_part_takes_the_next_number_when_it_enters_and_loses_it_when_it_leaves() {
        let now = items(&[("plate", 1), ("bolt", 2)]);
        // A nut takes the bolt's place in one write: 3, never the bolt's 2;
        // the plate keeps 1.
        let after = assigned(
            &now,
            &ids(&["plate", "bolt"]),
            &ids(&["nut", "plate"]),
            true,
        );
        assert_eq!(after, items(&[("plate", 1), ("nut", 3)]));
        // A reorder moves nothing.
        assert_eq!(
            assigned(
                &now,
                &ids(&["plate", "bolt"]),
                &ids(&["bolt", "plate"]),
                true
            ),
            now
        );
        // A part with a number it never lost keeps it.
        let stale = items(&[("pin", 7)]);
        assert_eq!(assigned(&stale, &[], &ids(&["pin"]), true), stale);
        // While a listed feature doesn't read, it may still place the bolt:
        // the nut enters, the bolt keeps 2.
        assert_eq!(
            assigned(
                &now,
                &ids(&["plate", "bolt"]),
                &ids(&["nut", "plate"]),
                false
            ),
            items(&[("plate", 1), ("bolt", 2), ("nut", 3)])
        );
        // Past the largest number, the smallest free one.
        assert_eq!(next(&items(&[("a", MAX_ITEM), ("b", 1)])), 2);
        assert_eq!(next(&Items::new()), 1);
    }

    /// An assembly `frame` placing a plate, two bolts and a nut, its
    /// items as given.
    fn frame(items: serde_json::Value) -> Project {
        let w = || json!("working");
        let mut f = files(&[
            ("plate", &[("b", block())]),
            ("bolt", &[("b", block())]),
            ("nut", &[("b", block())]),
            (
                "frame",
                &[
                    ("base", instance("plate", w())),
                    ("b1", instance("bolt", w())),
                    ("b2", instance("bolt", w())),
                    ("n1", instance("nut", w())),
                ],
            ),
        ]);
        let mut list: serde_json::Value =
            serde_json::from_slice(&f["parts/frame/part.json"]).unwrap();
        list["items"] = items;
        list["format"] = json!(3);
        f.insert(
            "parts/frame/part.json".into(),
            canonical::to_string(&list).into_bytes(),
        );
        Project::load_from(Path::new(""), &f).unwrap()
    }

    #[test]
    fn faults_name_missing_shared_and_unused_numbers() {
        let p = frame(json!({"plate": 1, "bolt": 1, "washer": 4}));
        let part = p.part(&id("frame")).unwrap();
        assert_eq!(placed(part), ids(&["plate", "bolt", "nut"]));
        let f = faults(&id("frame"), part);
        let got: Vec<(Code, serde_json::Value)> = f
            .iter()
            .map(|e| (e.code, e.details["parts"].clone()))
            .collect();
        assert_eq!(
            got,
            [
                (Code::BomItemMissing, json!(["nut"])),
                (Code::BomItemDuplicate, json!(["plate", "bolt"])),
                (Code::BomItemUnused, json!(["washer"])),
            ]
        );
        assert_eq!(f[1].details["item"], 1);
        assert_eq!(f[0].details["part"], "frame");
        // Each repair settles its own.
        let (i, pl) = (&part.file.as_ref().unwrap().items, placed(part));
        assert_eq!(
            with_missing(i, &pl),
            items(&[("plate", 1), ("bolt", 1), ("washer", 4), ("nut", 5)])
        );
        assert_eq!(
            deduplicated(i, &pl),
            items(&[("plate", 1), ("bolt", 5), ("washer", 4)])
        );
        assert_eq!(without_unused(i, &pl), items(&[("plate", 1), ("bolt", 1)]));
        // A sound assembly has none, and a plain part never does.
        let p = frame(json!({"plate": 1, "bolt": 2, "nut": 3}));
        assert!(faults(&id("frame"), p.part(&id("frame")).unwrap()).is_empty());
        assert!(faults(&id("bolt"), p.part(&id("bolt")).unwrap()).is_empty());
    }

    #[test]
    fn an_unreadable_instance_leaves_unused_numbers_unjudged() {
        let mut f = files(&[
            ("bolt", &[("b", block())]),
            ("frame", &[("b1", instance("bolt", json!("working")))]),
        ]);
        f.insert(
            "parts/frame/part.json".into(),
            canonical::to_string(&json!({"features": ["b1", "x"], "format": 3,
                "items": {"bolt": 1, "nut": 2}, "kind": "ryucad.part",
                "part_kind": "assembly"}))
            .into_bytes(),
        );
        f.insert("parts/frame/features/x.json".into(), b"{".to_vec());
        let p = Project::load_from(Path::new(""), &f).unwrap();
        assert!(faults(&id("frame"), p.part(&id("frame")).unwrap()).is_empty());
    }

    #[test]
    fn a_number_each_side_gave_another_part_is_a_collision() {
        use ryucad_doc::doc::canonical;
        let asm = |items: serde_json::Value| {
            let v = json!({"features": [], "format": 3, "items": items,
                           "kind": "ryucad.part", "part_kind": "assembly"});
            let mut f = std::collections::BTreeMap::new();
            f.insert(
                "parts/frame/part.json".to_owned(),
                canonical::to_string(&v).into_bytes(),
            );
            f
        };
        let ours = asm(json!({"plate": 1, "bolt": 2, "pin": 3}));
        let theirs = asm(json!({"plate": 1, "nut": 2, "washer": 3}));
        let merged = asm(json!({"plate": 1, "bolt": 2, "nut": 2, "pin": 3, "washer": 3}));
        let c = collisions(&ours, &theirs, &merged);
        let got: Vec<(u32, Vec<&str>, &str, u32)> = c
            .iter()
            .map(|c| {
                (
                    c.item,
                    c.parts.iter().map(Id::as_str).collect(),
                    c.theirs.as_str(),
                    c.to,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (2, vec!["bolt", "nut"], "nut", 4),
                (3, vec!["pin", "washer"], "washer", 5)
            ]
        );
        // Either repair settles both, so taking one then the other agrees.
        assert_eq!(c[0].repaired, c[1].repaired);
        assert_eq!(
            c[0].repaired["items"],
            json!({"plate": 1, "bolt": 2, "nut": 4, "pin": 3, "washer": 5})
        );
        // A number a side already shared is no fault of the merge's.
        let ours = asm(json!({"bolt": 2, "nut": 2}));
        assert!(
            collisions(&ours, &theirs, &merged)
                .iter()
                .all(|c| c.item != 2)
        );
    }

    #[test]
    fn renumbering_sets_numbers_together_and_refuses_what_would_clash() {
        let now = items(&[("plate", 1), ("bolt", 2), ("nut", 3)]);
        let pl = ids(&["plate", "bolt", "nut"]);
        // A swap in one request.
        assert_eq!(
            renumbered(&now, &pl, &[(id("plate"), 2), (id("bolt"), 1)]).unwrap(),
            items(&[("plate", 2), ("bolt", 1), ("nut", 3)])
        );
        let e = renumbered(&now, &pl, &[(id("plate"), 3)]).unwrap_err();
        assert_eq!(
            (e.code, &e.details["parts"]),
            (Code::BomItemDuplicate, &json!(["plate", "nut"]))
        );
        let e = renumbered(&now, &pl, &[(id("washer"), 9)]).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        for n in [0, 100_000] {
            let e = renumbered(&now, &pl, &[(id("nut"), n)]).unwrap_err();
            assert_eq!(e.code, Code::InvalidParam);
        }
        // Compacted: 1 to n by number, then those without one.
        let gappy = items(&[("plate", 10), ("nut", 4), ("washer", 2)]);
        assert_eq!(
            compacted(&gappy, &pl),
            items(&[("nut", 1), ("plate", 2), ("bolt", 3)])
        );
    }
}
