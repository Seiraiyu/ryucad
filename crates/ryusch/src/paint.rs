//! What a sheet draws, as items in the sheet's own frame (nanometres, y
//! up): every line, circle, arc, curve and text of its symbols, pins,
//! wires, junctions and labels, in drawing order, in one fixed look (the
//! theme is stage 2's). SVG and ryueda's PDF both draw from it, so they
//! draw alike; it also says where each label and block symbol is, for the
//! PDF's cross-references and links.

use crate::font::{Anchor, MIN_SIZE, SIZE};
use crate::geom::{GRID, Nm, Place, Point, Rect, Side};
use crate::input::{Block, Fill, Graphic, PartKind};
use crate::label;
use crate::layout::{LabelKind, Layout};
use crate::route;
use crate::shape::Shape;
use std::collections::BTreeMap;

/// Symbols' lines.
pub const INK: &str = "#7a1f1f";
/// Symbols' bodies.
pub const BODY: &str = "#fff6dc";
pub const WIRE: &str = "#1b6e3a";
pub const TEXT: &str = "#202020";
/// Power ports and ground.
pub const POWER: &str = "#a01818";
/// Net labels and not-connected marks.
pub const LABEL: &str = "#1d3f8f";
/// A sheet's name, beside it on the canvas.
pub const NAME: &str = "#606060";
/// Every line's width.
pub const LINE: Nm = 150_000;

/// One thing drawn. A fill of `None` draws the outline alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Polyline {
        points: Vec<Point>,
        stroke: &'static str,
        fill: Option<&'static str>,
    },
    /// Outlined when `stroke` is given (a symbol's circle), else filled
    /// alone (a junction's dot).
    Circle {
        center: Point,
        radius: Nm,
        stroke: Option<&'static str>,
        fill: Option<&'static str>,
    },
    /// The arc from `start` through `mid` to `end`.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
        stroke: &'static str,
    },
    /// A cubic Bézier.
    Bezier {
        points: [Point; 4],
        stroke: &'static str,
    },
    /// A text on its baseline through `at`, upright or reading upwards.
    Text {
        text: String,
        at: Point,
        size: Nm,
        anchor: Anchor,
        vertical: bool,
        colour: &'static str,
    },
}

/// A label where it is drawn: its net, the pin it serves and its box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub net: String,
    pub on: String,
    pub kind: LabelKind,
    pub text: String,
    pub at: Point,
    pub side: Side,
    pub rect: Rect,
}

/// What a sheet draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Painted {
    pub items: Vec<Item>,
    pub labels: Vec<Placed>,
    /// Each child block's symbol (`instance:<id>`) and its body's box.
    pub blocks: Vec<(String, Rect)>,
}

fn line(items: &mut Vec<Item>, points: Vec<Point>, stroke: &'static str) {
    items.push(Item::Polyline {
        points,
        stroke,
        fill: None,
    });
}

fn graphic(items: &mut Vec<Item>, shape: &Shape, g: &Graphic) {
    let pl = |p: Point| shape.place.apply(p);
    let fill = |x: Fill| match x {
        Fill::None => None,
        Fill::Body => Some(BODY),
        Fill::Outline => Some(INK),
    };
    match g {
        Graphic::Rect {
            start,
            end,
            fill: x,
        } => items.push(Item::Polyline {
            points: vec![
                pl(*start),
                pl([end[0], start[1]]),
                pl(*end),
                pl([start[0], end[1]]),
                pl(*start),
            ],
            stroke: INK,
            fill: fill(*x),
        }),
        Graphic::Polyline { points, fill: x } => items.push(Item::Polyline {
            points: points.iter().map(|p| pl(*p)).collect(),
            stroke: INK,
            fill: fill(*x),
        }),
        Graphic::Circle {
            center,
            radius,
            fill: x,
        } => items.push(Item::Circle {
            center: pl(*center),
            radius: *radius,
            stroke: Some(INK),
            fill: fill(*x),
        }),
        Graphic::Arc { start, mid, end } => items.push(Item::Arc {
            start: pl(*start),
            mid: pl(*mid),
            end: pl(*end),
            stroke: INK,
        }),
        Graphic::Bezier { points } => items.push(Item::Bezier {
            points: points.map(pl),
            stroke: INK,
        }),
        Graphic::Text { text, at, size } => items.push(Item::Text {
            text: text.clone(),
            at: pl(*at),
            size: (*size).max(MIN_SIZE),
            anchor: Anchor::Start,
            vertical: false,
            colour: TEXT,
        }),
    }
}

