//! The three-way merge of a block's layout: what Git's merge driver (stage
//! 4's) will call, after the block's connectivity is merged. Each key of
//! the layout (a sheet, a part's place, its text, a net's labels or
//! wires, a drawn bus, a note's place) changed on one side takes that
//! side; changed alike, either. Both sides changing one key differently,
//! or the boxes of what each side changed overlapping on a sheet, is a
//! conflict over that area. Instructions merge
//! by what they say: those either side added, less those either removed.
//!
//! A conflict is repaired by taking ours, taking theirs, or laying its area
//! out again: its keys go back to the base layout's and the engine places
//! the merged connectivity there with both sides' instructions. Then the
//! whole block is laid out over the merged layout, incrementally, so what
//! neither side touched keeps its bits.

use crate::engine::{self, Outcome};
use crate::geom::{Place, Rect};
use crate::input::Block;
use crate::instr::Refusal;
use crate::label;
use crate::layout::{Instruction, Layout, next_id};
use crate::shape::Shape;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A key of a layout: its section and its key there: a sheet's ID, a
/// part's key, `<net>|<part>/<pin>` for one label, `<net>|<sheet>` for a
/// net's wires on one sheet.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Key {
    /// `sheets`, `regions`, `placed`, `text`, `labels`, `routes`,
    /// `buses` (`<bus>|<sheet>`, plan 2.C) or `notes` (a note's place, by
    /// its instruction's merged ID).
    pub section: &'static str,
    pub key: String,
}

impl Key {
    /// What it belongs to: a part (its place, its text and the labels on
    /// its pins), a net's wires, a sheet.
    fn item(&self) -> String {
        match self.section {
            "placed" | "text" => format!("part {}", self.key),
            "labels" => {
                let on = self.key.split_once('|').map_or("", |(_, on)| on);
                let part = on.rsplit_once('/').map_or(on, |(p, _)| p);
                format!("part {part}")
            }
            "buses" => format!(
                "bus {}",
                self.key
                    .split_once('|')
                    .map_or(self.key.as_str(), |(b, _)| b)
            ),
            s => format!("{s} {}", self.key),
        }
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}/{}", self.section, self.key)
    }
}

/// An area both sides changed: the keys each side changed there, and its
/// box on each sheet it reaches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Conflict {
    /// By sheet ID: the box of everything in conflict there.
    pub areas: BTreeMap<String, Rect>,
    /// The keys ours changed in it, as `<section>/<key>`.
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
}

/// A merged layout: ours' values where the sides conflict, and the
/// conflicts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    pub layout: Layout,
    pub conflicts: Vec<Conflict>,
    /// Each conflict's keys, as [`Conflict`] lists them.
    keys: Vec<BTreeSet<Key>>,
}

/// How a conflict is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Repair {
    Ours,
    Theirs,
    /// Its area laid out again from the base layout.
    Relayout,
}

/// One layout's values by key, as JSON for comparing: so every section
/// compares alike.
fn values(l: &Layout) -> BTreeMap<Key, serde_json::Value> {
    let mut out = BTreeMap::new();
    let put = |out: &mut BTreeMap<Key, serde_json::Value>,
               section: &'static str,
               m: serde_json::Value| {
        if let serde_json::Value::Object(o) = m {
            for (k, v) in o {
                out.insert(Key { section, key: k }, v);
            }
        }
    };
    put(&mut out, "sheets", json(&l.sheets));
    put(&mut out, "regions", json(&l.regions));
    put(&mut out, "placed", json(&l.placed));
    put(&mut out, "text", json(&l.text));
    put(&mut out, "notes", json(&l.notes));
    for (net, list) in &l.labels {
        for lab in list {
            out.insert(
                Key {
                    section: "labels",
                    key: format!("{net}|{}", lab.on),
                },
                json(lab),
            );
        }
    }
    for (net, by_sheet) in &l.routes {
        for (sheet, runs) in by_sheet {
            out.insert(
                Key {
                    section: "routes",
                    key: format!("{net}|{sheet}"),
                },
                json(runs),
            );
        }
    }
    for (bus, by_sheet) in &l.buses {
        for (sheet, drawn) in by_sheet {
            out.insert(
                Key {
                    section: "buses",
                    key: format!("{bus}|{sheet}"),
                },
                json(drawn),
            );
        }
    }
    out
}

