//! Labels and power ports: their drawing, the box each takes, and where
//! one goes beside the pin it serves. Supplies point up and ground down,
//! by a short stub where the pin doesn't point that way; only where
//! neither fits does a port point along its pin. A net label points the
//! way its pin does.

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
            let shape = Rect::of([a([-GRID, GRID / 2]), a([GRID, 2 * GRID])]).expect("points");
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
            let len = w + GRID;
            let t = SIZE * 3 / 4;
            let pts = [
                [0, 0],
                [t, GRID / 2],
                [t, len],
                [-t, len],
                [-t, GRID / 2],
                [0, 0],
            ];
            let outline: Vec<Point> = pts.iter().map(|p| a(*p)).collect();
            let (base, anchor, vertical) = match side {
                Side::Right => (a([SIZE / 3, GRID * 3 / 4]), Anchor::Start, false),
                Side::Left => (a([-SIZE / 3, GRID * 3 / 4]), Anchor::End, false),
                Side::Up => (
                    [at[0] + SIZE / 3, at[1] + GRID * 3 / 4],
                    Anchor::Start,
                    true,
                ),
                Side::Down => ([at[0] + SIZE / 3, at[1] - GRID * 3 / 4], Anchor::End, true),
            };
            let bounds = Rect::of([a([-t, GRID / 2]), a([t, len])]).expect("points");
            Drawn {
                lines: vec![outline],
                text: Some((base, anchor, vertical)),
                bounds,
            }
        }
    }
}

/// The box a label takes.
pub fn label_box(kind: LabelKind, name: &str, at: Point, side: Side) -> Rect {
    drawn(kind, name, at, side).bounds
}

/// Which way a label of `kind` points, for a pin pointing `side`: a
/// supply up and ground down from a pin pointing up or down, along the
/// pin from a pin pointing sideways (pins sit two steps apart there, and a
/// port hanging off one would cover the next); a net label along its pin.
pub fn pointing(kind: LabelKind, side: Side) -> Side {
    match kind {
        LabelKind::Power | LabelKind::Ground if side.horizontal() => side,
        LabelKind::Power => Side::Up,
        LabelKind::Ground => Side::Down,
        LabelKind::Net => side,
    }
}

fn add(p: Point, s: Side, k: Nm) -> Point {
    let d = s.step();
    [p[0] + d[0] * k, p[1] + d[1] * k]
}

/// Where a label for a pin ending at `end`, pointing `side`, may go, best
/// first: its stub (a run from the pin's end, or none), its anchor and
/// which way it points. Stubs grow out to twenty steps, so a crowded pin
/// still finds room.
pub fn candidates(kind: LabelKind, end: Point, side: Side) -> Vec<(Vec<Point>, Point, Side)> {
    let to = pointing(kind, side);
    let mut out = Vec::new();
    if to == side.opposite() {
        // A supply on a pin pointing down, ground on one pointing up: out
        // a step, across, and round.
        for k in 2..=12 {
            for across in [side.turned(), side.turned().opposite()] {
                let a = add(end, side, 1);
                let at = add(a, across, k);
                out.push((vec![end, a, at], at, to));
            }
        }
    } else {
        out.push((Vec::new(), end, to));
        for k in 2..=20 {
            let at = add(end, side, k);
            out.push((vec![end, at], at, to));
        }
    }
    // Then the other ways, for a pin drawn pointing into its own body.
    for other in [side.turned(), side.turned().opposite(), side.opposite()] {
        for k in 2..=8 {
            let at = add(end, other, k);
            out.push((vec![end, at], at, pointing(kind, other)));
        }
    }
    // Last, dog-legs: out, across either way, and out again.
    for k in 1..=6 {
        for m in 2..=12 {
            for across in [side.turned(), side.turned().opposite()] {
                let a = add(end, side, k);
                let b = add(a, across, m);
                let at = add(b, side, 2);
                out.push((vec![end, a, b, at], at, pointing(kind, side)));
            }
        }
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
        let c = candidates(LabelKind::Ground, [0, 0], Side::Left);
        assert_eq!(c[0], (Vec::new(), [0, 0], Side::Left));
        let b = label_box(LabelKind::Ground, "GND", [0, 0], Side::Down);
        assert!(b.max[1] < 0);
        let n = label_box(LabelKind::Net, "SDA", [0, 0], Side::Left);
        assert!(n.max[0] < 0 && n.min[0] < -GRID);
        let p = label_box(LabelKind::Power, "3V3", [0, 0], Side::Right);
        assert!(p.min[0] > 0);
    }
}
