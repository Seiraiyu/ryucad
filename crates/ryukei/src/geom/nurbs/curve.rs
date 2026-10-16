//! NURBS curves in model space.

use super::KnotVector;
use super::bezier::BezierCurve;
use super::eval::Spline;
use crate::Error;
use crate::geom::{Box3, CurveEval, Point3, UnitVector3, Vector3};
use std::sync::OnceLock;

/// A rational B-spline curve in model space. Immutable once built.
///
/// Its Bézier pieces are built the first time something needs them
/// (bounds, projection) and kept; building them is deterministic, so the
/// cache never changes a result, and it takes no part in equality.
#[derive(Clone, Debug)]
pub struct NurbsCurve {
    spline: Spline<Vector3>,
    pieces: OnceLock<Box<[BezierCurve<Vector3>]>>,
}

impl PartialEq for NurbsCurve {
    fn eq(&self, other: &Self) -> bool {
        self.spline == other.spline
    }
}

impl NurbsCurve {
    /// A curve from its knots, control points and weights (`None` for all
    /// 1). Fails with `NonFinite`, `OutsideSizeBox` for a control point
    /// outside the size box, or `InvalidNurbs`: `ControlCount` when the
    /// point or weight count doesn't match the knots, `Weight` for a weight
    /// that is not positive.
    pub fn new(
        knots: KnotVector,
        points: Vec<Point3>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, Error> {
        let points = points
            .iter()
            .map(|p| Point3::try_new(p.x, p.y, p.z).map(Point3::to_vector))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_spline(Spline::new(knots, points, weights)?))
    }

    fn from_spline(spline: Spline<Vector3>) -> Self {
        Self {
            spline,
            pieces: OnceLock::new(),
        }
    }

    pub fn knots(&self) -> &KnotVector {
        &self.spline.knots
    }

    pub fn degree(&self) -> usize {
        self.spline.knots.degree()
    }

