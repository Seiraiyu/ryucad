//! Plane curves under affine maps, and lifted into planes in space.
//!
//! A bend's cylinder parameters `(u, v)` are affine in its strip's
//! `(x, y)`, so a curve on a bend face maps to an exact plane curve of its
//! own kind ([`mapped`]): a line to a line, a NURBS to a NURBS by its
//! control points, and a circle or an ellipse to an ellipse, its parameter
//! moved by a phase. The flat pattern takes the faces' pcurves into its own
//! frame the same way. [`lift`] carries a plane curve into a plane in
//! space, keeping its parameter.

use crate::Error;
use crate::geom::{
    Circle, Circle2, Curve, Curve2, Ellipse, Ellipse2, Frame, Frame2, Line, Line2, NurbsCurve,
    NurbsCurve2, Point2, Point3, Vector2,
};
use crate::math;
use core::f64::consts::FRAC_PI_2;
use std::sync::Arc;

/// An affine map of the plane, `p -> m p + o`, `m` by rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Affine2 {
    pub(crate) m: [[f64; 2]; 2],
    pub(crate) o: Vector2,
}

impl Affine2 {
    /// `(x, y) -> (a[0] + a[1] x, b[0] + b[1] y)`.
    pub(crate) fn diagonal(a: [f64; 2], b: [f64; 2]) -> Self {
        Self {
            m: [[a[1], 0.0], [0.0, b[1]]],
            o: Vector2::new(a[0], b[0]),
        }
    }

    pub(crate) fn vector(&self, v: Vector2) -> Vector2 {
        Vector2::new(
            self.m[0][0] * v.x + self.m[0][1] * v.y,
            self.m[1][0] * v.x + self.m[1][1] * v.y,
        )
    }

    pub(crate) fn point(&self, p: Point2) -> Point2 {
        let v = self.vector(Vector2::new(p.x, p.y)) + self.o;
        Point2::new(v.x, v.y)
    }

    fn det(&self) -> f64 {
        self.m[0][0] * self.m[1][1] - self.m[0][1] * self.m[1][0]
    }

    /// Whether `m` keeps lengths, as closely as the kernel's frames are
    /// unit (`1e-12`).
    fn keeps_lengths(&self) -> bool {
        let [[a, b], [c, d]] = self.m;
        (a * a + c * c - 1.0).abs() <= 1e-12
            && (b * b + d * d - 1.0).abs() <= 1e-12
            && (a * b + c * d).abs() <= 1e-12
    }
}

/// The image of `c` under `a`, and how parameters relate: the image's
/// parameter is `r[0] + r[1] s` at the curve's `s`, with `r[1] > 0`.
/// Fails like the curves' constructors where the map collapses the curve.
pub(crate) fn mapped(c: &Curve2, a: &Affine2) -> Result<(Curve2, [f64; 2]), Error> {
    match c {
        Curve2::Line2(l) => {
            let dir = a.vector(l.dir());
            let speed = dir.length();
            let line = Line2::new(a.point(l.origin()), dir)?;
            Ok((Curve2::Line2(line), [0.0, speed]))
        }
        Curve2::Circle2(k) if a.keeps_lengths() => {
            let f = k.frame();
            let ccw = f.is_ccw() == (a.det() > 0.0);
            let frame = Frame2::new(a.point(f.origin()), a.vector(f.x()), ccw)?;
            Ok((
                Curve2::Circle2(Circle2::new(frame, k.radius())?),
                [0.0, 1.0],
            ))
        }
        Curve2::Circle2(k) => {
            let f = k.frame();
            conic(a, f, k.radius(), k.radius())
        }
        Curve2::Ellipse2(e) => conic(a, e.frame(), e.major(), e.minor()),
        Curve2::Nurbs2(n) => {
            let points = n.control_points().map(|p| a.point(p)).collect();
            let curve = NurbsCurve2::new(n.knots().clone(), points, Some(n.weights().to_vec()))?;
            Ok((Curve2::Nurbs2(Arc::new(curve)), [0.0, 1.0]))
        }
    }
}

