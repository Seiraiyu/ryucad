use super::*;
use crate::geom::pipe::tests::{R, on_plane, round_cylinder};
use crate::geom::{Cone, Cylinder, Frame, Pipe, Plane, Sphere, Surface, Vector3};

/// The trace with no meter, as the tests before plan 15.A took it.
fn trace(surface: &Surface, other: &Surface, region: &Box3) -> Result<Topology, Failure> {
    super::trace(surface, other, region, &Meter::free()) // unmetered: a test
}

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

/// The plane of a sphere's equator cuts its spline form along a knot
/// line: one branch, traced on the grid that doesn't keep the breaks.
#[test]
fn a_plane_along_a_spline_sphere_s_knot_line() {
    let ball = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let (n, _) = ball
        .to_nurbs([
            [0.0, TAU],
            [-core::f64::consts::FRAC_PI_2, core::f64::consts::FRAC_PI_2],
        ])
        .unwrap();
    let spline = Surface::Nurbs(std::sync::Arc::new(n));
    let topo = trace(
        &spline,
        &plane(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)),
        &region(),
    )
    .unwrap();
    assert_eq!((topo.branches.len(), topo.points.len()), (1, 0));
    assert!(topo.branches[0].closed);
}

#[test]
fn a_trace_halts_at_its_budget_and_charges_alike_on_any_thread_count() {
    use crate::work::Work;
    let pipe = on_plane();
    let (a, b) = (
        sheet(&pipe),
        plane(Point3::new(1.5, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)),
    );
    let w = Work::unlimited();
    let m = Meter::new(&w);
    assert!(super::trace(&a, &b, &region(), &m).is_ok());
    let spent = m.spent();
    assert!(spent > 0);
    for threads in [1, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let got = pool.install(|| {
            let w = Work::unlimited();
            let m = Meter::new(&w);
            super::trace(&a, &b, &region(), &m).unwrap();
            m.spent()
        });
        assert_eq!(got, spent);
    }
    let w = Work::unlimited().with_budget(spent / 2);
    let m = Meter::new(&w);
    assert_eq!(
        super::trace(&a, &b, &region(), &m).err(),
        Some(Failure::Uncertain)
    );
    assert!(m.halted());
    let w = Work::unlimited();
    let m = Meter::new(&w);
    let r = m.capped_by(spent / 2, || super::trace(&a, &b, &region(), &m));
    assert_eq!(r.err(), Some(Failure::Uncertain));
    assert!(!m.halted());
}

#[test]
fn traced_points_carry_their_parameters() {
    // A NURBS sphere cut by a tilted plane: a closed branch.
    let ball = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let (n, _) = ball
        .to_nurbs([[0.0, core::f64::consts::TAU], [-1.5, 1.5]])
        .unwrap();
    let spline = Surface::Nurbs(std::sync::Arc::new(n));
    let plane = Surface::Plane(Plane::new(
        Frame::new(
            Point3::new(0.0, 0.0, 0.2),
            Vector3::new(0.3, 0.1, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
    ));
    let region = crate::geom::Box3::new(Point3::new(-2.0, -2.0, -2.0), Point3::new(2.0, 2.0, 2.0));
    // unmetered: a unit test
    let topo = super::trace(&spline, &plane, &region, &Meter::free()).unwrap();
    assert!(!topo.branches.is_empty());
    for b in &topo.branches {
        assert_eq!(b.params.len(), b.points.len());
        for (p, &[u, v]) in b.points.iter().zip(&b.params) {
            assert!(
                spline.point(u, v).distance(*p) <= 1e-12,
                "{p:?} at ({u}, {v})"
            );
        }
    }
}

fn contour_of(s: &crate::geom::Surface, t: crate::geom::Vector3) -> super::Topology {
    use crate::geom::{Box3, Point3};
    let region = Box3::new(Point3::new(-3.0, -3.0, -3.0), Point3::new(3.0, 3.0, 3.0));
    // unmetered: a unit test
    super::trace_contour(s, t, &region, &crate::work::Meter::free()).unwrap()
}

#[test]
fn a_torus_askew_has_a_traced_silhouette_on_its_normal_square_to_the_view() {
    use crate::geom::{Frame, Surface, Torus, Vector3};
    let torus = Surface::Torus(Torus::new(Frame::WORLD, 1.0, 0.3).unwrap());
    let t = Vector3::new(1.0, 0.0, 1.0) / core::f64::consts::SQRT_2;
    let topo = contour_of(&torus, t);
    assert!(!topo.branches.is_empty());
    for b in &topo.branches {
        for &[u, v] in &b.params {
            assert!(
                torus.normal(u, v).unwrap().get().dot(t).abs() < 1e-9,
                "({u}, {v})"
            );
        }
    }
}

#[test]
fn a_spline_cylinder_seen_side_on_has_two_rulings() {
    use crate::geom::{Cylinder, Frame, Surface, Vector3};
    use core::f64::consts::TAU;
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 0.5).unwrap());
    let (n, _) = cyl.to_nurbs([[0.0, TAU], [0.0, 1.0]]).unwrap();
    let spline = Surface::Nurbs(std::sync::Arc::new(n));
    let topo = contour_of(&spline, Vector3::new(0.0, -1.0, 0.0));
    assert_eq!(topo.branches.len(), 2, "{topo:?}");
    for b in &topo.branches {
        for p in &b.points {
            assert!((p.x.abs() - 0.5).abs() < 1e-9 && p.y.abs() < 1e-9, "{p:?}");
        }
    }
}

#[test]
fn a_plane_has_no_contour_field() {
    use crate::geom::{Box3, Frame, Plane, Point3, Surface, Vector3};
    let plane = Surface::Plane(Plane::new(Frame::WORLD));
    let region = Box3::new(Point3::new(-1.0, -1.0, -1.0), Point3::new(1.0, 1.0, 1.0));
    // unmetered: a unit test
    let r = super::trace_contour(
        &plane,
        Vector3::new(0.0, 0.0, 1.0),
        &region,
        &crate::work::Meter::free(),
    );
    assert_eq!(r.unwrap_err(), super::Failure::Unsupported);
}

#[test]
fn a_spline_kinked_along_a_knot_line_is_uncertain() {
    use crate::geom::{KnotVector, NurbsSurface, Vector3};
    // A plate bent along its middle (degree 1 across, a knot there): its
    // normal jumps, so its silhouette can't be traced (decision 6).
    let ku = KnotVector::clamped(1, &[0.0, 0.5, 1.0], &[2, 1, 2]).unwrap();
    let kv = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(0.0, 1.0, 0.0),
        Point3::new(0.5, 0.0, 0.3),
        Point3::new(0.5, 1.0, 0.3),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(1.0, 1.0, 0.0),
    ];
    let bent = Surface::Nurbs(std::sync::Arc::new(
        NurbsSurface::new([ku, kv], pts, None).unwrap(),
    ));
    assert!(super::creased(&bent));
    let region = Box3::new(Point3::new(-3.0, -3.0, -3.0), Point3::new(3.0, 3.0, 3.0));
    // unmetered: a unit test
    let r = super::trace_contour(&bent, Vector3::new(1.0, 0.0, 0.0), &region, &Meter::free());
    assert_eq!(r.unwrap_err(), Failure::Uncertain);
    // The exact NURBS form of a cylinder has double knots and no crease.
    let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 0.5).unwrap());
    let (n, _) = cyl
        .to_nurbs([[0.0, core::f64::consts::TAU], [0.0, 1.0]])
        .unwrap();
    assert!(!super::creased(&Surface::Nurbs(std::sync::Arc::new(n))));
}
