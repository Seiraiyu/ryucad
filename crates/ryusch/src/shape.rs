//! A part's geometry where it is placed: its body's box, its whole extent
//! and its pins' ends; and the parts ryusch draws itself, child blocks'
//! symbols and the block's ports.

use crate::font::{self, SIZE};
use crate::geom::{GRID, Nm, Place, Point, Rect, Side, on_grid};
use crate::input::{Fill, Flow, Graphic, Part, PartKind, Pin, PinKind};

/// A pin where its part is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinAt {
    /// Where wires connect.
    pub end: Point,
    /// Where it meets the body.
    pub base: Point,
    /// Which way it points from the body.
    pub side: Side,
}

/// A part placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape {
    pub place: Place,
    /// The body's box: what no wire may cross.
    pub body: Rect,
    /// The body with its pins: what keeps clear of other parts.
    pub extent: Rect,
    pub pins: Vec<PinAt>,
}

/// The box of a graphic, in the part's frame.
fn graphic_box(g: &Graphic) -> Option<Rect> {
    match g {
        Graphic::Rect { start, end, .. } => Some(Rect::new(*start, *end)),
        Graphic::Polyline { points, .. } => Rect::of(points.iter().copied()),
        Graphic::Arc { start, mid, end } => Rect::of([*start, *mid, *end]),
        Graphic::Circle { center, radius, .. } => Some(Rect::new(*center, *center).grow(*radius)),
        Graphic::Bezier { points } => Rect::of(points.iter().copied()),
        Graphic::Text { text, at, size } => {
            Some(font::text_box(text, *size, *at, font::Anchor::Start))
        }
    }
}

/// A pin's end and base in the part's frame.
pub fn pin_ends(p: &Pin) -> (Point, Point) {
    let [dx, dy] = p.side.step();
    let base = [
        p.at[0] - dx / GRID * p.length,
        p.at[1] - dy / GRID * p.length,
    ];
    (p.at, base)
}

/// The part's body and extent in its own frame.
pub fn local(part: &Part) -> (Rect, Rect) {
    let bases = part.pins.iter().map(|p| pin_ends(p).1);
    let body = part
        .graphics
        .iter()
        .filter_map(graphic_box)
        .fold(Rect::of(bases), |acc, r| {
            Some(acc.map_or(r, |a| a.union(r)))
        })
        .unwrap_or(Rect::new([0, 0], [0, 0]));
    let extent = part.pins.iter().fold(body, |r, p| r.with(p.at));
    (body, extent)
}

/// Where the part's origin goes so its first pin lands on the grid, from
/// where it would put `at`.
pub fn grid_origin(part: &Part, place: &Place, at: Point) -> Point {
    let Some(first) = part.pins.first() else {
        return [crate::geom::snap(at[0]), crate::geom::snap(at[1])];
    };
    let p = place.turn(first.at);
    let end = [at[0] + p[0], at[1] + p[1]];
    let snapped = [crate::geom::snap(end[0]), crate::geom::snap(end[1])];
    [snapped[0] - p[0], snapped[1] - p[1]]
}

/// Whether every pin is on the grid relative to the first.
pub fn pins_on_grid(part: &Part) -> bool {
    let Some(first) = part.pins.first() else {
        return true;
    };
    part.pins
        .iter()
        .all(|p| on_grid([p.at[0] - first.at[0], p.at[1] - first.at[1]]))
}

impl Shape {
    pub fn of(part: &Part, place: Place) -> Shape {
        let (body, extent) = local(part);
        Shape {
            place,
            body: place.rect(body),
            extent: place.rect(extent),
            pins: part
                .pins
                .iter()
                .map(|p| {
                    let (end, base) = pin_ends(p);
                    PinAt {
                        end: place.apply(end),
                        base: place.apply(base),
                        side: place.side(p.side),
                    }
                })
                .collect(),
        }
    }
}

/// Round `v` up to a whole number of grid steps.
fn up(v: Nm) -> Nm {
    (v + GRID - 1).div_euclid(GRID) * GRID
}

