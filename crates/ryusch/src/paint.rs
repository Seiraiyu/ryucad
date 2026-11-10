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
use std::collections::{BTreeMap, BTreeSet};

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
/// A bus wire: the wires' colour, three lines wide.
pub const BUS: &str = WIRE;
pub const BUS_LINE: Nm = 3 * LINE;
/// A block port's flag: a light blue tint, under dark text.
pub const PORT_FILL: &str = "#dce6f5";
/// A sheet's name, beside it on the canvas.
pub const NAME: &str = "#606060";
/// Every line's width.
pub const LINE: Nm = 150_000;
/// A region's frame: the symbols' colour at 45% over the paper.
pub const FRAME: &str = "#c39a9a";
/// The paper, under a region's title.
pub const PAPER: &str = "#ffffff";
/// A region frame's dash: 2 mm on, 1 mm off.
pub const DASH: [Nm; 2] = [2_000_000, 1_000_000];

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
    /// A region's frame: a dashed rectangle, and its title's tab (filled
    /// with the paper, outlined), drawn under everything on its sheet.
    Frame { rect: Rect, title_tab: Rect },
    /// A bus wire: a polyline drawn [`BUS_LINE`] wide in [`BUS`].
    Bus { points: Vec<Point> },
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
    /// Each block port's flag: its net's key and its body's box (plan 2.C:
    /// cross-references go under it).
    pub ports: Vec<(String, Rect)>,
    /// The keys of every net a pin on the sheet is on.
    pub nets: BTreeSet<String>,
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
        Fill::Port => Some(PORT_FILL),
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
fn label_items(
    items: &mut Vec<Item>,
    kind: LabelKind,
    name: &str,
    at: Point,
    side: Side,
    flow: crate::input::Flow,
) {
    let d = if kind == LabelKind::OffSheet {
        label::drawn_flow(name, at, side, flow)
    } else {
        label::drawn(kind, name, at, side)
    };
    let colour = if kind.signal() { LABEL } else { POWER };
    // A net label's line is the wire it names; a connector's outline is
    // its own.
    let ink = match kind {
        LabelKind::Net => WIRE,
        LabelKind::OffSheet => LABEL,
        _ => POWER,
    };
    for l in &d.lines {
        line(items, l.clone(), ink);
    }
    if let Some((p, anchor, vertical)) = d.text {
        items.push(Item::Text {
            text: name.to_owned(),
            at: p,
            size: if kind == LabelKind::OffSheet {
                label::FLAG_TEXT
            } else {
                SIZE
            },
            anchor,
            vertical,
            colour,
        });
    }
}

