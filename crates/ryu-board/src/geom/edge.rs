//! Segments and centre-form arcs.

use std::cmp::Ordering;

use super::{Point, V, div_round, orient};

/// A straight edge from `a` to `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    pub a: Point,
    pub b: Point,
}

/// Which way an arc turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Cw,
    Ccw,
}

impl Dir {
    pub fn reversed(self) -> Dir {
        match self {
            Dir::Cw => Dir::Ccw,
            Dir::Ccw => Dir::Cw,
        }
    }
}

/// A circular arc in centre form, as ODB++, Gerber and IPC-2581 write them.
///
/// Its radius is the distance from `centre` to `start`; it ends where the
/// ray from `centre` through `end` meets that circle, so an `end` rounded
/// off the circle still describes one arc. An `end` in the direction of
/// `start` (in particular `end == start`) makes a full circle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arc {
    pub start: Point,
    pub end: Point,
    pub centre: Point,
    pub dir: Dir,
}

impl Arc {
    /// The square of the radius, exactly.
    pub fn radius2(&self) -> i128 {
        self.start.minus(self.centre).len2()
    }

    pub fn radius(&self) -> f64 {
        ryu_math::sqrt(self.radius2() as f64)
    }

    /// Whether the arc has no radius (its start is its centre) or no end
    /// direction (its end is its centre).
    pub fn is_degenerate(&self) -> bool {
        self.start == self.centre || self.end == self.centre
    }

    /// Whether the arc is a whole circle.
    pub fn is_full(&self) -> bool {
        let s = self.start.minus(self.centre);
        let e = self.end.minus(self.centre);
        s.cross(e) == 0 && s.dot(e) > 0
    }

    /// How far the written end lies off the circle, in nanometres.
    pub fn end_error(&self) -> f64 {
        let e = ryu_math::sqrt(self.end.minus(self.centre).len2() as f64);
        (e - self.radius()).abs()
    }

    /// The angle the arc sweeps, in radians, in `(0, 2π]`.
    pub fn sweep(&self) -> f64 {
        if self.is_full() {
            return 2.0 * std::f64::consts::PI;
        }
        let s = self.start.minus(self.centre);
        let e = self.end.minus(self.centre);
        let mut a = ryu_math::atan2(s.cross(e) as f64, s.dot(e) as f64);
        if self.dir == Dir::Cw {
            a = -a;
        }
        if a <= 0.0 {
            a += 2.0 * std::f64::consts::PI;
        }
        a
    }

    /// The same arc run the other way.
    pub fn reversed(&self) -> Arc {
        Arc {
            start: self.end,
            end: self.start,
            centre: self.centre,
            dir: self.dir.reversed(),
        }
    }

    /// Whether the arc passes the direction `v` from its centre, its ends
    /// included. Exact.
    pub fn sweeps(&self, v: V) -> bool {
        if self.is_full() {
            return true;
        }
        let s = self.start.minus(self.centre);
        let e = self.end.minus(self.centre);
        match self.dir {
            Dir::Ccw => ccw_cmp(s, v, e) != Ordering::Greater,
            Dir::Cw => ccw_cmp(e, v, s) != Ordering::Greater,
        }
    }

    /// The arc from `a` through `m` to `b`, its centre rounded to the
    /// nearest nanometre (ryueda's three-point arcs). `None` when the
    /// points are collinear or `a == b`.
    pub fn through(a: Point, m: Point, b: Point) -> Option<Arc> {
        if a == b {
            return None;
        }
        let turn = orient(a, m, b);
        if turn == 0 {
            return None;
        }
        let mv = m.minus(a);
        let bv = b.minus(a);
        let d = 2 * mv.cross(bv);
        let ux = bv.1 * mv.len2() - mv.1 * bv.len2();
        let uy = mv.0 * bv.len2() - bv.0 * mv.len2();
        let centre = Point::new(a.x + div_round(ux, d) as i64, a.y + div_round(uy, d) as i64);
        Some(Arc {
            start: a,
            end: b,
            centre,
            dir: if turn > 0 { Dir::Ccw } else { Dir::Cw },
        })
    }
}

/// One edge of a contour or a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edge {
    Segment(Segment),
    Arc(Arc),
}

impl Edge {
    pub fn line(a: Point, b: Point) -> Edge {
        Edge::Segment(Segment { a, b })
    }

