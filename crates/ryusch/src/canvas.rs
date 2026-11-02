//! A block's canvas: where its sheets sit, as a block diagram reads. Power
//! sheets go in a row above, the rest in columns by the flow of the signals
//! between them (sources left, sinks right), a column's sheets one under
//! another. A sheet the previous layout placed keeps its place; a new one
//! takes its place by flow, else the free place nearest it.

use crate::geom::{GRID, Nm, Point, Rect};
use crate::input::{Block, Flow, NetKind, PartKind, PinKind};
use crate::layout::Layout;
use std::collections::{BTreeMap, BTreeSet};

/// Between sheets on a block's canvas.
pub const GAP: Nm = 10 * GRID;

/// The parts on each sheet of `layout`, by sheet ID.
fn parts_on(block: &Block, layout: &Layout) -> BTreeMap<String, Vec<usize>> {
    let mut out: BTreeMap<String, Vec<usize>> = layout
        .sheets
        .keys()
        .map(|s| (s.clone(), Vec::new()))
        .collect();
    for (p, part) in block.parts.iter().enumerate() {
        if let Some(x) = layout.placed.get(&part.key)
            && let Some(list) = out.get_mut(&x.sheet)
        {
            list.push(p);
        }
    }
    out
}

/// Whether a part belongs to the supply: it gives a supply (a power output
/// pin, a power port), or every pin of it that is connected is on a rail.
fn supplies(block: &Block, p: usize) -> bool {
    let part = &block.parts[p];
    if matches!(part.kind, PartKind::Port { flow: Flow::Power }) {
        return true;
    }
    if part.pins.iter().any(|x| x.kind == PinKind::PowerOut) {
        return true;
    }
    let mut on = part.pins.iter().filter_map(|x| x.net.filter(|_| !x.nc));
    let mut any = false;
    let all = on.all(|n| {
        any = true;
        matches!(block.nets[n].kind, NetKind::Supply | NetKind::Ground)
    });
    any && all
}

/// Whether a sheet is a power sheet: it gives a supply, and at least half
/// its parts belong to the supply.
fn is_power(block: &Block, parts: &[usize]) -> bool {
    let gives = parts.iter().any(|p| {
        let part = &block.parts[*p];
        part.pins.iter().any(|x| x.kind == PinKind::PowerOut)
            || matches!(part.kind, PartKind::Port { flow: Flow::Power })
    });
    let n = parts.iter().filter(|p| supplies(block, **p)).count();
    gives && 2 * n >= parts.len()
}

/// Whether a pin starts the signal on its net: it drives it, or it is a
/// port carrying a signal in.
fn starts(block: &Block, p: usize, k: usize) -> bool {
    let part = &block.parts[p];
    part.pins[k].kind.drives() || matches!(part.kind, PartKind::Port { flow: Flow::In })
}

/// Whether a pin ends the signal on its net: it takes it, or it is a port
/// carrying a signal out.
fn ends(block: &Block, p: usize, k: usize) -> bool {
    let part = &block.parts[p];
    part.pins[k].kind.takes() || matches!(part.kind, PartKind::Port { flow: Flow::Out })
}

