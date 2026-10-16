//! Seeded random NURBS curves and surfaces, for property tests, golden
//! sections, fuzzing and benchmarks.

use crate::Lcg;
use ryukei::geom::{KnotVector, NurbsCurve, NurbsCurve2, NurbsSurface, Point2, Point3};

/// A random knot vector of `degree` with `spans` spans of random length
/// starting near 0. Interior multiplicities are 1 to `degree`. A periodic
/// one needs at least `degree + 1` spans, so it always has enough control
/// points.
pub fn knots(g: &mut Lcg, degree: usize, spans: usize, periodic: bool) -> KnotVector {
    assert!(
        !periodic || spans > degree,
        "too few spans for a periodic vector"
    );
    let mut knots = vec![g.next_f64(-2.0, 2.0)];
    for _ in 0..spans {
        let last = knots[knots.len() - 1];
        knots.push(last + g.next_f64(0.2, 1.5));
    }
    let mut mults: Vec<usize> = (0..=spans)
        .map(|_| 1 + (g.next_u64() % degree as u64) as usize)
        .collect();
    if periodic {
        mults[spans] = mults[0];
        KnotVector::periodic(degree, &knots, &mults).unwrap()
    } else {
        mults[0] = degree + 1;
        mults[spans] = degree + 1;
        KnotVector::clamped(degree, &knots, &mults).unwrap()
    }
}

fn point(g: &mut Lcg) -> Point3 {
    Point3::new(
        g.next_f64(-5.0, 5.0),
        g.next_f64(-5.0, 5.0),
        g.next_f64(-5.0, 5.0),
    )
}

/// A random curve: control points in `[-5, 5]³`, weights in `[0.3, 3]`.
pub fn curve(g: &mut Lcg, degree: usize, spans: usize, periodic: bool) -> NurbsCurve {
    let k = knots(g, degree, spans, periodic);
    let n = k.control_count();
    let points = (0..n).map(|_| point(g)).collect();
    let weights = (0..n).map(|_| g.next_f64(0.3, 3.0)).collect();
    NurbsCurve::new(k, points, Some(weights)).unwrap()
}

/// A random curve in the parameter plane, control points in `[-5, 5]²`.
pub fn curve2(g: &mut Lcg, degree: usize, spans: usize, periodic: bool) -> NurbsCurve2 {
    let k = knots(g, degree, spans, periodic);
    let n = k.control_count();
    let points = (0..n)
        .map(|_| Point2::new(g.next_f64(-5.0, 5.0), g.next_f64(-5.0, 5.0)))
        .collect();
    let weights = (0..n).map(|_| g.next_f64(0.3, 3.0)).collect();
    NurbsCurve2::new(k, points, Some(weights)).unwrap()
}

/// Like [`any_curve`], in the plane.
pub fn any_curve2(g: &mut Lcg) -> NurbsCurve2 {
    let degree = 1 + (g.next_u64() % 5) as usize;
    let periodic = g.next_u64().is_multiple_of(4);
    let spans = if periodic { degree + 1 } else { 1 } + (g.next_u64() % 4) as usize;
    curve2(g, degree, spans, periodic)
}

/// A random surface, like [`curve`] in each direction.
pub fn surface(
    g: &mut Lcg,
    degrees: [usize; 2],
    spans: [usize; 2],
    periodic: [bool; 2],
) -> NurbsSurface {
    let ku = knots(g, degrees[0], spans[0], periodic[0]);
    let kv = knots(g, degrees[1], spans[1], periodic[1]);
    let n = ku.control_count() * kv.control_count();
    let points = (0..n).map(|_| point(g)).collect();
    let weights = (0..n).map(|_| g.next_f64(0.3, 3.0)).collect();
    NurbsSurface::new([ku, kv], points, Some(weights)).unwrap()
}

/// Degree 1 to 5, 1 to 4 spans, one in four periodic.
pub fn any_curve(g: &mut Lcg) -> NurbsCurve {
    let degree = 1 + (g.next_u64() % 5) as usize;
    let periodic = g.next_u64().is_multiple_of(4);
    let spans = if periodic { degree + 1 } else { 1 } + (g.next_u64() % 4) as usize;
    curve(g, degree, spans, periodic)
}

/// Degrees 1 to 3, 1 to 3 spans, each direction periodic one time in
/// four.
pub fn any_surface(g: &mut Lcg) -> NurbsSurface {
    let mut dir = || {
        let degree = 1 + (g.next_u64() % 3) as usize;
        let periodic = g.next_u64().is_multiple_of(4);
        let spans = if periodic { degree + 1 } else { 1 } + (g.next_u64() % 3) as usize;
        (degree, spans, periodic)
    };
    let (a, b) = (dir(), dir());
    surface(g, [a.0, b.0], [a.1, b.1], [a.2, b.2])
}

/// A parameter in `[a, b]`.
pub fn param(g: &mut Lcg, [a, b]: [f64; 2]) -> f64 {
    g.next_f64(a, b)
}
