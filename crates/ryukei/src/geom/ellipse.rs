//! The ellipse, and the robust nearest point on an ellipse that its
//! projection is built on.

use super::curve::check_length;
use super::{Box3, CurveEval, Frame, Point3, Rigid, Vector3, polar_angle};
use crate::Error;
use crate::math;

/// A full ellipse in its frame's xy plane:
/// `origin + a cos t x + b sin t y`, with the major semi-axis `a` along `x`
/// and the minor `b` along `y`, `a >= b`. `t` is the eccentric angle;
/// equal radii give a circle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipse {
    frame: Frame,
    major: f64,
    minor: f64,
}

impl Ellipse {
    /// Fails with `NonFinite`, or `Degenerate` for a radius below
    /// [`crate::tol::LINEAR_RESOLUTION`] or a minor radius larger than the
    /// major.
    pub fn new(frame: Frame, major: f64, minor: f64) -> Result<Self, Error> {
        let major = check_length(major)?;
        let minor = check_length(minor)?;
        if minor > major {
            return Err(Error::Degenerate);
        }
        Ok(Self {
            frame,
            major,
            minor,
        })
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    pub fn major(&self) -> f64 {
        self.major
    }

    pub fn minor(&self) -> f64 {
        self.minor
    }

    pub(super) fn eval(&self, t: f64) -> CurveEval {
        let (s, c) = math::sin_cos(t);
        let (x, y) = (self.frame.x().get(), self.frame.y().get());
        let (a, b) = (self.major, self.minor);
        let out = x * (a * c) + y * (b * s);
        CurveEval {
            point: self.frame.origin() + out,
            d1: x * (-a * s) + y * (b * c),
            d2: -out,
        }
    }

    pub(super) fn point(&self, t: f64) -> Point3 {
        self.eval(t).point
    }

    /// `ab / (a² sin² t + b² cos² t)^(3/2)`.
    pub(super) fn curvature(&self, t: f64) -> f64 {
        let (s, c) = math::sin_cos(t);
        let (a, b) = (self.major, self.minor);
        let q = a * a * s * s + b * b * c * c;
        a * b / (q * math::sqrt(q))
    }

    /// The parameter of the nearest point to `p`, in `[0, 2π)`. Ties go to
    /// the lower parameter.
    pub(super) fn nearest_param(&self, p: Point3) -> f64 {
        let q = self.frame.to_local(p);
        let (x, y) = nearest_on_ellipse(self.major, self.minor, q.x, q.y);
        polar_angle(y / self.minor, x / self.major)
    }

    /// The exact box: along each model axis the ellipse reaches
    /// `sqrt((a x_i)² + (b y_i)²)` from its centre.
    pub(super) fn bounds(&self) -> Box3 {
        let (x, y) = (self.frame.x().get(), self.frame.y().get());
        let (a, b) = (self.major, self.minor);
        let reach = |xi: f64, yi: f64| math::hypot(a * xi, b * yi);
        let d = Vector3::new(reach(x.x, y.x), reach(x.y, y.y), reach(x.z, y.z));
        let o = self.frame.origin();
        Box3::new(o - d, o + d)
    }

    pub(super) fn transformed(&self, r: &Rigid) -> Self {
        Self {
            frame: self.frame.transformed(r),
            ..*self
        }
    }

    /// Reflected, its parameter kept (see [`super::Curve::mirrored`]).
    pub(super) fn mirrored(&self, m: &super::Mirror) -> Result<Self, Error> {
        Ok(Self {
            frame: m.frame(&self.frame)?,
            ..*self
        })
    }
}

/// The nearest point of the ellipse `(x/a)² + (y/b)² = 1`, `a >= b > 0`, to
/// `(x, y)`, by Eberly's robust method ("Distance from a Point to an
/// Ellipse, an Ellipsoid, or a Hyperellipsoid"): reflect into the first
/// quadrant, then bisect the one-variable equation for the Lagrange
/// multiplier until the midpoint stops moving. Bisection on f64 always
/// ends, in at most about 1100 steps.
///
/// Where the nearest point is not unique (on the major axis near the
/// centre) the one with `y >= 0` is returned, the lower eccentric angle.
pub(super) fn nearest_on_ellipse(a: f64, b: f64, x: f64, y: f64) -> (f64, f64) {
    let (y0, y1) = (x.abs(), y.abs());
    let (x0, x1) = if y1 > 0.0 {
        if y0 > 0.0 {
            let (z0, z1) = (y0 / a, y1 / b);
            let g = z0 * z0 + z1 * z1 - 1.0;
            if g != 0.0 {
                let r0 = (a / b) * (a / b);
                let s = root(r0, z0, z1, g);
                (r0 * y0 / (s + r0), y1 / (s + 1.0))
            } else {
                (y0, y1)
            }
        } else {
            (0.0, b)
        }
    } else {
        let numer = a * y0;
        let denom = a * a - b * b;
        if numer < denom {
            let xde = numer / denom;
            (a * xde, b * math::sqrt((1.0 - xde * xde).max(0.0)))
        } else {
            (a, 0.0)
        }
    };
    let sx = if x < 0.0 { -x0 } else { x0 };
    let sy = if y < 0.0 { -x1 } else { x1 };
    (sx, sy)
}

/// The root `s` of `(r0 z0 / (s + r0))² + (z1 / (s + 1))² = 1`, by
/// bisection. `g` is the sign of the left side at `s = 0`, minus 1.
fn root(r0: f64, z0: f64, z1: f64, g: f64) -> f64 {
    let n0 = r0 * z0;
    let mut s0 = z1 - 1.0;
    let mut s1 = if g < 0.0 {
        0.0
    } else {
        math::hypot(n0, z1) - 1.0
    };
    loop {
        let s = (s0 + s1) / 2.0;
        if s == s0 || s == s1 {
            return s;
        }
        let (q0, q1) = (n0 / (s + r0), z1 / (s + 1.0));
        let g = q0 * q0 + q1 * q1 - 1.0;
        if g > 0.0 {
            s0 = s;
        } else if g < 0.0 {
            s1 = s;
        } else {
            return s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Curve, Vector3};
    use core::f64::consts::{FRAC_PI_2, PI, TAU};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    }

    /// Semi-axes 3 along x and 2 along y, centred at (1, 2, 3).
    fn ellipse() -> Curve {
        Curve::Ellipse(Ellipse::new(frame(), 3.0, 2.0).unwrap())
    }

    fn near(a: Point3, b: Point3) -> bool {
        a.distance(b) <= 1e-14
    }

    #[test]
    fn constructor_rejects_bad_input() {
        let f = Frame::WORLD;
        assert_eq!(Ellipse::new(f, 1.0, 2.0), Err(Error::Degenerate));
        assert_eq!(Ellipse::new(f, 1.0, 1e-9), Err(Error::Degenerate));
        assert_eq!(Ellipse::new(f, f64::NAN, 1.0), Err(Error::NonFinite));
        let e = Ellipse::new(f, 2.0, 2.0).unwrap();
        assert_eq!((e.major(), e.minor()), (2.0, 2.0));
        assert_eq!(e.frame(), &f);
    }

    #[test]
    fn evaluates_by_eccentric_angle() {
        let c = ellipse();
        assert!(near(c.point(0.0), Point3::new(4.0, 2.0, 3.0)));
        assert!(near(c.point(FRAC_PI_2), Point3::new(1.0, 4.0, 3.0)));
        let e = c.eval(PI);
        assert_eq!(e.point, c.point(PI));
        assert!((e.d1 - Vector3::new(0.0, -2.0, 0.0)).length() <= 1e-15);
        assert!((e.d2 - Vector3::new(3.0, 0.0, 0.0)).length() <= 1e-15);
        assert!((c.tangent(0.0).unwrap().get() - Vector3::new(0.0, 1.0, 0.0)).length() <= 1e-15);
        assert_eq!(c.period(), Some(TAU));
        assert_eq!(c.domain(), [0.0, TAU]);
    }

    #[test]
    fn curvature_at_the_vertices() {
        let c = ellipse();
        // a / b² at the ends of the major axis, b / a² at the minor.
        assert!((c.curvature(0.0).unwrap() - 0.75).abs() <= 1e-15);
        assert!((c.curvature(FRAC_PI_2).unwrap() - 2.0 / 9.0).abs() <= 1e-15);
        let general = c.eval(1.0).curvature().unwrap();
        assert!((c.curvature(1.0).unwrap() - general).abs() <= 1e-15);
    }

    #[test]
    fn bounds_are_tight() {
        // Tilted 45° about x: y and z each reach 2 cos 45° from the centre.
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, -1.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let c = Curve::Ellipse(Ellipse::new(f, 3.0, 2.0).unwrap());
        let b = c.bounds([0.0, 1.0]);
        let h = 2.0f64.sqrt();
        assert!(near(b.min(), Point3::new(-3.0, -h, -h)));
        assert!(near(b.max(), Point3::new(3.0, h, h)));
    }

    #[test]
    fn projection_of_points_on_the_curve() {
        let c = ellipse();
        for t in [0.0, 0.3, 1.0, FRAC_PI_2, 2.5, PI, 4.0, 5.5, 6.2] {
            let q = c.point(t);
            let foot = c.project(q);
            assert!((foot.param - t).abs() <= 1e-14, "t={t} got {}", foot.param);
            assert!(foot.distance <= 1e-14);
            assert_eq!(foot.point, c.point(foot.param));
            assert_eq!(c.distance(q), foot.distance);
        }
    }

    #[test]
    fn projection_off_the_curve() {
        let c = ellipse();
        // Beyond the end of the major axis, and above the plane.
        let foot = c.project(Point3::new(9.0, 2.0, 7.0));
        assert_eq!(foot.param, 0.0);
        assert!((foot.distance - (25.0f64 + 16.0).sqrt()).abs() <= 1e-14);
        // On the minor axis, inside: the nearest point is the minor vertex.
        let foot = c.project(Point3::new(1.0, 2.5, 3.0));
        assert!((foot.param - FRAC_PI_2).abs() <= 1e-15);
        assert!((foot.distance - 1.5).abs() <= 1e-15);
        let foot = c.project(Point3::new(1.0, 1.5, 3.0));
        assert!((foot.param - 3.0 * FRAC_PI_2).abs() <= 1e-15);
        // The centre: the minor vertices are nearest; the lower parameter
        // wins.
        let foot = c.project(Point3::new(1.0, 2.0, 3.0));
        assert!((foot.param - FRAC_PI_2).abs() <= 1e-15);
        assert!((foot.distance - 2.0).abs() <= 1e-15);
        // On the major axis near the centre there are two nearest points,
        // mirror images across the axis; the one at the lower parameter
        // wins on both sides.
        for (x, lo, hi) in [(1.5, 0.0, FRAC_PI_2), (0.5, FRAC_PI_2, PI)] {
            let foot = c.project(Point3::new(x, 2.0, 3.0));
            assert!(lo < foot.param && foot.param < hi, "x={x} t={}", foot.param);
            let mirror = c.point(TAU - foot.param);
            let mirror = mirror.distance(Point3::new(x, 2.0, 3.0));
            assert!((mirror - foot.distance).abs() <= 1e-14);
        }
    }

    #[test]
    fn projection_beats_dense_sampling() {
        let c = ellipse();
        for (x, y) in [
            (0.3, 0.1),
            (-2.0, 1.0),
            (5.0, -4.0),
            (-0.4, -1.9),
            (2.9, 0.01),
        ] {
            let p = Point3::new(1.0 + x, 2.0 + y, 3.5);
            let d = c.distance(p);
            for i in 0..=4096 {
                let t = TAU * i as f64 / 4096.0;
                assert!(c.point(t).distance(p) >= d - 1e-14, "({x}, {y}) t={t}");
            }
        }
    }

    #[test]
    fn equal_radii_behave_like_a_circle() {
        let e = Curve::Ellipse(Ellipse::new(frame(), 2.0, 2.0).unwrap());
        let c = Curve::Circle(crate::geom::Circle::new(frame(), 2.0).unwrap());
        for p in [
            Point3::new(4.0, 7.0, -1.0),
            Point3::new(1.0, 2.0, 9.0),
            Point3::new(0.5, 1.0, 3.0),
        ] {
            let (a, b) = (e.project(p), c.project(p));
            assert!((a.param - b.param).abs() <= 1e-15, "{p:?}");
            assert!((a.distance - b.distance).abs() <= 1e-14, "{p:?}");
        }
    }

    #[test]
    fn transformed_moves_the_frame() {
        let r = crate::geom::Rigid::translation(Vector3::new(0.0, 0.0, 10.0));
        let c = ellipse();
        let t = c.transformed(&r);
        assert!(near(
            t.point(0.7),
            c.point(0.7) + Vector3::new(0.0, 0.0, 10.0)
        ));
    }
}
