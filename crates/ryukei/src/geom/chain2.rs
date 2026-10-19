//! Trimmed plane curves as chains of rational Bézier pieces.
//!
//! With positive weights, each piece lies in the convex hull of its
//! Euclidean control points, which is what the certified tests in
//! [`super::intersect2`] work from. Pieces run in the direction of travel,
//! so a chain starts at `curve(t0)` and ends at `curve(t1)`, whichever
//! way round they are.

use super::{BezierCurve, Curve2, Frame2, Hom, NurbsCurve2, Vector2};
use crate::math;
use core::f64::consts::FRAC_PI_2;

/// One polynomial piece of a trimmed curve.
pub(crate) type Piece2 = BezierCurve<Vector2>;

/// A trimmed curve as Bézier pieces, with the box of their control hulls.
#[derive(Clone, Debug)]
pub(crate) struct Chain2 {
    pub(crate) pieces: Vec<Piece2>,
    /// The range is a whole period, so the end is the start.
    pub(crate) closed: bool,
    /// A NURBS, which can cross itself.
    pub(crate) free_form: bool,
    pub(crate) lo: Vector2,
    pub(crate) hi: Vector2,
    /// The pieces are circular or elliptic arcs, measured by angle.
    conic: bool,
}

/// How close, in units of the parameter's size, a NURBS range's end
/// must be to a knot to be taken as the knot: a few ulps, so a caller's
/// rounded knot value doesn't leave a piece a few ulps long.
const KNOT_SNAP: f64 = 16.0 * f64::EPSILON;

impl Chain2 {
    /// `curve` from `t0` to `t1`; `t1 < t0` runs it backwards. The range
    /// must not be empty, and must lie in a clamped curve's domain.
    pub(crate) fn new(curve: &Curve2, t0: f64, t1: f64) -> Self {
        let pieces = match curve {
            Curve2::Line2(_) => vec![line(curve, t0, t1)],
            Curve2::Circle2(c) => arc(c.frame(), c.radius(), c.radius(), t0, t1),
            Curve2::Ellipse2(e) => arc(e.frame(), e.major(), e.minor(), t0, t1),
            Curve2::Nurbs2(n) => nurbs(n, t0, t1),
        };
        let (lo, hi) = pieces
            .iter()
            .map(Piece2::hull)
            .reduce(|(a, b), (c, d)| (least(a, c), greatest(b, d)))
            .expect("a trimmed curve has pieces");
        Self {
            pieces,
            closed: curve.period() == Some((t1 - t0).abs()),
            free_form: matches!(curve, Curve2::Nurbs2(_)),
            lo,
            hi,
            conic: matches!(curve, Curve2::Circle2(_) | Curve2::Ellipse2(_)),
        }
    }

    /// The curve's parameter at local parameter `s` of piece `i`: affine
    /// for a line or a NURBS, and for an arc the half-angle substitution
    /// `tan((t - m) / 2) = tan(h / 2) (2 s - 1)`, as in
    /// [`super::chain3`].
    pub(crate) fn param(&self, i: usize, s: f64) -> f64 {
        let [a, b] = self.pieces[i].range;
        if s <= 0.0 {
            return a;
        }
        if s >= 1.0 {
            return b;
        }
        if self.conic {
            let (m, h) = ((a + b) / 2.0, (b - a) / 2.0);
            m + 2.0 * math::atan2(math::tan(h / 2.0) * (2.0 * s - 1.0), 1.0)
        } else {
            a + (b - a) * s
        }
    }

    /// The first point.
    pub(crate) fn start(&self) -> Vector2 {
        self.pieces[0].pts[0].p
    }

    /// The last point.
    pub(crate) fn end(&self) -> Vector2 {
        let last = &self.pieces[self.pieces.len() - 1];
        last.pts[last.pts.len() - 1].p
    }
}

/// The componentwise least of two vectors.
fn least(a: Vector2, b: Vector2) -> Vector2 {
    Vector2::new(a.x.min(b.x), a.y.min(b.y))
}

/// The componentwise greatest of two vectors.
fn greatest(a: Vector2, b: Vector2) -> Vector2 {
    Vector2::new(a.x.max(b.x), a.y.max(b.y))
}

fn hom(p: Vector2, w: f64) -> Hom<Vector2> {
    Hom { p, w }
}

/// A line segment: one degree 1 piece.
fn line(curve: &Curve2, t0: f64, t1: f64) -> Piece2 {
    Piece2 {
        range: [t0, t1],
        pts: vec![
            hom(curve.point(t0).to_vector(), 1.0),
            hom(curve.point(t1).to_vector(), 1.0),
        ],
    }
}

