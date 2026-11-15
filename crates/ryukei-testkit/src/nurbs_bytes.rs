//! The `nurbs` fuzz target's cases from bytes (plan 30.D): the target and
//! ryukei's regression tests read the same curves, surfaces and points,
//! so a find replays in release from its bytes.

use crate::euler_script::Bytes;
use ryukei::geom::{KnotVector, NurbsCurve, NurbsSurface, Point3};

/// A case: a curve or a surface, and the point projected onto it.
pub enum Case {
    Curve(NurbsCurve, Point3),
    Surface(NurbsSurface, Point3),
}

/// The case `data` makes: an even first byte a curve, an odd one a
/// surface; `None` when a constructor refuses the rest.
pub fn case(data: &[u8]) -> Option<Case> {
    let (&mode, rest) = data.split_first()?;
    let mut b = Bytes::new(rest);
    if mode.is_multiple_of(2) {
        curve_case(&mut b).map(|(c, p)| Case::Curve(c, p))
    } else {
        surface_case(&mut b).map(|(s, p)| Case::Surface(s, p))
    }
}

/// A knot vector from bytes; sometimes an invalid one.
pub fn knots(b: &mut Bytes, max_degree: usize) -> Option<KnotVector> {
    let periodic = b.byte().is_multiple_of(4);
    let degree = 1 + b.pick(max_degree);
    let spans = 1 + b.pick(5);
    let mut knots = vec![b.range(-3.0, 3.0)];
    for _ in 0..spans {
        let last = knots[knots.len() - 1];
        knots.push(last + b.range(0.0, 2.0));
    }
    let mut mults: Vec<usize> = (0..=spans).map(|_| 1 + b.pick(degree + 1)).collect();
    if !b.byte().is_multiple_of(8) {
        // Mostly well-formed ends.
        if periodic {
            mults[spans] = mults[0];
        } else {
            mults[0] = degree + 1;
            mults[spans] = degree + 1;
        }
    }
    let k = if periodic {
        KnotVector::periodic(degree, &knots, &mults)
    } else {
        KnotVector::clamped(degree, &knots, &mults)
    };
    k.ok()
}

/// A point, now and then outside the size box.
pub fn point(b: &mut Bytes) -> Point3 {
    let r = if b.byte().is_multiple_of(32) { 600.0 } else { 6.0 };
    Point3::new(b.range(-r, r), b.range(-r, r), b.range(-r, r))
}

/// Weights, now and then an invalid one.
pub fn weights(b: &mut Bytes, n: usize) -> Vec<f64> {
    (0..n)
        .map(|_| if b.byte().is_multiple_of(64) { -1.0 } else { b.range(0.05, 5.0) })
        .collect()
}

/// A curve (now and then with one control point too many) and the point
/// projected onto it.
pub fn curve_case(b: &mut Bytes) -> Option<(NurbsCurve, Point3)> {
    let k = knots(b, 6)?;
    let n = k.control_count() + usize::from(b.byte().is_multiple_of(32));
    let points = (0..n).map(|_| point(b)).collect();
    let w = weights(b, n);
    let c = NurbsCurve::new(k, points, Some(w)).ok()?;
    Some((c, point(b)))
}

/// A surface of degree up to 3 each way and the point projected onto it.
pub fn surface_case(b: &mut Bytes) -> Option<(NurbsSurface, Point3)> {
    let (Some(ku), Some(kv)) = (knots(b, 3), knots(b, 3)) else {
        return None;
    };
    let n = ku.control_count() * kv.control_count();
    let points = (0..n).map(|_| point(b)).collect();
    let w = weights(b, n);
    let s = NurbsSurface::new([ku, kv], points, Some(w)).ok()?;
    Some((s, point(b)))
}
