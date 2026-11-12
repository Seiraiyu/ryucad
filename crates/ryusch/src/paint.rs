//! What a sheet draws, as items in the sheet's own frame (nanometres, y
//! up): every line, circle, arc, curve and text of its symbols, pins,
//! wires, junctions and labels, in drawing order, in a look (`crate::style`,
//! plan 2.E): each role's colour and each object's pen. SVG and ryueda's PDF both draw from it, so they
//! draw alike; it also says where each label and block symbol is, for the
//! PDF's cross-references and links.

use crate::font::{Anchor, Font, MIN_SIZE, SIZE, Weight};
use crate::geom::{GRID, Nm, Place, Point, Rect, Side};
use crate::input::{Block, Fill, Graphic, PartKind};
use crate::label;
use crate::layout::{LabelKind, Layout};
use crate::route;
use crate::shape::Shape;
use crate::style::{Look, Pen, Rgb, Role};
use std::collections::{BTreeMap, BTreeSet};

/// A bus wire: three lines wide at the normal weight.
pub const BUS_LINE: Nm = 3 * LINE;
/// Every line's width at the normal weight.
pub const LINE: Nm = 150_000;
/// A dashed line's dash (a region's frame, a dashed pen's polylines):
/// 2 mm on, 1 mm off.
pub const DASH: [Nm; 2] = [2_000_000, 1_000_000];

/// A group a PDF reader can turn off (plan 2.E): [`Item::Begin`] and
/// [`Item::End`] mark what it holds. SVG writes it as a `<g>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    /// Region frames, their tabs and titles.
    Frames,
    Notes,
    /// Rails' and nets' intent.
    Intent,
    PinNumbers,
}

impl Layer {
    /// In the PDF's order.
    pub const ALL: [Layer; 4] = [
        Layer::Frames,
        Layer::Notes,
        Layer::Intent,
        Layer::PinNumbers,
    ];

    /// Its name in a PDF reader's layer panel.
    pub fn title(self) -> &'static str {
        match self {
            Layer::Frames => "Region frames",
            Layer::Notes => "Notes",
            Layer::Intent => "Intent",
            Layer::PinNumbers => "Pin numbers",
        }
    }

    /// Its key in SVG (`data-layer`).
    pub fn key(self) -> &'static str {
        match self {
            Layer::Frames => "frames",
            Layer::Notes => "notes",
            Layer::Intent => "intent",
            Layer::PinNumbers => "pin_numbers",
        }
    }

    /// Its place in [`Layer::ALL`].
    pub fn index(self) -> usize {
        Layer::ALL.iter().position(|l| *l == self).expect("listed")
    }
}

/// One thing drawn, in its colours and widths. A fill of `None` draws the
/// outline alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Polyline {
        points: Vec<Point>,
        stroke: Rgb,
        fill: Option<Rgb>,
        width: Nm,
        /// Dashed [`DASH`].
        dash: bool,
    },
    /// Outlined when `stroke` is given (a symbol's circle), else filled
    /// alone (a junction's dot).
    Circle {
        center: Point,
        radius: Nm,
        stroke: Option<Rgb>,
        fill: Option<Rgb>,
        width: Nm,
    },
    /// The arc from `start` through `mid` to `end`.
    Arc {
        start: Point,
        mid: Point,
        end: Point,
        stroke: Rgb,
        width: Nm,
    },
    /// A cubic Bézier.
    Bezier {
        points: [Point; 4],
        stroke: Rgb,
        width: Nm,
    },
    /// A region's frame: a dashed rectangle (unless its pen hides it,
    /// `frame: false`), and its title's tab (none when it has no title),
    /// filled with `paper` and outlined, drawn under everything on its
    /// sheet.
    Frame {
        rect: Rect,
        title_tab: Option<Rect>,
        stroke: Rgb,
        paper: Rgb,
        width: Nm,
        frame: bool,
    },
    /// A bus wire.
    Bus {
        points: Vec<Point>,
        stroke: Rgb,
        width: Nm,
    },
    /// A text on its baseline through `at`, upright or reading upwards, in
    /// SemiBold when `bold`.
    Text {
        text: String,
        at: Point,
        size: Nm,
        anchor: Anchor,
        vertical: bool,
        colour: Rgb,
        bold: bool,
    },
    /// What follows, to the matching [`Item::End`], is in a layer.
    Begin(Layer),
    End,
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
    /// Each drawn bus's off-sheet connector: its bus's key, its members'
    /// net keys on the sheet and its box (plan 2.C: cross-references go
    /// under it as under a net's).
    pub buses: Vec<BusConnector>,
    /// The keys of every net a pin on the sheet is on.
    pub nets: BTreeSet<String>,
}

