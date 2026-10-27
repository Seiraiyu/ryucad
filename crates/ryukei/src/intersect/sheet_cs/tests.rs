use super::*;
use crate::geom::{
    Circle, Cone, Cylinder, Extrusion, Frame, Line, Plane, Revolution, Sphere, Torus, Vector3,
};
use crate::intersect::curve_surface;
use core::f64::consts::TAU;
use std::sync::Arc;

const TOL: f64 = 1e-8;

fn line(o: (f64, f64, f64), d: (f64, f64, f64)) -> Curve {
    Curve::Line(Line::new(Point3::new(o.0, o.1, o.2), Vector3::new(d.0, d.1, d.2)).unwrap())
}

fn frame() -> Frame {
    Frame::new(
        Point3::new(0.1, -0.2, 0.3),
        Vector3::new(0.2, 0.3, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap()
}

/// An analytic surface's exact NURBS form over a box of its parameters.
fn as_nurbs(s: &Surface, range: [[f64; 2]; 2]) -> Surface {
    let (n, _) = s.to_nurbs(range).unwrap();
    Surface::Nurbs(Arc::new(n))
}

/// The hits' parameters and sides.
fn summary(hits: &[CurveHit]) -> Vec<(f64, Side, Side)> {
    hits.iter()
        .map(|h| match *h {
            CurveHit::Point {
                t, before, after, ..
            } => (t, before, after),
            CurveHit::On { t } => (t[0], Side::Beyond, Side::Beyond),
        })
        .collect()
}

/// The same hits within `slack` of each other's parameter.
fn same(a: &[CurveHit], b: &[CurveHit], slack: f64) {
    let (x, y) = (summary(a), summary(b));
    assert_eq!(x.len(), y.len(), "{a:?}\nvs\n{b:?}");
    for (p, q) in x.iter().zip(&y) {
        assert!((p.0 - q.0).abs() <= slack, "{a:?}\nvs\n{b:?}");
        assert_eq!((p.1, p.2), (q.1, q.2), "{a:?}\nvs\n{b:?}");
    }
}

#[test]
fn a_line_through_a_spline_plane() {
    let plane = Surface::Plane(Plane::new(Frame::WORLD));
    let n = as_nurbs(&plane, [[-1.0, 1.0], [-1.0, 1.0]]);
    let l = line((0.2, 0.3, -1.0), (0.1, 0.0, 1.0));
    let hits = curve_surface(&l, [0.0, 3.0], &n, TOL).unwrap();
    same(
        &hits,
        &curve_surface(&l, [0.0, 3.0], &plane, TOL).unwrap(),
        1e-12,
    );
    let CurveHit::Point { point, .. } = hits[0] else {
        panic!("{hits:?}")
    };
    assert!(point.z.abs() <= 1e-14);
    // Past the patch's edge it misses.
    let l = line((1.5, 0.3, -1.0), (0.0, 0.0, 1.0));
    assert!(curve_surface(&l, [0.0, 3.0], &n, TOL).unwrap().is_empty());
}

#[test]
fn curves_against_exact_forms_of_analytic_surfaces() {
    let f = frame();
    let surfaces = [
        Surface::Cylinder(Cylinder::new(f, 0.7).unwrap()),
        Surface::Cone(Cone::new(f, 0.7, 0.4).unwrap()),
        Surface::Sphere(Sphere::new(f, 0.9).unwrap()),
        Surface::Torus(Torus::new(f, 1.2, 0.4).unwrap()),
    ];
    let curves = [
        (line((-2.0, 0.05, 0.25), (1.0, 0.1, 0.05)), [0.0, 4.0]),
        (line((0.1, -2.0, 0.6), (0.05, 1.0, -0.2)), [0.0, 4.0]),
        (
            Curve::Circle(
                Circle::new(
                    Frame::new(
                        Point3::new(0.3, 0.0, 0.2),
                        Vector3::new(0.0, 1.0, 0.3),
                        Vector3::new(1.0, 0.0, 0.0),
                    )
                    .unwrap(),
                    1.0,
                )
                .unwrap(),
            ),
            [0.0, TAU],
        ),
    ];
    for s in &surfaces {
        // A box of parameters wide enough to hold every hit.
        let range = match s {
            Surface::Sphere(_) => [[0.0, TAU], [-1.5, 1.5]],
            Surface::Torus(_) => [[0.0, TAU], [0.0, TAU]],
            Surface::Cone(k) => [[0.0, TAU], [k.apex_v() + 0.01, 3.0]],
            _ => [[0.0, TAU], [-3.0, 3.0]],
        };
        let n = as_nurbs(s, range);
        for (c, r) in &curves {
            let want = curve_surface(c, *r, s, TOL).unwrap();
            // Only hits on the part the spline covers (one nappe of the
            // cone, the sphere short of its poles) can be compared.
            if want.iter().any(|h| match h {
                CurveHit::Point { point, .. } => n.distance(*point) > 1e-9,
                CurveHit::On { .. } => true,
            }) {
                continue;
            }
            let got = curve_surface(c, *r, &n, TOL).unwrap();
            same(&got, &want, 1e-9);
        }
    }
}

#[test]
fn a_curve_lying_on_a_spline_is_on_it() {
    let f = frame();
    let cyl = Surface::Cylinder(Cylinder::new(f, 0.7).unwrap());
    let n = as_nurbs(&cyl, [[0.0, TAU], [-2.0, 2.0]]);
    // A ruling, and a circle round the axis.
    let o = f.origin() + f.radial(0.4) * 0.7;
    let ruling = Curve::Line(Line::new(o, f.z().get()).unwrap());
    let hits = curve_surface(&ruling, [-1.0, 1.5], &n, TOL).unwrap();
    assert_eq!(hits, [CurveHit::On { t: [-1.0, 1.5] }]);
    let round = Curve::Circle(
        Circle::new(
            Frame::new(f.origin() + f.z().get() * 0.3, f.z().get(), f.x().get()).unwrap(),
            0.7,
        )
        .unwrap(),
    );
    let hits = curve_surface(&round, [0.0, TAU], &n, TOL).unwrap();
    assert_eq!(hits, [CurveHit::On { t: [0.0, TAU] }]);
    // A line along a ruling but a little outside touches nothing.
    let off = Curve::Line(Line::new(f.origin() + f.radial(0.4) * 0.7001, f.z().get()).unwrap());
    assert!(
        curve_surface(&off, [-1.0, 1.5], &n, TOL)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_tangent_line_touches_a_spline_sphere() {
    let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let n = as_nurbs(&sphere, [[0.0, TAU], [-1.5, 1.5]]);
    // Tangent at the point at u = 1, v = 0.3, at t = 2.
    let p = sphere.point(1.0, 0.3);
    let d = p
        .to_vector()
        .cross(Vector3::new(0.0, 0.0, 1.0))
        .normalize()
        .unwrap()
        .get();
    let l = Curve::Line(Line::new(p - d * 2.0, d).unwrap());
    let hits = curve_surface(&l, [0.0, 4.0], &n, TOL).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    let CurveHit::Point {
        t, before, after, ..
    } = hits[0]
    else {
        panic!("{hits:?}")
    };
    assert!((t - 2.0).abs() <= 1e-3);
    assert_eq!((before, after), (Side::Positive, Side::Positive));
}

#[test]
fn swept_surfaces() {
    // An extrusion of a NURBS circle is the cylinder it sweeps; a
    // revolution of a circle off the axis is a torus.
    let f = frame();
    let c = Circle::new(f, 0.7).unwrap();
    let (nc, _) = c.to_nurbs().unwrap();
    let ext = Surface::Extrusion(Arc::new(
        Extrusion::new(Curve::Nurbs(Arc::new(nc)), f.z().get()).unwrap(),
    ));
    let cyl = Surface::Cylinder(Cylinder::new(f, 0.7).unwrap());
    // Counterclockwise about -y, so the natural normal points out of the
    // tube as the torus's does.
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
    for (c, r) in [
        (line((-2.0, 0.05, 0.25), (1.0, 0.1, 0.05)), [0.0, 4.0]),
        (line((0.1, -2.0, 0.1), (0.05, 1.0, -0.02)), [0.0, 4.0]),
        (line((-2.0, 0.3, -0.5), (1.0, 0.0, 0.5)), [0.0, 4.0]),
    ] {
        same(
            &curve_surface(&c, r, &ext, TOL).unwrap(),
            &curve_surface(&c, r, &cyl, TOL).unwrap(),
            1e-9,
        );
        same(
            &curve_surface(&c, r, &rev, TOL).unwrap(),
            &curve_surface(&c, r, &torus, TOL).unwrap(),
            1e-9,
        );
    }
}

#[test]
fn krawczyk_counts_solutions() {
    // x = y = z = 0 at the origin, a linear system.
    let jac = [
        [Iv::point(2.0), Iv::point(0.5), Iv::point(0.0)],
        [Iv::point(0.0), Iv::point(1.0), Iv::point(0.3)],
        [Iv::point(0.1), Iv::point(0.0), Iv::point(1.5)],
    ];
    let at = |x: [f64; 3]| -> [Iv; 3] {
        core::array::from_fn(|r| {
            Iv::point(jac[r][0].lo() * x[0] + jac[r][1].lo() * x[1] + jac[r][2].lo() * x[2])
        })
    };
    let around = [[-0.1, 0.12], [-0.13, 0.1], [-0.1, 0.1]];
    assert_eq!(krawczyk3(at([0.01, -0.015, 0.0]), jac, around), Some(true));
    let away = [[0.5, 0.7], [0.5, 0.7], [0.5, 0.7]];
    assert_eq!(krawczyk3(at([0.6, 0.6, 0.6]), jac, away), Some(false));
    assert_eq!(
        solve3(
            [[2.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 1.0]],
            [2.0, 2.0, 3.0]
        ),
        Some([1.0, 0.5, 3.0])
    );
}

#[test]
fn a_near_miss_within_the_tolerance_touches() {
    // A circle whose lowest point passes half a tolerance above the
    // plane: the analytic plane reports a touch, and so must its exact
    // NURBS form (a box whose enclosure of `E` has a certified sign, or
    // holds no exact zero, may still hold a point within the tolerance).
    let plane = Surface::Plane(Plane::new(Frame::WORLD));
    let n = as_nurbs(&plane, [[-1.0, 1.0], [-1.0, 1.0]]);
    let c = Curve::Circle(
        Circle::new(
            Frame::new(
                Point3::new(0.0, 0.0, 0.5 + 5e-9),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
            0.5,
        )
        .unwrap(),
    );
    let want = curve_surface(&c, [0.5, 2.5], &plane, TOL).unwrap();
    assert!(
        matches!(
            want[..],
            [CurveHit::Point {
                before: Side::Positive,
                after: Side::Positive,
                ..
            }]
        ),
        "{want:?}"
    );
    same(
        &curve_surface(&c, [0.5, 2.5], &n, TOL).unwrap(),
        &want,
        1e-3,
    );
}

#[test]
fn a_kinked_polyline_is_on_a_spline_plane_only_up_to_its_kink() {
    // A degree 1 spline along the plane to t = 0.3, then up off it: the
    // stretch on the plane ends at the kink, where the first derivative
    // jumps, however the subdivision cuts the parameter.
    let plane = Surface::Plane(Plane::new(Frame::WORLD));
    let n = as_nurbs(&plane, [[-1.0, 1.0], [-1.0, 1.0]]);
    let knots = crate::geom::KnotVector::clamped(1, &[0.0, 0.3, 1.0], &[2, 1, 2]).unwrap();
    let poly = Curve::Nurbs(Arc::new(
        crate::geom::NurbsCurve::new(
            knots,
            vec![
                Point3::new(-0.5, 0.1, 0.0),
                Point3::new(0.1, 0.1, 0.0),
                Point3::new(0.1, 0.1, 0.7),
            ],
            None,
        )
        .unwrap(),
    ));
    for s in [&plane, &n] {
        let got = curve_surface(&poly, [0.0, 1.0], s, TOL).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        let CurveHit::On { t } = got[0] else {
            panic!("{got:?}")
        };
        assert_eq!(t[0], 0.0);
        // It may run on past the kink as far as the curve stays within the
        // tolerance (a hundredth of a micron up the second segment).
        assert!((t[1] - 0.3).abs() <= 2.0 * TOL, "{got:?}");
        // Every point of the stretch is within the tolerance of the plane.
        for k in 0..=64 {
            let p = poly.point(t[0] + (t[1] - t[0]) * f64::from(k) / 64.0);
            assert!(p.z.abs() <= TOL, "{got:?} at {p:?}");
        }
    }
}

#[test]
fn a_kink_within_the_tolerance_touches() {
    // A degree 1 spline dipping to a vertex half a tolerance above the
    // plane and back up: its nearest point is the kink, where the
    // distance has no turning point.
    let plane = Surface::Plane(Plane::new(Frame::WORLD));
    let n = as_nurbs(&plane, [[-1.0, 1.0], [-1.0, 1.0]]);
    let knots = crate::geom::KnotVector::clamped(1, &[0.0, 0.5, 1.0], &[2, 1, 2]).unwrap();
    let vee = Curve::Nurbs(Arc::new(
        crate::geom::NurbsCurve::new(
            knots,
            vec![
                Point3::new(-0.5, 0.1, 0.3),
                Point3::new(0.0, 0.1, 5e-9),
                Point3::new(0.5, 0.1, 0.3),
            ],
            None,
        )
        .unwrap(),
    ));
    let got = curve_surface(&vee, [0.0, 1.0], &n, TOL).unwrap();
    assert!(
        matches!(
            got[..],
            [CurveHit::Point {
                before: Side::Positive,
                after: Side::Positive,
                ..
            }]
        ),
        "{got:?}"
    );
}

/// `f`'s result, or a failure if it takes longer than `secs` seconds (a
/// loop that never ends fails the test instead of hanging the run).
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .expect("finished in time")
}

#[test]
fn kinks_past_counting_are_given_up_on() {
    // A few periods' worth are listed.
    assert_eq!(
        between(&(vec![0.25], Some(1.0)), -1.0, 2.0),
        Some(vec![-0.75, 0.25, 1.25])
    );
    // Far more than `MAX_KINKS` periods.
    assert_eq!(
        within(10, || between(&(vec![0.25], Some(1.0)), -1e6, 1e6)),
        None
    );
    // A range of a few periods (under `MAX_KINKS`) past 2⁵³ of them,
    // where `n + 1` can equal `n`.
    assert_eq!(
        within(10, || between(&(vec![0.25], Some(1.0)), 1e17, 1e17 + 32.0)),
        None
    );
    assert_eq!(
        within(10, || between(
            &(vec![0.25], Some(1.0)),
            -1e17 - 32.0,
            -1e17
        )),
        None
    );
    // Bounds that aren't finite.
    assert_eq!(
        between(&(vec![0.25], Some(1.0)), f64::NEG_INFINITY, 0.0),
        None
    );
    assert_eq!(between(&(vec![0.25], Some(1.0)), f64::NAN, 0.0), None);
}

#[test]
fn a_path_across_countless_crease_lines_is_uncertain_not_endless() {
    // A circle lying on a cylinder, against crease lines 1e-15 radians
    // apart round its axis: every stretch the search can reach crosses
    // more than `MAX_KINKS` of them, so no path can be bounded, no contact
    // is ever certified, and the search ends uncertain at its depth
    // limit.
    let got = within(60, || {
        let f = frame();
        let cyl = Surface::Cylinder(Cylinder::new(f, 0.7).unwrap());
        let round = Curve::Circle(
            Circle::new(
                Frame::new(f.origin() + f.z().get() * 0.3, f.z().get(), f.x().get()).unwrap(),
                0.7,
            )
            .unwrap(),
        );
        let s = Search {
            curve: &round,
            surface: &cyl,
            sheet: Sheet::of(&cyl),
            range: [0.0, 1.0],
            tolerance: TOL,
            kinks: (Vec::new(), None),
            lines: [(vec![0.0], Some(1e-15)), (Vec::new(), None)],
        };
        let bound = s.path_bound([0.25, 0.5], None);
        let near = round.bounds([0.0, 1.0]).grow(TOL);
        let mut found = Vec::new();
        let mut budget = MAX_BOXES;
        let mut result = Ok(());
        'all: for ts in s.t_cells(0.0) {
            for &[u, v] in &super::super::cells::cells(&cyl, &near) {
                result = s.search([ts, u, v], 0, &mut budget, &mut found);
                if result.is_err() {
                    break 'all;
                }
            }
        }
        (bound, result)
    });
    assert_eq!(got.0, f64::INFINITY);
    assert_eq!(got.1, Err(Failure::Uncertain));
}

/// Lines through a pole of a sphere's spline form meet it there; the
/// axis through both poles twice, the side read off the limit normal.
#[test]
fn lines_through_a_spline_sphere_s_poles() {
    let ball = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let spline = as_nurbs(
        &ball,
        [
            [0.0, TAU],
            [-core::f64::consts::FRAC_PI_2, core::f64::consts::FRAC_PI_2],
        ],
    );
    let axis = line((0.0, 0.0, -2.0), (0.0, 0.0, 1.0));
    let hits = curve_surface(&axis, [0.0, 4.0], &spline, TOL).unwrap();
    let want = curve_surface(&axis, [0.0, 4.0], &ball, TOL).unwrap();
    same(&hits, &want, 1e-6);
    let tilted = line((0.0, 0.0, 1.0), (0.3, 0.1, 1.0));
    let hits = curve_surface(&tilted, [-3.0, 3.0], &spline, TOL).unwrap();
    let want = curve_surface(&tilted, [-3.0, 3.0], &ball, TOL).unwrap();
    same(&hits, &want, 1e-6);
}

/// A line crossing a sphere's spline form exactly on its seam, and one
/// along its equator's plane through a knot line: found on the attempt
/// whose cells are cut off the surface's pieces.
#[test]
fn lines_on_seams_and_knot_lines() {
    let ball = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
    let spline = as_nurbs(
        &ball,
        [
            [0.0, TAU],
            [-core::f64::consts::FRAC_PI_2, core::f64::consts::FRAC_PI_2],
        ],
    );
    for c in [
        line((0.1, 0.0, -2.0), (0.0, 0.0, 1.0)),
        line((0.0, 0.0, 0.0), (1.0, 0.0, 0.0)),
    ] {
        let hits = curve_surface(&c, [0.0, 4.0], &spline, TOL).unwrap();
        let want = curve_surface(&c, [0.0, 4.0], &ball, TOL).unwrap();
        same(&hits, &want, 1e-6);
    }
}
