//! Signed areas of profile loops by Green's theorem, `½ ∮ (x dy − y dx)`,
//! measured from a centre near the loop to keep rounding small.

use super::Segment;
use crate::geom::{Box2, Curve2, Point2, Vector2};

/// Positive nodes of 16-point Gauss–Legendre quadrature on `[-1, 1]`.
const GAUSS_NODES: [f64; 8] = [
    0.095_012_509_837_637_44,
    0.281_603_550_779_258_9,
    0.458_016_777_657_227_4,
    0.617_876_244_402_643_8,
    0.755_404_408_355_003,
    0.865_631_202_387_831_8,
    0.944_575_023_073_232_6,
    0.989_400_934_991_649_9,
];

/// Their weights.
const GAUSS_WEIGHTS: [f64; 8] = [
    0.189_450_610_455_068_5,
    0.182_603_415_044_923_6,
    0.169_156_519_395_002_5,
    0.149_595_988_816_576_7,
    0.124_628_971_255_533_9,
    0.095_158_511_682_492_8,
    0.062_253_523_938_647_9,
    0.027_152_459_411_754_1,
];

/// The explicit rule for trusting an area's sign: `|area|` must exceed this
/// times the square of the loop's bounding-box diagonal. Rounding and
/// quadrature errors sit many orders of magnitude below it.
const AREA_RULE: f64 = 1e-12;

/// The box holding every segment of a loop.
pub(super) fn loop_bounds(segments: &[Segment]) -> Box2 {
    segments
        .iter()
        .map(Segment::bounds)
        .reduce(|a, b| a.union(&b))
        .expect("a loop has segments")
}

/// The loop's signed area, positive when it runs counterclockwise, or
/// `None` when it is too small to sign by [`AREA_RULE`]. The gaps between
/// a segment's end and the next one's start (within resolution) are closed
/// by straight lines.
pub(super) fn signed_area(segments: &[Segment]) -> Option<f64> {
    let b = loop_bounds(segments);
    let centre = b.min().lerp(b.max(), 0.5);
    let n = segments.len();
    let mut twice = 0.0;
    for (i, s) in segments.iter().enumerate() {
        twice += green(s, centre);
        let next = &segments[(i + 1) % n];
        twice += (s.end() - centre).cross(next.start() - centre);
    }
    let area = twice / 2.0;
    let diagonal = (b.max() - b.min()).length_squared();
    (area.abs() > AREA_RULE * diagonal).then_some(area)
}

/// `∫ (x dy − y dx)` along the segment, with coordinates taken from
/// `centre`.
fn green(s: &Segment, centre: Point2) -> f64 {
    match &s.curve {
        Curve2::Line2(_) => (s.start() - centre).cross(s.end() - centre),
        Curve2::Circle2(c) => {
            let f = c.frame();
            arc(s, f.origin() - centre, c.radius() * c.radius(), f.is_ccw())
        }
        Curve2::Ellipse2(e) => {
            let f = e.frame();
            arc(s, f.origin() - centre, e.major() * e.minor(), f.is_ccw())
        }
        other => quadrature(other, s.t0, s.t1, centre),
    }
}

/// The closed form for `c + a cos t X + b sin t Y`: the centre's share
/// `c × (C(t1) − C(t0))` plus `a b (t1 − t0)`, negated when the frame turns
/// clockwise. `ab` is `a b`.
fn arc(s: &Segment, c: Vector2, ab: f64, ccw: bool) -> f64 {
    let turn = ab * (s.t1 - s.t0);
    c.cross(s.end() - s.start()) + if ccw { turn } else { -turn }
}

/// Gauss–Legendre quadrature of `(C − centre) × C'` over each knot span
/// the range covers, signed by the direction of travel.
fn quadrature(curve: &Curve2, t0: f64, t1: f64, centre: Point2) -> f64 {
    let (lo, hi) = (t0.min(t1), t0.max(t1));
    let breaks = breakpoints(curve, lo, hi);
    let mut sum = 0.0;
    for w in breaks.windows(2) {
        let (mid, half) = ((w[0] + w[1]) / 2.0, (w[1] - w[0]) / 2.0);
        for (x, weight) in GAUSS_NODES.iter().zip(GAUSS_WEIGHTS) {
            for t in [mid - half * x, mid + half * x] {
                let e = curve.eval(t);
                sum += weight * half * (e.point - centre).cross(e.d1);
            }
        }
    }
    if t1 < t0 { -sum } else { sum }
}

