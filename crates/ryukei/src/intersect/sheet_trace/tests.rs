use super::*;
use crate::geom::pipe::tests::{R, on_plane, round_cylinder};
use crate::geom::{Cone, Cylinder, Frame, Pipe, Plane, Sphere, Surface, Vector3};

fn sheet(p: &Pipe) -> Surface {
    Surface::Pipe(std::sync::Arc::new(p.clone()))
}

fn region() -> Box3 {
    Box3::new(
        Point3::new(-10.0, -10.0, -10.0),
        Point3::new(10.0, 10.0, 10.0),
    )
}

fn plane(origin: Point3, normal: Vector3) -> Surface {
    let x = if normal.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    Surface::Plane(Plane::new(Frame::new(origin, normal, x).unwrap()))
}

/// Every traced point lies on both surfaces, and every sign change of
/// `F` on a fine grid of the pipe's parameters has a traced point near
/// it.
fn sound(pipe: &Pipe, other: &Surface, topo: &Topology) {
    let ps = Surface::Pipe(std::sync::Arc::new(pipe.clone()));
    let all: Vec<Point3> = topo
        .branches
        .iter()
        .flat_map(|b| b.points.iter().copied())
        .chain(topo.points.iter().copied())
        .collect();
    for &p in &all {
        assert!(ps.distance(p) <= 1e-9, "{p:?} off the pipe");
        assert!(other.distance(p) <= 1e-9, "{p:?} off the other");
    }
    let imp = Implicit::of(other).unwrap();
    let f = |s: f64, t: f64| imp.eval_grad(ps.point(s, t)).0;
    let [d0, d1] = pipe.spine().domain();
    let (n, m) = (120, 72);
    for i in 0..n {
        for j in 0..m {
            let s = d0 + (d1 - d0) * f64::from(i) / f64::from(n);
            let t = -PI + TAU * f64::from(j) / f64::from(m);
            let (s2, t2) = (s + (d1 - d0) / f64::from(n), t + TAU / f64::from(m));
            if (f(s, t) > 0.0) != (f(s2, t) > 0.0) || (f(s, t) > 0.0) != (f(s, t2) > 0.0) {
                let q = ps.point(s, t);
                // Within a grid step of a traced point or a chord between
                // neighbours.
                let step = ps.point(s2, t2).distance(q) + 0.05;
                let near = topo.branches.iter().any(|b| {
                    b.points.windows(2).any(|w| {
                        let d = w[1] - w[0];
                        let k =
                            ((q - w[0]).dot(d) / d.length_squared().max(1e-300)).clamp(0.0, 1.0);
                        (w[0] + d * k).distance(q) <= step
                    }) || b.points.iter().any(|p| p.distance(q) <= step)
                });
                assert!(near, "sign change at {s}, {t} ({q:?}) missed");
            }
        }
    }
}