    pub fn control_points(&self) -> impl ExactSizeIterator<Item = Point3> + '_ {
        self.spline.points.iter().map(|&v| Point3::ORIGIN + v)
    }

    pub fn weights(&self) -> &[f64] {
        &self.spline.weights
    }

    pub fn domain(&self) -> [f64; 2] {
        self.spline.knots.domain()
    }

    pub fn period(&self) -> Option<f64> {
        self.spline.knots.period()
    }

    pub fn point(&self, t: f64) -> Point3 {
        self.eval(t).point
    }

    /// The point and its first two derivatives. Periodic curves wrap `t`;
    /// clamped ones extrapolate their end spans outside the domain.
    pub fn eval(&self, t: f64) -> CurveEval {
        let [c, d1, d2] = self.spline.eval(t);
        CurveEval {
            point: Point3::ORIGIN + c,
            d1,
            d2,
        }
    }

    /// The derivatives against the domain normalized to length 1, so
    /// vanishing is judged in metres whatever the knots' units.
    fn unit_eval(&self, t: f64) -> CurveEval {
        let [a, b] = self.domain();
        self.eval(t).scaled(b - a)
    }

    /// The same curve over one period with clamped knots; a clamped curve
    /// is returned unchanged.
    pub fn to_clamped(&self) -> Self {
        Self::from_spline(self.spline.clamped())
    }

    /// The same curve with each of `knots` inserted once (The NURBS Book,
    /// A5.1), in clamped form. Fails with `NonFinite`, or `InvalidNurbs`:
    /// `KnotOrder` for a knot not strictly inside the domain,
    /// `KnotMultiplicity` for a knot that would exceed the degree.
    pub fn refine(&self, knots: &[f64]) -> Result<Self, Error> {
        Ok(Self::from_spline(self.spline.refine(knots)?))
    }

    /// The curve cut at `t` into two clamped curves covering `[t0, t]` and
    /// `[t, t1]`. Fails with `NonFinite`, or `InvalidNurbs(ShortDomain)`
    /// unless both pieces are at least [`crate::tol::LINEAR_RESOLUTION`]
    /// long in parameter.
    pub fn split(&self, t: f64) -> Result<(Self, Self), Error> {
        let [a, b] = self.spline.split(t)?;
        Ok((Self::from_spline(a), Self::from_spline(b)))
    }

    /// The Bézier pieces, in parameter order, built once.
    pub(crate) fn pieces(&self) -> &[BezierCurve<Vector3>] {
        self.pieces.get_or_init(|| self.spline.pieces().into())
    }

    /// A box holding the curve over the parts of `[t0, t1]` inside the
    /// domain (a periodic range wraps): the union of the control-hull boxes
    /// of the Bézier pieces it overlaps.
    pub fn bounds(&self, [t0, t1]: [f64; 2]) -> Box3 {
        let ranges = match self.period() {
            Some(period) if t1 - t0 < period => {
                let a = self.spline.knots.wrap(t0);
                let b = a + (t1 - t0);
                vec![[a, b], [a - period, b - period]]
            }
            Some(_) => vec![self.domain()],
            None => vec![[t0, t1]],
        };
        let pieces = self.pieces();
        let hit = pieces
            .iter()
            .filter(|p| {
                ranges
                    .iter()
                    .any(|&[a, b]| p.range[0] <= b && p.range[1] >= a)
            })
            .map(BezierCurve::bounds)
            .reduce(|x, y| x.union(&y));
        hit.unwrap_or_else(|| {
            pieces
                .iter()
                .map(BezierCurve::bounds)
                .fold(pieces[0].bounds(), |x, y| x.union(&y))
        })
    }

    /// The unit tangent; see [`CurveEval::tangent`].
    pub fn tangent(&self, t: f64) -> Option<UnitVector3> {
        self.unit_eval(t).tangent()
    }

    /// The curvature; see [`CurveEval::curvature`].
    pub fn curvature(&self, t: f64) -> Option<f64> {
        self.unit_eval(t).curvature()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NurbsError;
    use core::f64::consts::FRAC_1_SQRT_2;

    fn close(a: Vector3, b: Vector3, tol: f64) -> bool {
        (a - b).length() <= tol
    }

    /// The quarter of the unit circle from +x to +y, exactly.
    fn quarter() -> NurbsCurve {
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let p = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ];
        NurbsCurve::new(k, p, Some(vec![1.0, FRAC_1_SQRT_2, 1.0])).unwrap()
    }

    #[test]
    fn a_rational_quarter_circle() {
        let c = quarter();
        assert_eq!(c.degree(), 2);
        assert_eq!(c.domain(), [0.0, 1.0]);
        assert_eq!(c.period(), None);
        assert_eq!(c.point(0.0), Point3::new(1.0, 0.0, 0.0));
        assert_eq!(c.point(1.0), Point3::new(0.0, 1.0, 0.0));
        let mid = c.point(0.5);
        assert!(close(
            mid.to_vector(),
            Vector3::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2, 0.0),
            2e-16
        ));
        for i in 0..=16 {
            let p = c.point(f64::from(i) / 16.0);
            assert!((p.to_vector().length() - 1.0).abs() <= 2e-16);
        }
        // C'(0) = 2 (w1 / w0) (P1 - P0).
        let e = c.eval(0.0);
        assert!(close(
            e.d1,
            Vector3::new(0.0, 2.0 * FRAC_1_SQRT_2, 0.0),
            1e-15
        ));
        assert_eq!(e.point, c.point(0.0));
    }

    #[test]
    fn a_cubic_bezier_matches_bernstein() {
        let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
        let p = [
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 2.0, 0.0),
            Vector3::new(3.0, 2.0, 1.0),
            Vector3::new(4.0, 0.0, 2.0),
        ];
        let pts = p.iter().map(|v| Point3::ORIGIN + *v).collect();
        let c = NurbsCurve::new(k, pts, None).unwrap();
        assert_eq!(c.weights(), [1.0; 4]);
        assert_eq!(
            c.control_points().collect::<Vec<_>>()[2],
            Point3::new(3.0, 2.0, 1.0)
        );
        let t = 0.3;
        let s = 1.0 - t;
        let point = p[0] * (s * s * s)
            + p[1] * (3.0 * s * s * t)
            + p[2] * (3.0 * s * t * t)
            + p[3] * (t * t * t);
        let d1 =
            ((p[1] - p[0]) * (s * s) + (p[2] - p[1]) * (2.0 * s * t) + (p[3] - p[2]) * (t * t))
                * 3.0;
        let d2 = ((p[2] - p[1] * 2.0 + p[0]) * s + (p[3] - p[2] * 2.0 + p[1]) * t) * 6.0;
        let e = c.eval(t);
        assert!(close(e.point.to_vector(), point, 1e-15));
        assert!(close(e.d1, d1, 1e-14));
        assert!(close(e.d2, d2, 1e-13));
    }

    #[test]
    fn rational_derivatives_match_finite_differences() {
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0, 2.5], &[4, 1, 2, 4]).unwrap();
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 3.0, -1.0),
            Point3::new(2.0, -1.0, 0.5),
            Point3::new(4.0, 2.0, 2.0),
            Point3::new(5.0, 0.0, 1.0),
            Point3::new(6.0, 1.0, -2.0),
            Point3::new(8.0, 0.0, 0.0),
        ];
        let w = vec![1.0, 0.5, 2.0, 0.8, 1.5, 0.3, 1.0];
        let c = NurbsCurve::new(k, pts, Some(w)).unwrap();
        let h = 1e-5;
        for t in [0.1, 0.39, 0.7, 1.2, 2.0, 2.45] {
            let e = c.eval(t);
            let (a, m, b) = (c.point(t - h), c.point(t), c.point(t + h));
            let fd1 = (b - a) / (2.0 * h);
            let fd2 = ((b - m) - (m - a)) / (h * h);
            assert!(close(e.d1, fd1, 1e-7 * e.d1.length().max(1.0)), "t={t}");
            assert!(close(e.d2, fd2, 1e-3 * e.d2.length().max(1.0)), "t={t}");
        }
    }

    #[test]
    fn periodic_curves_repeat() {
        // A uniform periodic quadratic over a square: a closed, smooth loop.
        let k = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
        let pts = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(0.0, -1.0, 0.0),
        ];
        let c = NurbsCurve::new(k, pts, None).unwrap();
        assert_eq!(c.period(), Some(4.0));
        assert!(c.point(0.0).distance(c.point(4.0)) <= 1e-15);
        for t in [0.0, 0.3, 1.7, 3.9] {
            let (a, b) = (c.eval(t), c.eval(t + 4.0));
            assert!(a.point.distance(b.point) <= 1e-15);
            assert!(close(a.d1, b.d1, 1e-14) && close(a.d2, b.d2, 1e-14));
            let c2 = c.eval(t - 8.0);
            assert!(a.point.distance(c2.point) <= 1e-14);
        }
        // Smooth across the seam.
        let (a, b) = (c.eval(4.0 - 1e-9), c.eval(1e-9));
        assert!(close(a.d1, b.d1, 1e-7));
    }

    #[test]
    fn tangent_at_a_stationary_point() {
        // P0 = P1: the curve starts with zero speed and leaves along P2 - P0.
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let pts = vec![Point3::ORIGIN, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)];
        let c = NurbsCurve::new(k, pts, None).unwrap();
        assert_eq!(c.tangent(0.0), Some(crate::geom::UnitVector3::X));
        assert_eq!(c.curvature(0.0), None);
        assert_eq!(c.curvature(0.5), Some(0.0));
    }

    #[test]
    fn tangents_do_not_depend_on_the_parameter_scale() {
        // One metre over a billion parameter units: |d1| is 1e-9.
        let k = KnotVector::clamped(1, &[0.0, 1e9], &[2, 2]).unwrap();
        let pts = vec![Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0)];
        let c = NurbsCurve::new(k, pts, None).unwrap();
        assert_eq!(c.tangent(5e8), Some(crate::geom::UnitVector3::X));
    }

    #[test]
    fn rejections() {
        let k = || KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let p = |x: f64| Point3::new(x, 0.0, 0.0);
        let invalid = |e| Err(Error::InvalidNurbs(e));
        assert_eq!(
            NurbsCurve::new(k(), vec![p(0.0)], None),
            invalid(NurbsError::ControlCount)
        );
        assert_eq!(
            NurbsCurve::new(k(), vec![p(0.0), p(1.0)], Some(vec![1.0])),
            invalid(NurbsError::ControlCount)
        );
        assert_eq!(
            NurbsCurve::new(k(), vec![p(0.0), p(1.0)], Some(vec![1.0, 0.0])),
            invalid(NurbsError::Weight)
        );
        assert_eq!(
            NurbsCurve::new(k(), vec![p(0.0), p(1.0)], Some(vec![1.0, f64::NAN])),
            Err(Error::NonFinite)
        );
        assert_eq!(
            NurbsCurve::new(k(), vec![p(0.0), p(f64::INFINITY)], None),
            Err(Error::NonFinite)
        );
        assert_eq!(
            NurbsCurve::new(k(), vec![p(0.0), p(600.0)], None),
            Err(Error::OutsideSizeBox)
        );
        assert!(NurbsCurve::new(k(), vec![p(0.0), p(1.0)], None).is_ok());
    }
}
