use super::{
    Box3, CurveEval, Ellipse, Frame, Point3, Projection, Rigid, UnitVector3, Vector3, polar_angle,
};
use crate::Error;
use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use core::f64::consts::TAU;

/// A curve in model space, evaluated by a parameter `t`.
///
/// Only analytic curves so far. NURBS will be another variant; the
/// analytic ones stay as fast paths with closed-form distances.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Line(Line),
    Circle(Circle),
    Ellipse(Ellipse),
}

/// An infinite straight line `origin + t dir`; `t` is arc length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    origin: Point3,
    dir: UnitVector3,
}

/// A full circle in its frame's xy plane: `origin + r (cos t x + sin t y)`.
/// `t` is the angle from `x`, counterclockwise about the frame's `z`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    frame: Frame,
    radius: f64,
}

/// A radius or length from outside the kernel: finite and at least
/// [`LINEAR_RESOLUTION`].
pub(super) fn check_length(x: f64) -> Result<f64, Error> {
    if !x.is_finite() {
        Err(Error::NonFinite)
    } else if x < LINEAR_RESOLUTION {
        Err(Error::Degenerate)
    } else {
        Ok(x)
    }
}

/// The box of a circle of radius `r` about `frame`'s axis, grown by `pad`
/// on every side: along each model axis the circle reaches
/// `r sqrt(1 - z_i²)` from its centre.
pub(super) fn ring_box(frame: &Frame, r: f64, pad: f64) -> Box3 {
    let z = frame.z().get();
    let reach = |zi: f64| r * crate::math::sqrt((1.0 - zi * zi).max(0.0)) + pad;
    let d = Vector3::new(reach(z.x), reach(z.y), reach(z.z));
    let o = frame.origin();
    Box3::new(o - d, o + d)
}

impl Line {
    /// Fails with `NonFinite`, `OutsideSizeBox` (origin), or `Degenerate`
    /// when `dir` has no direction.
    pub fn new(origin: Point3, dir: Vector3) -> Result<Self, Error> {
        Vector3::try_new(dir.x, dir.y, dir.z)?;
        let origin = Point3::try_new(origin.x, origin.y, origin.z)?;
        let dir = dir.normalize().ok_or(Error::Degenerate)?;
        Ok(Self { origin, dir })
    }

    /// The line from `a` towards `b`; `b` is at `t = a.distance(b)`.
    pub fn through(a: Point3, b: Point3) -> Result<Self, Error> {
        Self::new(a, b - a)
    }

    pub fn origin(&self) -> Point3 {
        self.origin
    }

    pub fn dir(&self) -> UnitVector3 {
        self.dir
    }
}

impl Circle {
    /// Fails with `NonFinite` or `Degenerate` for a radius below
    /// [`LINEAR_RESOLUTION`]. The frame is already checked.
    pub fn new(frame: Frame, radius: f64) -> Result<Self, Error> {
        Ok(Self {
            frame,
            radius: check_length(radius)?,
        })
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }
}

impl Curve {
    pub fn point(&self, t: f64) -> Point3 {
        match self {
            Self::Line(l) => l.origin + l.dir.get() * t,
            Self::Circle(c) => c.frame.origin() + c.frame.radial(t) * c.radius,
            Self::Ellipse(e) => e.point(t),
        }
    }

    /// The point and its first two derivatives with respect to `t`.
    pub fn eval(&self, t: f64) -> CurveEval {
        match self {
            Self::Line(l) => CurveEval {
                point: l.origin + l.dir.get() * t,
                d1: l.dir.get(),
                d2: Vector3::ZERO,
            },
            Self::Circle(c) => {
                let (radial, turn) = c.frame.radial_and_turn(t);
                CurveEval {
                    point: c.frame.origin() + radial * c.radius,
                    d1: turn * c.radius,
                    d2: radial * -c.radius,
                }
            }
            Self::Ellipse(e) => e.eval(t),
        }
    }

    /// The unit tangent, in the direction of increasing `t`. See
    /// [`CurveEval::tangent`] for where it is `None`; a line or circle
    /// always has one.
    pub fn tangent(&self, t: f64) -> Option<UnitVector3> {
        match self {
            Self::Line(l) => Some(l.dir),
            Self::Circle(c) => {
                let (s, co) = math::sin_cos(t);
                Some(UnitVector3::new_unchecked(
                    c.frame.x().get() * -s + c.frame.y().get() * co,
                ))
            }
            Self::Ellipse(e) => e.eval(t).tangent(),
        }
    }

    /// The curvature at `t`, in closed form: 0 for a line, `1 / r` for a
    /// circle.
    pub fn curvature(&self, t: f64) -> Option<f64> {
        match self {
            Self::Line(_) => Some(0.0),
            Self::Circle(c) => Some(1.0 / c.radius),
            Self::Ellipse(e) => Some(e.curvature(t)),
        }
    }

