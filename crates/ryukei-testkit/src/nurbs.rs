//! Seeded random NURBS curves and surfaces, for property tests, golden
//! sections, fuzzing and benchmarks.

use crate::Lcg;
use ryukei::Error;
use ryukei::euler;
use ryukei::geom::{
    Curve, KnotVector, NurbsCurve, NurbsCurve2, NurbsSurface, Point2, Point3, Surface,
};
use ryukei::topo::{Body, EdgeId, FaceId};
use std::sync::Arc;

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

/// A random closed blob in the plane: a periodic quadratic with 8 to 12
/// control points on the circle of `radius` about `centre`, at increasing
/// angles each within a quarter step of even spacing, with weights in
/// `[0.5, 2]` and knots `0, 1, ..., n`. The control polygon is convex, so
/// the curve is a simple convex loop inside the circle; each point is a
/// combination of three consecutive control points, so it keeps more than
/// half the radius from the centre.
pub fn blob2(g: &mut Lcg, centre: Point2, radius: f64) -> NurbsCurve2 {
    let n = 8 + (g.next_u64() % 5) as usize;
    let step = core::f64::consts::TAU / n as f64;
    let points = (0..n)
        .map(|i| {
            let angle = step * (i as f64 + g.next_f64(-0.25, 0.25));
            let (s, c) = ryukei::math::sin_cos(angle);
            Point2::new(centre.x + radius * c, centre.y + radius * s)
        })
        .collect();
    let weights = (0..n).map(|_| g.next_f64(0.5, 2.0)).collect();
    let knots: Vec<f64> = (0..=n).map(|i| i as f64).collect();
    let k = KnotVector::periodic(2, &knots, &vec![1; n + 1]).unwrap();
    NurbsCurve2::new(k, points, Some(weights)).unwrap()
}

/// Every curve and surface of `b` replaced by its exact NURBS form through
/// the geometry setters. Edges keep their ends (the reparameterization
/// maps their intervals) and faces their senses. Open surfaces are
/// converted over a range a little larger than the face's boundary
/// reaches, cut to the surface's domain.
pub fn convert_body(b: &mut Body) -> Result<(), Error> {
    let edges: Vec<EdgeId> = b.edges().collect();
    for e in edges {
        let edge = b.edge(e).expect("live edge");
        let [t0, t1] = edge.interval;
        let curve = b.curve(edge.curve).expect("live curve").clone();
        let (n, r) = curve.to_nurbs([t0, t1])?;
        let id = b.add_curve(Curve::Nurbs(Arc::new(n)));
        euler::set_curve(b, e, id, [r.to_nurbs(t0), r.to_nurbs(t1)])?;
    }
    let faces: Vec<FaceId> = b.faces().collect();
    for f in faces {
        let face = b.face(f).expect("live face");
        let (surface, sense) = (
            b.surface(face.surface).expect("live surface").clone(),
            face.sense,
        );
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        let mut reach = |p: Point3| {
            let (u, v) = surface.project(p).param;
            for (k, x) in [u, v].into_iter().enumerate() {
                lo[k] = lo[k].min(x);
                hi[k] = hi[k].max(x);
            }
        };
        for l in b.face_loops(f) {
            let lp = b.loop_(l).expect("live loop");
            if let Some(v) = lp.vertex {
                reach(b.vertex(v).expect("live vertex").point);
            }
            for fin in b.loop_fins(l) {
                let edge = b
                    .edge(b.fin(fin).expect("live fin").edge)
                    .expect("live edge");
                let curve = b.curve(edge.curve).expect("live curve");
                let [t0, t1] = edge.interval;
                for i in 0..=8 {
                    reach(curve.point(t0 + (t1 - t0) * f64::from(i) / 8.0));
                }
            }
        }
        let domain = surface.domain();
        let ranges: [[f64; 2]; 2] = core::array::from_fn(|k| {
            let pad = 1e-3 * (hi[k] - lo[k]).max(0.0) + 1e-6;
            [
                (lo[k] - pad).max(domain[k][0]),
                (hi[k] + pad).min(domain[k][1]),
            ]
        });
        let (n, _) = surface.to_nurbs(ranges)?;
        let id = b.add_surface(Surface::Nurbs(Arc::new(n)));
        euler::set_surface(b, f, id, sense)?;
    }
    Ok(())
}
