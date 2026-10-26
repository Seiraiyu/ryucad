//! Curves in a surface's parameter plane (pcurves): `Line2`, `Circle2`
//! and `Ellipse2`, with the same evaluation and projection as the curves in
//! model space. Parameter space has no size box; values only need to be
//! finite.

use super::curve::check_length;
use super::ellipse::nearest_on_ellipse;
use super::{Box2, CurveEval2, NurbsCurve2, ParamFlip, Point2, Projection, Vector2, polar_angle};
use crate::Error;
use crate::math;
use core::f64::consts::TAU;
use std::sync::Arc;

/// An origin and a unit `x` direction in the plane, with `y` a quarter
/// turn from `x` counterclockwise (`ccw`) or clockwise. Circles and
/// ellipses sit in one, so a pcurve can run either way round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame2 {
    origin: Point2,
    x: Vector2,
    y: Vector2,
}

impl Frame2 {
    /// Fails with `NonFinite`, or `Degenerate` when `x` has no direction.
    pub fn new(origin: Point2, x: Vector2, ccw: bool) -> Result<Self, Error> {
        if ![origin.x, origin.y, x.x, x.y].iter().all(|c| c.is_finite()) {
            return Err(Error::NonFinite);
        }
        // Scaled down first where the squares would overflow, which would
        // make the direction zero.
        let x = if x.length_squared().is_infinite() {
            x / x.x.abs().max(x.y.abs())
        } else {
            x
        };
        let len = x.length();
        if len <= crate::tol::LINEAR_RESOLUTION {
            return Err(Error::Degenerate);
        }
        let x = x / len;
        let y = if ccw {
            Vector2::new(-x.y, x.x)
        } else {
            Vector2::new(x.y, -x.x)
        };
        Ok(Self { origin, x, y })
    }

    /// A frame whose `x` already has unit length as closely as the
    /// kernel's own do (`|x · x - 1| <= 1e-12`), kept bit for bit, so a
    /// frame the codec wrote reads back the same ([`Frame2::new`] would
    /// divide by a length a few ulps off 1). `None` otherwise, or for a
    /// coordinate that isn't finite.
    pub(crate) fn exact(origin: Point2, x: Vector2, ccw: bool) -> Option<Self> {
        let finite = [origin.x, origin.y, x.x, x.y].iter().all(|c| c.is_finite());
        if !finite || (x.length_squared() - 1.0).abs() > 1e-12 {
            return None;
        }
        let y = if ccw {
            Vector2::new(-x.y, x.x)
        } else {
            Vector2::new(x.y, -x.x)
        };
        Some(Self { origin, x, y })
    }

    pub fn origin(&self) -> Point2 {
        self.origin
    }

    pub fn x(&self) -> Vector2 {
        self.x
    }

    pub fn y(&self) -> Vector2 {
        self.y
    }

    pub fn is_ccw(&self) -> bool {
        self.x.cross(self.y) > 0.0
    }

    /// The local coordinates of `p` along `x` and `y`.
    fn local(&self, p: Point2) -> (f64, f64) {
        let d = p - self.origin;
        (d.dot(self.x), d.dot(self.y))
    }
}

/// A line in the plane, `origin + t dir`; `t` is arc length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line2 {
    origin: Point2,
    dir: Vector2,
}

/// A circle `origin + r (cos t x + sin t y)` in its frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle2 {
    frame: Frame2,
    radius: f64,
}

/// An ellipse `origin + a cos t x + b sin t y` in its frame, `a >= b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipse2 {
    frame: Frame2,
    major: f64,
    minor: f64,
}

impl Line2 {
    /// Fails with `NonFinite`, or `Degenerate` when `dir` has no direction.
    pub fn new(origin: Point2, dir: Vector2) -> Result<Self, Error> {
        let f = Frame2::new(origin, dir, true)?;
        Ok(Self { origin, dir: f.x() })
    }

    /// A line along a direction that is already unit, kept bit for bit
    /// (see [`Frame2::exact`]).
    pub(crate) fn exact(origin: Point2, dir: Vector2) -> Option<Self> {
        let f = Frame2::exact(origin, dir, true)?;
        Some(Self { origin, dir: f.x })
    }

    /// The line from `a` towards `b`; `b` is at `t = a.distance(b)`.
    pub fn through(a: Point2, b: Point2) -> Result<Self, Error> {
        Self::new(a, b - a)
    }

    pub fn origin(&self) -> Point2 {
        self.origin
    }

