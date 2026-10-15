use super::{Frame, Point3, Rigid, UnitVector3, Vector3, polar_angle};
use crate::Error;
use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use core::f64::consts::TAU;

/// A curve in model space, evaluated by a parameter `t`.
///
/// Phase 1 has only the analytic curves. NURBS will be another variant;
/// the analytic ones stay as fast paths with closed-form distances.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Line(Line),
    Circle(Circle),
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
        }
    }

    /// The unit tangent, in the direction of increasing `t`.
    pub fn tangent(&self, t: f64) -> UnitVector3 {
        match self {
            Self::Line(l) => l.dir,
            Self::Circle(c) => {
                let (s, co) = math::sin_cos(t);
                UnitVector3::new_unchecked(c.frame.x().get() * -s + c.frame.y().get() * co)
            }
        }
    }

    /// The parameter period of a closed curve: 2π for a circle.
    pub fn period(&self) -> Option<f64> {
        match self {
            Self::Line(_) => None,
            Self::Circle(_) => Some(TAU),
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
        }
    }

    /// The parameter of the nearest point of the curve to `p`. For a circle
    /// it is in `[0, 2π)`, and a point on the axis maps to 0.
    pub fn param_of(&self, p: Point3) -> f64 {
        match self {
            Self::Line(l) => (p - l.origin).dot(l.dir.get()),
            Self::Circle(c) => {
                let q = c.frame.to_local(p);
                polar_angle(q.y, q.x)
            }
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
        assert_eq!(l.tangent(1.0).get(), Vector3::new(0.6, 0.8, 0.0));
        assert_eq!(l.period(), None);
        assert_eq!(l.distance(Point3::new(0.0, 0.0, 2.0)), 2.0);
        assert_eq!(l.param_of(Point3::new(3.0, 4.0, 7.0)), 5.0);
    }

    #[test]
    fn circle_evaluates_by_angle() {
        let c = circle();
        assert!(near(c.point(0.0), Point3::new(3.0, 2.0, 3.0)));
        assert!(near(c.point(FRAC_PI_2), Point3::new(1.0, 4.0, 3.0)));
        assert!((c.tangent(0.0).get() - Vector3::new(0.0, 1.0, 0.0)).length() <= 1e-16);
        assert_eq!(c.period(), Some(TAU));
    }

    #[test]
    fn circle_distance_and_param() {
        let c = circle();
        // The centre is one radius from every point.
        assert_eq!(c.distance(Point3::new(1.0, 2.0, 3.0)), 2.0);
        // 3 m from the axis (1 m outside the circle) and 4 m above its plane.
        let p = Point3::new(1.0, 2.0 - 3.0, 7.0);
        assert_eq!(c.distance(p), 17.0f64.sqrt());
        assert_eq!(c.param_of(p), 3.0 * FRAC_PI_2);
        assert_eq!(c.param_of(Point3::new(-5.0, 2.0, 3.0)), PI);
        assert_eq!(c.param_of(Point3::new(1.0, 2.0, 9.0)), 0.0);
        for t in [0.0, 0.5, 2.0, 4.0, 6.0] {
            let q = c.point(t);
            assert!(c.distance(q) <= 1e-15);
            assert!((c.param_of(q) - t).abs() <= 1e-14);
        }
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
