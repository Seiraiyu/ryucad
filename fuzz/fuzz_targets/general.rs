//! Intersections where a surface has no implicit form, from bytes: pairs
//! of the analytic surfaces' exact NURBS forms placed on a coarse grid (so
//! that coincident, tangent and coaxial pairs come up often), smooth
//! random spline patches (height fields over a grid of a frame's plane),
//! and extrusions and revolutions of circles and ellipses, and a line or
//! circle against the first. Every result must be a structured failure,
//! or lie where it says: each branch, and each of its pcurves taken onto
//! its surface, within the branch's tolerance of both surfaces, each
//! point where they touch within ten resolutions of both, and each curve
//! hit within the tolerance of the surface. No pair is unsupported, and
//! any panic is a bug.

#![no_main]

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use libfuzzer_sys::fuzz_target;
use ryukei::geom::{
    Box3, Circle, Cone, Curve, Cylinder, Ellipse, Extrusion, Frame, KnotVector, Line, NurbsSurface,
    Plane, Point3, Revolution, Sphere, Surface, Torus, Vector3,
};
use ryukei::intersect::{CurveHit, Failure, SurfaceHit, curve_surface, surface_surface};
use ryukei::tol::LINEAR_RESOLUTION;
use ryukei_testkit::euler_script::Bytes;
use std::sync::Arc;

/// A grid value: one of `n` steps across `[lo, hi]`.
fn grid(b: &mut Bytes, lo: f64, hi: f64, n: usize) -> f64 {
    lo + (hi - lo) * b.pick(n + 1) as f64 / n as f64
}

/// A direction on a coarse grid of the sphere, mostly along an axis.
fn direction(b: &mut Bytes) -> Vector3 {
    match b.pick(4) {
        0 => Vector3::new(0.0, 0.0, 1.0),
        1 => Vector3::new(1.0, 0.0, 0.0),
        2 => Vector3::new(0.0, 1.0, 0.0),
        _ => {
            let (lon, lat) = (grid(b, 0.0, TAU, 8), grid(b, -FRAC_PI_2, FRAC_PI_2, 4));
            let (sl, cl) = ryukei::math::sin_cos(lon);
            let (sa, ca) = ryukei::math::sin_cos(lat);
            Vector3::new(ca * cl, ca * sl, sa)
        }
    }
}

fn frame(b: &mut Bytes) -> Option<Frame> {
    let o = Point3::new(
        grid(b, -1.0, 1.0, 4),
        grid(b, -1.0, 1.0, 4),
        grid(b, -1.0, 1.0, 4),
    );
    let z = direction(b);
    Frame::new(o, z, direction(b))
        .or_else(|_| Frame::new(o, z, Vector3::new(1.0, 2.0, 3.0)))
        .ok()
}

/// An analytic surface's exact NURBS form over a box of its parameters
/// near its frame.
fn spline_of_analytic(b: &mut Bytes) -> Option<Surface> {
    let f = frame(b)?;
    let r = grid(b, 0.5, 2.0, 3);
    let (s, range) = match b.pick(5) {
        0 => (Surface::Plane(Plane::new(f)), [[-2.0, 2.0], [-2.0, 2.0]]),
        1 => (
            Surface::Cylinder(Cylinder::new(f, r).ok()?),
            [[0.0, TAU], [-2.0, 2.0]],
        ),
        2 => {
            let c = Cone::new(f, r, PI / grid(b, 4.0, 8.0, 4)).ok()?;
            let a = c.apex_v();
            (Surface::Cone(c), [[0.0, TAU], [a + 0.1, a + 4.0]])
        }
        3 => (
            Surface::Sphere(Sphere::new(f, r).ok()?),
            [[0.0, TAU], [-1.4, 1.4]],
        ),
        _ => (
            Surface::Torus(Torus::new(f, r + grid(b, 0.5, 2.0, 3), r).ok()?),
            [[0.0, TAU], [0.0, TAU]],
        ),
    };
    let (n, _) = s.to_nurbs(range).ok()?;
    Some(Surface::Nurbs(Arc::new(n)))
}

/// A smooth spline patch: heights from the bytes over an even grid of a
/// frame's plane, degree 2 or 3 each way.
fn height_field(b: &mut Bytes) -> Option<Surface> {
    let f = frame(b)?;
    let degree = 2 + b.pick(2);
    let spans = 1 + b.pick(3);
    let n = degree + spans;
    let knots: Vec<f64> = (0..=spans).map(|k| k as f64).collect();
    let mut mults = vec![1; spans + 1];
    mults[0] = degree + 1;
    mults[spans] = degree + 1;
    let kv = KnotVector::clamped(degree, &knots, &mults).ok()?;
    let side = 3.0 / (n - 1) as f64;
    let mut points = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            let h = grid(b, -0.6, 0.6, 6);
            points.push(f.to_world(-1.5 + side * i as f64, -1.5 + side * j as f64, h));
        }
    }
    let s = NurbsSurface::new([kv.clone(), kv], points, None).ok()?;
    Some(Surface::Nurbs(Arc::new(s)))
}

