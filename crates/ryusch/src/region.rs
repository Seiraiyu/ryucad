//! Functional regions: titled rectangles on a sheet, each holding one
//! function's parts (a recognised circuit, a dense IC with its
//! satellites, a connector with its breakout, a connected group). They
//! are layout, never connectivity: nothing electrical reads them.

use crate::cluster::Cluster;
use crate::geom::{GRID, Nm, Place, Rect};
use crate::idiom::Idiom;
use crate::input::{Block, NetKind, PartKind};
use crate::instr;
use crate::layout::{IdiomKind, Layout};
use std::collections::{BTreeMap, BTreeSet};

/// A region's margin inside its frame, on every side.
pub const FRAME: Nm = 2 * GRID;
/// The band at the top of a frame its title stands in.
pub const TITLE: Nm = 3 * GRID;
/// How far paper fit widens the channels between regions, at most.
pub const WIDEN: Nm = 4 * GRID;

/// What a region was proposed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Basis {
    /// A `region` instruction.
    Named,
    /// A recognised circuit: a buck, an op-amp stage.
    Topology,
    /// An active part of more than 8 pins, its idioms and its guests.
    Ic,
    /// A connector and its breakout.
    Connector,
    /// A connected group of what is left.
    Rest,
}

/// A proposed region: its title and its parts, by index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposed {
    pub title: String,
    pub basis: Basis,
    pub parts: BTreeSet<usize>,
}

/// Pins past which an active part leads a region of its own.
pub const DENSE: usize = 8;

/// `<refdes> <value>`, trimmed: `J1` where it has no value.
fn name_of(block: &Block, p: usize) -> String {
    let part = &block.parts[p];
    format!("{} {}", part.refdes, part.value).trim().to_owned()
}