/// The buses drawn on sheet `sid` (plan 2.C): each member's stub and 45°
/// entry, its name on its stub, the spine through a column of entries,
/// the trunk, the bundle's name on the trunk and its off-sheet connector.
fn bus_items(block: &Block, layout: &Layout, sid: &str, items: &mut Vec<Item>) {
    for (key, by_sheet) in &layout.buses {
        let Some(drawn) = by_sheet.get(sid) else {
            continue;
        };
        let mut columns: BTreeMap<Nm, Vec<Point>> = BTreeMap::new();
        let mut members: Vec<usize> = Vec::new();
        let stub_len = crate::bus::stub_len(
            drawn
                .entries
                .iter()
                .filter_map(|e| block.net(&e.net).map(|n| block.nets[n].name.as_str())),
        );
        for e in &drawn.entries {
            let (stub, on_spine) = crate::bus::entry(e.at, e.side, stub_len);
            line(items, vec![e.at, stub, on_spine], WIRE);
            columns.entry(on_spine[0]).or_default().push(on_spine);
            if let Some(n) = block.net(&e.net) {
                if !members.contains(&n) {
                    members.push(n);
                }
                let name = &block.nets[n].name;
                if let Some((p, anchor, vertical)) =
                    label::drawn(LabelKind::Net, name, e.at, e.side).text
                {
                    items.push(Item::Text {
                        text: name.clone(),
                        at: p,
                        size: SIZE,
                        anchor,
                        vertical,
                        colour: LABEL,
                    });
                }
            }
        }
        for pts in columns.values() {
            if let Some([a, b]) = crate::bus::spine(pts)
                && a != b
            {
                items.push(Item::Bus { points: vec![a, b] });
            }
        }
        for run in &drawn.trunk {
            items.push(Item::Bus {
                points: run.clone(),
            });
        }
        let name = match block.bus(key) {
            Some(b) => crate::bus::name(block, b, &members),
            None => key.clone(),
        };
        if let Some(run) = drawn.trunk.first()
            && run.len() >= 2
        {
            let d = [run[1][0] - run[0][0], run[1][1] - run[0][1]];
            let side = match (d[0].signum(), d[1].signum()) {
                (1, _) => Side::Right,
                (-1, _) => Side::Left,
                (_, 1) => Side::Up,
                _ => Side::Down,
            };
            let side = label::pointing(LabelKind::Net, side);
            if let Some((p, anchor, vertical)) =
                label::drawn(LabelKind::Net, &name, run[0], side).text
            {
                items.push(Item::Text {
                    text: name.clone(),
                    at: p,
                    size: SIZE,
                    anchor,
                    vertical,
                    colour: LABEL,
                });
            }
        }
        if let Some(c) = &drawn.connector {
            let flow = members
                .first()
                .map_or(crate::input::Flow::Bidirectional, |n| {
                    label::flow_on_sheet(block, layout, *n, sid)
                });
            label_items(items, LabelKind::OffSheet, &c.text, c.at, c.side, flow);
        }
    }
}