/// How a line is drawn.
#[derive(Clone, Copy, Debug)]
struct Ink {
    colour: Rgb,
    width: Nm,
    dash: bool,
}

impl Ink {
    /// An object's lines: its pen's colour (else `role`'s), weight and dash.
    fn of(look: &Look, pen: &Pen, role: Role) -> Ink {
        Ink {
            colour: pen.colour.unwrap_or_else(|| look.role(role)),
            width: pen.weight.width(),
            dash: pen.dash,
        }
    }

    /// A plain line in `role`'s colour.
    fn plain(look: &Look, role: Role) -> Ink {
        Ink {
            colour: look.role(role),
            width: LINE,
            dash: false,
        }
    }
}

fn line(items: &mut Vec<Item>, points: Vec<Point>, ink: Ink) {
    items.push(Item::Polyline {
        points,
        stroke: ink.colour,
        fill: None,
        width: ink.width,
        dash: ink.dash,
    });
}

/// A text item, Regular unless `bold`.
fn text_item(text: &str, at: Point, size: Nm, anchor: Anchor, vertical: bool, colour: Rgb) -> Item {
    Item::Text {
        text: text.to_owned(),
        at,
        size,
        anchor,
        vertical,
        colour,
        bold: false,
    }
}

/// What a part's graphics are drawn in.
struct PartInk {
    ink: Ink,
    body: Rgb,
    port: Rgb,
    text: Rgb,
}

fn graphic(items: &mut Vec<Item>, shape: &Shape, g: &Graphic, k: &PartInk) {
    let pl = |p: Point| shape.place.apply(p);
    let fill = |x: Fill| match x {
        Fill::None => None,
        Fill::Body => Some(k.body),
        Fill::Outline => Some(k.ink.colour),
        Fill::Port => Some(k.port),
    };
    let (stroke, width, dash) = (k.ink.colour, k.ink.width, k.ink.dash);
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
            stroke,
            fill: fill(*x),
            width,
            dash,
        }),
        Graphic::Polyline { points, fill: x } => items.push(Item::Polyline {
            points: points.iter().map(|p| pl(*p)).collect(),
            stroke,
            fill: fill(*x),
            width,
            dash,
        }),
        Graphic::Circle {
            center,
            radius,
            fill: x,
        } => items.push(Item::Circle {
            center: pl(*center),
            radius: *radius,
            stroke: Some(stroke),
            fill: fill(*x),
            width,
        }),
        Graphic::Arc { start, mid, end } => items.push(Item::Arc {
            start: pl(*start),
            mid: pl(*mid),
            end: pl(*end),
            stroke,
            width,
        }),
        Graphic::Bezier { points } => items.push(Item::Bezier {
            points: points.map(pl),
            stroke,
            width,
        }),
        Graphic::Text { text, at, size } => items.push(text_item(
            text,
            pl(*at),
            (*size).max(MIN_SIZE),
            Anchor::Start,
            false,
            k.text,
        )),
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
fn name_at(at: &crate::shape::PinAt, size: Nm) -> (Point, Anchor) {
    let dir = at.side.step();
    let inward = [-dir[0] / GRID, -dir[1] / GRID];
    let off = GRID / 2;
    let p = [at.base[0] + inward[0] * off, at.base[1] + inward[1] * off];
    match at.side {
        Side::Left => ([p[0], p[1] - size / 3], Anchor::Start),
        Side::Right => ([p[0], p[1] - size / 3], Anchor::End),
        Side::Up => ([p[0] + size / 3, p[1]], Anchor::End),
        Side::Down => ([p[0] + size / 3, p[1]], Anchor::Start),
    }
}