/// Proposes regions for `block`'s parts, block ports aside, in design
/// 348's order: named regions (`r.regions`), each recognised topology,
/// each active part of more than 8 pins with the idioms it anchors and the
/// guests whose host it is, each connector with its idioms and guests,
/// then each connected group of what is left over signal nets (a part an
/// `edge` instruction holds a group of its own). Each part
/// is in the first region that takes it, and every idiom's parts follow
/// their anchor (clusters are rigid).
pub fn propose(block: &Block, idioms: &[Idiom], r: &instr::Resolved) -> Vec<Proposed> {
    let n = block.parts.len();
    let is_port = |p: usize| matches!(block.parts[p].kind, PartKind::Port { .. });
    let title_of = |p: usize, fallback: String| {
        let f = &block.parts[p].function;
        if f.is_empty() { fallback } else { f.clone() }
    };
    let mut by_key: Vec<usize> = (0..n).collect();
    by_key.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    let mut out: Vec<Proposed> = Vec::new();
    let push = |out: &mut Vec<Proposed>,
                taken: &mut BTreeSet<usize>,
                title: String,
                basis: Basis,
                parts: BTreeSet<usize>| {
        let parts: BTreeSet<usize> = parts
            .into_iter()
            .filter(|p| !taken.contains(p) && !is_port(*p))
            .collect();
        if parts.is_empty() {
            return;
        }
        taken.extend(parts.iter().copied());
        out.push(Proposed {
            title,
            basis,
            parts,
        });
    };
    // Named regions first: the agent's title wins over a function.
    for (title, parts) in &r.regions {
        push(
            &mut out,
            &mut taken,
            title.clone(),
            Basis::Named,
            parts.clone(),
        );
    }
    // Topologies.
    for i in idioms {
        if !matches!(i.kind, IdiomKind::Buck | IdiomKind::OpampStage) {
            continue;
        }
        let Some((a, _)) = i.anchor else { continue };
        if taken.contains(&a) {
            continue;
        }
        let part = &block.parts[a];
        let what = if part.value.is_empty() {
            &part.refdes
        } else {
            &part.value
        };
        let fallback = if i.kind == IdiomKind::Buck {
            format!("{what} converter")
        } else {
            format!("{what} stage")
        };
        let mut parts: BTreeSet<usize> = i.parts.iter().copied().collect();
        parts.insert(a);
        push(
            &mut out,
            &mut taken,
            title_of(a, fallback),
            Basis::Topology,
            parts,
        );
    }
    // Clusters as the engine makes them, for guests.
    let singles: Vec<Cluster> = (0..n)
        .map(|p| Cluster::single(p, Place::default()))
        .collect();
    let clusters = crate::idiom::clusters(block, idioms, singles, &crate::idiom::Asks::default());
    let leads = |out: &mut Vec<Proposed>, taken: &mut BTreeSet<usize>, p: usize, basis: Basis| {
        let mut parts: BTreeSet<usize> = BTreeSet::from([p]);
        for i in idioms {
            if i.anchor.is_some_and(|(a, _)| a == p) {
                parts.extend(i.parts.iter().copied().filter(|q| !taken.contains(q)));
            }
        }
        for cl in &clusters {
            let free = cl
                .members
                .iter()
                .all(|(q, _)| !taken.contains(q) && !parts.contains(q));
            if free
                && crate::engine::guest(block, cl, r)
                && crate::engine::host(block, cl) == Some(p)
            {
                parts.extend(cl.members.iter().map(|(q, _)| *q));
            }
        }
        push(out, taken, title_of(p, name_of(block, p)), basis, parts);
    };
    // Dense ICs.
    for &p in &by_key {
        let dense = matches!(block.parts[p].kind, PartKind::Symbol { .. })
            && !crate::cluster::is_connector(block, p)
            && block.parts[p].pins.len() > DENSE;
        if dense && !taken.contains(&p) {
            leads(&mut out, &mut taken, p, Basis::Ic);
        }
    }
    // Connectors.
    for &p in &by_key {
        if crate::cluster::is_connector(block, p) && !taken.contains(&p) {
            leads(&mut out, &mut taken, p, Basis::Connector);
        }
    }
    // The rest: connected groups over signal nets.
    let rest: Vec<usize> = by_key
        .iter()
        .copied()
        .filter(|p| !taken.contains(p) && !is_port(*p))
        .collect();
    let mut root: BTreeMap<usize, usize> = rest.iter().map(|p| (*p, *p)).collect();
    fn find(root: &mut BTreeMap<usize, usize>, p: usize) -> usize {
        let mut x = p;
        while root[&x] != x {
            x = root[&x];
        }
        root.insert(p, x);
        x
    }
    // By net and the sheet a `sheet` instruction asks for (plan 2.C): a
    // part asked onto another sheet is grouped with what goes there, not
    // with its neighbours left behind, or its region would span sheets.
    let mut on_net: BTreeMap<(usize, Option<&String>), usize> = BTreeMap::new();
    for &p in &rest {
        for pin in &block.parts[p].pins {
            for net in pin.nets() {
                if block.nets[net].kind != NetKind::Signal {
                    continue;
                }
                // A part an `edge` instruction holds is grouped with
                // nothing: its region goes to the edge, not its neighbours'.
                if r.edge.contains_key(&p) {
                    continue;
                }
                match on_net.get(&(net, r.sheet.get(&p))) {
                    Some(&q) => {
                        let (a, b) = (find(&mut root, p), find(&mut root, q));
                        if a != b {
                            let (lo, hi) = if block.parts[a].key < block.parts[b].key {
                                (a, b)
                            } else {
                                (b, a)
                            };
                            root.insert(hi, lo);
                        }
                    }
                    None => {
                        on_net.insert((net, r.sheet.get(&p)), p);
                    }
                }
            }
        }
    }
    let mut groups: Vec<(usize, BTreeSet<usize>)> = Vec::new();
    for &p in &rest {
        let g = find(&mut root, p);
        match groups.iter_mut().find(|(r0, _)| *r0 == g) {
            Some((_, set)) => {
                set.insert(p);
            }
            None => groups.push((g, BTreeSet::from([p]))),
        }
    }
    // Pins on each signal net, the whole block's: a group's busiest net.
    let mut pins_on: BTreeMap<usize, usize> = BTreeMap::new();
    for part in &block.parts {
        for pin in &part.pins {
            for net in pin.nets() {
                *pins_on.entry(net).or_default() += 1;
            }
        }
    }
    for (_, parts) in groups {
        let mut ordered: Vec<usize> = parts.iter().copied().collect();
        ordered.sort_by(|a, b| block.parts[*a].key.cmp(&block.parts[*b].key));
        let title = match ordered
            .iter()
            .find(|p| !block.parts[**p].function.is_empty())
        {
            Some(p) => block.parts[*p].function.clone(),
            None => {
                let nets: BTreeSet<usize> = parts
                    .iter()
                    .flat_map(|p| block.parts[*p].pins.iter().flat_map(|x| x.nets()))
                    .filter(|n| block.nets[*n].kind == NetKind::Signal)
                    .collect();
                nets.into_iter()
                    .max_by(|a, b| {
                        pins_on[a]
                            .cmp(&pins_on[b])
                            .then_with(|| block.nets[*b].name.cmp(&block.nets[*a].name))
                    })
                    .map_or_else(|| "Misc".to_owned(), |n| block.nets[n].name.clone())
            }
        };
        push(&mut out, &mut taken, title, Basis::Rest, parts);
    }
    // Every idiom's parts follow their anchor: clusters are rigid.
    for i in idioms {
        let Some((a, _)) = i.anchor else { continue };
        let Some(home) = out.iter().position(|g| g.parts.contains(&a)) else {
            continue;
        };
        for &q in &i.parts {
            if let Some(was) = out.iter().position(|g| g.parts.contains(&q))
                && was != home
                && out[was].basis != Basis::Named
            {
                out[was].parts.remove(&q);
                out[home].parts.insert(q);
            }
        }
    }
    out.retain(|g| !g.parts.is_empty());
    out
}

