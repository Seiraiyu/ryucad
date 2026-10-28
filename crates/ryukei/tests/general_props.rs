//! Surface–surface intersections where a surface has no implicit form,
//! against sampling: every branch, and its pcurves, within its tolerance
//! of both surfaces, and every place sampling sees one surface cross the
//! other near a branch or a point. The surfaces are random analytic
//! surfaces' exact NURBS forms, so the same pairs' analytic results are a
//! second reference. Release only: minutes in a debug build.

use proptest::prelude::*;
use ryukei::geom::{
    Box3, Cone, Cylinder, Frame, Plane, Point3, Sphere, Surface, Torus, UnitVector3, Vector3,
};
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
/// holding the part near the origin (one nappe of a cone; a sphere's `v`
/// converts whole, poles and all).
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

fn spline(s: &Surface, range: [[f64; 2]; 2]) -> Surface {
    let (n, _) = s.to_nurbs(range).unwrap();
    Surface::Nurbs(Arc::new(n))
}

/// Splines touching along a curve: one tangent branch, within the touch
/// rule's reach of both, along the curve.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn surfaces_touching_along_a_curve() {
    let tau = core::f64::consts::TAU;
    let region = Box3::new(Point3::new(-5.0, -5.0, -5.0), Point3::new(5.0, 5.0, 5.0));
    // A cylinder of radius 0.5 along x resting on a plane: they touch
    // along the line y = 0 on the plane.
    let plane = spline(
        &Surface::Plane(Plane::new(Frame::WORLD)),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    let axis = Frame::new(
        Point3::new(0.0, 0.0, 0.5),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, -1.0),
    )
    .unwrap();
    let cyl = spline(
        &Surface::Cylinder(Cylinder::new(axis, 0.5).unwrap()),
        [[0.0, tau], [-1.5, 1.5]],
    );
    let SurfaceHit::Meet { branches, points } = surface_surface(&plane, &cyl, &region).unwrap()
    else {
        panic!("coincident")
    };
    assert!(points.is_empty(), "{points:?}");
    assert_eq!(branches.len(), 1, "{branches:?}");
    let br = &branches[0];
    assert!(br.tangent && br.tolerance <= 1e-6, "{}", br.tolerance);
    let ends = br.range.map(|t| br.curve.point(t));
    assert!((ends[0].x - ends[1].x).abs() > 2.99, "{ends:?}");
    for k in 0..=16 {
        let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 16.0;
        let p = br.curve.point(t);
        assert!(p.y.abs() <= 1e-6 && p.z.abs() <= 1e-6, "{p:?}");
        for (s, (c2, map)) in [&plane, &cyl].into_iter().zip(&br.pcurves) {
            let q = c2.point(map[0] + map[1] * t);
            assert!(s.point(q.x, q.y).distance(p) <= br.tolerance);
        }
    }
    // A sphere inside a cylinder of its radius: along its equator.
    let ball = spline(
        &Surface::Sphere(Sphere::new(Frame::WORLD, 0.5).unwrap()),
        [[0.0, tau], [-1.4, 1.4]],
    );
    let tube = spline(
        &Surface::Cylinder(Cylinder::new(Frame::WORLD, 0.5).unwrap()),
        [[0.0, tau], [-1.0, 1.0]],
    );
    let SurfaceHit::Meet { branches, .. } = surface_surface(&ball, &tube, &region).unwrap() else {
        panic!("coincident")
    };
    assert_eq!(branches.len(), 1, "{branches:?}");
    let br = &branches[0];
    assert!(br.tangent);
    for k in 0..=8 {
        let p = br
            .curve
            .point(br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 8.0);
        assert!(
            p.z.abs() <= 1e-6 && (ryukei::math::hypot(p.x, p.y) - 0.5).abs() <= 1e-6,
            "{p:?}"
        );
    }
}

/// A spline cone passing 15 µm from a pole of a spline sphere (its `v`
/// converted whole) meets it as sampling sees. Found by
/// `spline_pairs_agree_with_sampling` on macos-latest (CI run
/// 40418433312) before its spheres stopped short of their poles; it was
/// `Uncertain` until the four-parameter trace took normals' limits at
/// collapsed rows (issue #128).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn passing_a_spline_pole() {
    let tau = core::f64::consts::TAU;
    let unit = |x, y, z| UnitVector3::exact(Vector3::new(x, y, z)).unwrap();
    let cone = Frame::from_axes(
        Point3::new(1.8320710486777743, -1.7513363077239754, -1.508676459436074),
        unit(-0.1664148424245527, 0.6807227608150997, -0.7133881293721391),
        unit(
            -0.15464301548726556,
            -0.7325411407615743,
            -0.662924592131516,
        ),
    )
    .unwrap();
    let cone = Cone::new(cone, 1.041812376422121, 0.7418606317016863).unwrap();
    let ball = Frame::from_axes(
        Point3::new(1.3213431565366103, -1.0632248787847285, -1.1177160827182717),
        unit(0.9567357986130439, 0.1556135058373798, -0.24584761225861207),
        unit(-0.1507170440917043, 0.9878180180584882, 0.03872900488341388),
    )
    .unwrap();
    let ball = Sphere::new(ball, 1.8321002391416865).unwrap();
    let apex = cone.apex_v();
    let a = spline(
        &Surface::Cone(cone),
        [[0.0, tau], [apex + 0.05, apex + 6.0]],
    );
    let b = spline(&Surface::Sphere(ball), [[0.0, tau], [-1.5, 1.5]]);
    agrees(&a, &b).unwrap();
}
