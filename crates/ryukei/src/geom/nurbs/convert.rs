//! Exact rational NURBS forms of the analytic curves and surfaces.

use super::{KnotVector, NurbsCurve};
use crate::Error;
use crate::geom::{Circle, Curve, Ellipse, Frame, Line};
use crate::math;
use core::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, FRAC_PI_4, PI, SQRT_2, TAU};

/// tan(π/8).
const TAN_PI_8: f64 = SQRT_2 - 1.0;

/// How a converted direction's NURBS parameter relates to the analytic
/// one. Both maps are increasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reparam {
    /// The NURBS parameter is the analytic one.
    Identity,
    /// The analytic parameter is an angle, and each quarter turn
    /// `[kπ/2, (k+1)π/2]` is one rational quadratic arc whose knots are its
    /// ends. With `ψ` the angle from the quarter's middle and `s` the
    /// fraction of the quarter's knot span, `tan(ψ/2) = tan(π/8)(2s - 1)`,
    /// so the two parameters agree at every multiple of π/4.
    Angle,
}

impl Reparam {
    /// The NURBS parameter for an analytic one.
    pub fn to_nurbs(self, t: f64) -> f64 {
        match self {
            Self::Identity => t,
            Self::Angle => {
                let q = (t / FRAC_PI_2).floor() * FRAC_PI_2;
                let psi = t - q - FRAC_PI_4;
                let s = 0.5 * (1.0 + math::tan(psi / 2.0) / TAN_PI_8);
                q + s * FRAC_PI_2
            }
        }
    }

    /// The analytic parameter for a NURBS one.
    pub fn from_nurbs(self, s: f64) -> f64 {
        match self {
            Self::Identity => s,
            Self::Angle => {
                let q = (s / FRAC_PI_2).floor() * FRAC_PI_2;
                let frac = (s - q) / FRAC_PI_2;
                let psi = 2.0 * math::atan2(TAN_PI_8 * (2.0 * frac - 1.0), 1.0);
                q + FRAC_PI_4 + psi
            }
        }
    }
}

const H: f64 = FRAC_1_SQRT_2;

/// The eight control points of a full turn as `(cos, sin, weight)`
/// coefficients, in the periodic knot vector's order: the corner before
/// angle 0 comes first, then the point at angle 0.
const TURN: [(f64, f64, f64); 8] = [
    (1.0, -1.0, H),
    (1.0, 0.0, 1.0),
    (1.0, 1.0, H),
    (0.0, 1.0, 1.0),
    (-1.0, 1.0, H),
    (-1.0, 0.0, 1.0),
    (-1.0, -1.0, H),
    (0.0, -1.0, 1.0),
];

/// The knots of a full turn: a periodic quadratic with a double knot at
/// every quarter.
fn turn_knots() -> Result<KnotVector, Error> {
    let quarters = [0.0, FRAC_PI_2, PI, PI + FRAC_PI_2, TAU];
    KnotVector::periodic(2, &quarters, &[2; 5])
}

/// A full turn in `frame`'s xy plane with semi-axes `a` along `x` and `b`
/// along `y`.
fn turn(frame: &Frame, a: f64, b: f64) -> Result<NurbsCurve, Error> {
    let (o, x, y) = (frame.origin(), frame.x().get(), frame.y().get());
    let points = TURN
        .iter()
        .map(|&(c, s, _)| o + x * (a * c) + y * (b * s))
        .collect();
    let weights = TURN.iter().map(|&(.., w)| w).collect();
    NurbsCurve::new(turn_knots()?, points, Some(weights))
}

impl Line {
    /// The segment over `range` as a degree 1 NURBS. Fails like
    /// [`KnotVector::clamped`] for a range that is not increasing or too
    /// short, or with `OutsideSizeBox`.
    pub fn to_nurbs(&self, [t0, t1]: [f64; 2]) -> Result<(NurbsCurve, Reparam), Error> {
        let knots = KnotVector::clamped(1, &[t0, t1], &[2, 2])?;
        let at = |t: f64| self.origin() + self.dir().get() * t;
        let curve = NurbsCurve::new(knots, vec![at(t0), at(t1)], None)?;
        Ok((curve, Reparam::Identity))
    }
}

impl Circle {
    /// The whole circle as a periodic rational quadratic: four quarter
    /// arcs. Fails with `OutsideSizeBox` when the control octagon, which
    /// reaches `r √2` from the centre, leaves the size box.
    pub fn to_nurbs(&self) -> Result<(NurbsCurve, Reparam), Error> {
        let r = self.radius();
        Ok((turn(self.frame(), r, r)?, Reparam::Angle))
    }
}

impl Ellipse {
    /// The whole ellipse, as for a circle, scaled along each axis.
    pub fn to_nurbs(&self) -> Result<(NurbsCurve, Reparam), Error> {
        Ok((
            turn(self.frame(), self.major(), self.minor())?,
            Reparam::Angle,
        ))
    }
}

