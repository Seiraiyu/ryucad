use super::*;
use crate::geom::{
    Circle, Curve, Frame, KnotVector, NurbsSurface, Revolution, Surface, Torus, Vector3,
};
use crate::math;
use core::f64::consts::TAU;
use std::sync::Arc;

/// A bicubic patch of two by three spans with uneven weights.
fn spline() -> Surface {
    let ku = KnotVector::clamped(3, &[0.0, 0.4, 1.0], &[4, 1, 4]).unwrap();
    let kv = KnotVector::clamped(3, &[0.0, 0.3, 0.7, 1.0], &[4, 1, 1, 4]).unwrap();
    let mut pts = Vec::new();
    let mut w = Vec::new();
    for i in 0..5 {
        for j in 0..6 {
            let (x, y) = (i as f64 * 0.5, j as f64 * 0.4);
            pts.push(Point3::new(
                x,
                y,
                0.3 * math::sin(x * 1.3) * math::cos(y + 0.2),
            ));
            w.push(1.0 + 0.3 * ((i + 2 * j) % 3) as f64);
        }
    }
    Surface::Nurbs(Arc::new(NurbsSurface::new([ku, kv], pts, Some(w)).unwrap()))
}

/// Surfaces with certified projections of their own: a spline, a torus's
/// exact NURBS form, a revolution of a circle and a pipe.
fn surfaces() -> Vec<Surface> {
    let torus = Surface::Torus(Torus::new(Frame::WORLD, 1.5, 0.4).unwrap());
    let (nt, _) = torus.to_nurbs([[0.0, TAU], [0.0, TAU]]).unwrap();
    let meridian = Frame::new(
        Point3::new(2.0, 0.0, 0.3),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let ring = Circle::new(meridian, 0.5).unwrap();
    vec![
        spline(),
        Surface::Nurbs(Arc::new(nt)),
        Surface::Revolution(Arc::new(
            Revolution::new(Frame::WORLD, Curve::Circle(ring)).unwrap(),
        )),
        Surface::Pipe(Arc::new(crate::geom::pipe::tests::on_plane())),
    ]
}

/// The domain cut into four by four cells.
fn cells(domain: Cell) -> Vec<Cell> {
    let mut out = Vec::new();
    let at = |r: [f64; 2], i: usize| r[0] + (r[1] - r[0]) * i as f64 / 4.0;
    for i in 0..4 {
        for j in 0..4 {
            out.push([
                [at(domain[0], i), at(domain[0], i + 1)],
                [at(domain[1], j), at(domain[1], j + 1)],
            ]);
        }
    }
    out
}

/// Points round each surface, near and far, on both sides.
fn points(s: &Surface) -> Vec<Point3> {
    let b = s.bounds();
    let (lo, hi) = (b.min(), b.max());
    let mut out = Vec::new();
    for i in 0..5 {
        for j in 0..5 {
            for k in 0..3 {
                let f = |a: f64, b: f64, t: f64| a + (b - a) * (t * 1.4 - 0.2);
                out.push(Point3::new(
                    f(lo.x, hi.x, f64::from(i) / 4.0 + 0.013),
                    f(lo.y, hi.y, f64::from(j) / 4.0 + 0.029),
                    f(lo.z, hi.z, f64::from(k) / 2.0 + 0.007),
                ));
            }
        }
    }
    out
}

#[test]
fn agrees_with_each_surfaces_own_projection() {
    for s in surfaces() {
        let sheet = Sheet::of(&s);
        let domain = s.domain();
        let starts = cells(domain);
        for p in points(&s) {
            let want = s.project(p);
            let got = nearest(&sheet, domain, s.periods(), &starts, p);
            assert!(
                (got.distance - want.distance).abs() <= 2.0 * LINEAR_RESOLUTION,
                "{s:?} at {p:?}: {} against {}",
                got.distance,
                want.distance
            );
            assert!(s.point(got.param.0, got.param.1).distance(got.point) <= 1e-12);
        }
    }
}

/// Points on the surface project to themselves.
#[test]
fn points_on_the_surface_are_their_own_feet() {
    for s in surfaces() {
        let sheet = Sheet::of(&s);
        let domain = s.domain();
        let starts = cells(domain);
        for (x, y) in [(0.1, 0.2), (0.5, 0.5), (0.93, 0.31)] {
            let u = domain[0][0] + (domain[0][1] - domain[0][0]) * x;
            let v = domain[1][0] + (domain[1][1] - domain[1][0]) * y;
            let p = s.point(u, v);
            let got = nearest(&sheet, domain, s.periods(), &starts, p);
            assert!(got.distance <= LINEAR_RESOLUTION, "{s:?} {x} {y}");
        }
    }
}

/// From the centre of a sphere's spline form every point is equally near:
/// no cell rules any out, the search stops at its cap and settles for a
/// point Newton's method finds, quickly.
#[test]
fn a_centre_every_point_is_equally_near_stops_at_the_cap() {
    let sphere = Surface::Sphere(crate::geom::Sphere::new(Frame::WORLD, 1.0).unwrap());
    let (n, _) = sphere.to_nurbs([[0.0, TAU], [-1.5, 1.5]]).unwrap();
    let s = Surface::Nurbs(Arc::new(n));
    let sheet = Sheet::of(&s);
    let domain = s.domain();
    let start = std::time::Instant::now();
    let got = nearest(&sheet, domain, s.periods(), &cells(domain), Point3::ORIGIN);
    assert!((got.distance - 1.0).abs() <= 1e-12);
    assert!(start.elapsed().as_secs() < 20);
}