/// Whether a part draws its pins' names: a block's always, a symbol's
/// when it has more than two pins, a port's never.
pub fn draws_names(part: &crate::input::Part) -> bool {
    match part.kind {
        PartKind::Port { .. } => false,
        PartKind::Block => true,
        PartKind::Symbol { .. } => part.pins.len() > 2,
    }
}

/// Where a pin's name is drawn: half a step inside the body from its base,
/// reading away from the pin (up the page beside a pin on the top or
/// bottom).
fn name_at(at: &crate::shape::PinAt) -> (Point, Anchor) {
    let dir = at.side.step();
    let inward = [-dir[0] / GRID, -dir[1] / GRID];
    let off = GRID / 2;
    let p = [at.base[0] + inward[0] * off, at.base[1] + inward[1] * off];
    match at.side {
        Side::Left => ([p[0], p[1] - SIZE / 3], Anchor::Start),
        Side::Right => ([p[0], p[1] - SIZE / 3], Anchor::End),
        Side::Up => ([p[0] + SIZE / 3, p[1]], Anchor::End),
        Side::Down => ([p[0] + SIZE / 3, p[1]], Anchor::Start),
    }
}

/// The box a pin's name takes, as [`sheet`] draws it.
pub fn name_box(name: &str, at: &crate::shape::PinAt) -> Rect {
    let (p, anchor) = name_at(at);
    if at.side.horizontal() {
        crate::font::text_box(name, SIZE, p, anchor)
    } else {
        // Reading up the page: turned a quarter counterclockwise about its
        // anchor.
        let flat = crate::font::text_box(name, SIZE, [0, 0], anchor);
        Rect::new(
            [p[0] - flat.max[1], p[1] + flat.min[0]],
            [p[0] - flat.min[1], p[1] + flat.max[0]],
        )
    }
}

/// A label's lines and text.
fn label_items(items: &mut Vec<Item>, kind: LabelKind, name: &str, at: Point, side: Side) {
    let d = label::drawn(kind, name, at, side);
    let colour = if kind == LabelKind::Net { LABEL } else { POWER };
    // A net label's line is the wire it names.
    let ink = if kind == LabelKind::Net { WIRE } else { POWER };
    for l in &d.lines {
        line(items, l.clone(), ink);
    }
    if let Some((p, anchor, vertical)) = d.text {
        items.push(Item::Text {
            text: name.to_owned(),
            at: p,
            size: SIZE,
            anchor,
            vertical,
            colour,
        });
    }
}