/// An arc of `origin + a cos t x + b sin t y` from `t0` to `t1`, in equal
/// rational quadratic pieces of at most a quarter turn: the exact form of
/// an arc of angle `2h` has its middle control point at the middle angle,
/// `1 / cos h` times as far out, with weight `cos h` (an ellipse is the
/// same affine image of the circle).
fn arc(f: &Frame2, a: f64, b: f64, t0: f64, t1: f64) -> Vec<Piece2> {
    let at = |t: f64, scale: f64| {
        let (s, c) = math::sin_cos(t);
        f.origin().to_vector() + (f.x() * (a * c) + f.y() * (b * s)) / scale
    };
    let n = ((t1 - t0).abs() / FRAC_PI_2).ceil().max(1.0);
    let step = (t1 - t0) / n;
    let w = math::cos(step / 2.0);
    let count = n as usize;
    (0..count)
        .map(|i| {
            let start = t0 + step * i as f64;
            let end = if i + 1 == count { t1 } else { start + step };
            Piece2 {
                range: [start, end],
                pts: vec![
                    hom(at(start, 1.0), 1.0),
                    hom(at((start + end) / 2.0, w), w),
                    hom(at(end, 1.0), 1.0),
                ],
            }
        })
        .collect()
}

/// The pieces of a NURBS over `[t0, t1]`: its own Bézier pieces (repeated
/// by the period for a periodic curve) cut to the range by de Casteljau
/// subdivision.
fn nurbs(n: &NurbsCurve2, t0: f64, t1: f64) -> Vec<Piece2> {
    let (lo, hi) = (t0.min(t1), t0.max(t1));
    let own = n.pieces();
    let shifts: Vec<f64> = match n.period() {
        Some(p) => {
            let d0 = n.domain()[0];
            let first = ((lo - d0) / p).floor() as i64 - 1;
            let last = ((hi - d0) / p).ceil() as i64 + 1;
            (first..=last).map(|m| m as f64 * p).collect()
        }
        None => vec![0.0],
    };
    let mut out = Vec::new();
    for shift in shifts {
        for piece in own {
            let [a, b] = piece.range.map(|t| t + shift);
            if let Some(cut) = cut(piece, [a, b], [lo, hi]) {
                out.push(cut);
            }
        }
    }
    if t1 < t0 {
        out.reverse();
        for p in &mut out {
            p.pts.reverse();
            p.range.reverse();
        }
    }
    out
}

/// True when `t` is within [`KNOT_SNAP`] of the knot `k`.
fn at_knot(t: f64, k: f64) -> bool {
    (t - k).abs() <= KNOT_SNAP * k.abs().max(1.0)
}

