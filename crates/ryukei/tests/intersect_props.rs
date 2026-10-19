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

/// A surface of each kind about `axis`'s frame, offset along it.
fn about(axis: Frame, kind: usize, r: f64, a: f64, shift: f64) -> Surface {
    let f = Frame::new(
        axis.origin() + axis.z().get() * shift,
        axis.z().get(),
        axis.x().get(),
    )
    .unwrap();
    match kind {
        0 => Surface::Plane(Plane::new(f)),
        1 => Surface::Cylinder(Cylinder::new(f, r).unwrap()),
        2 => Surface::Cone(Cone::new(f, r, a * 2.0).unwrap()),
        3 => Surface::Sphere(Sphere::new(f, r).unwrap()),
        _ => Surface::Torus(Torus::new(f, r + 1.0, r).unwrap()),
    }
}

fn coaxial() -> impl Strategy<Value = (Surface, Surface)> {
    (
        frame(),
        (0usize..5, 0.5f64..2.0, 0.2f64..0.4, -1.0f64..1.0),
        (0usize..5, 0.5f64..2.0, 0.2f64..0.4, -1.0f64..1.0),
    )
        .prop_filter("not two planes", |(_, a, b)| a.0 + b.0 > 0)
        .prop_map(|(f, a, b)| (about(f, a.0, a.1, a.2, a.3), about(f, b.0, b.1, b.2, b.3)))
}

/// Points on `s` on a grid of its parameters, row by row.
fn grid(s: &Surface, n: usize) -> Vec<Vec<Point3>> {
    let [[u0, u1], [v0, v1]] = s.domain();
    let clamp = |a: f64, b: f64| (a.max(-3.0), b.min(a.max(-3.0) + 6.0));
    let (u0, u1) = clamp(u0, u1);
    let (v0, v1) = clamp(v0, v1);
    (0..=n)
        .map(|i| {
            (0..=n)
                .map(|j| {
                    let u = u0 + (u1 - u0) * i as f64 / n as f64;
                    let v = v0 + (v1 - v0) * j as f64 / n as f64;
                    s.point(u, v)
                })
                .collect()
        })
        .collect()
}

/// Every place sampling sees `a` cross `b` is near a branch or a point.
fn complete(
    a: &Surface,
    b: &Surface,
    branches: &[ryukei::intersect::Branch],
    points: &[Point3],
) -> Result<(), TestCaseError> {
    let rows = grid(a, 60);
    let near = |p: Point3, reach: f64| {
        branches.iter().any(|br| br.curve.distance(p) <= reach)
            || points.iter().any(|q| q.distance(p) <= reach)
    };
    for (i, row) in rows.iter().enumerate() {
        for (j, &p) in row.iter().enumerate() {
            for q in [row.get(j + 1), rows.get(i + 1).map(|r| &r[j])]
                .into_iter()
                .flatten()
            {
                let (Some(sp), Some(sq)) = (side(b, p), side(b, *q)) else {
                    continue;
                };
                if sp != sq {
                    let m = p + (*q - p) * 0.5;
                    let reach = p.distance(*q);
                    prop_assert!(near(m, reach), "missed a crossing near {:?}", m);
                }
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn closed_forms_agree_with_sampling((a, b) in prop_oneof![coaxial(), (surface(), surface())]) {
        let region = ryukei::geom::Box3::new(Point3::new(-20.0, -20.0, -20.0), Point3::new(20.0, 20.0, 20.0));
        let Ok(hit) = ryukei::intersect::surface_surface(&a, &b, &region) else {
            return Ok(());
        };
        let ryukei::intersect::SurfaceHit::Meet { branches, points } = hit else {
            return Ok(());
        };
        for br in &branches {
            for k in 0..=32 {
                let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 32.0;
                let p = br.curve.point(t);
                prop_assert!(a.distance(p) <= br.tolerance + 1e-12);
                prop_assert!(b.distance(p) <= br.tolerance + 1e-12);
            }
        }
        complete(&a, &b, &branches, &points)?;
        complete(&b, &a, &branches, &points)?;
    }
}
