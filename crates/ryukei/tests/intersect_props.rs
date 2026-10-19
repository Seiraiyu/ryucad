//! Curve–surface intersections against sampling: every hit lies on the
//! surface, and between hits the curve stays on the side the hits say.

use proptest::prelude::*;
use ryukei::geom::{
    Circle, Cone, Curve, Cylinder, Frame, Line, Plane, Point3, Sphere, Surface, Torus, Vector3,
};
use ryukei::intersect::{CurveHit, Side, curve_surface};
use ryukei::tol::LINEAR_RESOLUTION;

fn unit() -> impl Strategy<Value = Vector3> {
    (-1.0f64..1.0, -1.0f64..1.0, -1.0f64..1.0)
        .prop_filter("a direction", |(x, y, z)| x * x + y * y + z * z > 0.05)
        .prop_map(|(x, y, z)| Vector3::new(x, y, z))
}

fn point() -> impl Strategy<Value = Point3> {
    (-2.0f64..2.0, -2.0f64..2.0, -2.0f64..2.0).prop_map(|(x, y, z)| Point3::new(x, y, z))
}

fn frame() -> impl Strategy<Value = Frame> {
    (point(), unit(), unit()).prop_filter_map("a frame", |(o, z, x)| Frame::new(o, z, x).ok())
}

fn surface() -> impl Strategy<Value = Surface> {
    (frame(), 0usize..5, 0.5f64..2.0, 0.2f64..0.4).prop_map(|(f, kind, r, a)| match kind {
        0 => Surface::Plane(Plane::new(f)),
        1 => Surface::Cylinder(Cylinder::new(f, r).unwrap()),
        2 => Surface::Cone(Cone::new(f, r, a * 2.0).unwrap()),
        3 => Surface::Sphere(Sphere::new(f, r).unwrap()),
        _ => Surface::Torus(Torus::new(f, r + 1.0, r).unwrap()),
    })
}

fn curve() -> impl Strategy<Value = (Curve, [f64; 2])> {
    prop_oneof![
        (point(), unit()).prop_map(|(o, d)| (Curve::Line(Line::new(o, d).unwrap()), [-6.0, 6.0])),
        (frame(), 0.5f64..4.0)
            .prop_map(|(f, r)| (Curve::Circle(Circle::new(f, r).unwrap()), [0.0, 6.0])),
    ]
}

/// The side of the surface `p` is on, when it is clearly off it.
fn side(s: &Surface, p: Point3) -> Option<Side> {
    let foot = s.project(p);
    if foot.distance < 1e-6 {
        return None;
    }
    let (u, v) = foot.param;
    let n = s.normal(u, v)?.get();
    let along = (p - foot.point).dot(n) / foot.distance;
    // Near an axis, a pole or an apex the foot's normal can be off.
    if along.abs() < 0.9 {
        return None;
    }
    Some(if along > 0.0 {
        Side::Positive
    } else {
        Side::Negative
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn hits_agree_with_sampling(s in surface(), (c, range) in curve()) {
        let hits = curve_surface(&c, range, &s, LINEAR_RESOLUTION);
        prop_assert!(hits.is_ok(), "{:?}", hits);
        let hits = hits.unwrap();
        // Each hit on the surface; and the parameter stretches between them,
        // with the sides the hits give either end.
        let mut gaps = Vec::new();
        let mut last = (range[0], Side::Beyond);
        for h in &hits {
            match *h {
                CurveHit::Point { t, point, before, after } => {
                    prop_assert!(s.distance(point) <= 2.0 * LINEAR_RESOLUTION);
                    gaps.push((last.0, t, last.1, before));
                    last = (t, after);
                }
                CurveHit::On { t } => {
                    for k in 0..=8 {
                        let q = c.point(t[0] + (t[1] - t[0]) * f64::from(k) / 8.0);
                        prop_assert!(s.distance(q) <= 2.0 * LINEAR_RESOLUTION);
                    }
                    gaps.push((last.0, t[0], last.1, Side::Beyond));
                    last = (t[1], Side::Beyond);
                }
            }
        }
        gaps.push((last.0, range[1], last.1, Side::Beyond));
        for (a, b, after, before) in gaps {
            prop_assert!(a <= b);
            for k in 1..32 {
                let t = a + (b - a) * f64::from(k) / 32.0;
                let Some(found) = side(&s, c.point(t)) else { continue };
                for want in [after, before] {
                    if want != Side::Beyond {
                        prop_assert_eq!(found, want, "at {} in {}..{}", t, a, b);
                    }
                }
            }
        }
    }
}