fn json<T: Serialize>(x: &T) -> serde_json::Value {
    serde_json::to_value(x).expect("layouts serialize")
}

/// Puts `v` (or nothing) at `k` in `l`.
fn set(l: &mut Layout, k: &Key, v: Option<&serde_json::Value>) {
    fn put<T: serde::de::DeserializeOwned>(
        m: &mut BTreeMap<String, T>,
        key: &str,
        v: Option<&serde_json::Value>,
    ) {
        match v {
            Some(v) => {
                m.insert(
                    key.to_owned(),
                    serde_json::from_value(v.clone()).expect("a value of this section"),
                );
            }
            None => {
                m.remove(key);
            }
        }
    }
    match k.section {
        "sheets" => put(&mut l.sheets, &k.key, v),
        "regions" => put(&mut l.regions, &k.key, v),
        "placed" => put(&mut l.placed, &k.key, v),
        "text" => put(&mut l.text, &k.key, v),
        "notes" => put(&mut l.notes, &k.key, v),
        "labels" => {
            let (net, on) = k.key.split_once('|').expect("a label's key");
            let list = l.labels.entry(net.to_owned()).or_default();
            list.retain(|x| x.on != on);
            if let Some(v) = v {
                list.push(serde_json::from_value(v.clone()).expect("a label"));
                list.sort_by(|a, b| (&a.sheet, &a.on).cmp(&(&b.sheet, &b.on)));
            }
            if list.is_empty() {
                l.labels.remove(net);
            }
        }
        "buses" => {
            let (bus, sheet) = k.key.split_once('|').expect("a bus's key");
            let by_sheet = l.buses.entry(bus.to_owned()).or_default();
            match v {
                Some(v) => {
                    by_sheet.insert(
                        sheet.to_owned(),
                        serde_json::from_value(v.clone()).expect("a drawn bus"),
                    );
                }
                None => {
                    by_sheet.remove(sheet);
                }
            }
            if by_sheet.is_empty() {
                l.buses.remove(bus);
            }
        }
        _ => {
            let (net, sheet) = k.key.split_once('|').expect("a route's key");
            let by_sheet = l.routes.entry(net.to_owned()).or_default();
            match v {
                Some(v) => {
                    by_sheet.insert(
                        sheet.to_owned(),
                        serde_json::from_value(v.clone()).expect("runs"),
                    );
                }
                None => {
                    by_sheet.remove(sheet);
                }
            }
            if by_sheet.is_empty() {
                l.routes.remove(net);
            }
        }
    }
}