/// `origin + p cos t x + q sin t y` mapped: `c + P cos t + Q sin t` with
/// `P`, `Q` the images of the semi-axes, written on its principal axes
/// `A`, `B` as `c + A cos(t - t0) + B sin(t - t0)`.
fn conic(a: &Affine2, f: &Frame2, p: f64, q: f64) -> Result<(Curve2, [f64; 2]), Error> {
    let pv = a.vector(f.x() * p);
    let qv = a.vector(f.y() * q);
    let mut t0 = 0.5 * math::atan2(2.0 * pv.dot(qv), pv.dot(pv) - qv.dot(qv));
    let (s, c) = math::sin_cos(t0);
    let (mut big, mut small) = (pv * c + qv * s, qv * c - pv * s);
    if big.length() < small.length() {
        t0 += FRAC_PI_2;
        (big, small) = (small, -big);
    }
    let ccw = big.cross(small) > 0.0;
    let frame = Frame2::new(a.point(f.origin()), big, ccw)?;
    let ellipse = Ellipse2::new(frame, big.length(), small.length())?;
    Ok((Curve2::Ellipse2(ellipse), [-t0, 1.0]))
}

/// `c` carried into the `xy` plane of `frame`, keeping its parameter.
pub(crate) fn lift(c: &Curve2, frame: &Frame) -> Result<Curve, Error> {
    let at = |p: Point2| frame.to_world(p.x, p.y, 0.0);
    let along = |v: Vector2| frame.x().get() * v.x + frame.y().get() * v.y;
    let in_plane = |f: &Frame2| {
        let z = if f.is_ccw() {
            frame.z().get()
        } else {
            -frame.z().get()
        };
        Frame::new(at(f.origin()), z, along(f.x()))
    };
    Ok(match c {
        Curve2::Line2(l) => Curve::Line(Line::new(at(l.origin()), along(l.dir()))?),
        Curve2::Circle2(k) => Curve::Circle(Circle::new(in_plane(k.frame())?, k.radius())?),
        Curve2::Ellipse2(e) => {
            Curve::Ellipse(Ellipse::new(in_plane(e.frame())?, e.major(), e.minor())?)
        }
        Curve2::Nurbs2(n) => {
            let points: Vec<Point3> = n.control_points().map(at).collect();
            let curve = NurbsCurve::new(n.knots().clone(), points, Some(n.weights().to_vec()))?;
            Curve::Nurbs(Arc::new(curve))
        }
    })
}

/// `c` run backwards, and how parameters relate: the new parameter is
/// `r[0] + r[1] s` at the old `s`, `r[1] < 0`. `None` for a periodic
/// NURBS.
pub(crate) fn reversed(c: &Curve) -> Option<(Curve, [f64; 2])> {
    Some(match c {
        Curve::Line(l) => {
            let line = Line::new(l.origin(), -l.dir().get()).ok()?;
            (Curve::Line(line), [0.0, -1.0])
        }
        Curve::Circle(k) => (
            Curve::Circle(Circle::new(k.frame().flipped(), k.radius()).ok()?),
            [0.0, -1.0],
        ),
        Curve::Ellipse(e) => (
            Curve::Ellipse(Ellipse::new(e.frame().flipped(), e.major(), e.minor()).ok()?),
            [0.0, -1.0],
        ),
        Curve::Nurbs(n) => {
            if n.period().is_some() {
                return None;
            }
            let [a, b] = n.domain();
            (Curve::Nurbs(Arc::new(n.reversed())), [a + b, -1.0])
        }
    })
}

