//! NURBS curves in a surface's parameter plane.

use super::KnotVector;
use super::bezier::BezierCurve;
use super::eval::Spline;
use super::project::nearest;
use crate::Error;
use crate::geom::{Box2, CurveEval2, Point2, Projection, Vector2};
use std::sync::OnceLock;

/// A rational B-spline curve in the parameter plane, the counterpart of
/// [`super::NurbsCurve`]. Control points only need to be finite.
#[derive(Clone, Debug)]
pub struct NurbsCurve2 {
    spline: Spline<Vector2>,
    pieces: OnceLock<Box<[BezierCurve<Vector2>]>>,
}

impl PartialEq for NurbsCurve2 {
    fn eq(&self, other: &Self) -> bool {
        self.spline == other.spline
    }
}

impl NurbsCurve2 {
    /// Fails with `NonFinite`, or `InvalidNurbs` as for
    /// [`super::NurbsCurve::new`].
    pub fn new(
        knots: KnotVector,
        points: Vec<Point2>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, Error> {
        if points.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
            return Err(Error::NonFinite);
        }
        let points = points.iter().map(|p| p.to_vector()).collect();
        Ok(Self::from_spline(Spline::new(knots, points, weights)?))
    }

    fn from_spline(spline: Spline<Vector2>) -> Self {
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

    pub fn control_points(&self) -> impl ExactSizeIterator<Item = Point2> + '_ {
        self.spline.points.iter().map(|&v| Point2::ORIGIN + v)
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

    pub fn point(&self, t: f64) -> Point2 {
        self.eval(t).point
    }

    pub fn eval(&self, t: f64) -> CurveEval2 {
        let [c, d1, d2] = self.spline.eval(t);
        CurveEval2 {
            point: Point2::ORIGIN + c,
            d1,
            d2,
        }
    }

    fn unit_eval(&self, t: f64) -> CurveEval2 {
        let [a, b] = self.domain();
        self.eval(t).scaled(b - a)
    }

    pub fn tangent(&self, t: f64) -> Option<Vector2> {
        self.unit_eval(t).tangent()
    }

    pub fn curvature(&self, t: f64) -> Option<f64> {
        self.unit_eval(t).curvature()
    }

    pub fn to_clamped(&self) -> Self {
        Self::from_spline(self.spline.clamped())
    }

    /// See [`super::NurbsCurve::refine`].
    pub fn refine(&self, knots: &[f64]) -> Result<Self, Error> {
        Ok(Self::from_spline(self.spline.refine(knots)?))
    }

    /// See [`super::NurbsCurve::split`].
    pub fn split(&self, t: f64) -> Result<(Self, Self), Error> {
        let [a, b] = self.spline.split(t)?;
        Ok((Self::from_spline(a), Self::from_spline(b)))
    }

    /// The Bézier pieces over the domain, in parameter order, built on
    /// first use.
    pub(crate) fn pieces(&self) -> &[BezierCurve<Vector2>] {
        self.pieces.get_or_init(|| self.spline.pieces().into())
    }

    /// See [`super::NurbsCurve::bounds`].
    pub fn bounds(&self, [t0, t1]: [f64; 2]) -> Box2 {
        let ranges = match self.period() {
            Some(period) if t1 - t0 < period => {
                let a = self.spline.knots.wrap(t0);
                let b = a + (t1 - t0);
                vec![[a, b], [a - period, b - period]]
            }
            Some(_) => vec![self.domain()],
            None => vec![[t0, t1]],
        };
        let boxed = |p: &BezierCurve<Vector2>| {
            let (lo, hi) = p.hull();
            Box2::new(Point2::ORIGIN + lo, Point2::ORIGIN + hi)
        };
        let pieces = self.pieces();
        let hit = pieces
            .iter()
            .filter(|p| {
                ranges
                    .iter()
                    .any(|&[a, b]| p.range[0] <= b && p.range[1] >= a)
            })
            .map(boxed)
            .reduce(|x, y| x.union(&y));
        hit.unwrap_or_else(|| {
            pieces
                .iter()
                .map(boxed)
                .fold(boxed(&pieces[0]), |x, y| x.union(&y))
        })
    }

    /// See [`super::NurbsCurve::project`].
    pub fn project(&self, p: Point2) -> Projection<f64, Point2> {
        let feet = self.spline.minima(self.pieces(), p.to_vector(), true);
        let (param, v, distance) = nearest(&feet);
        Projection {
            param,
            point: Point2::ORIGIN + v,
            distance,
        }
    }

    pub fn distance(&self, p: Point2) -> f64 {
        self.project(p).distance
    }

    /// See [`super::NurbsCurve::local_minima`].
    pub fn local_minima(&self, p: Point2) -> Vec<Projection<f64, Point2>> {
        self.spline
            .minima(self.pieces(), p.to_vector(), false)
            .into_iter()
            .map(|(param, v, distance)| Projection {
                param,
                point: Point2::ORIGIN + v,
                distance,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle2, Curve2, Ellipse2, Frame2, Line2, Reparam, Vector2};
    use core::f64::consts::{FRAC_1_SQRT_2, TAU};
    use std::sync::Arc;

    fn quarter() -> NurbsCurve2 {
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let p = vec![
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];
        NurbsCurve2::new(k, p, Some(vec![1.0, FRAC_1_SQRT_2, 1.0])).unwrap()
    }

    #[test]
    fn a_quarter_circle_in_the_plane() {
        let c = quarter();
        assert_eq!(c.degree(), 2);
        assert_eq!(c.domain(), [0.0, 1.0]);
        let mid = c.point(0.5);
        assert!((mid.to_vector().length() - 1.0).abs() <= 2e-16);
        let e = c.eval(0.0);
        assert!((e.d1 - Vector2::new(0.0, 2.0 * FRAC_1_SQRT_2)).length() <= 1e-15);
        assert!((c.curvature(0.3).unwrap() - 1.0).abs() <= 1e-14);
        let foot = c.project(Point2::new(2.0, 2.0));
        assert!((foot.param - 0.5).abs() <= 1e-12);
        assert!((foot.distance - (8.0f64.sqrt() - 1.0)).abs() <= 1e-15);
        assert_eq!(c.local_minima(Point2::new(3.0, -1.0))[0].param, 0.0);
        let b = c.bounds([0.0, 1.0]);
        assert_eq!((b.min(), b.max()), (Point2::ORIGIN, Point2::new(1.0, 1.0)));
        let (a, rest) = c.split(0.25).unwrap();
        assert!(a.point(0.25).distance(rest.point(0.25)) <= 1e-15);
        let r = c.refine(&[0.5]).unwrap();
        assert!(r.point(0.7).distance(c.point(0.7)) <= 1e-15);
        assert_eq!(c.to_clamped(), c);
    }

    #[test]
    fn rejections() {
        let k = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let bad = vec![Point2::ORIGIN, Point2::new(f64::NAN, 0.0)];
        assert_eq!(
            NurbsCurve2::new(k.clone(), bad, None),
            Err(crate::Error::NonFinite)
        );
        // Parameter space has no size box.
        let far = vec![Point2::ORIGIN, Point2::new(1e6, 0.0)];
        assert!(NurbsCurve2::new(k, far, None).is_ok());
    }

    /// The analytic pcurve and its conversion agree through the
    /// reparameterization.
    fn agree(c: &Curve2) {
        let (n, r) = c.to_nurbs([-1.0, 2.0]).unwrap();
        let n = Curve2::Nurbs2(Arc::new(n));
        for i in 0..=100 {
            let t = -1.0 + 3.0 * f64::from(i) / 100.0;
            let d = n.point(r.to_nurbs(t)).distance(c.point(t));
            assert!(d <= 1e-14, "{c:?} at {t}: {d}");
        }
    }

    #[test]
    fn every_pcurve_converts_exactly() {
        let o = Point2::new(1.0, -2.0);
        agree(&Curve2::Line2(
            Line2::new(o, Vector2::new(1.0, 2.0)).unwrap(),
        ));
        for ccw in [true, false] {
            let f = Frame2::new(o, Vector2::new(1.0, 1.0), ccw).unwrap();
            agree(&Curve2::Circle2(Circle2::new(f, 2.0).unwrap()));
            agree(&Curve2::Ellipse2(Ellipse2::new(f, 3.0, 1.5).unwrap()));
        }
        let n = Curve2::Nurbs2(Arc::new(quarter()));
        let (same, r) = n.to_nurbs([0.0, 1.0]).unwrap();
        assert_eq!((Curve2::Nurbs2(Arc::new(same)), r), (n, Reparam::Identity));
    }

    #[test]
    fn the_enum_reaches_the_nurbs() {
        let n = Curve2::Nurbs2(Arc::new(quarter()));
        assert_eq!(n.domain(), [0.0, 1.0]);
        assert_eq!(n.period(), None);
        assert!((n.curvature(0.4).unwrap() - 1.0).abs() <= 1e-14);
        let foot = n.project(Point2::new(2.0, 2.0));
        assert!((foot.param - 0.5).abs() <= 1e-12);
        assert_eq!(n.bounds([0.0, 1.0]).max(), Point2::new(1.0, 1.0));
        let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
        let (c, _) = Curve2::Circle2(Circle2::new(f, 1.0).unwrap())
            .to_nurbs([0.0, 1.0])
            .unwrap();
        assert_eq!(c.period(), Some(TAU));
    }
}
