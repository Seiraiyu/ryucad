use super::*;
use crate::geom::{Cylinder, Frame, Plane, Sphere, Vector3};
use crate::intersect::{SurfaceHit, surface_surface};
use core::f64::consts::TAU;
use std::sync::Arc;

fn region() -> Box3 {
    Box3::new(Point3::new(-5.0, -5.0, -5.0), Point3::new(5.0, 5.0, 5.0))
}

/// An analytic surface's exact NURBS form over a box of its parameters.
fn spline(s: &Surface, range: [[f64; 2]; 2]) -> Surface {
    let (n, _) = s.to_nurbs(range).unwrap();
    Surface::Nurbs(Arc::new(n))
}

fn frame(o: (f64, f64, f64), z: (f64, f64, f64)) -> Frame {
    Frame::new(
        Point3::new(o.0, o.1, o.2),
        Vector3::new(z.0, z.1, z.2),
        Vector3::new(1.0, 0.2, 0.1),
    )
    .unwrap()
}

/// Every traced point on both surfaces.
fn on_both(a: &Surface, b: &Surface, topo: &Topology) {
    for br in &topo.branches {
        for &p in &br.points {
            assert!(a.distance(p) <= 1e-9 && b.distance(p) <= 1e-9, "{p:?}");
        }
    }
    for &p in &topo.points {
        assert!(a.distance(p) <= 1e-8 && b.distance(p) <= 1e-8, "{p:?}");
    }
}

#[test]
fn two_spline_planes_meet_in_a_line() {
    let a = spline(
        &Surface::Plane(Plane::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)))),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    let b = spline(
        &Surface::Plane(Plane::new(frame((0.3, 0.0, 0.0), (1.0, 0.1, 0.4)))),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    let topo = trace(&a, &b, &region()).unwrap();
    assert_eq!(topo.branches.len(), 1, "{topo:?}");
    assert!(!topo.branches[0].closed);
    on_both(&a, &b, &topo);
}

#[test]
fn a_spline_cylinder_and_a_spline_plane_meet_in_a_loop() {
    let cyl =
        Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 0.7).unwrap());
    let a = spline(&cyl, [[0.0, TAU], [-2.0, 2.0]]);
    let b = spline(
        &Surface::Plane(Plane::new(frame((0.0, 0.0, 0.2), (0.3, 0.1, 1.0)))),
        [[-2.0, 2.0], [-2.0, 2.0]],
    );
    let topo = trace(&a, &b, &region()).unwrap();
    assert_eq!(topo.branches.len(), 1, "{topo:?}");
    assert!(topo.branches[0].closed, "{topo:?}");
    on_both(&a, &b, &topo);
    // Fitted, the loop is within its tolerance of both.
    let SurfaceHit::Meet { branches, .. } = surface_surface(&a, &b, &region()).unwrap() else {
        panic!("coincident")
    };
    assert_eq!(branches.len(), 1);
    let br = &branches[0];
    for k in 0..=64 {
        let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 64.0;
        let p = br.curve.point(t);
        assert!(a.distance(p) <= br.tolerance && b.distance(p) <= br.tolerance);
        assert!(cyl.distance(p) <= br.tolerance + 1e-12);
    }
}

#[test]
fn two_spline_spheres() {
    let s1 = Surface::Sphere(Sphere::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap());
    let s2 = Surface::Sphere(Sphere::new(frame((0.8, 0.3, 0.2), (0.2, 0.0, 1.0)), 0.9).unwrap());
    let a = spline(&s1, [[0.0, TAU], [-1.5, 1.5]]);
    let b = spline(&s2, [[0.0, TAU], [-1.5, 1.5]]);
    let topo = trace(&a, &b, &region()).unwrap();
    assert_eq!(topo.branches.len(), 1, "{topo:?}");
    on_both(&a, &b, &topo);
    // Touching from outside, at one point.
    let s3 = Surface::Sphere(Sphere::new(frame((1.5, 0.0, 0.0), (0.2, 0.0, 1.0)), 0.5).unwrap());
    let c = spline(&s3, [[0.0, TAU], [-1.5, 1.5]]);
    let topo = trace(&a, &c, &region()).unwrap();
    assert!(topo.branches.is_empty(), "{topo:?}");
    assert_eq!(topo.points.len(), 1, "{topo:?}");
    on_both(&a, &c, &topo);
    // Apart.
    let s4 = Surface::Sphere(Sphere::new(frame((3.0, 0.0, 0.1), (0.2, 0.0, 1.0)), 0.5).unwrap());
    let d = spline(&s4, [[0.0, TAU], [-1.5, 1.5]]);
    let topo = trace(&a, &d, &region()).unwrap();
    assert!(topo.branches.is_empty() && topo.points.is_empty());
}

#[test]
fn small_systems() {
    let m = [
        [2.0, 1.0, 0.0, 0.0],
        [0.0, 3.0, 1.0, 0.0],
        [1.0, 0.0, 4.0, 1.0],
        [0.0, 0.0, 1.0, 5.0],
    ];
    let x = solve4(m, [3.0, 4.0, 6.0, 6.0]).unwrap();
    for (row, r) in m.iter().zip([3.0, 4.0, 6.0, 6.0]) {
        let s: f64 = row.iter().zip(x).map(|(a, b)| a * b).sum();
        assert!((s - r).abs() < 1e-12);
    }
    let jac = m.map(|r| r.map(Iv::point));
    let f = |x: [f64; 4]| -> [Iv; 4] {
        core::array::from_fn(|i| Iv::point((0..4).map(|j| m[i][j] * x[j]).sum::<f64>()))
    };
    let around = [[-0.1, 0.1]; 4];
    assert_eq!(krawczyk4(f([0.0; 4]), jac, around), Some(true));
    let away = [[1.0, 1.2]; 4];
    assert_eq!(krawczyk4(f([1.1; 4]), jac, away), Some(false));
}

#[test]
fn coincident_surfaces_are_said_to_be() {
    let cyl =
        Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 0.7).unwrap());
    let a = spline(&cyl, [[0.0, TAU], [-2.0, 2.0]]);
    let b = spline(&cyl, [[0.5, 2.0], [-1.0, 1.0]]);
    for (x, y) in [(&a, &cyl), (&cyl, &a), (&a, &b), (&b, &a)] {
        assert_eq!(
            surface_surface(x, y, &region()),
            Ok(SurfaceHit::Coincident { same: true }),
            "{x:?} {y:?}"
        );
    }
}