/// The box a pin's name takes, as [`sheet`] draws it.
pub fn name_box(name: &str, at: &crate::shape::PinAt) -> Rect {
    name_box_sized(name, at, SIZE)
}

/// The box a pin's name takes drawn at `size`: what ryueda's symbol lint
/// measures a standards' name size by.
pub fn name_box_sized(name: &str, at: &crate::shape::PinAt, size: Nm) -> Rect {
    let (p, anchor) = name_at(at, size);
    if at.side.horizontal() {
        crate::font::text_box(name, size, p, anchor)
    } else {
        // Reading up the page: turned a quarter counterclockwise about its
        // anchor.
        let flat = crate::font::text_box(name, size, [0, 0], anchor);
        Rect::new(
            [p[0] - flat.max[1], p[1] + flat.min[0]],
            [p[0] - flat.min[1], p[1] + flat.max[0]],
        )
    }
}

/// A label's lines and text, in its net's pen.
#[allow(clippy::too_many_arguments)]
fn label_items(
    items: &mut Vec<Item>,
    kind: LabelKind,
    name: &str,
    at: Point,
    side: Side,
    flow: crate::input::Flow,
    intent: &str,
    look: &Look,
    pen: &Pen,
) {
    let d = if kind == LabelKind::OffSheet {
        label::drawn_flow_with(name, intent, at, side, flow)
    } else {
        label::drawn_with(kind, name, intent, at, side)
    };
    let power = pen.colour.unwrap_or_else(|| look.role(Role::Power));
    let colour = if kind.signal() {
        pen.colour.unwrap_or_else(|| look.role(Role::Label))
    } else {
        power
    };
    if let Some((p, anchor, text)) = &d.extra {
        items.push(Item::Begin(Layer::Intent));
        items.push(text_item(text, *p, MIN_SIZE, *anchor, false, colour));
        items.push(Item::End);
    }
    // A net label's line is the wire it names; a connector's outline is
    // its own.
    let role = match kind {
        LabelKind::Net => Role::Wire,
        LabelKind::OffSheet => Role::Label,
        _ => Role::Power,
    };
    let ink = Ink {
        dash: false,
        ..Ink::of(look, pen, role)
    };
    for l in &d.lines {
        line(items, l.clone(), ink);
    }
    if let Some((p, anchor, vertical)) = d.text {
        let size = if kind == LabelKind::OffSheet {
            label::FLAG_TEXT
        } else {
            SIZE
        };
        items.push(text_item(name, p, size, anchor, vertical, colour));
    }
}

/// The box a note's text takes from its lower left `at`: the text at the
/// normal size half a step in, grown by half a step, its far corner on
/// the grid (plan 2.C).
pub fn note_box(text: &str, at: Point) -> Rect {
    let far = [
        at[0] + crate::font::width(text, SIZE) + GRID,
        at[1] + SIZE + GRID,
    ];
    let up = |v: Nm| (v + GRID - 1).div_euclid(GRID) * GRID;
    Rect::new(at, [up(far[0]), up(far[1])])
}

/// A drawn bus's off-sheet connector, as [`Painted::buses`] lists it:
/// its bus's key, its members' net keys on the sheet and its box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusConnector {
    pub bus: String,
    pub nets: Vec<String>,
    pub rect: Rect,
}

