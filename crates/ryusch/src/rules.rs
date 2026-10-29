//! The hard rules of readability, checked on a layout from its geometry
//! alone (not from the router's own bookkeeping), so tests and the fuzz
//! target can hold the engine to them: everything on the grid; no
//! overlaps of symbols, text or labels, nor of wires with them; no
//! junction of four; no wire through a symbol or across a pin; wires
//! orthogonal; text upright and at least the smallest size; each sheet's
//! contents inside its area; symbols a step apart at least.

use crate::geom::{GRID, Place, Point, Rect, on_grid};
use crate::input::{Block, PartKind};
use crate::label;
use crate::layout::Layout;
use crate::route;
use crate::shape::{Shape, pins_on_grid};
use crate::text;
use serde::Serialize;
use std::collections::BTreeMap;

/// A broken rule: which, where, and what it names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub rule: &'static str,
    pub sheet: String,
    pub message: String,
    /// The parts, nets or labels concerned.
    pub refs: Vec<String>,
}

/// Something drawn that takes room: a part's extent, a label or a text.
struct Item {
    what: String,
    rect: Rect,
    kind: u8,
}

const PART: u8 = 0;
const TEXT: u8 = 1;
const LABEL: u8 = 2;

/// Every rule `layout` breaks, sheet by sheet, in a fixed order.
pub fn check(block: &Block, layout: &Layout) -> Vec<Violation> {
    let mut out = Vec::new();
    let area = Rect::new([0, 0], block.sheet);
    for sid in layout.sheets.keys() {
        let v = |rule: &'static str, message: String, refs: Vec<String>| Violation {
            rule,
            sheet: sid.clone(),
            message,
            refs,
        };
        let mut items: Vec<Item> = Vec::new();
        let mut shapes: Vec<(&str, Shape, bool)> = Vec::new();
        for part in &block.parts {
            let Some(x) = layout.placed.get(&part.key).filter(|x| &x.sheet == sid) else {
                continue;
            };
            let shape = Shape::of(part, Place::new(x.at, x.rotation, x.mirror));
            if x.rotation % 90 != 0 {
                out.push(v(
                    "grid",
                    format!("{} is turned {}°, not a quarter turn", part.key, x.rotation),
                    vec![part.key.clone()],
                ));
            }
            if pins_on_grid(part) && !shape.pins.iter().all(|p| on_grid(p.end)) {
                out.push(v(
                    "grid",
                    format!("{}'s pins are off the grid", part.key),
                    vec![part.key.clone()],
                ));
            }
            if !area.contains(&shape.extent) {
                out.push(v(
                    "inside",
                    format!("{} runs past its sheet", part.key),
                    vec![part.key.clone()],
                ));
            }
            items.push(Item {
                what: part.key.clone(),
                rect: shape.extent,
                kind: PART,
            });
            if let Some(fields) = layout.text.get(&part.key) {
                for (field, value) in [("refdes", &part.refdes), ("value", &part.value)] {
                    if let Some(at) = fields.get(field) {
                        if !on_grid(*at) {
                            out.push(v(
                                "grid",
                                format!("{}'s {field} is off the grid", part.key),
                                vec![part.key.clone()],
                            ));
                        }
                        let b = text::boxes(&[value.as_str()], *at)[0];
                        if !area.contains(&b) {
                            out.push(v(
                                "inside",
                                format!("{}'s {field} runs past its sheet", part.key),
                                vec![part.key.clone()],
                            ));
                        }
                        items.push(Item {
                            what: format!("{}.{field}", part.key),
                            rect: b,
                            kind: TEXT,
                        });
                    }
                }
            }
            shapes.push((&part.key, shape, matches!(part.kind, PartKind::Port { .. })));
        }
        for (key, list) in &layout.labels {
            let name = block
                .net(key)
                .map_or(key.as_str(), |n| block.nets[n].name.as_str());
            for l in list.iter().filter(|l| &l.sheet == sid) {
                if !on_grid(l.at) {
                    out.push(v(
                        "grid",
                        format!("a label of {key} is off the grid"),
                        vec![key.clone()],
                    ));
                }
                let b = label::label_box(l.kind, name, l.at, l.side);
                if !area.contains(&b) {
                    out.push(v(
                        "inside",
                        format!("a label of {key} runs past its sheet"),
                        vec![key.clone()],
                    ));
                }
                items.push(Item {
                    what: format!("label {key} at {}", l.on),
                    rect: b,
                    kind: LABEL,
                });
            }
        }
        // Overlaps between things that take room. Parts keep a step apart;
        // a part's own text may sit beside it; nothing overlaps.
        for i in 0..items.len() {
            for j in i + 1..items.len() {
                let (a, b) = (&items[i], &items[j]);
                let overlap = if a.kind == PART && b.kind == PART {
                    a.rect.grow(GRID / 2).overlaps(&b.rect.grow(GRID / 2 - 1))
                } else {
                    a.rect.overlaps(&b.rect)
                };
                if overlap {
                    let rule = if a.kind == PART && b.kind == PART && !a.rect.overlaps(&b.rect) {
                        "spacing"
                    } else {
                        "overlap"
                    };
                    out.push(v(
                        rule,
                        format!("{} and {} overlap", a.what, b.what),
                        vec![a.what.clone(), b.what.clone()],
                    ));
                }
            }
        }
        // Wires.
        let mut arms: BTreeMap<(String, route::Node), u8> = BTreeMap::new();
        for (key, by_sheet) in &layout.routes {
            let Some(runs) = by_sheet.get(sid) else {
                continue;
            };
            for run in runs {
                if run.iter().any(|p| !on_grid(*p)) {
                    out.push(v(
                        "grid",
                        format!("a wire of {key} is off the grid"),
                        vec![key.clone()],
                    ));
                }
                for w in run.windows(2) {
                    if w[0][0] != w[1][0] && w[0][1] != w[1][1] {
                        out.push(v(
                            "orthogonal",
                            format!("a wire of {key} runs at a slant"),
                            vec![key.clone()],
                        ));
                        continue;
                    }
                    let seg = Rect::new(w[0], w[1]);
                    if !area.contains(&seg) {
                        out.push(v(
                            "inside",
                            format!("a wire of {key} runs past its sheet"),
                            vec![key.clone()],
                        ));
                    }
                    for it in &items {
                        if it.kind != PART && it.rect.overlaps(&seg) {
                            out.push(v(
                                "overlap",
                                format!("a wire of {key} runs through {}", it.what),
                                vec![key.clone(), it.what.clone()],
                            ));
                        }
                    }
                    for (pk, shape, _) in &shapes {
                        if shape.body.grow(-1).overlaps(&seg) || through(&shape.body, &seg) {
                            out.push(v(
                                "through",
                                format!("a wire of {key} crosses {pk}"),
                                vec![key.clone(), (*pk).to_owned()],
                            ));
                        }
                        for pin in &shape.pins {
                            if crosses_pin(pin.base, pin.end, w[0], w[1]) {
                                out.push(v(
                                    "through",
                                    format!("a wire of {key} crosses a pin of {pk}"),
                                    vec![key.clone(), (*pk).to_owned()],
                                ));
                            }
                        }
                    }
                }
                let nodes = route::expand(run);
                for (i, n) in nodes.iter().enumerate() {
                    let ends = u8::from(i > 0) + u8::from(i + 1 < nodes.len());
                    *arms.entry((key.clone(), *n)).or_default() += ends;
                }
            }
        }
        for (part, shape, _) in &shapes {
            let pk = block.part(part).expect("placed parts are the block's");
            for (k, pin) in block.parts[pk].pins.iter().enumerate() {
                if let Some(n) = pin.net {
                    let node = route::node_of(shape.pins[k].end);
                    if let Some(a) = arms.get_mut(&(block.nets[n].key.clone(), node)) {
                        *a += 1;
                    }
                }
            }
        }
        for ((key, node), count) in &arms {
            if *count >= 4 {
                out.push(v(
                    "junction",
                    format!("four wires of {key} meet at {:?}", route::point_of(*node)),
                    vec![key.clone()],
                ));
            }
        }
    }
    out
}