/// Where a key draws in layout `l`: its box on each sheet.
fn boxes(block: &Block, l: &Layout, k: &Key) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    match k.section {
        "sheets" => {
            if l.sheets.contains_key(&k.key) {
                out.push((k.key.clone(), Rect::new([0, 0], block.area_in(l, &k.key))));
            }
        }
        "regions" => {
            if let Some(r) = l.regions.get(&k.key) {
                out.push((r.sheet.clone(), r.frame_on_sheet()));
            }
        }
        "placed" => {
            if let (Some(x), Some(p)) = (l.placed.get(&k.key), block.part(&k.key)) {
                let s = Shape::of(&block.parts[p], Place::new(x.at, x.rotation, x.mirror));
                out.push((x.sheet.clone(), s.extent));
            }
        }
        "text" => {
            if let (Some(fields), Some(p), Some(x)) =
                (l.text.get(&k.key), block.part(&k.key), l.placed.get(&k.key))
            {
                let part = &block.parts[p];
                for (field, line) in part.fields() {
                    if let Some(at) = fields.get(field) {
                        out.push((x.sheet.clone(), line.boxed(*at)));
                    }
                }
            }
        }
        "labels" => {
            let (net, on) = k.key.split_once('|').expect("a label's key");
            for lab in l
                .labels
                .get(net)
                .into_iter()
                .flatten()
                .filter(|x| x.on == on)
            {
                out.push((lab.sheet.clone(), label::boxed(block, net, lab)));
            }
        }
        "notes" => {
            let text = match l.instructions.get(&k.key) {
                Some(Instruction::Note { text, .. }) => Some(text.as_str()),
                _ => block.note_text(&k.key),
            };
            if let (Some(n), Some(text)) = (l.notes.get(&k.key), text) {
                out.push((n.sheet.clone(), crate::paint::note_box(text, n.at)));
            }
        }
        "buses" => {
            let (bus, sheet) = k.key.split_once('|').expect("a bus's key");
            if let Some(d) = l.buses.get(bus).and_then(|b| b.get(sheet)) {
                let pts = d
                    .entries
                    .iter()
                    .map(|e| e.at)
                    .chain(d.trunk.iter().flatten().copied())
                    .chain(d.connector.iter().map(|c| c.at));
                if let Some(r) = Rect::of(pts) {
                    out.push((sheet.to_owned(), r.grow(crate::geom::GRID)));
                }
            }
        }
        _ => {
            let (net, sheet) = k.key.split_once('|').expect("a route's key");
            for run in l
                .routes
                .get(net)
                .and_then(|r| r.get(sheet))
                .into_iter()
                .flatten()
            {
                if let Some(r) = Rect::of(run.iter().copied()) {
                    out.push((sheet.to_owned(), r));
                }
            }
        }
    }
    out
}

/// The instructions merged by what they say: the base's that neither side
/// removed, then ours' additions under ours' IDs, then theirs' under the
/// next free IDs.
fn instructions(base: &Layout, ours: &Layout, theirs: &Layout) -> BTreeMap<String, Instruction> {
    let has = |l: &Layout, i: &Instruction| l.instructions.values().any(|x| x == i);
    let mut out: BTreeMap<String, Instruction> = BTreeMap::new();
    for (id, i) in &base.instructions {
        if has(ours, i) && has(theirs, i) {
            let id = ours
                .instructions
                .iter()
                .find(|(_, x)| *x == i)
                .map_or(id.clone(), |(k, _)| k.clone());
            out.insert(id, i.clone());
        }
    }
    for (id, i) in &ours.instructions {
        if !has(base, i) && !out.values().any(|x| x == i) {
            let id = if out.contains_key(id) {
                next_id(out.keys())
            } else {
                id.clone()
            };
            out.insert(id, i.clone());
        }
    }
    for i in theirs.instructions.values() {
        if !has(base, i) && !out.values().any(|x| x == i) {
            let id = next_id(out.keys().chain(ours.instructions.keys()));
            out.insert(id, i.clone());
        }
    }
    out
}

/// Each part's place relative to the origin of the region it had in
/// `base` (as `l` has that region, else as `base` had it), and each note
/// on a region of `base` relative to it likewise: the form both sides
/// compare in, stable when a side joins regions or moves one whole.
/// `sign` 1 takes it back to the sheet.
fn shifted_by_base(l: &Layout, base: &Layout, sign: crate::geom::Nm) -> Layout {
    let mut out = l.clone();
    for (id, r) in &base.regions {
        let at = l.regions.get(id).map_or(r.at, |x| x.at);
        for k in &r.parts {
            if let Some(p) = out.placed.get_mut(k) {
                p.at = [p.at[0] - sign * at[0], p.at[1] - sign * at[1]];
            }
        }
    }
    for (id, n) in &mut out.notes {
        let Some(Instruction::Note { on, .. }) = l.instructions.get(id) else {
            continue;
        };
        let Some(r) = on.strip_prefix("region:") else {
            continue;
        };
        let Some(at) = base
            .regions
            .get(r)
            .map(|b| l.regions.get(r).map_or(b.at, |x| x.at))
        else {
            continue;
        };
        n.at = [n.at[0] - sign * at[0], n.at[1] - sign * at[1]];
    }
    out
}