#[test]
fn a_plane_across_the_tube() {
    // The plane x = 1.5 cuts the tube on the plane in one loop round it.
    let pipe = on_plane();
    let p = plane(Point3::new(1.5, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
    let topo = trace(&sheet(&pipe), &p, &region()).unwrap();
    assert_eq!(topo.branches.len(), 1);
    assert!(topo.branches[0].closed);
    assert!(topo.points.is_empty());
    sound(&pipe, &p, &topo);
}

#[test]
fn a_plane_along_the_tube() {
    // The plane z = R through the spine cuts the tube along both its
    // sides, from end to end.
    let pipe = on_plane();
    let p = plane(Point3::new(0.0, 0.0, R), Vector3::new(0.0, 0.0, 1.0));
    let topo = trace(&sheet(&pipe), &p, &region()).unwrap();
    assert_eq!(topo.branches.len(), 2);
    assert!(topo.branches.iter().all(|b| !b.closed));
    sound(&pipe, &p, &topo);
}

#[test]
fn spheres_cylinders_and_cones() {
    let pipe = on_plane();
    let c = pipe.spine().point(0.8);
    for other in [
        Surface::Sphere(
            Sphere::new(
                Frame::new(c, Vector3::new(0.0, 0.0, 1.0), Vector3::new(1.0, 0.0, 0.0)).unwrap(),
                0.4,
            )
            .unwrap(),
        ),
        Surface::Cylinder(
            Cylinder::new(
                Frame::new(
                    Point3::new(1.0, 0.0, 0.0),
                    Vector3::new(0.0, 0.3, 1.0),
                    Vector3::new(1.0, 0.0, 0.0),
                )
                .unwrap(),
                0.35,
            )
            .unwrap(),
        ),
        Surface::Cone(
            Cone::new(
                Frame::new(
                    Point3::new(2.5, 0.2, -1.0),
                    Vector3::new(0.1, 0.0, 1.0),
                    Vector3::new(1.0, 0.0, 0.0),
                )
                .unwrap(),
                0.1,
                0.3,
            )
            .unwrap(),
        ),
    ] {
        let topo = trace(&sheet(&pipe), &other, &region()).unwrap();
        assert!(!topo.branches.is_empty(), "{other:?}");
        sound(&pipe, &other, &topo);
    }
    // Round a cylinder, a plane across the closed tube.
    let tube = round_cylinder();
    let p = plane(Point3::new(0.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
    let topo = trace(&sheet(&tube), &p, &region()).unwrap();
    assert_eq!(topo.branches.len(), 2, "{topo:?}");
    sound(&tube, &p, &topo);
}

#[test]
fn a_touching_sphere_meets_at_a_point() {
    // A sphere outside the tube, touching it at one point.
    let pipe = on_plane();
    let ps = Surface::Pipe(std::sync::Arc::new(pipe.clone()));
    let (s, t) = (0.9, 2.0);
    let n = ps.normal(s, t).unwrap().get();
    let at = ps.point(s, t);
    let r = 0.3;
    let sphere = Surface::Sphere(
        Sphere::new(
            Frame::new(
                at + n * r,
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
            r,
        )
        .unwrap(),
    );
    let topo = trace(&sheet(&pipe), &sphere, &region()).unwrap();
    assert!(topo.branches.is_empty(), "{topo:?}");
    assert_eq!(topo.points.len(), 1);
    assert!(topo.points[0].distance(at) <= 1e-6, "{:?}", topo.points[0]);
}

#[test]
fn tangency_along_a_curve_is_uncertain() {
    // The anchor plane touches the tube along its whole contact line.
    let pipe = on_plane();
    let p = plane(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0));
    assert_eq!(
        trace(&sheet(&pipe), &p, &region()).err(),
        Some(Failure::Uncertain)
    );
}

#[test]
fn far_apart_and_out_of_the_region() {
    let pipe = on_plane();
    let p = plane(Point3::new(0.0, 0.0, 5.0), Vector3::new(0.0, 0.0, 1.0));
    let topo = trace(&sheet(&pipe), &p, &region()).unwrap();
    assert!(topo.branches.is_empty() && topo.points.is_empty());
    // A region away from the pipe sees nothing of a plane that cuts it.
    let across = plane(Point3::new(1.5, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
    let away = Box3::new(Point3::new(5.0, 5.0, 5.0), Point3::new(6.0, 6.0, 6.0));
    let topo = trace(&sheet(&pipe), &across, &away).unwrap();
    assert!(topo.branches.is_empty());
}

#[test]
fn splines_and_swept_surfaces() {
    use crate::geom::{Circle, Curve, Extrusion, Revolution, Torus};
    use core::f64::consts::TAU;
    use std::sync::Arc;
    // A NURBS circle extruded, cut by a tilted plane: one loop.
    let f = Frame::new(
        Point3::new(0.1, -0.2, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let (nc, _) = Circle::new(f, 0.7).unwrap().to_nurbs().unwrap();
    let ext = Surface::Extrusion(Arc::new(
        Extrusion::new(Curve::Nurbs(Arc::new(nc)), Vector3::new(0.0, 0.1, 1.0)).unwrap(),
    ));
    let p = plane(Point3::new(0.0, 0.0, 0.3), Vector3::new(0.2, 0.1, 1.0));
    let topo = trace(&ext, &p, &region()).unwrap();
    assert_eq!(topo.branches.len(), 1, "{topo:?}");
    assert!(topo.branches[0].closed);
    // A revolution of an off-axis circle (a torus) and the same torus's
    // exact NURBS, each cut by a plane through the axis: two loops.
    let meridian = Frame::new(
        Point3::new(1.2, 0.0, 0.0),
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let rev = Surface::Revolution(Arc::new(
        Revolution::new(
            Frame::WORLD,
            Curve::Circle(Circle::new(meridian, 0.4).unwrap()),
        )
        .unwrap(),
    ));
    let torus = Surface::Torus(Torus::new(Frame::WORLD, 1.2, 0.4).unwrap());
    let (nt, _) = torus.to_nurbs([[0.0, TAU], [0.0, TAU]]).unwrap();
    let through = plane(Point3::ORIGIN, Vector3::new(0.3, 1.0, 0.0));
    for s in [rev, Surface::Nurbs(Arc::new(nt))] {
        let topo = trace(&s, &through, &region()).unwrap();
        assert_eq!(topo.branches.len(), 2, "{s:?}: {topo:?}");
        for b in &topo.branches {
            assert!(b.closed);
            for &q in &b.points {
                assert!(s.distance(q) <= 1e-9 && through.distance(q) <= 1e-9);
            }
        }
    }
}

/// A plane through both poles of a sphere's spline form: each half
/// meridian a branch ending at the poles, which are on the plane, with
/// no point left over. Near the pole but off it, one branch round.
#[test]
fn planes_through_a_spline_sphere_s_poles() {
    let ball = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let (n, _) = ball
        .to_nurbs([
            [0.0, TAU],
            [-core::f64::consts::FRAC_PI_2, core::f64::consts::FRAC_PI_2],
        ])
        .unwrap();
    let spline = Surface::Nurbs(std::sync::Arc::new(n));
    let through = trace(
        &spline,
        &plane(Point3::ORIGIN, Vector3::new(1.0, -0.3, 0.0)),
        &region(),
    )
    .unwrap();
    assert_eq!((through.branches.len(), through.points.len()), (2, 0));
    let poles = [Point3::new(0.0, 0.0, 1.0), Point3::new(0.0, 0.0, -1.0)];
    for b in &through.branches {
        let ends = [b.points[0], b.points[b.points.len() - 1]];
        assert!(ends.iter().all(|e| poles.contains(e)), "{ends:?}");
    }
    let tilted = trace(
        &spline,
        &plane(Point3::new(0.0, 0.0, 1.0), Vector3::new(1.0, 0.0, 0.3)),
        &region(),
    )
    .unwrap();
    assert_eq!((tilted.branches.len(), tilted.points.len()), (1, 0));
    let near = trace(
        &spline,
        &plane(Point3::new(0.01, 0.0, 1.0), Vector3::new(1.0, 0.0, 0.3)),
        &region(),
    )
    .unwrap();
    assert_eq!((near.branches.len(), near.points.len()), (1, 0));
}