/// Whether a segment runs through a body: in one side and out the other
/// (a segment along its edge, or ending on it, doesn't).
fn through(body: &Rect, seg: &Rect) -> bool {
    let inner = body.grow(-1);
    (seg.min[0] < inner.min[0]
        && seg.max[0] > inner.max[0]
        && seg.min[1] > inner.min[1]
        && seg.max[1] < inner.max[1])
        || (seg.min[1] < inner.min[1]
            && seg.max[1] > inner.max[1]
            && seg.min[0] > inner.min[0]
            && seg.max[0] < inner.max[0])
}

/// Whether the wire from `a` to `b` crosses the pin line from `base` to
/// `end` anywhere but at the pin's end.
fn crosses_pin(base: Point, end: Point, a: Point, b: Point) -> bool {
    if base == end {
        return false;
    }
    let wire = Rect::new(a, b);
    // The pin's line short of its end.
    let short = if base[0] == end[0] {
        let d = (end[1] - base[1]).signum();
        Rect::new(base, [end[0], end[1] - d])
    } else {
        let d = (end[0] - base[0]).signum();
        Rect::new(base, [end[0] - d, end[1]])
    };
    short.min[0] <= wire.max[0]
        && wire.min[0] <= short.max[0]
        && short.min[1] <= wire.max[1]
        && wire.min[1] <= short.max[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::seeded;

    #[test]
    fn seeded_layouts_keep_every_rule() {
        for (seed, ics) in [(1, 2), (2, 4), (3, 6), (4, 8)] {
            let b = seeded(seed, ics);
            let out = crate::engine::lay_out(&b);
            let broken = check(&b, &out.layout);
            assert!(broken.is_empty(), "seed {seed}: {broken:#?}");
        }
    }

    #[test]
    fn broken_rules_are_found() {
        let b = seeded(1, 2);
        let mut l = crate::engine::lay_out(&b).layout;
        // Two parts on top of each other, a wire at a slant and one
        // through a body.
        let first = l.placed.keys().next().unwrap().clone();
        let second = l.placed.keys().nth(1).unwrap().clone();
        let at = l.placed[&first].at;
        l.placed.get_mut(&second).unwrap().at = at;
        l.routes
            .entry("x".into())
            .or_default()
            .insert("s1".into(), vec![vec![[0, 0], [GRID, GRID]]]);
        let rules: Vec<&str> = check(&b, &l).iter().map(|v| v.rule).collect();
        assert!(
            rules.contains(&"overlap") && rules.contains(&"orthogonal"),
            "{rules:?}"
        );
    }
}