    pub fn start(&self) -> Point {
        match self {
            Edge::Segment(s) => s.a,
            Edge::Arc(a) => a.start,
        }
    }

    pub fn end(&self) -> Point {
        match self {
            Edge::Segment(s) => s.b,
            Edge::Arc(a) => a.end,
        }
    }

    pub fn reversed(&self) -> Edge {
        match self {
            Edge::Segment(s) => Edge::line(s.b, s.a),
            Edge::Arc(a) => Edge::Arc(a.reversed()),
        }
    }
}

/// Which half-turn counterclockwise from `from` the vector `v` lies in:
/// 0 for `[0, π)`, 1 for `[π, 2π)`.
fn half_turn(from: V, v: V) -> u8 {
    let c = from.cross(v);
    if c > 0 || (c == 0 && from.dot(v) > 0) {
        0
    } else {
        1
    }
}

/// Compares the counterclockwise angles from `from` to `a` and to `b`,
/// each in `[0, 2π)`. Exact; no vector may be zero.
fn ccw_cmp(from: V, a: V, b: V) -> Ordering {
    half_turn(from, a)
        .cmp(&half_turn(from, b))
        .then_with(|| 0.cmp(&a.cross(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn angle_order_is_exact() {
        let x = V(1, 0);
        assert_eq!(ccw_cmp(x, V(0, 1), V(-1, 0)), Ordering::Less);
        assert_eq!(ccw_cmp(x, V(0, -1), V(-1, 0)), Ordering::Greater);
        assert_eq!(ccw_cmp(x, V(1, 0), V(5, 0)), Ordering::Equal);
        assert_eq!(ccw_cmp(x, V(-1, 0), V(-3, 0)), Ordering::Equal);
        assert_eq!(ccw_cmp(x, V(1, 0), V(1, -1)), Ordering::Less);
        // Directions a nanometre apart at the limit.
        let big = 1i128 << 41;
        assert_eq!(ccw_cmp(x, V(big, 1), V(big, 2)), Ordering::Less);
    }

    #[test]
    fn quarter_arcs_sweep_their_quadrant() {
        let a = Arc {
            start: p(10, 0),
            end: p(0, 10),
            centre: p(0, 0),
            dir: Dir::Ccw,
        };
        assert!(a.sweeps(V(1, 1)));
        assert!(a.sweeps(V(1, 0)) && a.sweeps(V(0, 1)));
        assert!(!a.sweeps(V(-1, 0)) && !a.sweeps(V(0, -1)));
        let b = Arc { dir: Dir::Cw, ..a };
        assert!(!b.sweeps(V(1, 1)));
        assert!(b.sweeps(V(-1, 0)) && b.sweeps(V(0, -1)));
        assert!((a.sweep() - std::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert!((b.sweep() - 3.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert!(!a.is_full());
    }

    #[test]
    fn full_circles() {
        let c = Arc {
            start: p(5, 0),
            end: p(5, 0),
            centre: p(0, 0),
            dir: Dir::Cw,
        };
        assert!(c.is_full());
        assert!(c.sweeps(V(-1, -7)));
        assert_eq!(c.sweep(), 2.0 * std::f64::consts::PI);
        assert_eq!(c.radius2(), 25);
    }

    #[test]
    fn three_point_arcs() {
        let a = Arc::through(p(0, 0), p(1000, 1000), p(2000, 0)).unwrap();
        assert_eq!(a.centre, p(1000, 0));
        assert_eq!(a.dir, Dir::Cw);
        let b = Arc::through(p(2000, 0), p(1000, 1000), p(0, 0)).unwrap();
        assert_eq!(b.dir, Dir::Ccw);
        assert!(Arc::through(p(0, 0), p(1, 1), p(2, 2)).is_none());
        assert!(Arc::through(p(0, 0), p(1, 1), p(0, 0)).is_none());
        // A centre that isn't on the grid rounds to the nearest nanometre.
        let c = Arc::through(p(0, 0), p(1, 2), p(3, 0)).unwrap();
        assert_eq!(c.centre, p(2, 0));
    }

    #[test]
    fn end_error_measures_rounding() {
        let a = Arc {
            start: p(1000, 0),
            end: p(0, 1001),
            centre: p(0, 0),
            dir: Dir::Ccw,
        };
        assert!((a.end_error() - 1.0).abs() < 1e-9);
    }
}