/// An extrusion or a revolution of a circle or an ellipse (as itself or
/// its exact NURBS form).
fn swept(b: &mut Bytes) -> Option<Surface> {
    let f = frame(b)?;
    let r = grid(b, 0.4, 1.2, 4);
    if b.pick(2) == 0 {
        let c = match b.pick(3) {
            0 => Curve::Circle(Circle::new(f, r).ok()?),
            1 => Curve::Ellipse(Ellipse::new(f, r * 1.5, r).ok()?),
            _ => Curve::Nurbs(Arc::new(Circle::new(f, r).ok()?.to_nurbs().ok()?.0)),
        };
        let d = f.z().get() + f.x().get() * grid(b, -0.5, 0.5, 4);
        Some(Surface::Extrusion(Arc::new(Extrusion::new(c, d).ok()?)))
    } else {
        // A circle in the frame's xz half-plane, off its axis.
        let centre = f.origin() + f.x().get() * (r + grid(b, 0.2, 1.0, 4));
        let meridian = Frame::new(centre, -f.y().get(), f.x().get()).ok()?;
        let c = Curve::Circle(Circle::new(meridian, r).ok()?);
        Some(Surface::Revolution(Arc::new(Revolution::new(f, c).ok()?)))
    }
}

fn surface(b: &mut Bytes) -> Option<Surface> {
    match b.pick(3) {
        0 => spline_of_analytic(b),
        1 => height_field(b),
        _ => swept(b),
    }
}

fn curve(b: &mut Bytes) -> Option<(Curve, [f64; 2])> {
    let f = frame(b)?;
    let r = grid(b, 0.5, 3.0, 5);
    Some(match b.pick(2) {
        0 => (
            Curve::Line(Line::new(f.origin(), f.z().get()).ok()?),
            [-4.0, 4.0],
        ),
        _ => (Curve::Circle(Circle::new(f, r).ok()?), [0.0, TAU]),
    })
}

fuzz_target!(|data: &[u8]| {
    let mut b = Bytes::new(data);
    let (Some(s1), Some(s2)) = (surface(&mut b), surface(&mut b)) else {
        return;
    };
    let region = Box3::new(Point3::new(-5.0, -5.0, -5.0), Point3::new(5.0, 5.0, 5.0));
    match surface_surface(&s1, &s2, &region) {
        Ok(SurfaceHit::Meet { branches, points }) => {
            for br in &branches {
                let [r0, r1] = br.range;
                for k in 0..=16 {
                    let t = r0 + (r1 - r0) * f64::from(k) / 16.0;
                    let p = br.curve.point(t);
                    for (s, (c2, map)) in [&s1, &s2].into_iter().zip(&br.pcurves) {
                        assert!(s.distance(p) <= br.tolerance + 1e-12);
                        let q = c2.point(map[0] + map[1] * t);
                        assert!(s.point(q.x, q.y).distance(p) <= br.tolerance + 1e-12);
                    }
                }
            }
            for p in points {
                assert!(s1.distance(p) <= 10.0 * LINEAR_RESOLUTION);
                assert!(s2.distance(p) <= 10.0 * LINEAR_RESOLUTION);
            }
        }
        Ok(SurfaceHit::Coincident { .. }) | Err(Failure::Uncertain) => {}
        Err(Failure::Unsupported) => panic!("every surface is supported"),
    }
    let Some((c, range)) = curve(&mut b) else {
        return;
    };
    match curve_surface(&c, range, &s1, LINEAR_RESOLUTION) {
        Ok(hits) => {
            for h in hits {
                match h {
                    CurveHit::Point { point, .. } => {
                        assert!(s1.distance(point) <= 2.0 * LINEAR_RESOLUTION)
                    }
                    CurveHit::On { t } => {
                        for k in 0..=8 {
                            let p = c.point(t[0] + (t[1] - t[0]) * f64::from(k) / 8.0);
                            assert!(s1.distance(p) <= 2.0 * LINEAR_RESOLUTION);
                        }
                    }
                }
            }
        }
        Err(Failure::Uncertain) => {}
        Err(Failure::Unsupported) => panic!("every surface is supported"),
    }
});