/// The part of `piece`, covering `[a, b]`, that lies in `[lo, hi]`, or
/// `None` when they overlap by no more than a snapped end.
fn cut(piece: &Piece2, [a, b]: [f64; 2], [lo, hi]: [f64; 2]) -> Option<Piece2> {
    let (c, d) = (lo.max(a), hi.min(b));
    if d <= c || at_knot(d, a) || at_knot(c, b) {
        return None;
    }
    let u0 = if at_knot(c, a) {
        0.0
    } else {
        (c - a) / (b - a)
    };
    let u1 = if at_knot(d, b) {
        1.0
    } else {
        (d - a) / (b - a)
    };
    let mut out = piece.clone();
    if u1 < 1.0 {
        let [left, _] = out.split(u1);
        out = left;
    }
    if u0 > 0.0 {
        let [_, right] = out.split(u0 / u1);
        out = right;
    }
    out.range = [c, d];
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle2, Ellipse2, KnotVector, Line2, Point2};
    use core::f64::consts::{PI, TAU};
    use std::sync::Arc;

    /// A curve and a range on it.
    struct Seg {
        curve: Curve2,
        t0: f64,
        t1: f64,
    }

    fn seg(curve: Curve2, t0: f64, t1: f64) -> Seg {
        Seg { curve, t0, t1 }
    }

    fn chain(s: &Seg) -> Chain2 {
        Chain2::new(&s.curve, s.t0, s.t1)
    }

    /// The point of a piece at local parameter `u`.
    fn point(p: &Piece2, u: f64) -> Vector2 {
        if u <= 0.0 {
            return p.pts[0].p;
        }
        if u >= 1.0 {
            return p.pts[p.pts.len() - 1].p;
        }
        p.split(u)[1].pts[0].p
    }

    /// Every piece follows the curve, and the chain runs from its start
    /// to its end.
    fn follows(s: &Seg, c: &Chain2, reparam: impl Fn(f64) -> f64) {
        assert!((c.start() - s.curve.point(s.t0).to_vector()).length() <= 1e-14);
        assert!((c.end() - s.curve.point(s.t1).to_vector()).length() <= 1e-14);
        for p in &c.pieces {
            let [a, b] = p.range;
            for i in 0..=8 {
                let u = f64::from(i) / 8.0;
                let t = reparam(a + (b - a) * u);
                let q = s.curve.point(t).to_vector();
                assert!((point(p, u) - q).length() <= 1e-13, "{t}");
            }
            let (lo, hi) = p.hull();
            assert!(lo.x >= c.lo.x && lo.y >= c.lo.y && hi.x <= c.hi.x && hi.y <= c.hi.y);
        }
    }

    #[test]
    fn arcs_in_quarter_turns_or_less() {
        for ccw in [true, false] {
            let f = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 1.0), ccw).unwrap();
            let e = Curve2::Ellipse2(Ellipse2::new(f, 3.0, 1.0).unwrap());
            let s = seg(e, 0.3, -2.0);
            let c = chain(&s);
            assert_eq!(c.pieces.len(), 2);
            assert!(!c.closed && !c.free_form);
            // The middle of a piece in local parameter is not its middle
            // angle, so compare by projection instead.
            for p in &c.pieces {
                for i in 0..=8 {
                    let q = point(p, f64::from(i) / 8.0);
                    assert!(s.curve.distance(Point2::ORIGIN + q) <= 1e-13);
                }
            }
            // The ends are computed exactly as the curve computes them.
            assert_eq!(c.start(), s.curve.point(s.t0).to_vector());
            for (i, p) in c.pieces.iter().enumerate() {
                for k in 0..=4 {
                    let u = f64::from(k) / 4.0;
                    let q = s.curve.point(c.param(i, u)).to_vector();
                    assert!((point(p, u) - q).length() <= 1e-13);
                }
            }
            assert_eq!(c.end(), s.curve.point(s.t1).to_vector());
            let circle = Curve2::Circle2(Circle2::new(f, 2.0).unwrap());
            let whole = chain(&seg(circle, 0.0, TAU));
            assert_eq!(whole.pieces.len(), 4);
            assert!(whole.closed);
        }
    }

    #[test]
    fn a_line_is_one_piece() {
        let (a, b) = (Point2::new(0.0, 1.0), Point2::new(3.0, -2.0));
        let l = seg(
            Curve2::Line2(Line2::through(a, b).unwrap()),
            0.0,
            a.distance(b),
        );
        let c = chain(&l);
        assert_eq!(c.pieces.len(), 1);
        follows(&l, &c, |t| t);
        assert_eq!(
            (c.lo, c.hi),
            (Vector2::new(0.0, -2.0), Vector2::new(3.0, 1.0))
        );
    }

    fn wavy() -> NurbsCurve2 {
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0, 2.5], &[4, 1, 2, 4]).unwrap();
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 3.0),
            Point2::new(2.0, -1.0),
            Point2::new(4.0, 2.0),
            Point2::new(5.0, 0.0),
            Point2::new(6.0, 1.0),
            Point2::new(8.0, 0.0),
        ];
        let w = vec![1.0, 0.5, 2.0, 0.8, 1.5, 0.3, 1.0];
        NurbsCurve2::new(k, pts, Some(w)).unwrap()
    }

    #[test]
    fn nurbs_cut_to_the_range_either_way() {
        let n = Curve2::Nurbs2(Arc::new(wavy()));
        for (t0, t1, count) in [(0.2, 1.7, 3), (1.7, 0.2, 3), (0.4, 1.0, 1), (0.0, 2.5, 3)] {
            let s = seg(n.clone(), t0, t1);
            let c = chain(&s);
            assert_eq!(c.pieces.len(), count, "{t0}..{t1}");
            assert!(c.free_form && !c.closed);
            follows(&s, &c, |t| t);
        }
        // A range a few ulps past a knot doesn't leave a sliver piece.
        let s = seg(n, 0.0, 1.0 + 2.0 * f64::EPSILON);
        assert_eq!(chain(&s).pieces.len(), 2);
    }

    #[test]
    fn periodic_nurbs_across_the_seam() {
        let f = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 0.0), true).unwrap();
        let (n, _) = Curve2::Circle2(Circle2::new(f, 1.5).unwrap())
            .to_nurbs([0.0, 1.0])
            .unwrap();
        let n = Curve2::Nurbs2(Arc::new(n));
        let s = seg(n.clone(), 5.0, 5.0 + PI);
        let c = chain(&s);
        assert_eq!(c.pieces.len(), 3);
        follows(&s, &c, |t| t);
        let s = seg(n, 1.0 - TAU, 1.0);
        let c = chain(&s);
        assert_eq!(c.pieces.len(), 5);
        assert!(c.closed);
        follows(&s, &c, |t| t);
    }
}