/// A plane's frame in space as an affine map of another's: where the
/// `(u, v)` of a plane on `from` lies in `to`'s `(x, y)`, for frames whose
/// `xy` planes coincide.
pub(crate) fn between(from: &Frame, to: &Frame) -> Affine2 {
    let d = to.to_local(from.origin());
    let x = from.x().get();
    let y = from.y().get();
    let tx = to.x().get();
    let ty = to.y().get();
    Affine2 {
        m: [[x.dot(tx), y.dot(tx)], [x.dot(ty), y.dot(ty)]],
        o: Vector2::new(d.x, d.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame2, Vector3};

    fn near(a: Point2, b: Point2) -> bool {
        a.distance(b) < 1e-12
    }

    /// Every sample of the image is the map of the curve's sample at the
    /// matching parameter.
    fn agrees(c: &Curve2, a: &Affine2, range: [f64; 2]) {
        let (img, [r0, r1]) = mapped(c, a).unwrap();
        assert!(r1 > 0.0);
        for i in 0..=16 {
            let s = range[0] + (range[1] - range[0]) * f64::from(i) / 16.0;
            let want = a.point(c.point(s));
            let got = img.point(r0 + r1 * s);
            assert!(near(want, got), "{want:?} against {got:?} at {s}");
        }
    }

    #[test]
    fn lines_circles_ellipses_and_splines_map_exactly() {
        let squash = Affine2::diagonal([0.5, 2.5], [-1.0, 0.75]);
        let turn = Affine2 {
            m: [[0.6, -0.8], [0.8, 0.6]],
            o: Vector2::new(3.0, -2.0),
        };
        let mirror = Affine2 {
            m: [[-1.0, 0.0], [0.0, 1.0]],
            o: Vector2::new(1.0, 0.0),
        };
        let frame = Frame2::new(Point2::new(0.2, -0.3), Vector2::new(1.0, 1.0), true).unwrap();
        let cw = Frame2::new(Point2::new(0.2, -0.3), Vector2::new(1.0, -2.0), false).unwrap();
        let line =
            Curve2::Line2(Line2::new(Point2::new(1.0, 2.0), Vector2::new(3.0, -1.0)).unwrap());
        let circle = Curve2::Circle2(Circle2::new(frame, 0.7).unwrap());
        let ellipse = Curve2::Ellipse2(Ellipse2::new(cw, 0.9, 0.4).unwrap());
        let (spline, _) = circle.to_nurbs([0.0, 1.0]).unwrap();
        let spline = Curve2::Nurbs2(Arc::new(spline));
        for a in [&squash, &turn, &mirror] {
            agrees(&line, a, [-2.0, 3.0]);
            agrees(&circle, a, [0.0, 6.0]);
            agrees(&ellipse, a, [-1.0, 4.0]);
            agrees(&spline, a, [0.5, 5.5]);
        }
        // A rigid map keeps a circle a circle.
        assert!(matches!(
            mapped(&circle, &turn).unwrap().0,
            Curve2::Circle2(_)
        ));
        assert!(matches!(
            mapped(&circle, &mirror).unwrap().0,
            Curve2::Circle2(_)
        ));
        assert!(matches!(
            mapped(&circle, &squash).unwrap().0,
            Curve2::Ellipse2(_)
        ));
    }

    #[test]
    fn lifted_curves_keep_their_parameters_and_reverse() {
        let frame = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 1.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let f2 = Frame2::new(Point2::new(0.2, -0.3), Vector2::new(1.0, 1.0), false).unwrap();
        let curves = [
            Curve2::Line2(Line2::new(Point2::new(1.0, 2.0), Vector2::new(3.0, -1.0)).unwrap()),
            Curve2::Circle2(Circle2::new(f2, 0.7).unwrap()),
            Curve2::Ellipse2(Ellipse2::new(f2, 0.9, 0.4).unwrap()),
        ];
        for c in &curves {
            let lifted = lift(c, &frame).unwrap();
            for i in 0..=8 {
                let s = f64::from(i) * 0.7 - 2.0;
                let p = c.point(s);
                let want = frame.to_world(p.x, p.y, 0.0);
                assert!(lifted.point(s).distance(want) < 1e-12);
            }
            let (back, [r0, r1]) = reversed(&lifted).unwrap();
            assert!(r1 < 0.0);
            for s in [-1.0, 0.0, 0.3, 2.0] {
                assert!(back.point(r0 + r1 * s).distance(lifted.point(s)) < 1e-12);
            }
        }
    }

    #[test]
    fn frames_in_one_plane_relate_by_a_rigid_map() {
        let to = Frame::new(
            Point3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let from = Frame::new(
            Point3::new(2.0, 1.0, 1.0),
            Vector3::new(0.0, 0.0, -1.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        let a = between(&from, &to);
        for (u, v) in [(0.0, 0.0), (1.0, -2.0), (0.5, 0.25)] {
            let p = from.to_world(u, v, 0.0);
            let q = to.to_local(p);
            let got = a.point(Point2::new(u, v));
            assert!(near(got, Point2::new(q.x, q.y)));
        }
    }
}
