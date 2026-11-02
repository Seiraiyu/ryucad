//! Labels and power ports: their drawing, the box each takes, and where
//! one goes beside the pin it serves. Supplies point up and ground down,
//! from any pin, by a short stub where the pin doesn't point that way. A
//! net label is a name on its wire, always reading across the page: along
//! a pin pointing sideways, out of a stub from one pointing up or down.

use crate::font::{self, Anchor, SIZE};
use crate::geom::{GRID, Nm, Point, Rect, Side};
use crate::layout::LabelKind;

/// A label drawn: its lines, its text and the box it all takes (apart
/// from its anchor's own node, which its pin or stub holds).
pub struct Drawn {
    pub lines: Vec<Vec<Point>>,
    /// The text, its anchor point and anchoring, and whether it reads up
    /// the page.
    pub text: Option<(Point, Anchor, bool)>,
    pub bounds: Rect,
}

/// `p` given with y along `side` (x to its right), from `at`.
fn along(at: Point, side: Side, p: Point) -> Point {
    let [x, y] = p;
    let q = match side {
        Side::Up => [x, y],
        Side::Down => [-x, -y],
        Side::Left => [-y, x],
        Side::Right => [y, -x],
    };
    [at[0] + q[0], at[1] + q[1]]
}

/// Whether a ground symbol writes its rail's name: every ground but one
/// called just `GND`.
pub fn ground_named(name: &str) -> bool {
    name != "GND"
}

/// A label of `kind` naming `name`, anchored at `at`, pointing `side`.
pub fn drawn(kind: LabelKind, name: &str, at: Point, side: Side) -> Drawn {
    let w = font::width(name, SIZE);
    let a = |p: Point| along(at, side, p);
    // Text past the tip `tip` (local), upright, away from the anchor.
    let text_past = |tip: Nm| -> (Point, Anchor, bool, Rect) {
        let t = a([0, tip]);
        let (anchor, base) = match side {
            Side::Up => (Anchor::Middle, [t[0], t[1] + GRID / 4 + SIZE / 4]),
            Side::Down => (Anchor::Middle, [t[0], t[1] - GRID / 4 - SIZE]),
            Side::Left => (Anchor::End, [t[0] - GRID / 2, t[1] - SIZE / 3]),
            Side::Right => (Anchor::Start, [t[0] + GRID / 2, t[1] - SIZE / 3]),
        };
        (
            base,
            anchor,
            false,
            font::text_box(name, SIZE, base, anchor),
        )
    };
    match kind {
        LabelKind::Power => {
            let lines = vec![
                vec![a([0, 0]), a([0, GRID])],
                vec![a([-GRID, GRID]), a([GRID, GRID])],
            ];
            let (base, anchor, v, tb) = text_past(GRID);
            let shape = Rect::of([a([-GRID, GRID / 2]), a([GRID, GRID])]).expect("points");
            Drawn {
                lines,
                text: Some((base, anchor, v)),
                bounds: shape.union(tb),
            }
        }
        LabelKind::Ground => {
            let mut lines = vec![vec![a([0, 0]), a([0, GRID])]];
            for (i, half) in [GRID, GRID * 2 / 3, GRID / 3].into_iter().enumerate() {
                let y = GRID + GRID / 3 * i as Nm;
                lines.push(vec![a([-half, y]), a([half, y])]);
            }
            // To just past its last bar (at a step and two thirds), short
            // of the row two steps out, which a neighbouring pin's wire or
            // label may take.
            let shape =
                Rect::of([a([-GRID, GRID / 2]), a([GRID, 2 * GRID - GRID / 4])]).expect("points");
            if ground_named(name) {
                let (base, anchor, v, tb) = text_past(2 * GRID);
                Drawn {
                    lines,
                    text: Some((base, anchor, v)),
                    bounds: shape.union(tb),
                }
            } else {
                Drawn {
                    lines,
                    text: None,
                    bounds: shape,
                }
            }
        }
        LabelKind::Net => {
            // A name on its wire: a line out from the anchor as long as the
            // name, the name just above it (beside it, reading up, on a
            // vertical one). No outline, so it can't be taken for a port's
            // flag, which leaves the sheet.
            let len = w + GRID;
            let line = vec![at, a([0, len])];
            let gap = GRID / 4;
            let (base, anchor, vertical) = match side {
                Side::Right => (
                    [at[0] + GRID / 2, at[1] + gap + SIZE / 4],
                    Anchor::Start,
                    false,
                ),
                Side::Left => (
                    [at[0] - GRID / 2, at[1] + gap + SIZE / 4],
                    Anchor::End,
                    false,
                ),
                Side::Up => (
                    [at[0] - gap - SIZE / 4, at[1] + GRID / 2],
                    Anchor::Start,
                    true,
                ),
                Side::Down => (
                    [at[0] - gap - SIZE / 4, at[1] - GRID / 2],
                    Anchor::End,
                    true,
                ),
            };
            // The text's box, and the line's half a step out from the
            // anchor (whose node its pin or stub holds) a quarter each way.
            let tb = if vertical {
                let flat = font::text_box(name, SIZE, [0, 0], anchor);
                // Turned a quarter counterclockwise about the baseline's
                // anchor: x up the page, y to the left.
                Rect::new(
                    [base[0] - flat.max[1], base[1] + flat.min[0]],
                    [base[0] - flat.min[1], base[1] + flat.max[0]],
                )
            } else {
                font::text_box(name, SIZE, base, anchor)
            };
            let wire = Rect::of([a([-gap, GRID / 2]), a([gap, len])]).expect("points");
            Drawn {
                lines: vec![line],
                text: Some((base, anchor, vertical)),
                bounds: wire.union(tb),
            }
        }
    }
}