/// The buses drawn on sheet `sid` (plan 2.C): each member's stub and 45°
/// entry, its name on its stub, the spine through a column of entries,
/// the trunk, the bundle's name on the trunk and its off-sheet connector.
/// Gives the connectors, for the cross-references under them.
fn bus_items(
    block: &Block,
    layout: &Layout,
    sid: &str,
    items: &mut Vec<Item>,
    look: &Look,
) -> Vec<BusConnector> {
    let mut connectors = Vec::new();
    for (key, by_sheet) in &layout.buses {
        let Some(drawn) = by_sheet.get(sid) else {
            continue;
        };
        let bus_pen = look.pen(key);
        let bus_wire = Ink::of(look, &bus_pen, Role::Wire);
        let bus_label = bus_pen.colour.unwrap_or_else(|| look.role(Role::Label));
        let mut columns: BTreeMap<Nm, Vec<Point>> = BTreeMap::new();
        let mut members: Vec<usize> = Vec::new();
        let stub_len = drawn.stub;
        for e in &drawn.entries {
            let (stub, on_spine) = crate::bus::entry(e.at, e.side, stub_len);
            let net_pen = block.net(&e.net).map(|n| look.pen(&block.nets[n].key));
            let ink = net_pen.as_ref().map_or_else(
                || Ink::plain(look, Role::Wire),
                |p| Ink::of(look, p, Role::Wire),
            );
            line(items, vec![e.at, stub, on_spine], ink);
            columns.entry(on_spine[0]).or_default().push(on_spine);
            if let Some(n) = block.net(&e.net) {
                let colour = net_pen
                    .and_then(|p| p.colour)
                    .unwrap_or_else(|| look.role(Role::Label));
                if !members.contains(&n) {
                    members.push(n);
                }
                let name = &block.nets[n].name;
                if let Some((p, anchor, vertical)) =
                    label::drawn(LabelKind::Net, name, e.at, e.side).text
                {
                    items.push(text_item(name, p, SIZE, anchor, vertical, colour));
                }
            }
        }
        for pts in columns.values() {
            if let Some([a, b]) = crate::bus::spine(pts)
                && a != b
            {
                items.push(Item::Bus {
                    points: vec![a, b],
                    stroke: bus_wire.colour,
                    width: 3 * bus_wire.width,
                });
            }
        }
        for run in &drawn.trunk {
            items.push(Item::Bus {
                points: run.clone(),
                stroke: bus_wire.colour,
                width: 3 * bus_wire.width,
            });
        }
        let name = match block.bus(key) {
            Some(b) => crate::bus::name(block, b, &members),
            None => key.clone(),
        };
        if let Some(run) = drawn.trunk.first().filter(|_| drawn.connector.is_none())
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
                items.push(text_item(&name, p, SIZE, anchor, vertical, bus_label));
            }
        }
        if let Some(c) = &drawn.connector {
            let flow = members
                .first()
                .map_or(crate::input::Flow::Bidirectional, |n| {
                    label::flow_on_sheet(block, layout, *n, sid)
                });
            label_items(
                items,
                LabelKind::OffSheet,
                &c.text,
                c.at,
                c.side,
                flow,
                "",
                look,
                &bus_pen,
            );
            connectors.push(BusConnector {
                bus: key.clone(),
                nets: members.iter().map(|n| block.nets[*n].key.clone()).collect(),
                rect: label::drawn_flow(&c.text, c.at, c.side, flow).bounds,
            });
        }
    }
    connectors
}

/// [`sheet_in`] in the built-in light look.
pub fn sheet(block: &Block, layout: &Layout, sid: &str) -> Painted {
    sheet_in(block, layout, sid, &Look::default())
}