/// What sheet `sid` of `layout` draws: parts in key order (each its
/// graphics, pins, pin names and numbers, not-connected marks and text),
/// then wires and junctions, then labels.
pub fn sheet(block: &Block, layout: &Layout, sid: &str) -> Painted {
    let mut out = Painted::default();
    let items = &mut out.items;
    let mut arms: BTreeMap<(usize, route::Node), u8> = BTreeMap::new();
    for part in &block.parts {
        let Some(x) = layout.placed.get(&part.key).filter(|x| x.sheet == sid) else {
            continue;
        };
        let shape = Shape::of(part, Place::new(x.at, x.rotation, x.mirror));
        if part.kind == PartKind::Block {
            out.blocks.push((part.key.clone(), shape.body));
        }
        for g in &part.graphics {
            graphic(items, &shape, g);
        }
        let names = draws_names(part);
        for (k, pin) in part.pins.iter().enumerate() {
            let at = shape.pins[k];
            if at.base != at.end {
                line(items, vec![at.base, at.end], INK);
            }
            if let Some(n) = pin.net {
                *arms.entry((n, route::node_of(at.end))).or_default() += 1;
            }
            if names && !pin.name.is_empty() {
                let (p, anchor) = name_at(&at);
                items.push(Item::Text {
                    text: pin.name.clone(),
                    at: p,
                    size: SIZE,
                    anchor,
                    vertical: !at.side.horizontal(),
                    colour: TEXT,
                });
            }
            if pin.show_number {
                let mid = [(at.base[0] + at.end[0]) / 2, (at.base[1] + at.end[1]) / 2];
                let (p, vertical) = if at.side.horizontal() {
                    ([mid[0], mid[1] + GRID / 4], false)
                } else {
                    ([mid[0] - GRID / 4, mid[1]], true)
                };
                items.push(Item::Text {
                    text: pin.number.clone(),
                    at: p,
                    size: MIN_SIZE,
                    anchor: Anchor::Middle,
                    vertical,
                    colour: INK,
                });
            }
            if pin.nc {
                let d = GRID / 3;
                let e = at.end;
                line(
                    items,
                    vec![[e[0] - d, e[1] - d], [e[0] + d, e[1] + d]],
                    LABEL,
                );
                line(
                    items,
                    vec![[e[0] - d, e[1] + d], [e[0] + d, e[1] - d]],
                    LABEL,
                );
            }
        }
        if let Some(fields) = layout.text.get(&part.key) {
            for (field, value) in [("refdes", &part.refdes), ("value", &part.value)] {
                if let Some(at) = fields.get(field) {
                    items.push(Item::Text {
                        text: value.clone(),
                        at: *at,
                        size: SIZE,
                        anchor: Anchor::Start,
                        vertical: false,
                        colour: TEXT,
                    });
                }
            }
        }
    }
    // Wires and junctions.
    for (key, by_sheet) in &layout.routes {
        let Some(runs) = by_sheet.get(sid) else {
            continue;
        };
        let n = block.net(key).unwrap_or(usize::MAX);
        for run in runs {
            line(items, run.clone(), WIRE);
            let nodes = route::expand(run);
            for (i, node) in nodes.iter().enumerate() {
                let ends = u8::from(i > 0) + u8::from(i + 1 < nodes.len());
                *arms.entry((n, *node)).or_default() += ends;
            }
        }
    }
    for ((_, node), count) in &arms {
        if *count >= 3 {
            items.push(Item::Circle {
                center: route::point_of(*node),
                radius: GRID / 4,
                stroke: None,
                fill: Some(WIRE),
            });
        }
    }
    for (key, list) in &layout.labels {
        for l in list.iter().filter(|l| l.sheet == sid) {
            label_items(items, l.kind, &l.text, l.at, l.side);
            out.labels.push(Placed {
                net: key.clone(),
                on: l.on.clone(),
                kind: l.kind,
                text: l.text.clone(),
                at: l.at,
                side: l.side,
                rect: label::label_box(l.kind, &l.text, l.at, l.side),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::seeded;

    #[test]
    fn a_sheet_paints_its_labels_and_says_where_they_are() {
        let b = seeded(2, 4);
        let out = crate::engine::lay_out(&b).unwrap();
        let sid = out.layout.sheets.keys().next().unwrap();
        let p = sheet(&b, &out.layout, sid);
        let labels: usize = out
            .layout
            .labels
            .values()
            .map(|l| l.iter().filter(|x| &x.sheet == sid).count())
            .sum();
        assert_eq!(p.labels.len(), labels);
        assert!(labels > 0);
        // Every net label's text is painted, inside its box.
        for l in p.labels.iter().filter(|l| l.kind == LabelKind::Net) {
            assert!(p.items.iter().any(|it| matches!(it,
                Item::Text { text, at, .. } if *text == l.text && l.rect.holds(*at))));
        }
        assert!(p.blocks.is_empty());
    }
}
