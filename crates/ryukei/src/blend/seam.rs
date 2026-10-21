//! Moving the seam of a closed branch.
//!
//! A traced intersection that closes up (a pipe cut all round, the
//! spine of a small fillet round a pipe's end) is fitted as a clamped
//! spline that starts and ends at the same point, somewhere the tracer
//! chose. A blend that needs a stretch across that point gets it by
//! moving the start: the spline is cut there and its two pieces joined
//! the other way round, the second's parameters a period on. Its pcurves
//! move with it, the second piece shifted by however far the pcurve
//! winds in the surface's parameters over the period (round a pipe, a
//! turn in its angle).

use crate::geom::{Curve, Curve2, KnotVector, NurbsCurve, NurbsCurve2, Point2, Point3, Vector2};
use crate::intersect::Branch;
use crate::tol::LINEAR_RESOLUTION;
use std::sync::Arc;

/// The flat knots and weights of two clamped splines of one degree joined
/// end to start, the second's parameters moved by `shift`: the joint
/// keeps the first's last control point, the second's weights scaled to
/// match it.
fn join(
    degree: usize,
    (ka, wa): (&KnotVector, &[f64]),
    (kb, wb): (&KnotVector, &[f64]),
    shift: f64,
) -> (Vec<f64>, Vec<f64>) {
    let p = degree;
    let (fa, fb) = (ka.flat(), kb.flat());
    let end = fa[fa.len() - 1];
    let mut flat: Vec<f64> = fa[..fa.len() - (p + 1)].to_vec();
    flat.extend(std::iter::repeat_n(end, p));
    flat.extend(fb[p + 1..].iter().map(|k| k + shift));
    let scale = wa[wa.len() - 1] / wb[0];
    let mut weights = wa.to_vec();
    weights.extend(wb[1..].iter().map(|w| w * scale));
    (flat, weights)
}

/// The closed clamped curve over `[t0, t1]`, its ends within resolution,
/// over `[s, s + t1 - t0]` instead.
fn reseam3(c: &NurbsCurve, s: f64) -> Option<NurbsCurve> {
    let [t0, t1] = c.domain();
    let (left, right) = c.split(s).ok()?;
    let (flat, weights) = join(
        c.degree(),
        (right.knots(), right.weights()),
        (left.knots(), left.weights()),
        t1 - t0,
    );
    let mut points: Vec<Point3> = right.control_points().collect();
    points.extend(left.control_points().skip(1));
    let knots = KnotVector::clamped_flat(c.degree(), &flat);
    NurbsCurve::new(knots, points, Some(weights)).ok()
}

/// The clamped plane curve over `[t0, t1]` over `[s, s + t1 - t0]`
/// instead, the piece moved on shifted by `wind`, how far its end is
/// from its start.
fn reseam2(c: &NurbsCurve2, s: f64, wind: Vector2) -> Option<NurbsCurve2> {
    let [t0, t1] = c.domain();
    let (left, right) = c.split(s).ok()?;
    let (flat, weights) = join(
        c.degree(),
        (right.knots(), right.weights()),
        (left.knots(), left.weights()),
        t1 - t0,
    );
    let mut points: Vec<Point2> = right.control_points().collect();
    points.extend(left.control_points().skip(1).map(|p| p + wind));
    let knots = KnotVector::clamped_flat(c.degree(), &flat);
    NurbsCurve2::new(knots, points, Some(weights)).ok()
}

/// `b` with its start moved to the parameter `s` inside its range, when
/// it is a closed clamped spline (its ends within its tolerance) with
/// spline pcurves, or pcurves on a line; otherwise unchanged.
pub(crate) fn reseamed(mut b: Branch, s: f64) -> Branch {
    let [r0, r1] = b.range;
    let tol = b.tolerance.max(LINEAR_RESOLUTION);
    let Curve::Nurbs(n) = &b.curve else {
        return b;
    };
    let edge = (r1 - r0) * 1e-6;
    if n.period().is_some()
        || n.domain() != [r0, r1]
        || n.point(r0).distance(n.point(r1)) > tol
        || !(r0 + edge < s && s < r1 - edge)
    {
        return b;
    }
    let Some(curve) = reseam3(n, s) else {
        return b;
    };
    let mut pcurves = b.pcurves.clone();
    for (pc, m) in &mut pcurves {
        match pc {
            Curve2::Line2(_) => {}
            Curve2::Nurbs2(c) => {
                let [a, z] = [r0, r1].map(|t| m[0] + m[1] * t);
                let wind = c.point(z.max(a)) - c.point(z.min(a));
                if m[1] <= 0.0 || c.domain() != [a, z] {
                    return b;
                }
                let Some(moved) = reseam2(c, m[0] + m[1] * s, wind) else {
                    return b;
                };
                *pc = Curve2::Nurbs2(Arc::new(moved));
            }
            _ => return b,
        }
    }
    b.curve = Curve::Nurbs(Arc::new(curve));
    b.pcurves = pcurves;
    b.range = [s, s + (r1 - r0)];
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Frame};

    #[test]
    fn a_circle_fitted_as_a_spline_starts_where_it_is_told() {
        let circle = Curve::Circle(Circle::new(Frame::WORLD, 1.0).unwrap());
        let n = circle
            .to_nurbs([0.0, core::f64::consts::TAU])
            .unwrap()
            .0
            .to_clamped();
        let [t0, t1] = n.domain();
        let s = t0 + 0.3 * (t1 - t0);
        let moved = reseam3(&n, s).unwrap();
        assert_eq!(moved.domain(), [s, s + (t1 - t0)]);
        for i in 0..=20 {
            let t = s + (t1 - t0) * f64::from(i) / 20.0;
            let back = if t > t1 { t - (t1 - t0) } else { t };
            assert!(moved.point(t).distance(n.point(back)) < 1e-12, "{t}");
        }
    }
}
