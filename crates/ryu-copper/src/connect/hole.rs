//! Holes against solids, exactly: whether a solid holds a hole strictly
//! inside it, and whether a solid reaches outside one. A hole is its core
//! segment (a point for a round hole) with half its diameter round it, so
//! both are comparisons of distances to that segment, `pair::core_cmp`'s.

use std::cmp::Ordering;

use ryu_board::geom::{Nm, Point, Rect};

use crate::layout::HoleShape;
use crate::measure::core::{Core, Pt, SegP};
use crate::measure::{Solid, pair};
use crate::tree::grown;

/// A hole's core segment's ends (the same point for a round hole) and its
/// diameter.
fn core_of(h: &HoleShape) -> (Point, Point, Nm) {
    match *h {
        HoleShape::Round { at, d } => (at, at, d),
        HoleShape::Slot { a, b, d } => (a, b, d),
    }
}

fn dot(p: Point) -> Core {
    Core::Seg(SegP::new(p, p))
}

/// Whether the closed hole lies in the solid's interior. A thick piece
/// holds it when every point of the hole's core lies nearer the piece's
/// core than `(w - d) / 2`; along a segment core the distance is convex,
/// so the hole core's ends decide; along an arc core only a round hole is
/// tried. A region holds it when it holds the hole's core start and no
/// outline piece comes within `d / 2` of the hole's core.
pub(crate) fn holds(solid: &Solid, h: &HoleShape) -> bool {
    let (a, b, d) = core_of(h);
    match solid {
        Solid::Thick { core, w } => {
            if *w <= d {
                return false;
            }
            let dd = i128::from(*w - d);
            let inside = |p: Point| pair::core_cmp(core, &dot(p), dd) == Ordering::Less;
            match core {
                Core::Seg(_) => inside(a) && inside(b),
                Core::Arc(_) => a == b && inside(a),
            }
        }
        Solid::Region(s) => {
            if !s.contains(a) {
                return false;
            }
            let hole = Core::Seg(SegP::new(a, b));
            let q = grown(&Rect::new(a, b), d / 2 + 1);
            s.near(&q)
                .all(|c| pair::core_cmp(c, &hole, i128::from(d)) == Ordering::Greater)
        }
    }
}

/// The integer points a piece is known to pass through: a segment's
/// integer ends, an arc's start and its true end when that is integer.
fn points(c: &Core) -> Vec<Point> {
    match c {
        Core::Seg(s) => s
            .ends()
            .into_iter()
            .filter_map(|p| match p {
                Pt::At(q) => Some(q),
                Pt::End { .. } => None,
            })
            .collect(),
        Core::Arc(r) => {
            let mut out = vec![r.arc.start];
            if let Pt::At(q) = r.q {
                out.push(q);
            }
            out
        }
    }
}

/// Whether some point of the solid lies outside the closed hole: a thick
/// piece wider than the hole, or one with a core point farther than
/// `(d - w) / 2` from the hole's core; a region with an outline point
/// farther than `d / 2`. Only known points are tried, so `false` may be
/// wrong for an arc bulging out between them; callers treat `false` as
/// "may lie inside", which only costs a boolean.
pub(crate) fn reaches_out(solid: &Solid, h: &HoleShape) -> bool {
    let (a, b, d) = core_of(h);
    let hole = Core::Seg(SegP::new(a, b));
    let far = |q: Point, dd: Nm| {
        dd < 0 || pair::core_cmp(&hole, &dot(q), i128::from(dd)) == Ordering::Greater
    };
    match solid {
        Solid::Thick { core, w } => points(core).into_iter().any(|q| far(q, d - w)),
        Solid::Region(s) => s
            .cores
            .iter()
            .any(|c| points(c).into_iter().any(|q| far(q, d))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::{Body, BodyOptions};
    use ryu_board::geom::Region;
    use ryu_board::model::Polarity;
    use ryu_board::shape::{Shape, circle};

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn disc(c: Point, r: Nm) -> Body {
        let shape = Shape::regions(Polarity::Positive, [Region::solid(circle(c, r).unwrap())]);
        Body::of_shape(&shape, &BodyOptions::default())
    }

    fn one(b: &Body) -> &Solid {
        &b.solids[0]
    }

    #[test]
    fn solids_hold_holes_strictly_inside() {
        let hole = HoleShape::Round {
            at: p(0, 0),
            d: 200,
        };
        // A round pad as a region, and as a round pen's dot.
        assert!(holds(one(&disc(p(0, 0), 300)), &hole));
        assert!(holds(one(&Body::stroke(p(0, 0), p(0, 0), 600)), &hole));
        // The hole's edge on the solid's: not strictly inside.
        assert!(!holds(one(&disc(p(0, 0), 100)), &hole));
        assert!(!holds(one(&Body::stroke(p(0, 0), p(0, 0), 200)), &hole));
        assert!(!holds(one(&Body::stroke(p(200, 0), p(200, 0), 600)), &hole));
        assert!(holds(one(&Body::stroke(p(199, 0), p(199, 0), 600)), &hole));
        // A slot along a wide trace, then past its end.
        let trace = Body::stroke(p(-1000, 0), p(1000, 0), 500);
        let inside = HoleShape::Slot {
            a: p(-500, 0),
            b: p(500, 0),
            d: 200,
        };
        let past = HoleShape::Slot {
            a: p(-500, 0),
            b: p(1200, 0),
            d: 200,
        };
        assert!(holds(one(&trace), &inside));
        assert!(!holds(one(&trace), &past));
        // A pad off to the side holds nothing.
        assert!(!holds(one(&disc(p(1000, 0), 300)), &hole));
    }

    #[test]
    fn solids_reach_outside_holes() {
        let hole = HoleShape::Round {
            at: p(0, 0),
            d: 200_000,
        };
        // A trace from the centre out; a stub wholly inside; a pad round it.
        let out = Body::stroke(p(0, 0), p(2_000_000, 0), 100_000);
        assert!(reaches_out(one(&out), &hole));
        let stub = Body::stroke(p(0, 0), p(30_000, 0), 50_000);
        assert!(!reaches_out(one(&stub), &hole));
        assert!(reaches_out(one(&disc(p(0, 0), 300_000)), &hole));
        // Wider than the hole: out whatever its place.
        let dot = Body::stroke(p(0, 0), p(0, 0), 300_000);
        assert!(reaches_out(one(&dot), &hole));
        // A small disc inside it isn't.
        assert!(!reaches_out(one(&disc(p(10_000, 0), 50_000)), &hole));
    }
}