/// What sheet `sid` of `layout` draws in `look`: parts in key order (each
/// its graphics, pins, pin names and numbers, not-connected marks and
/// text), then wires and junctions, then labels.
pub fn sheet_in(block: &Block, layout: &Layout, sid: &str, look: &Look) -> Painted {
    let mut out = Painted::default();
    let items = &mut out.items;
    // Regions first, under everything: a dashed frame and a titled tab in
    // its title band's top left; an untitled one (plan 3.A) has no tab,
    // its band kept so frames don't move when it gets a title.
    for (rid, r) in &layout.regions {
        if r.sheet != sid {
            continue;
        }
        let pen = look.pen(&format!("region:{rid}"));
        let rect = r.frame_on_sheet();
        let w = crate::font::width_in(&r.title, Font::DESIGNATOR);
        let base = [rect.min[0] + GRID, rect.max[1] - 2 * GRID];
        let title_tab = Rect::new(
            [rect.min[0] + GRID / 2, base[1] - GRID / 2],
            [base[0] + w + GRID / 2, base[1] + SIZE + GRID / 2],
        );
        let titled = !r.title.is_empty();
        items.push(Item::Begin(Layer::Frames));
        items.push(Item::Frame {
            rect,
            title_tab: titled.then_some(title_tab),
            stroke: pen.colour.unwrap_or_else(|| look.role(Role::Frame)),
            paper: look.role(Role::Paper),
            width: pen.weight.width(),
            frame: pen.frame,
        });
        if titled {
            items.push(Item::Text {
                text: r.title.clone(),
                at: base,
                size: SIZE,
                anchor: Anchor::Start,
                vertical: false,
                colour: pen.colour.unwrap_or_else(|| look.role(Role::Text)),
                bold: true,
            });
        }
        items.push(Item::End);
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
        let pen = look.pen(&part.key);
        let k = PartInk {
            ink: Ink::of(look, &pen, Role::Ink),
            body: pen.fill.unwrap_or_else(|| look.role(Role::Body)),
            port: pen.fill.unwrap_or_else(|| look.role(Role::PortFill)),
            text: pen.colour.unwrap_or_else(|| look.role(Role::Text)),
        };
        for g in &part.graphics {
            graphic(items, &shape, g, &k);
        }
        let names = draws_names(part);
        let mut numbers: Vec<Item> = Vec::new();
        for (i, pin) in part.pins.iter().enumerate() {
            let at = shape.pins[i];
            if at.base != at.end {
                line(items, vec![at.base, at.end], k.ink);
            }
            if let Some(n) = pin.net {
                *arms.entry((n, route::node_of(at.end))).or_default() += 1;
            }
            if names && !pin.name.is_empty() {
                let (p, anchor) = name_at(&at, SIZE);
                items.push(text_item(
                    &pin.name,
                    p,
                    SIZE,
                    anchor,
                    !at.side.horizontal(),
                    k.text,
                ));
            }
            if pin.show_number && pen.pin_numbers {
                let mid = [(at.base[0] + at.end[0]) / 2, (at.base[1] + at.end[1]) / 2];
                let (p, vertical) = if at.side.horizontal() {
                    ([mid[0], mid[1] + GRID / 4], false)
                } else {
                    ([mid[0] - GRID / 4, mid[1]], true)
                };
                numbers.push(text_item(
                    &pin.number,
                    p,
                    MIN_SIZE,
                    Anchor::Middle,
                    vertical,
                    pen.colour.unwrap_or_else(|| look.role(Role::PinNumber)),
                ));
            }
            if pin.nc {
                let d = GRID / 3;
                let e = at.end;
                let ink = Ink::plain(look, Role::Label);
                line(items, vec![[e[0] - d, e[1] - d], [e[0] + d, e[1] + d]], ink);
                line(items, vec![[e[0] - d, e[1] + d], [e[0] + d, e[1] - d]], ink);
            }
        }
        if !numbers.is_empty() {
            items.push(Item::Begin(Layer::PinNumbers));
            items.append(&mut numbers);
            items.push(Item::End);
        }
        if let Some(fields) = layout.text.get(&part.key) {
            for (field, line) in part.fields() {
                if let Some(at) = fields.get(field) {
                    items.push(Item::Text {
                        text: line.text.to_owned(),
                        at: *at,
                        size: line.font.size,
                        anchor: Anchor::Start,
                        vertical: false,
                        colour: k.text,
                        bold: line.font.weight == Weight::SemiBold,
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
        let ink = Ink::of(look, &look.pen(key), Role::Wire);
        for run in runs {
            line(items, run.clone(), ink);
            let nodes = route::expand(run);
            for (i, node) in nodes.iter().enumerate() {
                let ends = u8::from(i > 0) + u8::from(i + 1 < nodes.len());
                *arms.entry((n, *node)).or_default() += ends;
            }
        }
    }
    for ((n, node), count) in &arms {
        if *count >= 3 {
            let colour = block.nets.get(*n).map_or_else(
                || look.role(Role::Wire),
                |net| Ink::of(look, &look.pen(&net.key), Role::Wire).colour,
            );
            items.push(Item::Circle {
                center: route::point_of(*node),
                radius: GRID / 4,
                stroke: None,
                fill: Some(colour),
                width: LINE,
            });
        }
    }
    out.buses = bus_items(block, layout, sid, items, look);
    for (key, list) in &layout.labels {
        for l in list.iter().filter(|l| l.sheet == sid) {
            let flow = match (l.kind, block.net(key)) {
                (LabelKind::OffSheet, Some(n)) => label::flow_on_sheet(block, layout, n, sid),
                _ => crate::input::Flow::Bidirectional,
            };
            label_items(
                items,
                l.kind,
                &l.text,
                l.at,
                l.side,
                flow,
                label::intent_for(block, key, l.kind),
                look,
                &look.pen(key),
            );
            out.labels.push(Placed {
                net: key.clone(),
                on: l.on.clone(),
                kind: l.kind,
                text: l.text.clone(),
                at: l.at,
                side: l.side,
                rect: label::boxed(block, key, l),
            });
        }
    }
    // Notes (plan 2.C): the text in a thin box.
    for (id, n) in layout.notes.iter().filter(|(_, n)| n.sheet == sid) {
        let Some(text) = block.note_text(id) else {
            continue;
        };
        let b = note_box(text, n.at);
        out.items.push(Item::Begin(Layer::Notes));
        out.items.push(Item::Polyline {
            points: vec![
                b.min,
                [b.max[0], b.min[1]],
                b.max,
                [b.min[0], b.max[1]],
                b.min,
            ],
            stroke: look.role(Role::Name),
            fill: None,
            width: LINE,
            dash: false,
        });
        out.items.push(text_item(
            text,
            [n.at[0] + GRID / 2, n.at[1] + GRID / 2],
            SIZE,
            Anchor::Start,
            false,
            look.role(Role::Text),
        ));
        out.items.push(Item::End);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_box_at_its_size_is_the_drawn_one_at_the_drawn_size() {
        let b = crate::testing::seeded(1, 2);
        let part = b
            .parts
            .iter()
            .find(|p| draws_names(p) && !p.pins.is_empty())
            .unwrap();
        let sh = crate::shape::Shape::of(part, crate::geom::Place::new([0, 0], 0, false));
        let at = &sh.pins[0];
        assert_eq!(name_box("VDD", at), name_box_sized("VDD", at, SIZE));
        let big = name_box_sized("VDD", at, 2 * SIZE);
        assert!(big.width() > name_box("VDD", at).width());
    }

    /// Plan 3.A: an untitled region is framed with no tab and no title.
    #[test]
    fn an_untitled_region_has_no_tab() {
        let block = crate::testing::regions_block();
        let mut out = crate::engine::lay_out(&block).unwrap();
        for g in out.layout.regions.values_mut() {
            g.title = String::new();
        }
        let (sid, _) = out.layout.sheets.iter().next().unwrap();
        let painted = sheet(&block, &out.layout, sid);
        let frames: Vec<&Item> = painted
            .items
            .iter()
            .filter(|i| matches!(i, Item::Frame { .. }))
            .collect();
        assert_eq!(frames.len(), out.layout.regions.len());
        assert!(frames.iter().all(|i| matches!(
            i,
            Item::Frame {
                title_tab: None,
                ..
            }
        )));
    }

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
                    stub,
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
            .filter(|i| matches!(i, Item::Polyline { points, stroke, .. } if points.len() == 3 && *stroke == crate::style::Look::default().role(crate::style::Role::Wire)))
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

    #[test]
    fn a_look_colours_and_weighs_what_its_pens_name() {
        use crate::style::{Look, Pen, Rgb, Role, Theme, Weight};
        let b = seeded(2, 4);
        let out = crate::engine::lay_out(&b).unwrap();
        let sid = out.layout.sheets.keys().next().unwrap().clone();
        let plain = sheet(&b, &out.layout, &sid);
        let dark = sheet_in(&b, &out.layout, &sid, &Look::builtin(Theme::Dark));
        assert_eq!(plain.items.len(), dark.items.len(), "a theme moves no item");
        let wire = Look::default().role(Role::Wire);
        assert!(
            plain
                .items
                .iter()
                .any(|i| matches!(i, Item::Polyline { stroke, .. } if *stroke == wire))
        );
        // One net drawn red and heavy, dashed.
        let (key, _) = out
            .layout
            .routes
            .iter()
            .find(|(_, s)| s.contains_key(&sid))
            .expect("a wired net on the sheet");
        let red = Rgb(0xff, 0, 0);
        let mut look = Look::default();
        look.pens.insert(
            key.clone(),
            Pen {
                colour: Some(red),
                weight: Weight::Heavy,
                dash: true,
                ..Pen::default()
            },
        );
        let p = sheet_in(&b, &out.layout, &sid, &look);
        assert!(p.items.iter().any(|i| matches!(i,
            Item::Polyline { stroke, width, dash: true, .. } if *stroke == red && *width == Weight::Heavy.width())));
        // Layers come in pairs.
        let begins = p
            .items
            .iter()
            .filter(|i| matches!(i, Item::Begin(_)))
            .count();
        let ends = p.items.iter().filter(|i| matches!(i, Item::End)).count();
        assert_eq!(begins, ends);
    }

    #[test]
    fn pin_numbers_hide_by_pen_and_sit_in_their_layer() {
        use crate::style::{Look, Pen};
        let b = seeded(1, 2);
        let out = crate::engine::lay_out(&b).unwrap();
        let sid = out.layout.sheets.keys().next().unwrap().clone();
        let numbered = b
            .parts
            .iter()
            .find(|p| {
                p.pins.iter().any(|x| x.show_number)
                    && out
                        .layout
                        .placed
                        .get(&p.key)
                        .is_some_and(|x| x.sheet == sid)
            })
            .expect("a part with numbers");
        let count = |p: &Painted| {
            p.items
                .windows(2)
                .filter(|w| matches!(w, [Item::Begin(Layer::PinNumbers), Item::Text { .. }]))
                .count()
        };
        let all = sheet(&b, &out.layout, &sid);
        assert!(count(&all) > 0);
        let mut look = Look::default();
        look.pens.insert(
            numbered.key.clone(),
            Pen {
                pin_numbers: false,
                ..Pen::default()
            },
        );
        assert!(count(&sheet_in(&b, &out.layout, &sid, &look)) < count(&all));
    }

    #[test]
    fn a_note_is_its_text_in_a_thin_box() {
        use crate::layout::{Instruction, PlacedNote, Sheet};
        let mut b = Block::default();
        b.instructions.insert(
            "i9".into(),
            Instruction::Note {
                on: "s1".into(),
                text: "Rev {rev}".into(),
            },
        );
        b.notes.insert("i9".into(), "Rev B".into());
        let mut l = Layout::default();
        l.sheets
            .insert("s1".into(), Sheet::new("s1".into(), [0, 0]));
        l.notes.insert(
            "i9".into(),
            PlacedNote {
                sheet: "s1".into(),
                at: [10 * GRID, 10 * GRID],
            },
        );
        let p = sheet(&b, &l, "s1");
        assert!(p.items.iter().any(|i| matches!(i, Item::Polyline { points, stroke, fill: None, width: _, dash: false } if points.len() == 5 && points[0] == points[4] && *stroke == crate::style::Look::default().role(crate::style::Role::Name))));
        assert!(p.items.iter().any(|i| matches!(i, Item::Text { text, size, colour, .. } if text == "Rev B" && *size == SIZE && *colour == crate::style::Look::default().role(crate::style::Role::Text))));
    }
}