    /// The parameter range: unbounded for a line, `[0, 2π]` for a circle or
    /// an ellipse.
    pub fn domain(&self) -> [f64; 2] {
        match self {
            Self::Line(_) => [f64::NEG_INFINITY, f64::INFINITY],
            Self::Circle(_) | Self::Ellipse(_) => [0.0, TAU],
        }
    }

    /// The parameter period of a closed curve: 2π for a circle or an
    /// ellipse.
    pub fn period(&self) -> Option<f64> {
        match self {
            Self::Line(_) => None,
            Self::Circle(_) | Self::Ellipse(_) => Some(TAU),
        }
    }

    /// The distance from `p` to the nearest point of the curve.
    pub fn distance(&self, p: Point3) -> f64 {
        match self {
            Self::Line(l) => {
                let d = p - l.origin;
                (d - l.dir.get() * d.dot(l.dir.get())).length()
            }
            Self::Circle(c) => {
                let q = c.frame.to_local(p);
                let rho = math::sqrt(q.x * q.x + q.y * q.y);
                let dr = rho - c.radius;
                math::sqrt(dr * dr + q.z * q.z)
            }
            Self::Ellipse(e) => e.point(e.nearest_param(p)).distance(p),
        }
    }

    /// The nearest point of the curve to `p`, in closed form (for an
    /// ellipse, by a bisection that always converges). For a circle or an
    /// ellipse the parameter is in `[0, 2π)`. A point on a circle's axis
    /// maps to 0; where an ellipse has two nearest points the lower
    /// parameter wins.
    pub fn project(&self, p: Point3) -> Projection<f64, Point3> {
        let param = match self {
            Self::Line(l) => (p - l.origin).dot(l.dir.get()),
            Self::Circle(c) => {
                let q = c.frame.to_local(p);
                polar_angle(q.y, q.x)
            }
            Self::Ellipse(e) => e.nearest_param(p),
        };
        Projection {
            param,
            point: self.point(param),
            distance: self.distance(p),
        }
    }

    /// A box holding the curve over the parameter range `[t0, t1]`. A circle
    /// or an ellipse gives the box of the whole curve. An unbounded range on
    /// a line gives the size box.
    pub fn bounds(&self, range: [f64; 2]) -> Box3 {
        match self {
            Self::Line(_) if !(range[0].is_finite() && range[1].is_finite()) => {
                crate::tol::size_box()
            }
            Self::Line(_) => Box3::new(self.point(range[0]), self.point(range[1])),
            Self::Circle(c) => ring_box(&c.frame, c.radius, 0.0),
            Self::Ellipse(e) => e.bounds(),
        }
    }

    /// The curve moved by a rigid transform. Parameters are unchanged.
    pub fn transformed(&self, r: &Rigid) -> Self {
        match self {
            Self::Line(l) => Self::Line(Line {
                origin: r.apply_point(l.origin),
                dir: r.apply_unit(l.dir),
            }),
            Self::Circle(c) => Self::Circle(Circle {
                frame: c.frame.transformed(r),
                radius: c.radius,
            }),
            Self::Ellipse(e) => Self::Ellipse(e.transformed(r)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::{FRAC_PI_2, PI};

    fn near(a: Point3, b: Point3) -> bool {
        a.distance(b) <= 1e-14
    }

    fn circle() -> Curve {
        let f = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        Curve::Circle(Circle::new(f, 2.0).unwrap())
    }

    #[test]
    fn line_evaluates_by_arc_length() {
        let l = Curve::Line(Line::through(Point3::ORIGIN, Point3::new(3.0, 4.0, 0.0)).unwrap());
        assert!(near(l.point(5.0), Point3::new(3.0, 4.0, 0.0)));
        assert_eq!(l.tangent(1.0).unwrap().get(), Vector3::new(0.6, 0.8, 0.0));
        assert_eq!(l.period(), None);
        assert_eq!(l.distance(Point3::new(0.0, 0.0, 2.0)), 2.0);
        assert_eq!(l.project(Point3::new(3.0, 4.0, 7.0)).param, 5.0);
    }

    #[test]
    fn circle_evaluates_by_angle() {
        let c = circle();
        assert!(near(c.point(0.0), Point3::new(3.0, 2.0, 3.0)));
        assert!(near(c.point(FRAC_PI_2), Point3::new(1.0, 4.0, 3.0)));
        assert!((c.tangent(0.0).unwrap().get() - Vector3::new(0.0, 1.0, 0.0)).length() <= 1e-16);
        assert_eq!(c.period(), Some(TAU));
    }

    #[test]
    fn derivatives_in_closed_form() {
        let c = circle();
        let e = c.eval(FRAC_PI_2);
        assert_eq!(e.point, c.point(FRAC_PI_2));
        assert!((e.d1 - Vector3::new(-2.0, 0.0, 0.0)).length() <= 1e-15);
        assert!((e.d2 - Vector3::new(0.0, -2.0, 0.0)).length() <= 1e-15);
        assert_eq!(c.curvature(1.0), Some(0.5));
        let l = Curve::Line(Line::through(Point3::ORIGIN, Point3::new(3.0, 4.0, 0.0)).unwrap());
        let e = l.eval(5.0);
        assert_eq!(e.point, l.point(5.0));
        assert_eq!(e.d1, Vector3::new(0.6, 0.8, 0.0));
        assert_eq!(e.d2, Vector3::ZERO);
        assert_eq!(l.curvature(5.0), Some(0.0));
    }

    #[test]
    fn domains() {
        assert_eq!(circle().domain(), [0.0, TAU]);
        let l = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)).unwrap());
        assert_eq!(l.domain(), [f64::NEG_INFINITY, f64::INFINITY]);
    }