/// `l` with its instructions as `merged` numbers them (those `merged`
/// holds) and its notes' places under those IDs: theirs' added notes are
/// renumbered when merged, and a note whose instruction went goes too.
fn renumbered(l: &Layout, merged: &BTreeMap<String, Instruction>) -> Layout {
    let mut out = l.clone();
    out.instructions = merged
        .iter()
        .filter(|(_, i)| l.instructions.values().any(|x| x == *i))
        .map(|(id, i)| (id.clone(), i.clone()))
        .collect();
    out.notes = l
        .notes
        .iter()
        .filter_map(|(id, n)| {
            let i = l.instructions.get(id)?;
            let (to, _) = merged.iter().find(|(_, x)| *x == i)?;
            Some((to.clone(), n.clone()))
        })
        .collect();
    out
}

/// Merges `ours` and `theirs`, both made from `base`, for `block` (the
/// merged connectivity). Placements compare relative to the regions their
/// parts had in the base (plan 2.A), so a region moved on one side and a part
/// edited inside it on the other merge cleanly; boxes are on the sheet.
pub fn merge(block: &Block, base: &Layout, ours: &Layout, theirs: &Layout) -> Merged {
    let merged = instructions(base, ours, theirs);
    // What each merged instruction moved stood before it: as the side its
    // instruction came from remembered it.
    let before = merged
        .iter()
        .filter_map(|(id, i)| {
            [ours, theirs, base].into_iter().find_map(|l| {
                let (k, _) = l.instructions.iter().find(|(_, x)| *x == i)?;
                l.before.get(k).map(|m| (id.clone(), m.clone()))
            })
        })
        .collect();
    // Each side's notes under the merged IDs, from here on.
    let (base, ours, theirs) = (
        &renumbered(base, &merged),
        &renumbered(ours, &merged),
        &renumbered(theirs, &merged),
    );
    let (b, o, t) = (
        values(&shifted_by_base(base, base, 1)),
        values(&shifted_by_base(ours, base, 1)),
        values(&shifted_by_base(theirs, base, 1)),
    );
    let all: BTreeSet<&Key> = b.keys().chain(o.keys()).chain(t.keys()).collect();
    let mut layout = Layout {
        instructions: merged,
        before,
        ..Layout::default()
    };
    let mut ours_changed: BTreeSet<Key> = BTreeSet::new();
    let mut theirs_changed: BTreeSet<Key> = BTreeSet::new();
    let mut clash: BTreeSet<Key> = BTreeSet::new();
    for k in all {
        let (vb, vo, vt) = (b.get(k), o.get(k), t.get(k));
        let v = if vo == vt {
            vo
        } else if vo == vb {
            vt
        } else if vt == vb {
            vo
        } else {
            clash.insert(k.clone());
            vo
        };
        if vo != vb && vo != vt {
            ours_changed.insert(k.clone());
        }
        if vt != vb && vo != vt {
            theirs_changed.insert(k.clone());
        }
        set(&mut layout, k, v);
    }
    let layout = shifted_by_base(&layout, base, -1);
    // Conflicts: keys changed differently on both sides, joined with every
    // key the other side changed whose boxes overlap theirs on a sheet.
    let mut groups: Vec<BTreeSet<Key>> = clash.iter().map(|k| [k.clone()].into()).collect();
    let mine: Vec<(Key, Vec<(String, Rect)>)> = ours_changed
        .iter()
        .map(|k| (k.clone(), boxes(block, ours, k)))
        .collect();
    let other: Vec<(Key, Vec<(String, Rect)>)> = theirs_changed
        .iter()
        .map(|k| (k.clone(), boxes(block, theirs, k)))
        .collect();
    let meet = |a: &[(String, Rect)], b: &[(String, Rect)]| {
        a.iter()
            .any(|(sa, ra)| b.iter().any(|(sb, rb)| sa == sb && ra.overlaps(rb)))
    };
    for (ka, ba) in &mine {
        for (kb, bb) in &other {
            if ka != kb && meet(ba, bb) {
                groups.push([ka.clone(), kb.clone()].into());
            }
        }
    }
    // Join groups sharing an item (a part's place, text and labels are one)
    // or overlapping on a sheet, until none do: a conflict is an area.
    let area = |g: &BTreeSet<Key>| -> Vec<(String, Rect)> {
        g.iter()
            .flat_map(|k| {
                [base, ours, theirs]
                    .into_iter()
                    .flat_map(move |l| boxes(block, l, k))
            })
            .collect()
    };
    let mut joined: Vec<BTreeSet<Key>> = groups;
    loop {
        joined.sort();
        joined.dedup();
        let boxes_of: Vec<Vec<(String, Rect)>> = joined.iter().map(area).collect();
        let items: Vec<BTreeSet<String>> = joined
            .iter()
            .map(|g| g.iter().map(Key::item).collect())
            .collect();
        let pair = (0..joined.len()).find_map(|i| {
            (i + 1..joined.len())
                .find(|j| !items[i].is_disjoint(&items[*j]) || meet(&boxes_of[i], &boxes_of[*j]))
                .map(|j| (i, j))
        });
        let Some((i, j)) = pair else { break };
        let other = joined.remove(j);
        joined[i].extend(other);
    }
    let conflicts = joined
        .iter()
        .map(|g| {
            let mut areas: BTreeMap<String, Rect> = BTreeMap::new();
            for k in g {
                for l in [base, ours, theirs] {
                    for (s, r) in boxes(block, l, k) {
                        areas.entry(s).and_modify(|a| *a = a.union(r)).or_insert(r);
                    }
                }
            }
            Conflict {
                areas,
                ours: g
                    .iter()
                    .filter(|k| ours_changed.contains(*k))
                    .map(Key::to_string)
                    .collect(),
                theirs: g
                    .iter()
                    .filter(|k| theirs_changed.contains(*k))
                    .map(Key::to_string)
                    .collect(),
            }
        })
        .collect();
    Merged {
        layout,
        conflicts,
        keys: joined,
    }
}