    pub fn dir(&self) -> Vector2 {
        self.dir
    }
}

impl Circle2 {
    pub fn new(frame: Frame2, radius: f64) -> Result<Self, Error> {
        Ok(Self {
            frame,
            radius: check_length(radius)?,
        })
    }

    pub fn frame(&self) -> &Frame2 {
        &self.frame
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }
}

impl Ellipse2 {
    /// Fails like [`crate::geom::Ellipse::new`].
    pub fn new(frame: Frame2, major: f64, minor: f64) -> Result<Self, Error> {
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

    pub fn frame(&self) -> &Frame2 {
        &self.frame
    }

    pub fn major(&self) -> f64 {
        self.major
    }

    pub fn minor(&self) -> f64 {
        self.minor
    }
}

/// A curve in a surface's `(u, v)` parameter plane.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Curve2 {
    Line2(Line2),
    Circle2(Circle2),
    Ellipse2(Ellipse2),
    Nurbs2(Arc<NurbsCurve2>),
}

/// `origin + a cos t x + b sin t y` and its derivatives.
fn turn(f: &Frame2, a: f64, b: f64, t: f64) -> CurveEval2 {
    let (s, c) = math::sin_cos(t);
    let out = f.x * (a * c) + f.y * (b * s);
    CurveEval2 {
        point: f.origin + out,
        d1: f.x * (-a * s) + f.y * (b * c),
        d2: -out,
    }
}

impl Curve2 {
    pub fn point(&self, t: f64) -> Point2 {
        self.eval(t).point
    }

    /// The point and its first two derivatives with respect to `t`.
    pub fn eval(&self, t: f64) -> CurveEval2 {
        match self {
            Self::Line2(l) => CurveEval2 {
                point: l.origin + l.dir * t,
                d1: l.dir,
                d2: Vector2::ZERO,
            },
            Self::Circle2(c) => turn(&c.frame, c.radius, c.radius, t),
            Self::Ellipse2(e) => turn(&e.frame, e.major, e.minor, t),
            Self::Nurbs2(n) => n.eval(t),
        }
    }

    /// The unit tangent in the direction of increasing `t`.
    pub fn tangent(&self, t: f64) -> Option<Vector2> {
        match self {
            Self::Nurbs2(n) => n.tangent(t),
            _ => self.eval(t).tangent(),
        }
    }

    /// The curvature, in closed form.
    pub fn curvature(&self, t: f64) -> Option<f64> {
        match self {
            Self::Line2(_) => Some(0.0),
            Self::Circle2(c) => Some(1.0 / c.radius),
            Self::Ellipse2(e) => {
                let (s, c) = math::sin_cos(t);
                let (a, b) = (e.major, e.minor);
                let q = a * a * s * s + b * b * c * c;
                Some(a * b / (q * math::sqrt(q)))
            }
            Self::Nurbs2(n) => n.curvature(t),
        }
    }

    /// Unbounded for a line, `[0, 2π]` for a circle or an ellipse, the
    /// knot domain for a NURBS.
    pub fn domain(&self) -> [f64; 2] {
        match self {
            Self::Line2(_) => [f64::NEG_INFINITY, f64::INFINITY],
            Self::Circle2(_) | Self::Ellipse2(_) => [0.0, TAU],
            Self::Nurbs2(n) => n.domain(),
        }
    }

    pub fn period(&self) -> Option<f64> {
        match self {
            Self::Line2(_) => None,
            Self::Circle2(_) | Self::Ellipse2(_) => Some(TAU),
            Self::Nurbs2(n) => n.period(),
        }
    }

    /// The nearest point to `p`: in closed form as for
    /// [`crate::geom::Curve::project`], or by [`NurbsCurve2::project`].
    pub fn project(&self, p: Point2) -> Projection<f64, Point2> {
        let param = match self {
            Self::Nurbs2(n) => return n.project(p),
            Self::Line2(l) => (p - l.origin).dot(l.dir),
            Self::Circle2(c) => {
                let (x, y) = c.frame.local(p);
                polar_angle(y, x)
            }
            Self::Ellipse2(e) => {
                let (x, y) = e.frame.local(p);
                let (x, y) = nearest_on_ellipse(e.major, e.minor, x, y);
                polar_angle(y / e.minor, x / e.major)
            }
        };
        let point = self.point(param);
        Projection {
            param,
            point,
            distance: point.distance(p),
        }
    }