    #[test]
    fn circle_distance_and_param() {
        let c = circle();
        // The centre is one radius from every point.
        assert_eq!(c.distance(Point3::new(1.0, 2.0, 3.0)), 2.0);
        // 3 m from the axis (1 m outside the circle) and 4 m above its plane.
        let p = Point3::new(1.0, 2.0 - 3.0, 7.0);
        assert_eq!(c.distance(p), 17.0f64.sqrt());
        assert_eq!(c.project(p).param, 3.0 * FRAC_PI_2);
        assert_eq!(c.project(Point3::new(-5.0, 2.0, 3.0)).param, PI);
        assert_eq!(c.project(Point3::new(1.0, 2.0, 9.0)).param, 0.0);
        for t in [0.0, 0.5, 2.0, 4.0, 6.0] {
            let q = c.point(t);
            assert!(c.distance(q) <= 1e-15);
            let foot = c.project(q);
            assert!((foot.param - t).abs() <= 1e-14);
            assert_eq!(foot.point, c.point(foot.param));
            assert_eq!(foot.distance, c.distance(q));
        }
    }

    #[test]
    fn bounds() {
        let l = Curve::Line(Line::through(Point3::ORIGIN, Point3::new(3.0, 4.0, 0.0)).unwrap());
        let b = l.bounds([-5.0, 5.0]);
        assert_eq!(b.min(), Point3::new(-3.0, -4.0, 0.0));
        assert_eq!(b.max(), Point3::new(3.0, 4.0, 0.0));
        assert_eq!(l.bounds([0.0, f64::INFINITY]), crate::tol::size_box());
        // A circle tilted 45° about x: its box is tight on every axis.
        let f = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, -1.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let c = Curve::Circle(Circle::new(f, 2.0).unwrap());
        let b = c.bounds([0.0, 1.0]);
        let h = 2.0f64.sqrt();
        let close = |a: Point3, b: Point3| a.distance(b) <= 1e-15;
        assert!(close(b.min(), Point3::new(-1.0, 2.0 - h, 3.0 - h)));
        assert!(close(b.max(), Point3::new(3.0, 2.0 + h, 3.0 + h)));
    }

    #[test]
    fn transformed_moves_the_curve() {
        let r = Rigid::translation(Vector3::new(0.0, 0.0, 1.0))
            .then_after(&Rigid::rotation(UnitVector3::Z, FRAC_PI_2));
        let c = circle().transformed(&r);
        assert!(near(c.point(0.0), Point3::new(-2.0, 3.0, 4.0)));
        let l = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)).unwrap());
        assert!(near(
            l.transformed(&r).point(1.0),
            Point3::new(0.0, 1.0, 1.0)
        ));
    }

    #[test]
    fn constructors_reject_bad_input() {
        let o = Point3::ORIGIN;
        assert_eq!(Line::new(o, Vector3::ZERO), Err(Error::Degenerate));
        assert_eq!(
            Line::new(o, Vector3::new(f64::NAN, 0.0, 0.0)),
            Err(Error::NonFinite)
        );
        assert_eq!(
            Line::new(Point3::new(0.0, -501.0, 0.0), Vector3::new(1.0, 0.0, 0.0)),
            Err(Error::OutsideSizeBox)
        );
        assert_eq!(Circle::new(Frame::WORLD, 1e-9), Err(Error::Degenerate));
        assert_eq!(
            Circle::new(Frame::WORLD, f64::INFINITY),
            Err(Error::NonFinite)
        );
        assert_eq!(
            Circle::new(Frame::WORLD, LINEAR_RESOLUTION)
                .unwrap()
                .radius(),
            1e-8
        );
    }
}