impl Curve {
    /// The exact NURBS form of an analytic curve and how its parameter
    /// relates. `range` bounds an unbounded curve (a line); closed curves
    /// convert whole and ignore it.
    pub fn to_nurbs(&self, range: [f64; 2]) -> Result<(NurbsCurve, Reparam), Error> {
        match self {
            Self::Line(l) => l.to_nurbs(range),
            Self::Circle(c) => c.to_nurbs(),
            Self::Ellipse(e) => e.to_nurbs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Curve, Ellipse, Frame, Line, Point3, Vector3};
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.3, -0.2, 1.0),
            Vector3::new(1.0, 0.5, 0.0),
        )
        .unwrap()
    }

    #[test]
    fn identity_reparam() {
        assert_eq!(Reparam::Identity.to_nurbs(1.25), 1.25);
        assert_eq!(Reparam::Identity.from_nurbs(-3.5), -3.5);
    }

    #[test]
    fn angle_reparam_fixes_the_quarters() {
        let r = Reparam::Angle;
        for k in -2..=4 {
            let q = f64::from(k) * FRAC_PI_2;
            assert!((r.to_nurbs(q) - q).abs() <= 1e-15, "k={k}");
            assert!((r.from_nurbs(q) - q).abs() <= 1e-15, "k={k}");
            // The middle of a quarter is the middle of its knot span.
            assert!((r.to_nurbs(q + FRAC_PI_4) - (q + FRAC_PI_4)).abs() <= 1e-15);
        }
        let mut last = r.to_nurbs(-PI);
        for i in 1..=400 {
            let t = -PI + 3.0 * PI * f64::from(i) / 400.0;
            let s = r.to_nurbs(t);
            assert!(s > last, "not increasing at {t}");
            assert!((r.from_nurbs(s) - t).abs() <= 1e-14, "round trip at {t}");
            last = s;
        }
    }

    #[test]
    fn a_line_segment() {
        let l = Line::new(Point3::new(1.0, 2.0, 3.0), Vector3::new(1.0, 2.0, 2.0)).unwrap();
        let (n, r) = l.to_nurbs([-1.0, 2.0]).unwrap();
        assert_eq!(r, Reparam::Identity);
        assert_eq!(n.degree(), 1);
        assert_eq!(n.domain(), [-1.0, 2.0]);
        let c = Curve::Line(l);
        for t in [-1.0, 0.0, 0.7, 2.0] {
            assert!(n.point(t).distance(c.point(t)) <= 1e-15);
        }
        assert_eq!(
            l.to_nurbs([2.0, -1.0]),
            Err(crate::Error::InvalidNurbs(crate::NurbsError::KnotOrder))
        );
    }

    /// The analytic curve and its conversion agree through the
    /// reparameterization, over two turns.
    fn agree(c: &Curve, n: &NurbsCurve, r: Reparam) {
        for i in 0..=200 {
            let t = -TAU + 2.0 * TAU * f64::from(i) / 200.0;
            let d = n.point(r.to_nurbs(t)).distance(c.point(t));
            assert!(d <= 1e-14, "{c:?} at {t}: {d}");
        }
    }

    #[test]
    fn a_circle() {
        let circle = Circle::new(frame(), 2.0).unwrap();
        let (n, r) = circle.to_nurbs().unwrap();
        assert_eq!(r, Reparam::Angle);
        assert_eq!(n.degree(), 2);
        assert_eq!(n.period(), Some(TAU));
        assert_eq!(n.control_points().len(), 8);
        let h = core::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(n.weights(), [h, 1.0, h, 1.0, h, 1.0, h, 1.0]);
        let c = Curve::Circle(circle);
        // The on-curve control points are the quarter points.
        let pts: Vec<_> = n.control_points().collect();
        assert!(pts[1].distance(c.point(0.0)) <= 1e-15);
        assert!(pts[3].distance(c.point(FRAC_PI_2)) <= 1e-15);
        agree(&c, &n, r);
    }

    #[test]
    fn an_ellipse() {
        let e = Ellipse::new(frame(), 3.0, 1.25).unwrap();
        let (n, r) = e.to_nurbs().unwrap();
        assert_eq!(r, Reparam::Angle);
        agree(&Curve::Ellipse(e), &n, r);
    }

    #[test]
    fn through_the_curve_enum() {
        let c = Curve::Circle(Circle::new(frame(), 2.0).unwrap());
        let (n, r) = c.to_nurbs([0.0, 1.0]).unwrap();
        assert_eq!(n.period(), Some(TAU));
        agree(&c, &n, r);
        let l = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap());
        let (n, _) = l.to_nurbs([0.0, 1.0]).unwrap();
        assert_eq!(n.domain(), [0.0, 1.0]);
    }

    #[test]
    fn control_points_must_fit_the_size_box() {
        // The corners of the control octagon reach r √2 from the centre.
        let f = Frame::new(
            Point3::new(100.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 1.0, 0.0),
        )
        .unwrap();
        let c = Circle::new(f, 399.0).unwrap();
        assert_eq!(c.to_nurbs(), Err(crate::Error::OutsideSizeBox));
    }
}