/// Sets each region's frame from what it holds: its parts' extents,
/// their text and the labels on their pins, grown by `FRAME`, the title
/// band on top, relative to the region's origin. Two frames on a sheet
/// that overlap (a part an instruction moved, a longer value's text, the
/// labels drawn) can't both hold: the later unnamed region joins the
/// earlier, and the frames are worked out again, until none overlap.
pub fn frames(block: &Block, layout: &mut Layout) {
    // Parts `region none` keeps out of every named region (plan 3.A: a
    // join below took them in from nothing, and laid out again they left).
    let none: BTreeSet<String> = block
        .instructions
        .values()
        .filter(|i| matches!(i, crate::layout::Instruction::RegionNone { .. }))
        .flat_map(|i| {
            i.parts()
                .iter()
                .flat_map(|s| crate::instr::parts_named(block, s))
        })
        .map(|p| block.parts[p].key.clone())
        .collect();
    loop {
        frames_once(block, layout);
        let ids: Vec<String> = layout.regions.keys().cloned().collect();
        let mut merged = false;
        'pairs: for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                let (ra, rb) = (&layout.regions[a], &layout.regions[b]);
                if ra.sheet != rb.sheet || !ra.frame_on_sheet().overlaps(&rb.frame_on_sheet()) {
                    continue;
                }
                let (keep, gone) = match (ra.named, rb.named) {
                    (_, false) => (a, b),
                    (false, true) => (b, a),
                    (true, true) => continue,
                };
                let mut parts = layout.regions.remove(gone).expect("a region").parts;
                let r = layout.regions.get_mut(keep).expect("a region");
                if r.named {
                    parts.retain(|k| !none.contains(k));
                }
                r.parts.extend(parts);
                r.parts.sort();
                r.parts.dedup();
                merged = true;
                break 'pairs;
            }
        }
        if !merged {
            return;
        }
    }
}

