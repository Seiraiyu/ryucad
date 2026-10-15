//! Each closed-form distance and nearest parameter must agree with brute
//! force: no sampled point of the curve or surface is closer than the
//! closed-form distance, and the closed-form nearest point attains it.

use core::f64::consts::TAU;
use proptest::prelude::*;
use ryukei::geom::{
    Circle, Cone, Curve, Cylinder, Frame, Line, Plane, Point3, Sphere, Surface, Torus, Vector3,
};

/// Rounding allowance at these sizes (coordinates up to ~30 m).
const EPS: f64 = 1e-12;
const STEPS: usize = 48;

fn coord() -> impl Strategy<Value = f64> {
    -10.0f64..10.0
}

fn point() -> impl Strategy<Value = Point3> {
    (coord(), coord(), coord()).prop_map(|(x, y, z)| Point3::new(x, y, z))
}

fn frame() -> impl Strategy<Value = Frame> {
    (point(), point(), point()).prop_filter_map("degenerate frame", |(o, z, x)| {
        Frame::new(o, z.to_vector(), x.to_vector()).ok()
    })
}

fn radius() -> impl Strategy<Value = f64> {
    0.1f64..5.0
}

fn curve() -> impl Strategy<Value = Curve> {
    prop_oneof![
        (point(), point()).prop_filter_map("degenerate line", |(o, d)| {
            Line::new(o, d.to_vector()).ok().map(Curve::Line)
        }),
        (frame(), radius()).prop_map(|(f, r)| Curve::Circle(Circle::new(f, r).unwrap())),
    ]
}

fn surface() -> impl Strategy<Value = Surface> {
    prop_oneof![
        frame().prop_map(|f| Surface::Plane(Plane::new(f))),
        (frame(), radius()).prop_map(|(f, r)| Surface::Cylinder(Cylinder::new(f, r).unwrap())),
        (frame(), radius(), 0.05f64..1.5)
            .prop_map(|(f, r, a)| Surface::Cone(Cone::new(f, r, a).unwrap())),
        (frame(), radius()).prop_map(|(f, r)| Surface::Sphere(Sphere::new(f, r).unwrap())),
        (frame(), radius(), 0.05f64..0.95)
            .prop_map(|(f, big, k)| { Surface::Torus(Torus::new(f, big, big * k).unwrap()) }),
    ]
}

/// A parameter range to sample: around the nearest parameter for open
/// directions, the whole period for closed ones.
fn range(period: Option<f64>, near: f64, lowest: f64) -> (f64, f64) {
    match period {
        Some(p) => (0.0, p),
        None => ((near - 20.0).max(lowest), near + 20.0),
    }
}

fn samples(lo: f64, hi: f64) -> impl Iterator<Item = f64> {
    (0..=STEPS).map(move |i| lo + (hi - lo) * i as f64 / STEPS as f64)
}

proptest! {
    #[test]
    fn curve_distance_is_attained_and_minimal(c in curve(), p in point()) {
        let d = c.distance(p);
        let foot = c.point(c.param_of(p));
        prop_assert!((foot.distance(p) - d).abs() <= EPS, "{c:?} {p:?}");
        let (lo, hi) = range(c.period(), c.param_of(p), f64::NEG_INFINITY);
        for t in samples(lo, hi) {
            prop_assert!(c.point(t).distance(p) >= d - EPS, "{c:?} {p:?} t={t}");
        }
    }

    #[test]
    fn surface_distance_is_attained_and_minimal(s in surface(), p in point()) {
        let d = s.distance(p);
        let (u, v) = s.params_of(p);
        prop_assert!((s.point(u, v).distance(p) - d).abs() <= EPS, "{s:?} {p:?}");
        let lowest = match s {
            Surface::Cone(c) => c.apex_v(),
            Surface::Sphere(_) => -TAU / 4.0,
            _ => f64::NEG_INFINITY,
        };
        let (pu, pv) = s.periods();
        let (ulo, uhi) = range(pu, u, f64::NEG_INFINITY);
        let (vlo, vhi) = match s {
            Surface::Sphere(_) => (-TAU / 4.0, TAU / 4.0),
            _ => range(pv, v, lowest),
        };
        for a in samples(ulo, uhi) {
            for b in samples(vlo, vhi) {
                prop_assert!(s.point(a, b).distance(p) >= d - EPS, "{s:?} {p:?} ({a}, {b})");
            }
        }
    }

    #[test]
    fn normals_are_unit_and_perpendicular_to_the_surface(s in surface(), u in 0.0f64..TAU, v in -1.0f64..1.0) {
        let n = s.normal(u, v).get();
        prop_assert!((n.length() - 1.0).abs() <= 1e-15);
        // A small step along the surface stays perpendicular to the normal
        // to first order.
        let h = 1e-6;
        let p = s.point(u, v);
        for q in [s.point(u + h, v), s.point(u, v + h)] {
            let step: Vector3 = q - p;
            prop_assert!(step.dot(n).abs() <= 1e-10, "{s:?} ({u}, {v})");
        }
    }
}