    pub fn distance(&self, p: Point2) -> f64 {
        self.project(p).distance
    }

    /// A box holding the curve over `[t0, t1]`: exact for a segment, the
    /// whole curve's for a circle or an ellipse, the hull boxes of the
    /// overlapping pieces for a NURBS. A line's unbounded range gives an
    /// unbounded box.
    pub fn bounds(&self, [t0, t1]: [f64; 2]) -> Box2 {
        let ring = |f: &Frame2, a: f64, b: f64| {
            let d = Vector2::new(
                math::hypot(a * f.x.x, b * f.y.x),
                math::hypot(a * f.x.y, b * f.y.y),
            );
            Box2::new(f.origin - d, f.origin + d)
        };
        match self {
            Self::Line2(_) => Box2::new(self.point(t0), self.point(t1)),
            Self::Circle2(c) => ring(&c.frame, c.radius, c.radius),
            Self::Ellipse2(e) => ring(&e.frame, e.major, e.minor),
            Self::Nurbs2(n) => n.bounds([t0, t1]),
        }
    }

    /// The curve with its parameter plane mapped by `flip`, as a mirrored
    /// surface maps it ([`super::Surface::mirrored`]). Its own parameter
    /// is unchanged: a circle or an ellipse whose plane is reflected runs
    /// the other way round.
    pub fn mirrored(&self, flip: ParamFlip) -> Self {
        let f = |p: Point2| flip.apply(p);
        let v = |d: Vector2| flip.apply_vector(d);
        let frame = |fr: &Frame2| Frame2 {
            origin: f(fr.origin),
            x: v(fr.x),
            y: v(fr.y),
        };
        match self {
            Self::Line2(l) => Self::Line2(Line2 {
                origin: f(l.origin),
                dir: v(l.dir),
            }),
            Self::Circle2(c) => Self::Circle2(Circle2 {
                frame: frame(&c.frame),
                ..*c
            }),
            Self::Ellipse2(e) => Self::Ellipse2(Ellipse2 {
                frame: frame(&e.frame),
                ..*e
            }),
            Self::Nurbs2(n) => Self::Nurbs2(Arc::new(n.mapped(f))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::{FRAC_PI_2, PI, TAU};

    fn near(a: Point2, b: Point2) -> bool {
        a.distance(b) <= 1e-14
    }

    fn close(a: Vector2, b: Vector2) -> bool {
        (a - b).length() <= 1e-14
    }

    #[test]
    fn mirrored_pcurves_follow_their_planes() {
        let f = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 0.5), true).unwrap();
        let circle = Curve2::Circle2(Circle2::new(f, 0.5).unwrap());
        let nurbs = circle.to_nurbs([0.0, PI]).unwrap().0;
        for c in [
            Curve2::Line2(Line2::new(Point2::new(1.0, 0.0), Vector2::new(0.3, 1.0)).unwrap()),
            circle,
            Curve2::Ellipse2(Ellipse2::new(f, 2.0, 1.0).unwrap()),
            Curve2::Nurbs2(Arc::new(nurbs)),
        ] {
            for flip in [ParamFlip::Kept, ParamFlip::U(TAU), ParamFlip::V(0.0)] {
                let r = c.mirrored(flip);
                for t in [0.1, 1.0, 2.5] {
                    assert!(near(flip.apply(c.point(t)), r.point(t)), "{c:?} {flip:?}");
                }
            }
        }
    }

    #[test]
    fn frames_turn_either_way() {
        let ccw = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(0.0, 3.0), true).unwrap();
        assert!(close(ccw.x(), Vector2::new(0.0, 1.0)));
        assert!(close(ccw.y(), Vector2::new(-1.0, 0.0)));
        assert!(ccw.is_ccw());
        let cw = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(0.0, 3.0), false).unwrap();
        assert!(close(cw.y(), Vector2::new(1.0, 0.0)));
        assert_eq!(
            Frame2::new(Point2::ORIGIN, Vector2::ZERO, true),
            Err(Error::Degenerate)
        );
        assert_eq!(
            Frame2::new(Point2::new(f64::NAN, 0.0), Vector2::new(1.0, 0.0), true),
            Err(Error::NonFinite)
        );
    }

    fn circle(ccw: bool) -> Curve2 {
        let f = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 0.0), ccw).unwrap();
        Curve2::Circle2(Circle2::new(f, 2.0).unwrap())
    }

    fn ellipse() -> Curve2 {
        let f = Frame2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 0.0), true).unwrap();
        Curve2::Ellipse2(Ellipse2::new(f, 3.0, 2.0).unwrap())
    }

    #[test]
    fn evaluation_and_derivatives() {
        let l = Curve2::Line2(Line2::through(Point2::ORIGIN, Point2::new(3.0, 4.0)).unwrap());
        let e = l.eval(5.0);
        assert!(near(e.point, Point2::new(3.0, 4.0)));
        assert_eq!(e.d1, Vector2::new(0.6, 0.8));
        assert_eq!(e.d2, Vector2::ZERO);
        assert_eq!(l.curvature(1.0), Some(0.0));
        assert_eq!(l.domain(), [f64::NEG_INFINITY, f64::INFINITY]);
        let c = circle(true);
        assert!(near(c.point(FRAC_PI_2), Point2::new(1.0, 4.0)));
        let e = c.eval(0.0);
        assert!(close(e.d1, Vector2::new(0.0, 2.0)) && close(e.d2, Vector2::new(-2.0, 0.0)));
        assert_eq!(c.curvature(0.3), Some(0.5));
        assert!(close(c.tangent(0.0).unwrap(), Vector2::new(0.0, 1.0)));
        // Clockwise: a quarter turn goes down.
        assert!(near(circle(false).point(FRAC_PI_2), Point2::new(1.0, 0.0)));
        assert_eq!(c.period(), Some(TAU));
        assert_eq!(c.domain(), [0.0, TAU]);
        let el = ellipse();
        assert!(near(el.point(PI), Point2::new(-2.0, 2.0)));
        assert!((el.curvature(0.0).unwrap() - 0.75).abs() <= 1e-15);
    }

    #[test]
    fn projection() {
        let l = Curve2::Line2(Line2::new(Point2::new(1.0, 1.0), Vector2::new(1.0, 0.0)).unwrap());
        let foot = l.project(Point2::new(4.0, -2.0));
        assert_eq!((foot.param, foot.distance), (3.0, 3.0));
        assert_eq!(foot.point, Point2::new(4.0, 1.0));
        for ccw in [true, false] {
            let c = circle(ccw);
            for t in [0.0, 1.0, 2.5, 4.0, 6.0] {
                let q = c.point(t);
                let foot = c.project(q);
                assert!((foot.param - t).abs() <= 1e-14, "ccw={ccw} t={t}");
                assert!(foot.distance <= 1e-14);
            }
            // The centre maps to 0.
            assert_eq!(c.project(Point2::new(1.0, 2.0)).param, 0.0);
        }
        let el = ellipse();
        let foot = el.project(Point2::new(1.0, 2.0));
        assert!((foot.param - FRAC_PI_2).abs() <= 1e-15);
        assert!((foot.distance - 2.0).abs() <= 1e-15);
        assert_eq!(el.distance(Point2::new(9.0, 2.0)), 5.0);
    }

    #[test]
    fn bounds() {
        let b = circle(false).bounds([0.0, 1.0]);
        assert_eq!(
            (b.min(), b.max()),
            (Point2::new(-1.0, 0.0), Point2::new(3.0, 4.0))
        );
        let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 1.0), true).unwrap();
        let e = Curve2::Ellipse2(Ellipse2::new(f, 3.0, 1.0).unwrap());
        let reach = 5.0f64.sqrt();
        let b = e.bounds([0.0, 1.0]);
        assert!(near(b.max(), Point2::new(reach, reach)));
        let l = Curve2::Line2(Line2::through(Point2::ORIGIN, Point2::new(3.0, -4.0)).unwrap());
        let b = l.bounds([0.0, 5.0]);
        assert_eq!(
            (b.min(), b.max()),
            (Point2::new(0.0, -4.0), Point2::new(3.0, 0.0))
        );
        let u = b.union(&Box2::from_point(Point2::new(-1.0, 7.0)));
        assert_eq!(
            (u.min(), u.max()),
            (Point2::new(-1.0, -4.0), Point2::new(3.0, 7.0))
        );
    }

    #[test]
    fn constructors_reject_bad_input() {
        let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
        assert_eq!(Circle2::new(f, 0.0), Err(Error::Degenerate));
        assert_eq!(Ellipse2::new(f, 1.0, 2.0), Err(Error::Degenerate));
        assert_eq!(
            Line2::new(Point2::ORIGIN, Vector2::ZERO),
            Err(Error::Degenerate)
        );
    }
}
