//! The hard rules of readability, checked on a layout from its geometry
//! alone (not from the router's own bookkeeping), so tests and the fuzz
//! target can hold the engine to them: everything on the grid; no
//! overlaps of symbols, text or labels, nor of wires with them; no
//! junction of four; no wire through a symbol or across a pin; every pin
//! on a net drawn joined to it, by a wire or a label; wires
//! orthogonal; text upright and at least the smallest size; each sheet's
//! contents inside its area; symbols a step apart at least; regions apart
//! (`region_overlap`), each holding its parts, their text and labels, and
//! a part in one region at most (`region_holds`).

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
    /// What a part draws: its body and its pins' lines, which text and
    /// labels may not touch (the rest of its box is empty).
    ink: Vec<Rect>,
    /// A part of two pins lying across the page, its pins' row: two such
    /// on different rows may stand closer than a step (a bank's members,
    /// rows two steps apart), their extents still apart.
    row: Option<i64>,
    /// A net label's net and its line (a name on its wire): a wire of
    /// that net may run along the line, clear of the text alone.
    on_wire: Option<(String, Rect, Rect)>,
}

const PART: u8 = 0;
const TEXT: u8 = 1;
const LABEL: u8 = 2;

/// Every rule `layout` breaks, sheet by sheet, in a fixed order.
pub fn check(block: &Block, layout: &Layout) -> Vec<Violation> {
    let mut out = Vec::new();
    // Which region each part is in, across sheets.
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for sid in layout.sheets.keys() {
        let area = Rect::new([0, 0], block.area_in(layout, sid));
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
            let mut ink = vec![shape.body];
            ink.extend(shape.pins.iter().map(|p| Rect::new(p.base, p.end)));
            let row = (shape.pins.len() == 2
                && shape.pins.iter().all(|p| p.side.horizontal())
                && shape.pins[0].end[1] == shape.pins[1].end[1])
                .then(|| shape.pins[0].end[1]);
            items.push(Item {
                what: part.key.clone(),
                rect: shape.extent,
                kind: PART,
                ink,
                row,
                on_wire: None,
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
                            ink: Vec::new(),
                            row: None,
                            on_wire: None,
                        });
                    }
                }
            }
            shapes.push((&part.key, shape, matches!(part.kind, PartKind::Port { .. })));
        }
        for (key, list) in &layout.labels {
            for l in list.iter().filter(|l| &l.sheet == sid) {
                let name = l.text.as_str();
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
                let on_wire = (l.kind == crate::layout::LabelKind::Net).then(|| {
                    let d = label::drawn(l.kind, name, l.at, l.side);
                    let line = Rect::of(d.lines.iter().flatten().copied()).expect("a line");
                    let text = d
                        .text
                        .map(|(at, anchor, _)| {
                            crate::font::text_box(name, crate::font::SIZE, at, anchor)
                        })
                        .unwrap_or(line);
                    (key.clone(), line, text)
                });
                items.push(Item {
                    what: format!("label {key} at {}", l.on),
                    rect: b,
                    kind: LABEL,
                    ink: Vec::new(),
                    row: None,
                    on_wire,
                });
            }
        }
        // Overlaps between things that take room. Parts keep a step apart;
        // a part's own text may sit beside it; nothing overlaps.
        for i in 0..items.len() {
            for j in i + 1..items.len() {
                let (a, b) = (&items[i], &items[j]);
                let overlap = match (a.kind == PART, b.kind == PART) {
                    (true, true) => match (a.row, b.row) {
                        (Some(ra), Some(rb)) if ra != rb => a.rect.overlaps(&b.rect),
                        _ => a.rect.grow(GRID / 2).overlaps(&b.rect.grow(GRID / 2 - 1)),
                    },
                    (true, false) => a.ink.iter().any(|r| touches(r, &b.rect)),
                    (false, true) => b.ink.iter().any(|r| touches(r, &a.rect)),
                    (false, false) => a.rect.overlaps(&b.rect),
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
                        // A net label lies along a wire of its own net: the
                        // wire may run on its line, never through its text.
                        let rect = match &it.on_wire {
                            Some((net, line, text))
                                if net == key
                                    && (line.min[1] == line.max[1]
                                        && seg.min[1] == seg.max[1]
                                        && seg.min[1] == line.min[1]
                                        || line.min[0] == line.max[0]
                                            && seg.min[0] == seg.max[0]
                                            && seg.min[0] == line.min[0]) =>
                            {
                                *text
                            }
                            _ => it.rect,
                        };
                        if it.kind != PART && rect.overlaps(&seg) {
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
        // Every pin on a net is drawn connected: on one of its wires, or
        // with a label of its own (a port's flag names its net itself).
        for (part, shape, port) in &shapes {
            let pk = block.part(part).expect("placed parts are the block's");
            for (k, pin) in block.parts[pk].pins.iter().enumerate() {
                let Some(n) = pin.drawn_net() else {
                    continue;
                };
                let key = &block.nets[n].key;
                let node = route::node_of(shape.pins[k].end);
                let wired = layout
                    .routes
                    .get(key)
                    .and_then(|r| r.get(sid))
                    .is_some_and(|runs| runs.iter().any(|run| route::expand(run).contains(&node)));
                let on = format!("{part}/{}", pin.number);
                let labelled = *port
                    || layout
                        .labels
                        .get(key)
                        .is_some_and(|l| l.iter().any(|x| &x.sheet == sid && x.on == on));
                if !wired && !labelled {
                    out.push(v(
                        "connected",
                        format!("{on} is on {key} but drawn joined to nothing"),
                        vec![on.clone(), key.clone()],
                    ));
                }
                // A stacked pin joins its layers' nets by its label alone.
                if pin.stacked() && !labelled {
                    for m in pin.nets().into_iter().filter(|m| *m != n) {
                        let other = &block.nets[m].key;
                        out.push(v(
                            "connected",
                            format!("{on} is on {other} in a layer but has no label naming it"),
                            vec![on.clone(), other.clone()],
                        ));
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
        // Regions: apart from each other, each holding its parts, their
        // text and their labels; a part in one region at most.
        let regions: Vec<(&String, &crate::layout::Region)> = layout
            .regions
            .iter()
            .filter(|(_, r)| &r.sheet == sid)
            .collect();
        for (i, (a, ra)) in regions.iter().enumerate() {
            for (b, rb) in &regions[i + 1..] {
                if ra.frame_on_sheet().overlaps(&rb.frame_on_sheet()) {
                    out.push(v(
                        "region_overlap",
                        format!("regions {a} ({}) and {b} ({}) overlap", ra.title, rb.title),
                        vec![(*a).clone(), (*b).clone()],
                    ));
                }
            }
        }
        for (id, r) in &regions {
            let frame = r.frame_on_sheet();
            for k in &r.parts {
                if let Some(was) = seen.insert(k.as_str(), id.as_str()) {
                    out.push(v(
                        "region_holds",
                        format!("{k} is in two regions, {was} and {id}"),
                        vec![k.clone(), (*id).clone()],
                    ));
                }
                let text = format!("{k}.");
                let label = format!(" at {k}/");
                let inside = items
                    .iter()
                    .filter(|it| {
                        it.what == *k
                            || it.what.starts_with(&text)
                            || (it.kind == LABEL && it.what.contains(&label))
                    })
                    .all(|it| frame.contains(&it.rect));
                let placed_here = layout.placed.get(k).is_some_and(|x| &x.sheet == sid);
                if !placed_here || !inside {
                    out.push(v(
                        "region_holds",
                        format!("region {id} ({}) doesn't hold {k}", r.title),
                        vec![k.clone(), (*id).clone()],
                    ));
                }
            }
        }
    }
    out
}

/// Whether a box touches what a part draws there: its body, or a pin's
/// line (a box with no area).
fn touches(ink: &Rect, r: &Rect) -> bool {
    ink.min[0] <= r.max[0]
        && r.min[0] <= ink.max[0]
        && ink.min[1] <= r.max[1]
        && r.min[1] <= ink.max[1]
        && (ink.overlaps(r) || ink.width() == 0 || ink.height() == 0)
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
            let out = crate::engine::lay_out(&b).unwrap();
            let broken = check(&b, &out.layout);
            assert!(broken.is_empty(), "seed {seed}: {broken:#?}");
        }
    }

    #[test]
    fn broken_rules_are_found() {
        let b = seeded(1, 2);
        let mut l = crate::engine::lay_out(&b).unwrap().layout;
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

    /// Two parts of two pins lying across the page on rows two steps
    /// apart (a bank's members) may stand closer than a step; on one row,
    /// or with extents overlapping, they break `spacing` or `overlap`.
    #[test]
    fn parts_across_the_page_on_different_rows_may_stand_close() {
        use crate::layout::Placed;
        let b = crate::testing::dense_block(false);
        let l = crate::engine::lay_out(&b).unwrap().layout;
        assert_eq!(check(&b, &l), [], "the bank's rows two steps apart");
        // R2 standing up a step past R1's end: not across the page, too
        // close.
        let mut close = l.clone();
        let r1: Placed = close.placed["r1"].clone();
        let r2 = close.placed.get_mut("r2").unwrap();
        r2.at = [r1.at[0] + 4 * GRID, r1.at[1]];
        r2.rotation = 0;
        let rules: Vec<&str> = check(&b, &close).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"spacing"), "{rules:?}");
        // R2 a step under R1: their extents overlap.
        let mut over = l.clone();
        over.placed.get_mut("r2").unwrap().at = [r1.at[0], r1.at[1] - GRID];
        let rules: Vec<&str> = check(&b, &over).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"overlap"), "{rules:?}");
    }

    /// A net label lies along a wire of its own net (a bank's row on a
    /// port's net), but no wire may cross its text.
    #[test]
    fn a_net_label_lies_along_its_wire_but_not_across_its_text() {
        let b = crate::testing::dense_block(true);
        let l = crate::engine::lay_out(&b).unwrap().layout;
        assert_eq!(check(&b, &l), []);
        let label = l.labels["s0"]
            .iter()
            .find(|x| x.kind == crate::layout::LabelKind::Net && x.on.starts_with("u1/"))
            .expect("the row's label")
            .clone();
        // A wire of S0 down through the label's text.
        let x = label.at[0] + 2 * GRID;
        let mut across = l.clone();
        across
            .routes
            .get_mut("s0")
            .unwrap()
            .get_mut(&label.sheet)
            .unwrap()
            .push(vec![[x, label.at[1] + 3 * GRID], [x, label.at[1]]]);
        let rules: Vec<&str> = check(&b, &across).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"overlap"), "{rules:?}");
    }

    #[test]
    fn regions_that_overlap_or_leave_out_their_parts_break_rules() {
        let block = crate::testing::regions_block();
        let out = crate::engine::lay_out(&block).unwrap();
        assert!(check(&block, &out.layout).is_empty());
        let mut bad = out.layout.clone();
        let ids: Vec<String> = bad.regions.keys().cloned().collect();
        let first = bad.regions[&ids[0]].clone();
        let second = bad.regions.get_mut(&ids[1]).unwrap();
        second.sheet = first.sheet.clone();
        second.at = first.at;
        second.frame = first.frame;
        let rules: Vec<&str> = check(&block, &bad).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"region_overlap"), "{rules:?}");
        assert!(rules.contains(&"region_holds"), "{rules:?}");
        let mut twice = out.layout.clone();
        let k = twice.regions[&ids[0]].parts[0].clone();
        twice.regions.get_mut(&ids[1]).unwrap().parts.push(k);
        assert!(
            check(&block, &twice)
                .iter()
                .any(|v| v.rule == "region_holds")
        );
    }
}
