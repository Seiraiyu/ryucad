//! Intersections from bytes: two analytic surfaces placed on a coarse grid
//! of positions, axes, radii and angles, so that coaxial, parallel,
//! coincident and tangent pairs come up often, and a line, circle or
//! ellipse against the first of them. Every result must be a structured
//! failure, or lie where it says: each branch, and each of its pcurves
//! taken onto its surface, within the branch's tolerance of both
//! surfaces, and each curve hit within the tolerance of the surface. Any
//! panic is a bug.

#![no_main]

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use libfuzzer_sys::fuzz_target;
use ryukei::geom::{
    Box3, Circle, Cone, Curve, Cylinder, Ellipse, Frame, Line, Plane, Point3, Sphere, Surface,
    Torus, Vector3,
};
use ryukei::intersect::{CurveHit, Failure, SurfaceHit, curve_surface, surface_surface};
use ryukei::tol::LINEAR_RESOLUTION;
use ryukei_testkit::euler_script::Bytes;

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

fn surface(b: &mut Bytes) -> Option<Surface> {
    let f = frame(b)?;
    let r = grid(b, 0.5, 2.0, 3);
    Some(match b.pick(5) {
        0 => Surface::Plane(Plane::new(f)),
        1 => Surface::Cylinder(Cylinder::new(f, r).ok()?),
        2 => Surface::Cone(Cone::new(f, r, PI / grid(b, 4.0, 8.0, 4)).ok()?),
        3 => Surface::Sphere(Sphere::new(f, r).ok()?),
        _ => Surface::Torus(Torus::new(f, r + grid(b, 0.5, 2.0, 3), r).ok()?),
    })
}

fn curve(b: &mut Bytes) -> Option<(Curve, [f64; 2])> {
    let f = frame(b)?;
    let r = grid(b, 0.5, 3.0, 5);
    Some(match b.pick(3) {
        0 => (
            Curve::Line(Line::new(f.origin(), f.z().get()).ok()?),
            [-4.0, 4.0],
        ),
        1 => (Curve::Circle(Circle::new(f, r).ok()?), [0.0, TAU]),
        _ => (
            Curve::Ellipse(Ellipse::new(f, r, r / grid(b, 1.0, 3.0, 4)).ok()?),
            [0.0, TAU],
        ),
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
        Err(Failure::Unsupported) => panic!("analytic surfaces are supported"),
    }
    let Some((c, range)) = curve(&mut b) else {
        return;
    };
    if let Ok(hits) = curve_surface(&c, range, &s1, LINEAR_RESOLUTION) {
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
});
