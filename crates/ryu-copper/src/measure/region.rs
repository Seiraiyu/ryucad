//! Whether a point lies inside a region: on its outline, or with a
//! winding number other than zero (the nonzero rule the rasterizer fills
//! by), counted exactly.
//!
//! The winding number is the signed count of the times the outline passes
//! the horizontal through the point to its right, with a point at the
//! point's height counted as above it: a piece adds `+1` where it goes
//! from below to above and `-1` back. A segment does that at most once,
//! from its ends. An arc does it where it meets the horizontal: once
//! where it crosses; at the top of its circle twice, which cancel; at the
//! bottom not at all; and at one of its own ends once if it goes below
//! from there, the piece before or after it taking the other side.

use std::cmp::Ordering;

use ryu_board::geom::{Dir, Point};

use super::core::{ArcP, Core, SegP};
use super::exact::{Dirn, Fp, sweeps};
use crate::surd::{sign1, sign2};
use crate::wide::Int;

fn i(v: i128) -> Int {
    Int::from(v)
}

/// Whether an exact point is at or above height `y`.
fn above(p: &Fp, y: i64) -> bool {
    sign1(p.ny - i(y.into()) * p.s, p.my, p.x) != Ordering::Less
}

/// What a segment adds to the winding number about `at`.
fn seg_winding(s: &SegP, at: Point) -> i32 {
    let [a, b] = s.ends();
    let (ua, ub) = (above(&Fp::of(a), at.y), above(&Fp::of(b), at.y));
    if ua == ub {
        return 0;
    }
    // The crossing's x against at.x: (b.x - x)U.y + (y - b.y)U.x over U.y.
    let (ux, uy) = s.u;
    let num = i128::from(s.b.x - at.x) * uy + i128::from(at.y - s.b.y) * ux;
    let right = (num > 0) == (uy > 0);
    if !right || num == 0 {
        return 0;
    }
    let up = if ub { 1 } else { -1 };
    if s.rev { -up } else { up }
}

/// What an arc adds to the winding number about `at`.
fn arc_winding(a: &ArcP, at: Point) -> i32 {
    let c = a.arc.centre;
    let dy = i128::from(at.y - c.y);
    let k = a.r2 - dy * dy;
    if k < 0 {
        return 0;
    }
    let turn: i128 = match a.arc.dir {
        Dir::Ccw => 1,
        Dir::Cw => -1,
    };
    let q = Fp::of(a.q);
    let mut total = 0;
    let sides: &[i128] = if k == 0 { &[1] } else { &[1, -1] };
    for &sigma in sides {
        // The meeting (c.x + σ√k, at.y), as a direction from the centre.
        let d = Dirn {
            n: (Int::ZERO, i(dy)),
            m: (i(sigma), Int::ZERO),
            x: i(k),
        };
        if !sweeps(a, &d) {
            continue;
        }
        // Only meetings to the right count.
        if sign1(i((c.x - at.x).into()), i(sigma), i(k)) != Ordering::Greater {
            continue;
        }
        let s = a.arc.start;
        let at_start = !a.arc.is_full()
            && s.y == at.y
            && sign1(i((s.x - c.x).into()), i(-sigma), i(k)) == Ordering::Equal;
        let at_end = !a.arc.is_full()
            && sign1(q.ny - i(at.y.into()) * q.s, q.my, q.x) == Ordering::Equal
            && sign2(
                q.nx - i(c.x.into()) * q.s,
                q.mx,
                -(i(sigma) * q.s),
                Int::ZERO,
                q.x,
                i(k),
            ) == Ordering::Equal;
        if k == 0 {
            // Tangent: the top (dy > 0) goes below both ways.
            let top = dy > 0;
            if at_start && top {
                total -= 1;
            } else if at_end && top {
                total += 1;
            }
            continue;
        }
        // Going forward, the arc rises here when σ·turn > 0.
        let rising = sigma * turn > 0;
        if at_start {
            // Leaves the start downward: from above to below.
            if !rising {
                total -= 1;
            }
        } else if at_end {
            // Arrives at the end from below.
            if rising {
                total += 1;
            }
        } else if rising {
            total += 1;
        } else {
            total -= 1;
        }
    }
    total
}