/// How far each copy behind a stacked block symbol sits from the one in
/// front of it, down and to the right.
pub const STACK_STEP: Nm = GRID / 2;
/// The most copies drawn behind a stacked block symbol, however many
/// layers it has (its badge says how many).
pub const STACK_DRAWN: usize = 2;

/// A child block's symbol: a box named by the block, its ports as pins,
/// inputs on the left, outputs on the right, power on top, the rest on
/// the left. `ports` are `(number, name, flow, net)`.
pub fn block_symbol(
    key: &str,
    name: &str,
    ports: &[(String, String, Flow, Option<usize>)],
) -> Part {
    let ports: Vec<(String, String, Flow, Vec<Option<usize>>)> = ports
        .iter()
        .map(|(n, m, f, net)| (n.clone(), m.clone(), *f, vec![*net]))
        .collect();
    stack_symbol(key, name, &ports)
}

/// A stack's symbol: a block placed several times by one parent, drawn
/// once with copies behind it, offset down and to the right, and a badge
/// saying how many. `ports` are `(number, name, flow, nets)`, `nets` the
/// port's net in each layer, the front one first; with one layer it is a
/// plain block symbol. Pins on the right reach past the copies.
pub fn stack_symbol(
    key: &str,
    name: &str,
    ports: &[(String, String, Flow, Vec<Option<usize>>)],
) -> Part {
    let layers = ports.iter().map(|p| p.3.len()).max().unwrap_or(1).max(1);
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut top = Vec::new();
    for p in ports {
        match p.2 {
            Flow::Out => right.push(p),
            Flow::Power => top.push(p),
            Flow::In | Flow::Bidirectional | Flow::Passive => left.push(p),
        }
    }
    let widest = |list: &[&(String, String, Flow, Vec<Option<usize>>)]| {
        list.iter()
            .map(|p| font::width(&p.1, SIZE))
            .max()
            .unwrap_or(0)
    };
    let badge = (layers > 1).then(|| format!("×{layers}"));
    let badge_w = badge
        .as_deref()
        .map_or(0, |b| font::width(b, SIZE * 4 / 5) + GRID);
    let tops: Nm =
        top.iter().map(|p| font::width(&p.1, SIZE)).sum::<Nm>() + 2 * GRID * top.len() as Nm;
    let w = up((widest(&left) + widest(&right) + 4 * GRID)
        .max(tops)
        .max(8 * GRID)
        .max(badge_w + 4 * GRID));
    let rows = left.len().max(right.len()).max(1) as Nm;
    let h = 2 * GRID * rows + 2 * GRID + if top.is_empty() { 0 } else { 2 * GRID };
    // What the copies stick out past the front, on the right, rounded up
    // to the grid so the right pins' ends stay on it.
    let copies = (layers - 1).min(STACK_DRAWN) as Nm;
    let over = up(copies * STACK_STEP);
    let mut pins = Vec::new();
    let pin = |(number, name, flow, nets): &(String, String, Flow, Vec<Option<usize>>),
               at: Point,
               side: Side,
               length: Nm| Pin {
        number: number.clone(),
        name: name.clone(),
        at,
        side,
        length,
        kind: match flow {
            Flow::In => PinKind::Input,
            Flow::Out => PinKind::Output,
            Flow::Bidirectional => PinKind::Bidirectional,
            Flow::Power => PinKind::PowerIn,
            Flow::Passive => PinKind::Passive,
        },
        net: nets.first().copied().flatten(),
        nc: false,
        show_number: false,
        layers: nets.iter().skip(1).copied().collect(),
    };
    // The box spans x 0..w, y -h..0; rows start two steps below the top
    // (four with power pins above).
    let first = if top.is_empty() { -2 * GRID } else { -4 * GRID };
    for (i, p) in left.iter().enumerate() {
        pins.push(pin(
            p,
            [-2 * GRID, first - 2 * GRID * i as Nm],
            Side::Left,
            2 * GRID,
        ));
    }
    for (i, p) in right.iter().enumerate() {
        pins.push(pin(
            p,
            [w + over + 2 * GRID, first - 2 * GRID * i as Nm],
            Side::Right,
            over + 2 * GRID,
        ));
    }
    let step = if top.is_empty() {
        0
    } else {
        up(w / (top.len() as Nm + 1)).max(2 * GRID)
    };
    for (i, p) in top.iter().enumerate() {
        pins.push(pin(p, [step * (i as Nm + 1), 2 * GRID], Side::Up, 2 * GRID));
    }
    // The copies first, the farthest behind first, so the front covers
    // them.
    let mut graphics: Vec<Graphic> = (1..=copies)
        .rev()
        .map(|k| Graphic::Rect {
            start: [k * STACK_STEP, -k * STACK_STEP],
            end: [w + k * STACK_STEP, -h - k * STACK_STEP],
            fill: Fill::Body,
        })
        .collect();
    graphics.push(Graphic::Rect {
        start: [0, 0],
        end: [w, -h],
        fill: Fill::Body,
    });
    if let Some(b) = badge {
        let size = SIZE * 4 / 5;
        graphics.push(Graphic::Text {
            at: [w - font::width(&b, size) - GRID / 2, -GRID],
            text: b,
            size,
        });
    }
    Part {
        key: key.to_owned(),
        kind: PartKind::Block,
        refdes: name.to_owned(),
        value: String::new(),
        graphics,
        pins,
    }
}