/// The box a label takes.
pub fn label_box(kind: LabelKind, name: &str, at: Point, side: Side) -> Rect {
    drawn(kind, name, at, side).bounds
}

/// Which way a label of `kind` points, for a pin pointing `side`: a
/// supply always up and ground always down; a net label along a pin
/// pointing sideways, and to the right of a pin pointing up or down (a
/// label never reads up the page).
pub fn pointing(kind: LabelKind, side: Side) -> Side {
    match kind {
        LabelKind::Power => Side::Up,
        LabelKind::Ground => Side::Down,
        LabelKind::Net if side.horizontal() => side,
        LabelKind::Net => Side::Right,
    }
}

/// Whether a label of `kind` may point `to`: a supply only up, ground
/// only down, a net label only sideways.
pub fn upright(kind: LabelKind, to: Side) -> bool {
    match kind {
        LabelKind::Power => to == Side::Up,
        LabelKind::Ground => to == Side::Down,
        LabelKind::Net => to.horizontal(),
    }
}

fn add(p: Point, s: Side, k: Nm) -> Point {
    let d = s.step();
    [p[0] + d[0] * k, p[1] + d[1] * k]
}

/// Where a label for a pin ending at `end`, pointing `side`, may go, best
/// first, straight out of the pin or round it (see [`detours`] for the
/// rest): its stub (a run from the pin's end, or none), its anchor and
/// which way it points. Every one points as [`upright`] lets it, and never
/// back along the stub's last step. Stubs grow out to twenty steps, so a
/// crowded pin still finds room.
pub fn candidates(kind: LabelKind, end: Point, side: Side) -> Vec<(Vec<Point>, Point, Side)> {
    let mut out = Vec::new();
    // The ways a label may point from the end of a stub whose last step
    // went `last`, the one along it first.
    let ways = |last: Side| -> Vec<Side> {
        let mut v: Vec<Side> = [last, Side::Right, Side::Left, Side::Up, Side::Down]
            .into_iter()
            .filter(|t| upright(kind, *t) && *t != last.opposite())
            .collect();
        v.dedup();
        v
    };
    // Straight out from the pin.
    for to in ways(side) {
        if to == side {
            out.push((Vec::new(), end, to));
        }
    }
    // Two steps at least: a port turning off a stub of one would touch
    // the next pin's line.
    for k in 2..=20 {
        let at = add(end, side, k);
        for to in ways(side) {
            out.push((vec![end, at], at, to));
        }
    }
    if !ways(side).contains(&side) && matches!(kind, LabelKind::Power | LabelKind::Ground) {
        // A supply on a pin pointing down, ground on one pointing up: out
        // a step, across, and round.
        for k in 2..=12 {
            for across in [side.turned(), side.turned().opposite()] {
                let a = add(end, side, 1);
                let at = add(a, across, k);
                for to in ways(across) {
                    out.push((vec![end, a, at], at, to));
                }
            }
        }
    }
    out
}