/// The winding number of closed outlines (their pieces in any order)
/// about a point not on them.
pub fn winding(cores: &[Core], at: Point) -> i32 {
    cores
        .iter()
        .map(|c| match c {
            Core::Seg(s) => seg_winding(s, at),
            Core::Arc(a) => arc_winding(a, at),
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::core::edge_cores;
    use ryu_board::geom::{Arc, Contour, Edge};

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn cores(c: &Contour) -> Vec<Core> {
        let mut out = Vec::new();
        for e in c.edges() {
            edge_cores(e, &mut out);
        }
        out
    }

    fn square() -> Contour {
        let q = [p(0, 0), p(10, 0), p(10, 10), p(0, 10)];
        Contour::new((0..4).map(|k| Edge::line(q[k], q[(k + 1) % 4])).collect()).unwrap()
    }

    #[test]
    fn a_square() {
        let c = cores(&square());
        assert_eq!(winding(&c, p(5, 5)), 1);
        assert_eq!(winding(&c, p(15, 5)), 0);
        assert_eq!(winding(&c, p(-5, 5)), 0);
        // At the height of a corner, outside and inside.
        assert_eq!(winding(&c, p(-5, 0)), 0);
        assert_eq!(winding(&c, p(-5, 10)), 0);
        let r = cores(&square().reversed());
        assert_eq!(winding(&r, p(5, 5)), -1);
    }

    #[test]
    fn a_circle_and_a_half_disc() {
        let circle = Contour::new(vec![Edge::Arc(Arc {
            start: p(100, 0),
            end: p(100, 0),
            centre: p(0, 0),
            dir: Dir::Ccw,
        })])
        .unwrap();
        let c = cores(&circle);
        assert_eq!(winding(&c, p(0, 0)), 1);
        assert_eq!(winding(&c, p(-99, 0)), 1);
        assert_eq!(winding(&c, p(0, 99)), 1);
        assert_eq!(winding(&c, p(0, 101)), 0);
        assert_eq!(winding(&c, p(-101, 0)), 0);
        assert_eq!(winding(&c, p(-50, 100)), 0);
        assert_eq!(winding(&c, p(-50, -100)), 0);
        assert_eq!(winding(&c, p(70, 71)), 1);
        assert_eq!(winding(&c, p(71, 71)), 0);
        // A half disc above the axis, its diameter at the start's height.
        let half = Contour::new(vec![
            Edge::line(p(-100, 0), p(100, 0)),
            Edge::Arc(Arc {
                start: p(100, 0),
                end: p(-100, 0),
                centre: p(0, 0),
                dir: Dir::Ccw,
            }),
        ])
        .unwrap();
        let h = cores(&half);
        assert_eq!(winding(&h, p(0, 50)), 1);
        assert_eq!(winding(&h, p(0, -50)), 0);
        assert_eq!(winding(&h, p(-200, 0)), 0);
        assert_eq!(winding(&h, p(-200, 100)), 0);
        assert_eq!(winding(&h, p(0, 100 - 1)), 1);
        // A half disc below, clockwise from the left.
        let low = Contour::new(vec![
            Edge::Arc(Arc {
                start: p(-100, 0),
                end: p(100, 0),
                centre: p(0, 0),
                dir: Dir::Ccw,
            }),
            Edge::line(p(100, 0), p(-100, 0)),
        ])
        .unwrap();
        let l = cores(&low);
        assert_eq!(winding(&l, p(0, -50)), 1);
        assert_eq!(winding(&l, p(0, 50)), 0);
        assert_eq!(winding(&l, p(-200, 0)), 0);
        assert_eq!(winding(&l, p(-200, -100)), 0);
    }

    #[test]
    fn a_rounded_square_with_off_circle_ends() {
        // A 2000 square whose corners are quarter arcs of radius 300, one
        // written end a nanometre off its circle.
        let k = 300;
        let edges = vec![
            Edge::line(p(k, 0), p(2000 - k, 0)),
            Edge::Arc(Arc {
                start: p(2000 - k, 0),
                end: p(2000, k + 1),
                centre: p(2000 - k, k),
                dir: Dir::Ccw,
            }),
            Edge::line(p(2000, k + 1), p(2000, 2000 - k)),
            Edge::Arc(Arc {
                start: p(2000, 2000 - k),
                end: p(2000 - k, 2000),
                centre: p(2000 - k, 2000 - k),
                dir: Dir::Ccw,
            }),
            Edge::line(p(2000 - k, 2000), p(k, 2000)),
            Edge::Arc(Arc {
                start: p(k, 2000),
                end: p(0, 2000 - k),
                centre: p(k, 2000 - k),
                dir: Dir::Ccw,
            }),
            Edge::line(p(0, 2000 - k), p(0, k)),
            Edge::Arc(Arc {
                start: p(0, k),
                end: p(k, 0),
                centre: p(k, k),
                dir: Dir::Ccw,
            }),
        ];
        let c = cores(&Contour::new(edges).unwrap());
        for (x, y, w) in [
            (1000, 1000, 1),
            (1, 1000, 1),
            (1999, 1000, 1),
            (1999, k, 1),
            (1999, k + 1, 1),
            (2001, k, 0),
            (10, 10, 0),
            (1990, 1990, 0),
            (-1, k, 0),
            (1000, 2001, 0),
        ] {
            assert_eq!(winding(&c, p(x, y)), w, "({x}, {y})");
        }
    }
}