/// A port of the block: a flag holding its name, its one pin at the
/// origin where the signal crosses into the sheet. Inputs point right
/// (they sit on the left edge), outputs left.
pub fn port_symbol(key: &str, name: &str, flow: Flow, net: Option<usize>) -> Part {
    let w = up(font::width(name, SIZE) + 2 * GRID);
    let h = GRID / 2;
    let inward = !matches!(flow, Flow::Out);
    // The flag's outline, from the pin end round its body.
    let (pts, side, text_x): (Vec<Point>, Side, Nm) = if inward {
        (
            vec![
                [0, 0],
                [-GRID / 2, h],
                [-w, h],
                [-w, -h],
                [-GRID / 2, -h],
                [0, 0],
            ],
            Side::Right,
            -w + GRID / 2,
        )
    } else {
        (
            vec![
                [0, 0],
                [GRID / 2, h],
                [w, h],
                [w, -h],
                [GRID / 2, -h],
                [0, 0],
            ],
            Side::Left,
            GRID,
        )
    };
    Part {
        key: key.to_owned(),
        kind: PartKind::Port { flow },
        refdes: String::new(),
        value: String::new(),
        graphics: vec![
            Graphic::Polyline {
                points: pts,
                fill: Fill::None,
            },
            Graphic::Text {
                text: name.to_owned(),
                at: [text_x, -SIZE / 3],
                size: SIZE * 4 / 5,
            },
        ],
        pins: vec![Pin {
            number: "1".into(),
            name: String::new(),
            at: [0, 0],
            side,
            length: 0,
            kind: match flow {
                Flow::In => PinKind::Input,
                Flow::Out => PinKind::Output,
                Flow::Bidirectional => PinKind::Bidirectional,
                Flow::Power => PinKind::PowerIn,
                Flow::Passive => PinKind::Passive,
            },
            net,
            nc: false,
            show_number: false,
            layers: Vec::new(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn resistor() -> Part {
        Part {
            key: "r1".into(),
            kind: PartKind::Symbol { prefix: "R".into() },
            refdes: "R1".into(),
            value: "10k".into(),
            graphics: vec![Graphic::Rect {
                start: [-GRID / 2, GRID * 3 / 2],
                end: [GRID / 2, -GRID * 3 / 2],
                fill: Fill::None,
            }],
            pins: vec![
                Pin {
                    number: "1".into(),
                    name: String::new(),
                    at: [0, 3 * GRID],
                    side: Side::Up,
                    length: GRID * 3 / 2,
                    kind: PinKind::Passive,
                    net: None,
                    nc: false,
                    show_number: false,
                    layers: Vec::new(),
                },
                Pin {
                    number: "2".into(),
                    name: String::new(),
                    at: [0, -3 * GRID],
                    side: Side::Down,
                    length: GRID * 3 / 2,
                    kind: PinKind::Passive,
                    net: None,
                    nc: false,
                    show_number: false,
                    layers: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn shapes_turn_with_their_place() {
        let r = resistor();
        let s = Shape::of(&r, Place::new([10 * GRID, 0], 90, false));
        assert_eq!(s.pins[0].end, [7 * GRID, 0]);
        assert_eq!(s.pins[0].side, Side::Left);
        assert_eq!(s.pins[0].base, [10 * GRID - GRID * 3 / 2, 0]);
        assert_eq!(
            s.extent,
            Rect::new([7 * GRID, -GRID / 2], [13 * GRID, GRID / 2])
        );
        assert!(pins_on_grid(&r));
        assert_eq!(
            grid_origin(&r, &Place::default(), [GRID / 3, GRID + 1]),
            [0, GRID]
        );
    }

    #[test]
    fn blocks_and_ports_draw_their_pins_on_the_grid() {
        let b = block_symbol(
            "instance:s1",
            "Sensor",
            &[
                ("a".into(), "SDA".into(), Flow::Bidirectional, None),
                ("b".into(), "INT".into(), Flow::Out, None),
                ("c".into(), "VDD".into(), Flow::Power, None),
            ],
        );
        assert!(pins_on_grid(&b));
        assert!(b.pins.iter().all(|p| on_grid(p.at)));
        let sides: Vec<Side> = b.pins.iter().map(|p| p.side).collect();
        assert_eq!(sides, [Side::Left, Side::Right, Side::Up]);
        let p = port_symbol("port:x", "SDA", Flow::Out, None);
        let s = Shape::of(&p, Place::default());
        assert_eq!(s.pins[0].end, [0, 0]);
        assert_eq!(s.pins[0].side, Side::Left);
        assert!(s.body.min[0] >= 0);
    }

    #[test]
    fn a_stack_draws_copies_behind_and_its_right_pins_past_them() {
        let ports = [
            (
                "a".into(),
                "IN".into(),
                Flow::In,
                vec![Some(0), Some(1), Some(2)],
            ),
            (
                "b".into(),
                "OUT".into(),
                Flow::Out,
                vec![Some(3), Some(4), Some(5)],
            ),
            ("c".into(), "VDD".into(), Flow::Power, vec![Some(6); 3]),
        ];
        let st = stack_symbol("instance:ch_a", "Channel", &ports);
        // Two copies (at most), the front last, and the badge.
        let rects = st
            .graphics
            .iter()
            .filter(|g| matches!(g, Graphic::Rect { .. }))
            .count();
        assert_eq!(rects, 1 + STACK_DRAWN);
        assert!(matches!(
            st.graphics.last(),
            Some(Graphic::Text { text, .. }) if text == "×3"
        ));
        assert!(pins_on_grid(&st));
        let out = st.pins.iter().find(|p| p.number == "b").unwrap();
        let (end, base) = pin_ends(out);
        let (body, _) = local(&st);
        assert_eq!(base[0] + out.length, end[0]);
        assert!(end[0] > body.max[0], "{end:?} {body:?}");
        assert_eq!(out.layers, [Some(4), Some(5)]);
        assert!(out.stacked());
        let vdd = st.pins.iter().find(|p| p.number == "c").unwrap();
        assert!(!vdd.stacked());
        // One layer is the plain block symbol, bit for bit.
        let one: Vec<_> = ports
            .iter()
            .map(|(n, m, f, nets)| (n.clone(), m.clone(), *f, nets[0]))
            .collect();
        let single: Vec<_> = ports
            .iter()
            .map(|(n, m, f, nets)| (n.clone(), m.clone(), *f, vec![nets[0]]))
            .collect();
        assert_eq!(
            stack_symbol("k", "Channel", &single),
            block_symbol("k", "Channel", &one)
        );
    }
}
