//! Profile curves placed in model space.

use crate::Error;
use crate::geom::{
    Circle, Curve, Curve2, Ellipse, Frame, Frame2, Line, NurbsCurve, NurbsCurve2, Point2, Point3,
    Rigid, Vector2, Vector3,
};
use std::sync::Arc;

/// The model point of a profile point.
pub(crate) fn point(frame: &Frame, p: Point2) -> Point3 {
    frame.to_world(p.x, p.y, 0.0)
}

/// The model direction of a profile direction.
pub(crate) fn vector(frame: &Frame, v: Vector2) -> Vector3 {
    frame.x().get() * v.x + frame.y().get() * v.y
}

/// A circle's or ellipse's model frame: its centre and `x`, with `z` the
/// profile's normal when the curve's own frame turns counterclockwise.
fn frame3(frame: &Frame, f: &Frame2, reverse: bool) -> Result<Frame, Error> {
    let z = frame.z().get();
    let z = if f.is_ccw() != reverse { z } else { -z };
    Frame::new(point(frame, f.origin()), z, vector(frame, f.x()))
}

/// A NURBS in the profile plane, placed.
fn nurbs3(frame: &Frame, n: &NurbsCurve2) -> Result<NurbsCurve, Error> {
    let points = n.control_points().map(|p| point(frame, p)).collect();
    NurbsCurve::new(n.knots().clone(), points, Some(n.weights().to_vec()))
}

/// A whole profile curve placed in model space, with the same parameter.
pub(crate) fn curve(frame: &Frame, c: &Curve2) -> Result<Curve, Error> {
    Ok(match c {
        Curve2::Line2(l) => {
            Curve::Line(Line::new(point(frame, l.origin()), vector(frame, l.dir()))?)
        }
        Curve2::Circle2(c) => {
            Curve::Circle(Circle::new(frame3(frame, c.frame(), false)?, c.radius())?)
        }
        Curve2::Ellipse2(e) => Curve::Ellipse(Ellipse::new(
            frame3(frame, e.frame(), false)?,
            e.major(),
            e.minor(),
        )?),
        Curve2::Nurbs2(n) => Curve::Nurbs(Arc::new(nurbs3(frame, n)?)),
    })
}

/// The part of a profile curve from `t0` to `t1` placed in model space and
/// moved by `at`, as a curve and an increasing interval that run from
/// `curve(t0)` to `curve(t1)`, either way round. A line becomes the chord
/// between its ends; a circle or an ellipse run backwards turns its frame
/// over; a NURBS is cut to the part, and reversed if need be.
pub(crate) fn span(
    frame: &Frame,
    c: &Curve2,
    [t0, t1]: [f64; 2],
    at: &Rigid,
) -> Result<(Curve, [f64; 2]), Error> {
    let reverse = t1 < t0;
    let (curve, interval) = match c {
        Curve2::Line2(_) => {
            let (p, q) = (point(frame, c.point(t0)), point(frame, c.point(t1)));
            (Curve::Line(Line::through(p, q)?), [0.0, p.distance(q)])
        }
        Curve2::Circle2(k) => {
            let f = frame3(frame, k.frame(), reverse)?;
            let interval = if reverse { [-t0, -t1] } else { [t0, t1] };
            (Curve::Circle(Circle::new(f, k.radius())?), interval)
        }
        Curve2::Ellipse2(e) => {
            let f = frame3(frame, e.frame(), reverse)?;
            let interval = if reverse { [-t0, -t1] } else { [t0, t1] };
            (
                Curve::Ellipse(Ellipse::new(f, e.major(), e.minor())?),
                interval,
            )
        }
        Curve2::Nurbs2(n) => {
            let part = n.part([t0.min(t1), t0.max(t1)])?;
            let part = if reverse { part.reversed() } else { part };
            let interval = part.domain();
            (Curve::Nurbs(Arc::new(nurbs3(frame, &part)?)), interval)
        }
    };
    Ok((curve.transformed(at), interval))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle2, Ellipse2, KnotVector, Line2};
    use core::f64::consts::{FRAC_PI_2, PI};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, -2.0, 3.0),
            Vector3::new(0.2, 0.3, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    }

    fn curves() -> Vec<Curve2> {
        let f2 = |ccw| Frame2::new(Point2::new(0.5, 0.25), Vector2::new(1.0, 1.0), ccw).unwrap();
        let knots = KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 1, 3]).unwrap();
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 2.0),
            Point2::new(2.0, -1.0),
            Point2::new(3.0, 0.5),
        ];
        let n = NurbsCurve2::new(knots, pts, Some(vec![1.0, 0.5, 2.0, 1.0])).unwrap();
        vec![
            Curve2::Line2(Line2::new(Point2::new(0.0, 1.0), Vector2::new(3.0, 4.0)).unwrap()),
            Curve2::Circle2(Circle2::new(f2(true), 2.0).unwrap()),
            Curve2::Circle2(Circle2::new(f2(false), 2.0).unwrap()),
            Curve2::Ellipse2(Ellipse2::new(f2(false), 2.0, 1.0).unwrap()),
            Curve2::Nurbs2(Arc::new(n)),
        ]
    }

    #[test]
    fn whole_curves_keep_their_parameter() {
        let f = frame();
        for c in curves() {
            let placed = curve(&f, &c).unwrap();
            for t in [0.0, 0.3, 1.1, 1.9] {
                assert!(
                    placed.point(t).distance(point(&f, c.point(t))) <= 1e-14,
                    "{c:?}"
                );
            }
        }
    }

    #[test]
    fn spans_run_from_start_to_end_either_way() {
        let f = frame();
        let at = Rigid::translation(Vector3::new(0.0, 0.0, 2.0));
        for c in curves() {
            for range in [[0.2, 1.7], [1.7, 0.2], [PI, -FRAC_PI_2 + 0.1]] {
                if matches!(c, Curve2::Nurbs2(_))
                    && range.iter().any(|&t| !(0.0..=2.0).contains(&t))
                {
                    continue;
                }
                let (placed, [a, b]) = span(&f, &c, range, &at).unwrap();
                assert!(a < b, "{c:?} {range:?}");
                for (t, s) in [(range[0], a), (range[1], b)] {
                    let want = at.apply_point(point(&f, c.point(t)));
                    assert!(placed.point(s).distance(want) <= 1e-13, "{c:?} {range:?}");
                }
                let mid = placed.point((a + b) / 2.0);
                let on = at.inverse().apply_point(mid);
                let local = f.to_local(on);
                assert!(local.z.abs() <= 1e-13);
                let p = Point2::new(local.x, local.y);
                assert!(c.distance(p) <= 1e-13, "{c:?}");
            }
        }
    }
}