/// Settles `merged`'s conflicts, one repair each in order, and lays the
/// block out over the result; refused as the engine refuses when the
/// merged instructions can't hold together.
pub fn resolve(
    block: &Block,
    base: &Layout,
    ours: &Layout,
    theirs: &Layout,
    merged: &Merged,
    repairs: &[Repair],
) -> Result<Outcome, Refusal> {
    let ins = &merged.layout.instructions;
    let (base, ours, theirs) = (
        &renumbered(base, ins),
        &renumbered(ours, ins),
        &renumbered(theirs, ins),
    );
    let (b, o, t) = (
        values(&shifted_by_base(base, base, 1)),
        values(&shifted_by_base(ours, base, 1)),
        values(&shifted_by_base(theirs, base, 1)),
    );
    let mut l = shifted_by_base(&merged.layout, base, 1);
    for (keys, repair) in merged.keys.iter().zip(repairs) {
        for k in keys {
            let from = match repair {
                Repair::Ours => &o,
                Repair::Theirs => &t,
                Repair::Relayout => &b,
            };
            set(&mut l, k, from.get(k));
        }
        if *repair == Repair::Relayout {
            // What the area's parts are on is drawn again with them.
            let parts: BTreeSet<&str> = keys
                .iter()
                .filter(|k| k.section == "placed")
                .map(|k| k.key.as_str())
                .collect();
            for p in parts.iter().filter_map(|k| block.part(k)) {
                for n in block.parts[p].pins.iter().flat_map(|x| x.nets()) {
                    let key = &block.nets[n].key;
                    l.routes.remove(key);
                    l.labels.remove(key);
                }
                l.text.remove(&block.parts[p].key);
            }
        }
    }
    let l = shifted_by_base(&l, base, -1);
    let mut again = block.clone();
    again.instructions = l.instructions.clone();
    again.previous = Some(l);
    again.moves = Vec::new();
    engine::lay_out(&again)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{resistor, seeded};

    /// A region moved on one side and edited inside on the other merge
    /// cleanly: placements are relative to the region; both sides moving
    /// one region is a conflict over its area.
    #[test]
    fn a_region_moved_and_edited_merges_cleanly() {
        let block = crate::testing::regions_block();
        let base = crate::engine::lay_out(&block).unwrap().layout;
        let id = base
            .regions
            .iter()
            .find(|(_, r)| r.parts.iter().any(|k| k == "r9"))
            .map(|(id, _)| id.clone())
            .unwrap();
        let mut ours = base.clone();
        let r = ours.regions.get_mut(&id).unwrap();
        r.at = [r.at[0] + 20 * crate::geom::GRID, r.at[1]];
        let moved = r.parts.clone();
        for k in &moved {
            let p = ours.placed.get_mut(k).unwrap();
            p.at[0] += 20 * crate::geom::GRID;
        }
        let mut theirs = base.clone();
        theirs.placed.get_mut("c9").unwrap().rotation = 180;
        let m = merge(&block, &base, &ours, &theirs);
        assert!(m.conflicts.is_empty(), "{:#?}", m.conflicts);
        assert_eq!(m.layout.regions[&id].at, ours.regions[&id].at);
        assert_eq!(m.layout.placed["c9"].rotation, 180);
        assert_eq!(
            m.layout.relative().placed["r9"].at,
            base.relative().placed["r9"].at,
            "R9 keeps its place in the region"
        );
        let mut theirs2 = base.clone();
        theirs2.regions.get_mut(&id).unwrap().at[1] += 10 * crate::geom::GRID;
        let m2 = merge(&block, &base, &ours, &theirs2);
        assert!(!m2.conflicts.is_empty());
        assert!(
            m2.conflicts[0]
                .ours
                .iter()
                .any(|k| k == &format!("regions/{id}"))
        );
    }

    /// `regions_block` laid out with a note on its first region (`i9`).
    fn noted() -> (Block, Layout, String) {
        let mut block = crate::testing::regions_block();
        let first = engine::lay_out(&block).unwrap().layout;
        let rid = first.regions.keys().next().expect("a region").clone();
        block.instructions.insert(
            "i9".into(),
            Instruction::Note {
                on: format!("region:{rid}"),
                text: "Fused at 2 A".into(),
            },
        );
        block.previous = Some(first);
        let base = engine::lay_out(&block).unwrap().layout;
        assert!(base.notes.contains_key("i9"));
        (block, base, rid)
    }

    /// Notes merge by their instruction's ID: kept by a merge that changes
    /// nothing, moved where one side moved it, relative to their region
    /// where the other side moved that, and renumbered with their
    /// instruction when theirs adds one under an ID ours took.
    #[test]
    fn notes_merge_by_instruction_and_move_with_their_region() {
        use crate::geom::GRID;
        let (block, base, rid) = noted();
        let m = merge(&block, &base, &base, &base);
        assert!(m.conflicts.is_empty());
        assert_eq!(m.layout.notes, base.notes);
        // Ours moves the note within the region; theirs moves the region.
        let mut ours = base.clone();
        ours.notes.get_mut("i9").unwrap().at[0] += GRID;
        let mut theirs = base.clone();
        let r = theirs.regions.get_mut(&rid).unwrap();
        r.at[1] -= 40 * GRID;
        for k in r.parts.clone() {
            theirs.placed.get_mut(&k).unwrap().at[1] -= 40 * GRID;
        }
        theirs.notes.get_mut("i9").unwrap().at[1] -= 40 * GRID;
        let m = merge(&block, &base, &ours, &theirs);
        assert!(m.conflicts.is_empty(), "{:#?}", m.conflicts);
        let was = base.notes["i9"].at;
        assert_eq!(m.layout.notes["i9"].at, [was[0] + GRID, was[1] - 40 * GRID]);
        // Both sides add a note as i10: theirs' is renumbered, its place
        // with it.
        let sheet = base.regions[&rid].sheet.clone();
        let add = |l: &Layout, text: &str, at: crate::geom::Point| {
            let mut l = l.clone();
            l.instructions.insert(
                "i10".into(),
                Instruction::Note {
                    on: sheet.clone(),
                    text: text.into(),
                },
            );
            l.notes.insert(
                "i10".into(),
                crate::layout::PlacedNote {
                    sheet: sheet.clone(),
                    at,
                },
            );
            l
        };
        let ours = add(&base, "Rev A", [GRID, GRID]);
        let theirs = add(&base, "Rev B", [GRID, 4 * GRID]);
        let m = merge(&block, &base, &ours, &theirs);
        let (id, _) = m
            .layout
            .instructions
            .iter()
            .find(|(_, i)| matches!(i, Instruction::Note { text, .. } if text == "Rev B"))
            .expect("theirs' note");
        assert_ne!(id, "i10");
        assert_eq!(m.layout.notes["i10"].at, [GRID, GRID]);
        assert_eq!(m.layout.notes[id].at, [GRID, 4 * GRID]);
        // Both sides moving the note differently: a conflict over it.
        let mut ours = base.clone();
        ours.notes.get_mut("i9").unwrap().at[0] += GRID;
        let mut theirs = base.clone();
        theirs.notes.get_mut("i9").unwrap().at[0] += 2 * GRID;
        let m = merge(&block, &base, &ours, &theirs);
        assert!(
            m.conflicts
                .iter()
                .any(|c| c.ours.contains(&"notes/i9".to_owned()) && !c.areas.is_empty()),
            "{:#?}",
            m.conflicts
        );
    }

    #[test]
    fn sides_merge_by_key_and_overlaps_conflict() {
        let b = seeded(3, 4);
        let base = engine::lay_out(&b).unwrap().layout;
        let gnd = b.net("rail:gnd").unwrap();
        let vcc = b.net("rail:vcc").unwrap();
        // Ours adds R901, theirs R902: both land in the same free space.
        let with = |key: &str, refdes: &str| {
            let mut e = b.clone();
            e.parts
                .push(resistor(key, refdes, "1k", [Some(vcc), Some(gnd)]));
            e.previous = Some(base.clone());
            (e.clone(), engine::lay_out(&e).unwrap().layout)
        };
        let (_, ours) = with("r901", "R901");
        let (_, theirs) = with("r902", "R902");
        let mut both = b.clone();
        both.parts
            .push(resistor("r901", "R901", "1k", [Some(vcc), Some(gnd)]));
        both.parts
            .push(resistor("r902", "R902", "1k", [Some(vcc), Some(gnd)]));
        let m = merge(&both, &base, &ours, &theirs);
        // What neither side touched keeps its bits.
        for (k, x) in &base.placed {
            assert_eq!(m.layout.placed.get(k), Some(x), "{k}");
        }
        assert_eq!(m.conflicts.len(), 1, "{:#?}", m.conflicts);
        let c = &m.conflicts[0];
        assert!(c.ours.contains(&"placed/r901".to_owned()), "{c:?}");
        assert!(c.theirs.contains(&"placed/r902".to_owned()), "{c:?}");
        // Laid out again, both fit and every rule holds; the rest stays.
        let out = resolve(&both, &base, &ours, &theirs, &m, &[Repair::Relayout]).unwrap();
        assert_eq!(crate::rules::check(&both, &out.layout), []);
        for (k, x) in &base.placed {
            assert_eq!(out.layout.placed.get(k), Some(x), "{k}");
        }
        assert!(out.layout.placed.contains_key("r901") && out.layout.placed.contains_key("r902"));
        // Taking ours keeps R901 where ours put it, and R902 goes where
        // there is room.
        let out = resolve(&both, &base, &ours, &theirs, &m, &[Repair::Ours]).unwrap();
        assert_eq!(out.layout.placed["r901"], ours.placed["r901"]);
        assert_eq!(crate::rules::check(&both, &out.layout), []);
    }

    #[test]
    fn changes_apart_merge_cleanly_and_instructions_by_what_they_say() {
        let b = seeded(4, 6);
        let base = engine::lay_out(&b).unwrap().layout;
        let lay = |e: &mut Block| {
            e.previous = Some(base.clone());
            engine::lay_out(e).unwrap().layout
        };
        // Ours revalues U0; theirs renames a net far from it, and both
        // add the same instruction and one each of their own.
        let mut eo = b.clone();
        let p = eo.part("u0").unwrap();
        eo.parts[p].value = "OURS".into();
        eo.instructions.insert(
            "i1".into(),
            Instruction::Flow {
                parts: vec!["u1".into()],
            },
        );
        let ours = lay(&mut eo);
        let mut et = b.clone();
        let n = et.net("out5_0").unwrap();
        et.nets[n].name = "THEIRS".into();
        et.instructions.insert(
            "i1".into(),
            Instruction::Flow {
                parts: vec!["u2".into()],
            },
        );
        let theirs = lay(&mut et);
        let mut both = eo.clone();
        both.nets[n].name = "THEIRS".into();
        let m = merge(&both, &base, &ours, &theirs);
        assert_eq!(m.conflicts, [], "{:#?}", m.conflicts);
        assert_eq!(m.layout.instructions.len(), 2);
        assert_eq!(
            m.layout.instructions["i2"],
            Instruction::Flow {
                parts: vec!["u2".into()]
            }
        );
        let out = resolve(&both, &base, &ours, &theirs, &m, &[]).unwrap();
        assert_eq!(crate::rules::check(&both, &out.layout), []);
        // A key both sides changed differently: one conflict, no area
        // shared with anything else.
        let mut l1 = base.clone();
        let mut l2 = base.clone();
        let first = base.sheets.keys().next().unwrap().clone();
        l1.sheets.get_mut(&first).unwrap().name = "A".into();
        l2.sheets.get_mut(&first).unwrap().name = "B".into();
        let m = merge(&b, &base, &l1, &l2);
        assert_eq!(m.conflicts.len(), 1);
        assert_eq!(m.conflicts[0].ours, [format!("sheets/{first}")]);
        let out = resolve(&b, &base, &l1, &l2, &m, &[Repair::Theirs]).unwrap();
        assert_eq!(out.layout.sheets[&first].name, "B");
    }

    #[test]
    fn a_label_made_a_connector_on_one_side_merges_cleanly() {
        use crate::layout::LabelKind;
        let block = crate::testing::leaving_block();
        let base = crate::engine::lay_out(&block).unwrap().layout;
        let mut ours = base.clone();
        let l = ours
            .labels
            .get_mut("o3")
            .and_then(|v| v.first_mut())
            .expect("O3's label");
        l.kind = LabelKind::OffSheet;
        let mut theirs = base.clone();
        theirs.placed.get_mut("r1").unwrap().rotation = 270;
        let m = merge(&block, &base, &ours, &theirs);
        assert!(m.conflicts.is_empty(), "{:#?}", m.conflicts);
        assert_eq!(m.layout.labels["o3"][0].kind, LabelKind::OffSheet);
        assert_eq!(m.layout.placed["r1"].rotation, 270);
    }

    #[test]
    fn a_bus_merges_as_one_key_a_sheet() {
        let block = crate::testing::bus_block();
        let base = crate::engine::lay_out(&block).unwrap().layout;
        let sid = base.placed["u1"].sheet.clone();
        let key = format!("buses/bus:q|{sid}");
        // Ours moves the trunk; theirs changes nothing near it.
        let mut ours = base.clone();
        let bus = ours.buses.get_mut("bus:q").unwrap().get_mut(&sid).unwrap();
        for p in bus.trunk.iter_mut().flatten() {
            p[1] += crate::geom::GRID;
        }
        let mut theirs = base.clone();
        theirs.instructions.insert(
            "i1".into(),
            Instruction::Flow {
                parts: vec!["u2".into()],
            },
        );
        let m = merge(&block, &base, &ours, &theirs);
        assert!(m.conflicts.is_empty(), "{:#?}", m.conflicts);
        assert_eq!(m.layout.buses, ours.buses);
        assert!(m.layout.instructions.contains_key("i1"));
        // Both change it differently: one conflict naming it.
        let mut theirs2 = base.clone();
        let bus = theirs2
            .buses
            .get_mut("bus:q")
            .unwrap()
            .get_mut(&sid)
            .unwrap();
        for p in bus.trunk.iter_mut().flatten() {
            p[1] -= crate::geom::GRID;
        }
        let m = merge(&block, &base, &ours, &theirs2);
        assert_eq!(m.conflicts.len(), 1);
        assert!(m.conflicts[0].ours.contains(&key), "{:?}", m.conflicts[0]);
    }
}