/// What sheet `sid` of `layout` draws: parts in key order (each its
/// graphics, pins, pin names and numbers, not-connected marks and text),
/// then wires and junctions, then labels.
pub fn sheet(block: &Block, layout: &Layout, sid: &str) -> Painted {
    let mut out = Painted::default();
    let items = &mut out.items;
    // Regions first, under everything: a dashed frame and a titled tab in
    // its title band's top left. An untitled region isn't drawn.
    for r in layout.regions.values() {
        if r.sheet != sid || r.title.is_empty() {
            continue;
        }
        let rect = r.frame_on_sheet();
        let w = crate::font::width(&r.title, SIZE);
        let base = [rect.min[0] + GRID, rect.max[1] - 2 * GRID];
        let title_tab = Rect::new(
            [rect.min[0] + GRID / 2, base[1] - GRID / 2],
            [base[0] + w + GRID / 2, base[1] + SIZE + GRID / 2],
        );
        items.push(Item::Frame { rect, title_tab });
        items.push(Item::Text {
            text: r.title.clone(),
            at: base,
            size: SIZE,
            anchor: Anchor::Start,
            vertical: false,
            colour: TEXT,
        });
    }
    let mut arms: BTreeMap<(usize, route::Node), u8> = BTreeMap::new();
    for part in &block.parts {
        let Some(x) = layout.placed.get(&part.key).filter(|x| x.sheet == sid) else {
            continue;
        };
        let shape = Shape::of(part, Place::new(x.at, x.rotation, x.mirror));
        if part.kind == PartKind::Block {
            out.blocks.push((part.key.clone(), shape.body));
        }
        for pin in &part.pins {
            for n in pin.nets() {
                out.nets.insert(block.nets[n].key.clone());
            }
        }
        if matches!(part.kind, PartKind::Port { .. })
            && let Some(n) = part.pins.first().and_then(|p| p.drawn_net())
        {
            out.ports.push((block.nets[n].key.clone(), shape.body));
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
    bus_items(block, layout, sid, items);
    for (key, list) in &layout.labels {
        for l in list.iter().filter(|l| l.sheet == sid) {
            let flow = match (l.kind, block.net(key)) {
                (LabelKind::OffSheet, Some(n)) => label::flow_on_sheet(block, layout, n, sid),
                _ => crate::input::Flow::Bidirectional,
            };
            label_items(items, l.kind, &l.text, l.at, l.side, flow);
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

    #[test]
    fn a_region_is_drawn_as_a_dashed_frame_and_a_title_tab() {
        let block = crate::testing::regions_block();
        let out = crate::engine::lay_out(&block).unwrap();
        let (sid, _) = out.layout.sheets.iter().next().unwrap();
        let painted = sheet(&block, &out.layout, sid);
        let frames: Vec<&Item> = painted
            .items
            .iter()
            .filter(|i| matches!(i, Item::Frame { .. }))
            .collect();
        assert_eq!(frames.len(), out.layout.regions.len());
        assert!(
            painted
                .items
                .iter()
                .any(|i| matches!(i, Item::Text { text, .. } if text == "U1 LAN8710A"))
        );
        let svg = crate::svg::draw(&block, &out.layout, &[]);
        assert!(svg.contains("stroke-dasharray"), "frames are dashed");
    }
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

    #[test]
    fn a_drawn_bus_paints_its_entries_spines_and_trunk() {
        use crate::input::{Bus, NetKind, PinKind};
        use crate::layout::{BusEntry, DrawnBus, Placed, Sheet};
        use crate::testing;
        let mut b = Block::default();
        for i in 0..4 {
            b.nets.push(testing::net(
                &format!("d{i}"),
                &format!("QSPI_SD{i}"),
                NetKind::Signal,
            ));
        }
        b.buses.push(Bus {
            key: "bus:q".into(),
            name: "QSPI_SD".into(),
            members: vec![0, 1, 2, 3],
        });
        let side: Vec<(&str, PinKind, Option<usize>)> = ["IO0", "IO1", "IO2", "IO3"]
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, PinKind::Bidirectional, Some(i)))
            .collect();
        b.parts.push(testing::ic("u1", "U1", "MCU", &[], &side));
        let mut l = Layout::default();
        l.sheets
            .insert("s1".into(), Sheet::new("s1".into(), [0, 0]));
        l.placed.insert(
            "u1".into(),
            Placed {
                sheet: "s1".into(),
                at: [20 * GRID, 20 * GRID],
                rotation: 0,
                mirror: false,
                held: false,
            },
        );
        let shape = Shape::of(&b.parts[0], Place::new([20 * GRID, 20 * GRID], 0, false));
        let entries: Vec<BusEntry> = (0..4)
            .map(|k| BusEntry {
                net: format!("d{k}"),
                on: format!("u1/{}", b.parts[0].pins[k].number),
                at: shape.pins[k].end,
                side: shape.pins[k].side,
            })
            .collect();
        let stub = crate::bus::stub_len(["QSPI_SD0"]);
        let x = crate::bus::entry(entries[0].at, entries[0].side, stub).1[0];
        l.buses.insert(
            "bus:q".into(),
            BTreeMap::from([(
                "s1".into(),
                DrawnBus {
                    trunk: vec![vec![[x, 20 * GRID], [x + 10 * GRID, 20 * GRID]]],
                    entries,
                    connector: None,
                },
            )]),
        );
        let p = sheet(&b, &l, "s1");
        let buses = p
            .items
            .iter()
            .filter(|i| matches!(i, Item::Bus { .. }))
            .count();
        assert_eq!(buses, 2, "one spine and the trunk");
        let entry_lines = p
            .items
            .iter()
            .filter(|i| matches!(i, Item::Polyline { points, stroke, .. } if points.len() == 3 && *stroke == WIRE))
            .count();
        assert_eq!(entry_lines, 4);
        let texts: Vec<&str> = p
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        for i in 0..4 {
            assert!(texts.contains(&format!("QSPI_SD{i}").as_str()), "{texts:?}");
        }
        assert!(texts.contains(&"QSPI_SD[0..3]"));
    }
}
