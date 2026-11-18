//! The BOM diff (design 734): what changed in items' BOMs between two
//! revisions of the catalogue, line by line: own lines keyed by line ID,
//! representation lines by product, representation and key. ECOs (stage
//! 4) cover this diff.

use crate::bom::{Joined, SCHEMA, line_json, of};
use crate::error::Result;
use crate::id::Id;
use crate::load::Catalogue;
use crate::reps::{At, Reps};
#[cfg(test)]
use crate::reps::Fixed;
use crate::session::Session;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The fields a changed line reports, `{from, to}` each.
fn fields(cat: &Catalogue, j: &Joined) -> BTreeMap<&'static str, Option<String>> {
    let _ = cat;
    BTreeMap::from([
        ("item", j.item.as_ref().map(|r| r.text())),
        ("quantity", Some(j.quantity.clone())),
        ("unit", j.unit.clone()),
        ("find", j.find.clone()),
        ("refdes", j.refdes.clone()),
        ("notes", j.notes.clone()),
    ])
}

/// What changed from line `x` to line `y` of one key, `None` if nothing.
fn change(a: &Catalogue, x: &Joined, b: &Catalogue, y: &Joined) -> Option<Value> {
    let (fx, fy) = (fields(a, x), fields(b, y));
    let mut v = Map::new();
    for (k, old) in &fx {
        let new = &fy[k];
        if old != new {
            v.insert((*k).to_owned(), json!({"from": old, "to": new}));
        }
    }
    if v.is_empty() {
        return None;
    }
    let mut row = line_json(b, y);
    for (k, c) in v {
        row[k] = c;
    }
    Some(row)
}

/// Lines `x` (of `a`) against `y` (of `b`), each list in key order:
/// `(added, removed, changed)`.
fn lines_diff(
    a: &Catalogue,
    x: &[Joined],
    b: &Catalogue,
    y: &[Joined],
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let xs: BTreeMap<String, &Joined> = x.iter().map(|j| (j.key(), j)).collect();
    let ys: BTreeMap<String, &Joined> = y.iter().map(|j| (j.key(), j)).collect();
    let keys: BTreeSet<&String> = xs.keys().chain(ys.keys()).collect();
    let (mut added, mut removed, mut changed) = (Vec::new(), Vec::new(), Vec::new());
    for k in keys {
        match (xs.get(k), ys.get(k)) {
            (Some(p), Some(q)) => changed.extend(change(a, p, b, q)),
            (Some(p), None) => removed.push(line_json(a, p)),
            (None, Some(q)) => added.push(line_json(b, q)),
            (None, None) => {}
        }
    }
    (added, removed, changed)
}

/// What changed in BOMs from catalogue `a` (its representations read at
/// `at_a`) to `b` (at `at_b`), for item `only` or every item either holds:
/// `{schema, summary: {added, removed, changed}, items: [{item, number,
/// added, removed, changed}]}`, items with no change left out. A line's
/// own text (an own line) or count (a representation's) is compared as
/// written; keys never move, so a reordered list is no change.
pub fn between(
    a: &Catalogue,
    at_a: &At,
    b: &Catalogue,
    at_b: &At,
    only: Option<&Id>,
    reps: &mut dyn Reps,
) -> Value {
    let (la, lb) = (crate::links::linked(a), crate::links::linked(b));
    let ids: BTreeSet<&Id> = a
        .items
        .keys()
        .chain(b.items.keys())
        .filter(|i| only.is_none_or(|o| *i == o))
        .collect();
    let mut items = Vec::new();
    let (mut na, mut nr, mut nc) = (0, 0, 0);
    for id in ids {
        let x = match a.items.contains_key(id) {
            true => of(a, &la, id, reps, at_a).0,
            false => Vec::new(),
        };
        let y = match b.items.contains_key(id) {
            true => of(b, &lb, id, reps, at_b).0,
            false => Vec::new(),
        };
        let (added, removed, changed) = lines_diff(a, &x, b, &y);
        if added.is_empty() && removed.is_empty() && changed.is_empty() {
            continue;
        }
        na += added.len();
        nr += removed.len();
        nc += changed.len();
        let number = b.items.get(id).or_else(|| a.items.get(id)).map(|i| i.number.clone());
        items.push(json!({
            "item": id, "number": number,
            "added": added, "removed": removed, "changed": changed,
        }));
    }
    json!({
        "schema": SCHEMA,
        "summary": {"added": na, "removed": nr, "changed": nc},
        "items": items,
    })
}

/// `bom.diff`: [`between`] the catalogue at `from` (`HEAD` by default)
/// and at `to` (the working files by default), for `item` or every item.
pub fn diff(
    session: &mut Session,
    item: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    within: &[String],
) -> Result<Value> {
    let from = from.unwrap_or("HEAD");
    let (a, at_a) = crate::bom::at_rev(session, Some(from))?;
    let (b, at_b) = crate::bom::at_rev(session, to)?;
    let only = item.map(|i| b.find(i).or_else(|_| a.find(i))).transpose()?;
    let mut reps = crate::bom::disk(session, within);
    let mut v = between(&a, &at_a, &b, &at_b, only.as_ref(), &mut reps);
    v["from"] = json!(from);
    v["to"] = json!(to.unwrap_or("working"));
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bom::tests::{id, line, pcba, rep};
    use crate::catalogue::tests::pid;

    #[test]
    fn a_diff_keys_own_lines_by_id_and_read_lines_by_key() {
        let (f, mut reps) = pcba();
        let a = Catalogue::load(&f).unwrap();
        let mut b = a.clone();
        {
            let pcba = b.items.get_mut(&id("0000000000001")).unwrap();
            pcba.bom[0].quantity = "0.75".into(); // changed
            pcba.bom.remove(1); // removed
            pcba.bom.push(line(3, "0000000000003", "2")); // added
        }
        let mut after = reps.clone();
        after.0.insert(
            ("ryucad".into(), pid(3), crate::id::Id::parse("box", "part").unwrap()),
            vec![rep(Some(3), "screw", 6), rep(Some(3), "lid", 1)],
        );
        // One reader serving both sides: the "to" side's lines differ.
        struct Two(Fixed, Fixed);
        impl Reps for Two {
            fn lines(&mut self, l: &crate::item::Link, at: &At) -> crate::error::Result<crate::reps::Read> {
                match at {
                    At::Working => self.1.lines(l, at),
                    At::Commit(_) => self.0.lines(l, at),
                }
            }
        }
        let mut two = Two(reps.clone(), after);
        let d = between(&a, &At::Commit("c0".into()), &b, &At::Working, None, &mut two);
        assert_eq!(d["summary"], serde_json::json!({"added": 1, "removed": 1, "changed": 2}));
        let it = &d["items"][0];
        assert_eq!(it["item"], "0000000000001");
        assert_eq!(it["number"], "PCBA-1");
        assert_eq!(it["changed"][0]["quantity"], serde_json::json!({"from": "0.5", "to": "0.75"}));
        assert_eq!(it["changed"][1]["from"], "ryucad");
        assert_eq!(it["changed"][1]["quantity"], serde_json::json!({"from": "4", "to": "6"}));
        assert_eq!(it["removed"][0]["number"], "SCREW-1");
        assert_eq!(it["added"][0]["number"], "MCU-1");
        // Asked about one item, only it; an unchanged catalogue, nothing.
        let one = between(&a, &At::Working, &a, &At::Working, Some(&id("0000000000002")), &mut reps);
        assert_eq!(one["items"], serde_json::json!([]));
    }
}