/// Each sheet's column by the flow between sheets: the longest path to it
/// from a sheet nothing flows into, edges closing a cycle left out (a
/// depth-first walk in sheet ID order).
fn columns(
    block: &Block,
    on: &BTreeMap<String, Vec<usize>>,
    power: &BTreeSet<String>,
) -> BTreeMap<String, usize> {
    let sheet_of: BTreeMap<usize, &String> = on
        .iter()
        .flat_map(|(s, list)| list.iter().map(move |p| (*p, s)))
        .collect();
    let mut from: BTreeMap<usize, BTreeSet<&String>> = BTreeMap::new();
    let mut to: BTreeMap<usize, BTreeSet<&String>> = BTreeMap::new();
    for (p, s) in &sheet_of {
        for (k, pin) in block.parts[*p].pins.iter().enumerate() {
            let Some(n) = pin.net.filter(|_| !pin.nc) else {
                continue;
            };
            if block.nets[n].kind != NetKind::Signal {
                continue;
            }
            if starts(block, *p, k) {
                from.entry(n).or_default().insert(*s);
            }
            if ends(block, *p, k) {
                to.entry(n).or_default().insert(*s);
            }
        }
    }
    let mut next: BTreeMap<&String, BTreeSet<&String>> = BTreeMap::new();
    for (n, a) in &from {
        for b in to.get(n).into_iter().flatten() {
            for s in a {
                if s != b && !power.contains(*s) && !power.contains(*b) {
                    next.entry(*s).or_default().insert(*b);
                }
            }
        }
    }
    // Edges closing a cycle go: a depth-first walk keeps the rest.
    let mut kept: BTreeMap<&String, BTreeSet<&String>> = BTreeMap::new();
    let mut state: BTreeMap<&String, u8> = BTreeMap::new();
    fn walk<'a>(
        s: &'a String,
        next: &BTreeMap<&'a String, BTreeSet<&'a String>>,
        state: &mut BTreeMap<&'a String, u8>,
        kept: &mut BTreeMap<&'a String, BTreeSet<&'a String>>,
    ) {
        state.insert(s, 1);
        for t in next.get(s).into_iter().flatten() {
            match state.get(t).copied().unwrap_or(0) {
                0 => {
                    kept.entry(s).or_default().insert(*t);
                    walk(t, next, state, kept);
                }
                2 => {
                    kept.entry(s).or_default().insert(*t);
                }
                _ => {}
            }
        }
        state.insert(s, 2);
    }
    for s in on.keys() {
        if !state.contains_key(s) {
            walk(s, &next, &mut state, &mut kept);
        }
    }
    // Longest paths, in an order every edge goes forward in.
    let mut col: BTreeMap<String, usize> = on.keys().map(|s| (s.clone(), 0)).collect();
    for _ in 0..on.len() {
        let mut moved = false;
        for (s, list) in &kept {
            let c = col[*s] + 1;
            for t in list {
                if col[*t] < c {
                    col.insert((*t).clone(), c);
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
    col
}

/// Where each sheet would sit on a canvas laid out from nothing: its
/// corner, by sheet ID.
fn by_flow(block: &Block, layout: &Layout) -> BTreeMap<String, Point> {
    let on = parts_on(block, layout);
    let power: BTreeSet<String> = on
        .iter()
        .filter(|(_, list)| is_power(block, list))
        .map(|(s, _)| s.clone())
        .collect();
    let col = columns(block, &on, &power);
    let (w, h) = (block.sheet[0] + GAP, block.sheet[1] + GAP);
    let mut out = BTreeMap::new();
    let mut rows: BTreeMap<usize, Nm> = BTreeMap::new();
    let mut across = 0;
    for s in on.keys() {
        if power.contains(s) {
            out.insert(s.clone(), [across * w, h]);
            across += 1;
        } else {
            let c = col[s];
            let r = rows.entry(c).or_insert(0);
            out.insert(s.clone(), [c as Nm * w, -*r * h]);
            *r += 1;
        }
    }
    out
}

/// A sheet's rectangle on the canvas, its corner at `at`.
fn rect(block: &Block, at: Point) -> Rect {
    Rect::new(at, [at[0] + block.sheet[0], at[1] + block.sheet[1]])
}

/// Places `layout`'s sheets on the canvas: those `prev` placed where it
/// placed them (unless they would now overlap: the paper changed), the rest
/// by flow, else at the free place on the canvas's grid of sheets nearest
/// where flow puts them.
pub fn arrange(block: &Block, layout: &mut Layout, prev: Option<&Layout>) {
    let want = by_flow(block, layout);
    let mut placed: BTreeMap<String, Point> = BTreeMap::new();
    if let Some(prev) = prev {
        for id in layout.sheets.keys() {
            if let Some(s) = prev.sheets.get(id) {
                placed.insert(id.clone(), s.at);
            }
        }
        let rects: Vec<Rect> = placed.values().map(|at| rect(block, *at)).collect();
        let overlap = rects
            .iter()
            .enumerate()
            .any(|(i, a)| rects[i + 1..].iter().any(|b| a.overlaps(b)));
        if overlap {
            placed.clear();
        }
    }
    let (w, h) = (block.sheet[0] + GAP, block.sheet[1] + GAP);
    for (id, at) in &want {
        if placed.contains_key(id) {
            continue;
        }
        let free = |p: Point| {
            placed
                .values()
                .all(|q| !rect(block, p).overlaps(&rect(block, *q)))
        };
        let at = if free(*at) {
            *at
        } else {
            let (c0, r0) = (at[0] / w, at[1] / h);
            let n = placed.len() as Nm + 1;
            // Nearest first; at one distance not above it (the row above
            // is the power sheets'), then the leftmost, then the highest.
            let mut slots: Vec<(Nm, bool, Nm, Nm)> = Vec::new();
            for c in 0..=c0 + n {
                for r in -n..=1 {
                    slots.push(((c - c0).abs() + (r - r0).abs(), r > r0, c, -r));
                }
            }
            slots.sort();
            slots
                .into_iter()
                .map(|(_, _, c, r)| [c * w, -r * h])
                .find(|p| free(*p))
                .expect("some place is free")
        };
        placed.insert(id.clone(), at);
    }
    for (id, s) in &mut layout.sheets {
        s.at = placed[id];
    }
}

/// The sheets in canvas order: the top row first, each row left to right.
pub fn order(layout: &Layout) -> Vec<String> {
    let mut list: Vec<(&String, Point)> = layout.sheets.iter().map(|(k, s)| (k, s.at)).collect();
    list.sort_by_key(|(k, at)| (-at[1], at[0], (*k).clone()));
    list.into_iter().map(|(k, _)| k.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Net, Part};
    use crate::layout::{Placed, Sheet};
    use crate::testing;

    fn sheet(l: &mut Layout, id: &str) {
        l.sheets.insert(
            id.into(),
            Sheet {
                name: id.into(),
                at: [0, 0],
            },
        );
    }

    fn put(l: &mut Layout, key: &str, s: &str) {
        l.placed.insert(
            key.into(),
            Placed {
                sheet: s.into(),
                at: [0, 0],
                rotation: 0,
                mirror: false,
                held: false,
            },
        );
    }

    /// A buffer driving net `out` from net `inp`.
    fn buffer(key: &str, inp: usize, out: usize) -> Part {
        testing::ic(
            key,
            "U1",
            "",
            &[("IN", PinKind::Input, Some(inp))],
            &[("OUT", PinKind::Output, Some(out))],
        )
    }

    #[test]
    fn sheets_go_by_flow_with_power_above() {
        let a4 = [250_000_000, 170_000_000];
        let nets = vec![
            Net {
                key: "a".into(),
                name: "A".into(),
                kind: NetKind::Signal,
            },
            Net {
                key: "b".into(),
                name: "B".into(),
                kind: NetKind::Signal,
            },
            Net {
                key: "c".into(),
                name: "C".into(),
                kind: NetKind::Signal,
            },
            Net {
                key: "rail:v".into(),
                name: "3V3".into(),
                kind: NetKind::Supply,
            },
            Net {
                key: "rail:g".into(),
                name: "GND".into(),
                kind: NetKind::Ground,
            },
        ];
        let mut ldo = testing::resistor("ldo", "U9", "", [Some(3), Some(4)]);
        ldo.pins[0].kind = PinKind::PowerOut;
        let block = Block {
            sheet: a4,
            parts: vec![
                buffer("u3", 1, 2),
                buffer("u1", 0, 1),
                ldo,
                testing::capacitor("c1", "C1", "", [Some(3), Some(4)]),
            ],
            nets,
            ..Block::default()
        };
        let mut l = Layout::default();
        for s in ["s1", "s2", "s3"] {
            sheet(&mut l, s);
        }
        // s1 takes B and drives C; s2 drives B; s3 is the supply.
        put(&mut l, "u3", "s1");
        put(&mut l, "u1", "s2");
        put(&mut l, "ldo", "s3");
        put(&mut l, "c1", "s3");
        arrange(&block, &mut l, None);
        let (w, h) = (a4[0] + GAP, a4[1] + GAP);
        assert_eq!(l.sheets["s2"].at, [0, 0]);
        assert_eq!(l.sheets["s1"].at, [w, 0]);
        assert_eq!(l.sheets["s3"].at, [0, h]);
        assert_eq!(order(&l), ["s3", "s2", "s1"]);
        // Kept where they were; a new sheet takes the place nearest its
        // flow's that is free.
        let prev = l.clone();
        sheet(&mut l, "s4");
        put(&mut l, "u1", "s4");
        arrange(&block, &mut l, Some(&prev));
        for s in ["s1", "s2", "s3"] {
            assert_eq!(l.sheets[s].at, prev.sheets[s].at, "{s}");
        }
        assert_eq!(l.sheets["s4"].at, [0, -h]);
        // A paper that makes kept sheets overlap lays the canvas out again.
        let big = Block {
            sheet: [2 * a4[0], a4[1]],
            ..block.clone()
        };
        arrange(&big, &mut l, Some(&prev));
        let rects: Vec<Rect> = l.sheets.values().map(|s| rect(&big, s.at)).collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(rects[i + 1..].iter().all(|b| !a.overlaps(b)));
        }
    }
}