fn frames_once(block: &Block, layout: &mut Layout) {
    let mut out: BTreeMap<String, Rect> = BTreeMap::new();
    for (id, r) in &layout.regions {
        let mut b: Option<Rect> = None;
        let mut add = |x: Rect| b = Some(b.map_or(x, |y| y.union(x)));
        for k in &r.parts {
            let Some(p) = block.part(k) else { continue };
            let Some(x) = layout.placed.get(k).filter(|x| x.sheet == r.sheet) else {
                continue;
            };
            let part = &block.parts[p];
            let shape = crate::shape::Shape::of(part, Place::new(x.at, x.rotation, x.mirror));
            add(shape.extent);
            if let Some(fields) = layout.text.get(k) {
                for (field, line) in part.fields() {
                    if let Some(at) = fields.get(field) {
                        add(line.boxed(*at));
                    }
                }
            }
        }
        for (key, list) in &layout.labels {
            for l in list.iter().filter(|l| l.sheet == r.sheet) {
                let part = l.on.rsplit_once('/').map_or(l.on.as_str(), |(p, _)| p);
                if r.parts.iter().any(|k| k == part) {
                    add(crate::label::boxed(block, key, l));
                }
            }
        }
        // A bus's entries at its parts, their names and a connector the
        // bus leaves the sheet by (plan 2.C).
        for by in layout.buses.values() {
            let Some(bus) = by.get(&r.sheet) else {
                continue;
            };
            let stub = bus.stub;
            let mut mine = false;
            for e in &bus.entries {
                let part = e.on.rsplit_once('/').map_or(e.on.as_str(), |(p, _)| p);
                if !r.parts.iter().any(|k| k == part) {
                    continue;
                }
                mine = true;
                let (_, on_spine) = crate::bus::entry(e.at, e.side, stub);
                add(Rect::new(e.at, on_spine));
                if let Some(n) = block.net(&e.net) {
                    let name = block.nets[n].name.as_str();
                    if let Some((at, anchor, _)) =
                        crate::label::drawn(crate::layout::LabelKind::Net, name, e.at, e.side).text
                    {
                        add(crate::font::text_box(name, crate::font::SIZE, at, anchor));
                    }
                }
            }
            if mine && let Some(c) = &bus.connector {
                add(crate::label::label_box(
                    crate::layout::LabelKind::OffSheet,
                    &c.text,
                    c.at,
                    c.side,
                ));
                for run in &bus.trunk {
                    if let Some(x) = Rect::of(run.iter().copied()) {
                        add(x);
                    }
                }
            }
        }
        // Its notes (plan 2.C).
        for (nid, n) in &layout.notes {
            let on = match block.instructions.get(nid) {
                Some(crate::layout::Instruction::Note { on, .. }) => on,
                _ => continue,
            };
            if on.strip_prefix("region:") == Some(id.as_str())
                && n.sheet == r.sheet
                && let Some(text) = block.note_text(nid)
            {
                add(crate::paint::note_box(text, n.at));
            }
        }
        let Some(mut f) = b else { continue };
        f = f.grow(FRAME);
        f.max[1] += TITLE;
        out.insert(id.clone(), f.shift([-r.at[0], -r.at[1]]));
    }
    for (id, f) in out {
        if let Some(r) = layout.regions.get_mut(&id) {
            r.frame = f;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_are_proposed_from_function_in_order() {
        let block = crate::testing::regions_block();
        let r = crate::instr::resolve(&block).unwrap();
        let idioms = crate::idiom::find(&block, &r.idiom);
        let got = propose(&block, &idioms, &r);
        let titled: Vec<(&str, Vec<&str>)> = got
            .iter()
            .map(|g| {
                let mut keys: Vec<&str> = g
                    .parts
                    .iter()
                    .map(|p| block.parts[*p].key.as_str())
                    .collect();
                keys.sort();
                (g.title.as_str(), keys)
            })
            .collect();
        assert_eq!(
            titled,
            vec![
                ("TPS54331 converter", vec!["c2", "l1", "u2"]),
                ("U1 LAN8710A", vec!["c1", "r1", "u1"]),
                ("J1", vec!["j1"]),
                ("F1", vec!["c9", "r9"]),
            ],
            "topologies, dense ICs, connectors, then connected groups; ports in none"
        );
        let mut named = block.clone();
        named.parts[7].function = "Anti-alias filter".into();
        let got = propose(&named, &idioms, &r);
        assert_eq!(
            got[3].title, "Anti-alias filter",
            "a function names its region"
        );
    }

    #[test]
    fn named_regions_are_kept_and_none_drops_a_part_back() {
        use crate::layout::Instruction;
        let mut block = crate::testing::regions_block();
        block.instructions.insert(
            "i1".into(),
            Instruction::Region {
                title: "Reset".into(),
                parts: vec!["r1".into(), "j1".into()],
            },
        );
        let out = crate::engine::lay_out(&block).unwrap();
        let reset = out
            .layout
            .regions
            .values()
            .find(|r| r.title == "Reset")
            .expect("the named region");
        assert!(reset.named);
        assert_eq!(reset.parts, ["j1", "r1"]);
        // Taken by a named region, R1 leaves U1's.
        assert!(
            out.layout
                .regions
                .values()
                .all(|r| r.title == "Reset" || !r.parts.contains(&"r1".to_string()))
        );
        // A region can be a subject: `sheet region:<id> S` moves its parts.
        let id = out
            .layout
            .regions
            .iter()
            .find(|(_, r)| r.title == "F1")
            .map(|(id, _)| id.clone())
            .unwrap();
        let mut moved = block.clone();
        moved.previous = Some(out.layout.clone());
        moved.moves = vec![Instruction::Sheet {
            parts: vec![format!("region:{id}")],
            sheet: "Filters".into(),
        }];
        let out2 = crate::engine::lay_out(&moved).unwrap();
        let sheet_of = |k: &str| out2.layout.placed[k].sheet.clone();
        assert_eq!(sheet_of("r9"), sheet_of("c9"));
        assert_ne!(sheet_of("r9"), sheet_of("u1"));
        // Two regions naming one part: refused.
        let mut both = block.clone();
        both.instructions.insert(
            "i2".into(),
            Instruction::Region {
                title: "Other".into(),
                parts: vec!["r1".into()],
            },
        );
        let e = crate::engine::lay_out(&both).unwrap_err();
        assert_eq!(e.code, "instruction_conflict");
    }
}