/// `lo`, every knot strictly between `lo` and `hi` (repeated by the period
/// for a periodic curve), and `hi`, in increasing order.
pub(super) fn breakpoints(curve: &Curve2, lo: f64, hi: f64) -> Vec<f64> {
    let mut out = vec![lo];
    if let Curve2::Nurbs2(n) = curve {
        let knots = n.knots().knots();
        let shifts: Vec<f64> = match n.period() {
            Some(p) => {
                let first = ((lo - knots[0]) / p).floor() as i64 - 1;
                let last = ((hi - knots[0]) / p).ceil() as i64 + 1;
                (first..=last).map(|m| m as f64 * p).collect()
            }
            None => vec![0.0],
        };
        for shift in shifts {
            out.extend(
                knots
                    .iter()
                    .map(|k| k + shift)
                    .filter(|&t| lo < t && t < hi),
            );
        }
        out.sort_by(f64::total_cmp);
        out.dedup();
    }
    out.push(hi);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle2, Ellipse2, Frame2, KnotVector, Line2, NurbsCurve2};
    use crate::sweep::SegmentKey;
    use core::f64::consts::{PI, TAU};
    use std::sync::Arc;

    fn seg(curve: Curve2, t0: f64, t1: f64) -> Segment {
        Segment {
            key: SegmentKey(0),
            curve,
            t0,
            t1,
        }
    }

    fn line(a: (f64, f64), b: (f64, f64)) -> Segment {
        let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
        seg(
            Curve2::Line2(Line2::through(a, b).unwrap()),
            0.0,
            a.distance(b),
        )
    }

    #[test]
    fn gauss_legendre_is_exact_to_degree_31() {
        for k in 0..=31 {
            let mut sum = 0.0;
            for (x, w) in GAUSS_NODES.iter().zip(GAUSS_WEIGHTS) {
                sum += w * (power(*x, k) + power(-x, k));
            }
            let exact = if k % 2 == 0 {
                2.0 / f64::from(k + 1)
            } else {
                0.0
            };
            assert!((sum - exact).abs() <= 1e-15, "x^{k}: {sum} vs {exact}");
        }
    }

    fn power(x: f64, k: u32) -> f64 {
        (0..k).fold(1.0, |acc, _| acc * x)
    }

    #[test]
    fn a_square_both_ways() {
        let sq = [
            line((0.0, 0.0), (2.0, 0.0)),
            line((2.0, 0.0), (2.0, 3.0)),
            line((2.0, 3.0), (0.0, 3.0)),
            line((0.0, 3.0), (0.0, 0.0)),
        ];
        assert_eq!(signed_area(&sq), Some(6.0));
        let back: Vec<_> = sq.iter().rev().map(Segment::reversed).collect();
        assert_eq!(signed_area(&back), Some(-6.0));
    }

    #[test]
    fn circles_and_ellipses_in_either_frame() {
        for ccw in [true, false] {
            let f = Frame2::new(Point2::new(3.0, -1.0), Vector2::new(1.0, 1.0), ccw).unwrap();
            let sign = if ccw { 1.0 } else { -1.0 };
            let c = Curve2::Circle2(Circle2::new(f, 2.0).unwrap());
            let a = signed_area(&[seg(c.clone(), 0.5, 0.5 + TAU)]).unwrap();
            assert!((a - sign * 4.0 * PI).abs() <= 1e-13, "{a}");
            let e = Curve2::Ellipse2(Ellipse2::new(f, 3.0, 1.0).unwrap());
            let a = signed_area(&[seg(e, TAU, 0.0)]).unwrap();
            assert!((a + sign * 3.0 * PI).abs() <= 1e-13, "{a}");
            // A half disk: the arc and its diameter.
            let half = [
                seg(c.clone(), 0.0, PI),
                seg(
                    Curve2::Line2(Line2::through(c.point(PI), c.point(0.0)).unwrap()),
                    0.0,
                    4.0,
                ),
            ];
            let a = signed_area(&half).unwrap();
            assert!((a - sign * 2.0 * PI).abs() <= 1e-13, "{a}");
        }
    }

    #[test]
    fn a_nurbs_circle_matches_the_analytic_one() {
        let f = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 0.0), true).unwrap();
        let (n, _) = Curve2::Circle2(Circle2::new(f, 1.5).unwrap())
            .to_nurbs([0.0, 1.0])
            .unwrap();
        let c = Curve2::Nurbs2(Arc::new(n));
        let a = signed_area(&[seg(c.clone(), 0.3, 0.3 + TAU)]).unwrap();
        // Rational pieces are not polynomials: the quadrature is close,
        // not exact.
        assert!((a - 2.25 * PI).abs() <= 1e-9, "{a}");
        let a = signed_area(&[seg(c, 2.0, 2.0 - TAU)]).unwrap();
        assert!((a + 2.25 * PI).abs() <= 1e-9, "{a}");
    }

    #[test]
    fn periodic_breakpoints_cover_every_period() {
        let k = KnotVector::periodic(1, &[0.0, 1.0, 2.0, 3.0], &[1, 1, 1, 1]).unwrap();
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(0.0, 1.0),
        ];
        let c = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap()));
        assert_eq!(
            breakpoints(&c, -1.5, 4.5),
            [-1.5, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.5]
        );
        let l = Curve2::Line2(Line2::through(Point2::ORIGIN, Point2::new(1.0, 0.0)).unwrap());
        assert_eq!(breakpoints(&l, 0.0, 1.0), [0.0, 1.0]);
    }

    #[test]
    fn slivers_are_not_signed() {
        let thin = [
            line((0.0, 0.0), (1.0, 0.0)),
            line((1.0, 0.0), (0.5, 1e-13)),
            line((0.5, 1e-13), (0.0, 0.0)),
        ];
        assert_eq!(signed_area(&thin), None);
        let ok = [
            line((0.0, 0.0), (1.0, 0.0)),
            line((1.0, 0.0), (0.5, 1e-11)),
            line((0.5, 1e-11), (0.0, 0.0)),
        ];
        assert!(signed_area(&ok).unwrap() > 0.0);
    }
}
