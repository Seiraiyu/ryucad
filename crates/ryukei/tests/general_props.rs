//! Surface–surface intersections where a surface has no implicit form,
//! against sampling: every branch, and its pcurves, within its tolerance
//! of both surfaces, and every place sampling sees one surface cross the
//! other near a branch or a point. The surfaces are random analytic
//! surfaces' exact NURBS forms, so the same pairs' analytic results are a
//! second reference. Release only: minutes in a debug build.

use proptest::prelude::*;
use ryukei::geom::{Box3, Cone, Cylinder, Frame, Plane, Point3, Sphere, Surface, Torus, Vector3};
use ryukei::intersect::{Branch, Side, SurfaceHit, surface_surface};
use std::sync::Arc;

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

fn analytic() -> impl Strategy<Value = Surface> {
    (frame(), 0usize..5, 0.5f64..2.0, 0.2f64..0.4).prop_map(|(f, kind, r, a)| match kind {
        0 => Surface::Plane(Plane::new(f)),
        1 => Surface::Cylinder(Cylinder::new(f, r).unwrap()),
        2 => Surface::Cone(Cone::new(f, r, a * 2.0).unwrap()),
        3 => Surface::Sphere(Sphere::new(f, r).unwrap()),
        _ => Surface::Torus(Torus::new(f, r + 1.0, r).unwrap()),
    })
}

/// An analytic surface's exact NURBS form over a box of its parameters
/// holding the part near the origin (one nappe of a cone, a sphere short
/// of its poles).
fn spline_of(s: &Surface) -> Surface {
    let tau = core::f64::consts::TAU;
    let range = match s {
        Surface::Plane(_) => [[-4.0, 4.0], [-4.0, 4.0]],
        Surface::Cone(k) => [[0.0, tau], [k.apex_v() + 0.05, k.apex_v() + 6.0]],
        Surface::Sphere(_) => [[0.0, tau], [-1.5, 1.5]],
        Surface::Torus(_) => [[0.0, tau], [0.0, tau]],
        _ => [[0.0, tau], [-4.0, 4.0]],
    };
    let (n, _) = s.to_nurbs(range).unwrap();
    Surface::Nurbs(Arc::new(n))
}

/// The side of the surface `p` is on, when it is clearly off it and its
/// foot is inside the surface's domain (off a spline's edge it has no
/// side).
fn side(s: &Surface, p: Point3) -> Option<Side> {
    let foot = s.project(p);
    if foot.distance < 1e-6 {
        return None;
    }
    let (u, v) = foot.param;
    let [du, dv] = s.domain();
    let (pu, pv) = s.periods();
    let edge = |x: f64, d: [f64; 2], period: Option<f64>| {
        period.is_none() && (x - d[0]).abs().min((x - d[1]).abs()) <= 1e-9 * (1.0 + x.abs())
    };
    if edge(u, du, pu) || edge(v, dv, pv) {
        return None;
    }
    let n = s.normal(u, v)?.get();
    let along = (p - foot.point).dot(n) / foot.distance;
    // Near an axis, a pole, an apex or a spline's edge the foot's normal
    // can be off.
    if along.abs() < 0.9 {
        return None;
    }
    Some(if along > 0.0 {
        Side::Positive
    } else {
        Side::Negative
    })
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
    branches: &[Branch],
    points: &[Point3],
) -> Result<(), TestCaseError> {
    let rows = grid(a, 40);
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

/// Every branch, and its pcurve on each surface, within the branch's
/// tolerance of both surfaces; and nothing sampling sees missed. A
/// refusal fails: these pairs are all certifiable.
fn agrees(a: &Surface, b: &Surface) -> Result<(), TestCaseError> {
    let region = Box3::new(
        Point3::new(-20.0, -20.0, -20.0),
        Point3::new(20.0, 20.0, 20.0),
    );
    let hit = surface_surface(a, b, &region);
    prop_assert!(hit.is_ok(), "{:?}", hit);
    let SurfaceHit::Meet { branches, points } = hit.unwrap() else {
        return Ok(());
    };
    for br in &branches {
        for k in 0..=32 {
            let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 32.0;
            let p = br.curve.point(t);
            prop_assert!(a.distance(p) <= br.tolerance + 1e-12);
            prop_assert!(b.distance(p) <= br.tolerance + 1e-12);
            for (s, (c2, map)) in [a, b].into_iter().zip(&br.pcurves) {
                let q = c2.point(map[0] + map[1] * t);
                prop_assert!(s.point(q.x, q.y).distance(p) <= br.tolerance + 1e-12);
            }
        }
    }
    complete(a, b, &branches, &points)?;
    complete(b, a, &branches, &points)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A spline against an analytic surface, traced in the spline's
    /// parameters against the other's implicit form.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "slow in a debug build: CI runs it in release")]
    fn spline_and_analytic_pairs_agree_with_sampling(a in analytic(), b in analytic()) {
        agrees(&spline_of(&a), &b)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Two splines, traced in their four parameters.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "slow in a debug build: CI runs it in release")]
    fn spline_pairs_agree_with_sampling(a in analytic(), b in analytic()) {
        agrees(&spline_of(&a), &spline_of(&b))?;
    }
}