/// Where a label goes when [`candidates`] has no room and no wire reaches
/// its net: off the pin another way (for a pin drawn pointing into its own
/// body), then dog-legs, pointing as [`upright`] lets it.
pub fn detours(kind: LabelKind, end: Point, side: Side) -> Vec<(Vec<Point>, Point, Side)> {
    let mut out = Vec::new();
    let ways = |last: Side| -> Vec<Side> {
        let mut v: Vec<Side> = [last, Side::Right, Side::Left, Side::Up, Side::Down]
            .into_iter()
            .filter(|t| upright(kind, *t) && *t != last.opposite())
            .collect();
        v.dedup();
        v
    };
    for other in [side.turned(), side.turned().opposite(), side.opposite()] {
        for k in 2..=8 {
            let at = add(end, other, k);
            for to in ways(other) {
                out.push((vec![end, at], at, to));
            }
        }
    }
    // Last, dog-legs: out, across either way, and out again.
    for k in 1..=6 {
        for m in 2..=12 {
            for across in [side.turned(), side.turned().opposite()] {
                let a = add(end, side, k);
                let b = add(a, across, m);
                let at = add(b, side, 2);
                for to in ways(side) {
                    out.push((vec![end, a, b, at], at, to));
                }
            }
        }
    }
    out
}

/// Where a label goes when no upright place is free: along its pin, as labels once all went, a stub out to twenty
/// steps (lint scores it lower). The first is where a label with nowhere
/// at all goes.
pub fn sideways(end: Point, side: Side) -> Vec<(Vec<Point>, Point, Side)> {
    let mut out = vec![(Vec::new(), end, side)];
    for k in 2..=20 {
        let at = add(end, side, k);
        out.push((vec![end, at], at, side));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplies_point_up_and_ground_down_by_stubs() {
        let c = candidates(LabelKind::Power, [0, 0], Side::Up);
        assert_eq!(c[0], (Vec::new(), [0, 0], Side::Up));
        let c = candidates(LabelKind::Power, [0, 0], Side::Down);
        assert_eq!(c[0].0, vec![[0, 0], [0, -GRID], [2 * GRID, -GRID]]);
        // From a pin pointing sideways, out a step and round.
        let c = candidates(LabelKind::Ground, [0, 0], Side::Left);
        assert_eq!(
            c[0],
            (vec![[0, 0], [-2 * GRID, 0]], [-2 * GRID, 0], Side::Down)
        );
        let c = candidates(LabelKind::Power, [0, 0], Side::Right);
        assert_eq!(c[0], (vec![[0, 0], [2 * GRID, 0]], [2 * GRID, 0], Side::Up));
        for side in [Side::Up, Side::Down, Side::Left, Side::Right] {
            for kind in [LabelKind::Power, LabelKind::Ground, LabelKind::Net] {
                let mut all = candidates(kind, [0, 0], side);
                all.extend(detours(kind, [0, 0], side));
                for (stub, _, to) in all {
                    assert!(upright(kind, to), "{kind:?} from {side:?} points {to:?}");
                    // Never back along its stub.
                    if let [.., a, b] = stub[..] {
                        let back = [a[0] - b[0], a[1] - b[1]];
                        assert_ne!(to.step().map(|v| v.signum()), back.map(|v| v.signum()));
                    }
                }
            }
        }
        // A net label from a pin pointing up reads across, out of a stub.
        let c = candidates(LabelKind::Net, [0, 0], Side::Up);
        assert_eq!(
            c[0],
            (vec![[0, 0], [0, 2 * GRID]], [0, 2 * GRID], Side::Right)
        );
        let b = label_box(LabelKind::Ground, "GND", [0, 0], Side::Down);
        assert!(b.max[1] < 0);
        let n = label_box(LabelKind::Net, "SDA", [0, 0], Side::Left);
        assert!(n.max[0] < 0 && n.min[0] < -GRID);
        // A net label's name sits above its line, which is all it draws.
        let d = drawn(LabelKind::Net, "SDA", [0, 0], Side::Right);
        assert_eq!(d.lines.len(), 1);
        assert_eq!(d.lines[0].len(), 2);
        let (base, _, vertical) = d.text.unwrap();
        assert!(!vertical && base[1] > 0);
        let p = label_box(LabelKind::Power, "3V3", [0, 0], Side::Right);
        assert!(p.min[0] > 0);
    }
}
