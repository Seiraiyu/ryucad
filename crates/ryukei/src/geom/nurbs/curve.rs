//! NURBS curves in model space.

use super::KnotVector;
use super::eval::Spline;
use crate::Error;
use crate::geom::{CurveEval, Point3, Vector3};

/// A rational B-spline curve in model space. Immutable once built.
#[derive(Clone, Debug, PartialEq)]
pub struct NurbsCurve {
    spline: Spline<Vector3>,
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
        Ok(Self {
            spline: Spline::new(knots, points, weights)?,
        })
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
