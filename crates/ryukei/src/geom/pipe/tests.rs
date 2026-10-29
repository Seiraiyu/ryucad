use super::*;
use crate::geom::{Circle, Cylinder, Ellipse, Frame, KnotVector, Line, Plane, Sphere};
use core::f64::consts::FRAC_PI_2;
use std::sync::Arc;

pub(crate) const R: f64 = 0.25;

/// A wavy cubic spine at height `R` over the plane `z = 0`: a ball of
/// radius `R` on it rolls on the plane.
pub(crate) fn on_plane() -> Pipe {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
    let pts = vec![
        Point3::new(0.0, 0.0, R),
        Point3::new(1.0, 1.0, R),
        Point3::new(2.0, -0.5, R),
        Point3::new(3.0, 0.5, R),
        Point3::new(4.0, 0.0, R),
    ];
    let spine = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, None).unwrap()));
    Pipe::new(spine, R, Surface::Plane(Plane::new(Frame::WORLD))).unwrap()
}

/// An ellipse where a plane tilted by 30° about `x` meets the cylinder
/// of radius `3 + R` about `z`: a ball on it rolls round the cylinder of
/// radius 3.
pub(crate) fn round_cylinder() -> Pipe {
    let tilt = 30f64.to_radians();
    let (s, c) = math::sin_cos(tilt);
    let major = Vector3::new(0.0, c, s);
    let normal = Vector3::new(0.0, -s, c);
    let frame = Frame::new(Point3::new(0.0, 0.0, 1.0), normal, major).unwrap();
    let rho = 3.0 + R;
    let spine = Curve::Ellipse(Ellipse::new(frame, rho / c, rho).unwrap());
    let anchor = Surface::Cylinder(Cylinder::new(Frame::WORLD, 3.0).unwrap());
    Pipe::new(spine, R, anchor).unwrap()
}

/// [`on_plane`] anchored on a rail instead: the plane as a NURBS, the
/// rail the spine's shadow (the same control points with `z` dropped). It
/// is the same pipe.
pub(crate) fn on_spline_plane() -> Pipe {
    let p = on_plane();
    let (n, _) = Surface::Plane(Plane::new(Frame::WORLD))
        .to_nurbs([[-1.0, 5.0], [-2.0, 2.0]])
        .unwrap();
    let Curve::Nurbs(spine) = p.spine() else {
        unreachable!()
    };
    let shadow = crate::geom::NurbsCurve2::new(
        spine.knots().clone(),
        spine
            .control_points()
            .map(|q| crate::geom::Point2::new(q.x, q.y))
            .collect(),
        None,
    )
    .unwrap();
    Pipe::on_rail(
        p.spine().clone(),
        R,
        Surface::Nurbs(Arc::new(n)),
        Curve2::Nurbs2(Arc::new(shadow)),
        [0.0, 1.0],
    )
    .unwrap()
}

/// A pipe round an ellipse a little outside a cylinder of radius 3
/// about `z`, anchored on the cylinder as an extrusion of a circle, its
/// rail the circle at the ellipse's height by the same angle.
pub(crate) fn round_extrusion() -> Pipe {
    let e = Ellipse::new(
        Frame::new(
            Point3::new(0.0, 0.0, 0.5),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        3.4,
        3.3,
    )
    .unwrap();
    let base = Curve::Circle(Circle::new(Frame::WORLD, 3.0).unwrap());
    let anchor = Surface::Extrusion(Arc::new(
        crate::geom::Extrusion::new(base, Vector3::new(0.0, 0.0, 1.0)).unwrap(),
    ));
    let rail = Curve2::Line2(
        crate::geom::Line2::new(
            crate::geom::Point2::new(0.0, 0.5),
            crate::geom::Vector2::new(1.0, 0.0),
        )
        .unwrap(),
    );
    Pipe::on_rail(Curve::Ellipse(e), R, anchor, rail, [0.0, 1.0]).unwrap()
}

fn both() -> [Pipe; 3] {
    [on_plane(), round_cylinder(), on_spline_plane()]
}

/// A pipe on a rail is the pipe on an analytic anchor where the rail is
/// that anchor's foot.
#[test]
fn rails_anchor_like_feet() {
    let (a, b) = (on_plane(), on_spline_plane());
    for (s, t) in grid(&a) {
        assert!(
            a.point(s, t).distance(b.point(s, t)) <= 1e-14,
            "at {s}, {t}"
        );
        let (ea, eb) = (a.eval(s, t), b.eval(s, t));
        for (x, y) in [(ea.du, eb.du), (ea.dv, eb.dv), (ea.duu, eb.duu)] {
            assert!((x - y).length() <= 1e-12 * (1.0 + x.length()));
        }
    }
    assert!(b.rail().is_some() && a.rail().is_none());
    assert_ne!(a, b);
}

/// On any rail, `θ = 0` is a radius from the spine towards the rail,
/// across the spine's tangent; derivatives and enclosures hold.
#[test]
fn a_curved_rail() {
    let p = round_extrusion();
    let rail = |s: f64| Point3::new(3.0 * math::cos(s), 3.0 * math::sin(s), 0.5);
    for (s, t) in grid(&p) {
        let c = p.spine().eval(s);
        let w = rail(s) - c.point;
        let t1 = c.d1 / c.d1.length();
        let toward = (w - t1 * w.dot(t1)).normalize().unwrap().get();
        if t == 0.0 {
            assert!(p.point(s, 0.0).distance(c.point + toward * R) <= 1e-13);
        }
        let h = 1e-5;
        let e = p.eval(s, t);
        let close = |a: Vector3, b: Vector3| (a - b).length() <= 1e-5 * (1.0 + b.length());
        assert!(close(
            e.du,
            (p.point(s + h, t) - p.point(s - h, t)) / (2.0 * h)
        ));
        assert!(close(
            e.duu,
            (p.eval(s + h, t).du - p.eval(s - h, t).du) / (2.0 * h)
        ));
    }
    // Enclosures over stretches of a twentieth of a radian: the frame's
    // over a much wider one need not be certified (callers halve).
    for cell in [[[0.1, 0.15], [-0.5, 0.5]], [[2.0, 2.05], [1.0, 3.0]]] {
        let [q, _] = p.jet_box(cell).unwrap();
        for i in 0..=4 {
            for j in 0..=4 {
                let s = cell[0][0] + (cell[0][1] - cell[0][0]) * f64::from(i) / 4.0;
                let t = cell[1][0] + (cell[1][1] - cell[1][0]) * f64::from(j) / 4.0;
                let e = p.eval(s, t);
                for (k, x) in e.point.to_vector().to_array().into_iter().enumerate() {
                    assert!(q[k].v.lo() - 1e-12 <= x && x <= q[k].v.hi() + 1e-12);
                }
                for (k, x) in e.duu.to_array().into_iter().enumerate() {
                    assert!(q[k].h[0].lo() - 1e-9 <= x && x <= q[k].h[0].hi() + 1e-9);
                }
            }
        }
    }
    let b = p.derivative_bound([[0.0, TAU], [-PI, PI]]);
    for (s, t) in grid(&p) {
        assert!(p.eval(s, t).duu.length() <= b.duu * (1.0 + 1e-9));
    }
    // Offsets keep the rail; mirrors map it.
    let o = p.offset(0.05).unwrap();
    assert_eq!(o.rail(), p.rail());
    let m = crate::geom::Mirror::new(
        Point3::new(0.5, -1.0, 2.0),
        Vector3::new(1.0, 0.3, -0.6).normalize().unwrap(),
    );
    let mp = p.mirrored(&m).unwrap();
    for (s, t) in grid(&p) {
        let want = m.apply_point(p.point(s, t));
        assert!(mp.point(s, -t).distance(want) <= 1e-12, "at {s}, {t}");
    }
}

#[test]
fn rail_refusals() {
    let p = on_plane();
    let rail = || on_spline_plane().rail().unwrap().0.clone();
    let anchor = on_spline_plane().anchor().clone();
    let at = |map: [f64; 2], r: f64, anchor: Surface| {
        Pipe::on_rail(p.spine().clone(), r, anchor, rail(), map)
    };
    assert_eq!(at([0.0, 0.0], R, anchor.clone()), Err(Error::Degenerate));
    assert_eq!(
        at([f64::NAN, 1.0], R, anchor.clone()),
        Err(Error::NonFinite)
    );
    assert_eq!(at([0.0, 1.0], 1e-9, anchor.clone()), Err(Error::Degenerate));
    // A rail on the spine itself: nothing to point towards.
    let up = anchor.transformed(&Rigid::translation(Vector3::new(0.0, 0.0, R)));
    assert_eq!(at([0.0, 1.0], R, up), Err(Error::Degenerate));
}

/// Parameters spread over a pipe's domain.
fn grid(p: &Pipe) -> Vec<(f64, f64)> {
    let [d0, d1] = p.spine().domain();
    let mut out = Vec::new();
    for i in 0..=12 {
        for j in 0..=12 {
            let s = d0 + (d1 - d0) * f64::from(i) / 12.0;
            let t = -PI + TAU * f64::from(j) / 12.0;
            out.push((s, t));
        }
    }
    out
}

#[test]
fn circles_about_the_spine() {
    for p in both() {
        for (s, t) in grid(&p) {
            let c = p.spine().eval(s);
            let x = p.point(s, t);
            assert!((x.distance(c.point) - R).abs() <= 1e-14, "at {s}, {t}");
            assert!((x - c.point).dot(c.d1).abs() <= 1e-13 * c.d1.length());
            // The natural normal points away from the spine.
            let n = p.normal(s, t).unwrap().get();
            assert!((n - (x - c.point) / R).length() <= 1e-13);
        }
    }
}

#[test]
fn theta_zero_touches_the_anchor() {
    for p in both() {
        let [d0, d1] = p.spine().domain();
        for i in 0..=40 {
            let s = d0 + (d1 - d0) * f64::from(i) / 40.0;
            let x = p.point(s, 0.0);
            assert!(p.anchor().distance(x) <= 1e-13, "at {s}");
            // And the seam is as far from it as the pipe goes.
            assert!((p.anchor().distance(p.point(s, PI)) - 2.0 * R).abs() <= 1e-3);
        }
    }
    // On the plane the contact is the spine's shadow.
    let p = on_plane();
    let c = p.spine().point(0.7);
    let x = p.point(0.7, 0.0);
    assert!((x - Point3::new(c.x, c.y, 0.0)).length() <= 1e-15);
}

#[test]
fn derivatives_against_differences() {
    for p in both() {
        let [d0, d1] = p.spine().domain();
        for (s, t) in grid(&p) {
            // The spine's third derivative jumps at its knot (s = 1 on
            // the plane's), and with it Suu.
            if s - d0 < 1e-3 || d1 - s < 1e-3 || (p.spine().period().is_none() && s == 1.0) {
                continue;
            }
            let e = p.eval(s, t);
            assert_eq!(e.point, p.point(s, t));
            let h = 1e-5;
            let du = (p.point(s + h, t) - p.point(s - h, t)) / (2.0 * h);
            let dv = (p.point(s, t + h) - p.point(s, t - h)) / (2.0 * h);
            assert!(
                (e.du - du).length() <= 1e-7 * (1.0 + du.length()),
                "{s} {t}"
            );
            assert!((e.dv - dv).length() <= 1e-8, "{s} {t}");
            let duu = (p.eval(s + h, t).du - p.eval(s - h, t).du) / (2.0 * h);
            let duv = (p.eval(s, t + h).du - p.eval(s, t - h).du) / (2.0 * h);
            let dvv = (p.eval(s, t + h).dv - p.eval(s, t - h).dv) / (2.0 * h);
            assert!(
                (e.duu - duu).length() <= 1e-6 * (1.0 + duu.length()),
                "{s} {t} {:?} {duu:?}",
                e.duu
            );
            assert!(
                (e.duv - duv).length() <= 1e-6 * (1.0 + duv.length()),
                "{s} {t}"
            );
            assert!((e.dvv - dvv).length() <= 1e-7, "{s} {t}");
            // du × dv along the natural normal.
            let n = e.normal().unwrap().get();
            assert!((n - p.normal(s, t).unwrap().get()).length() <= 1e-12);
        }
    }
}

#[test]
fn domain_and_periods() {
    let p = on_plane();
    assert_eq!(p.domain(), [[0.0, 2.0], [-PI, PI]]);
    assert_eq!(p.periods(), (None, Some(TAU)));
    assert_eq!(p.closed_genus(), None);
    let q = round_cylinder();
    assert_eq!(q.periods(), (Some(TAU), Some(TAU)));
    assert_eq!(q.closed_genus(), Some(1));
    let s = Surface::Pipe(Arc::new(q.clone()));
    assert_eq!(s.frame(), None);
    assert_eq!(s.domain(), q.domain());
    assert_eq!(s.offset(0.1).unwrap().unwrap().point(1.0, 0.5), {
        let o = q.offset(0.1).unwrap();
        o.point(1.0, 0.5)
    });
}

#[test]
fn offsets_grow_the_radius() {
    for p in both() {
        let o = p.offset(0.05).unwrap();
        assert_eq!(o.radius(), R + 0.05);
        for (s, t) in grid(&p) {
            let n = p.normal(s, t).unwrap().get();
            assert!(o.point(s, t).distance(p.point(s, t) + n * 0.05) <= 1e-13);
        }
        // Down to nothing, or past the spine's curvature, is refused.
        assert_eq!(p.offset(-R), Err(Error::Degenerate));
    }
}

#[test]
fn refusals() {
    let plane = Surface::Plane(Plane::new(Frame::WORLD));
    let line =
        Curve::Line(Line::new(Point3::new(0.0, 0.0, 1.0), Vector3::new(1.0, 0.0, 0.0)).unwrap());
    assert_eq!(Pipe::new(line, 0.5, plane.clone()), Err(Error::Degenerate));
    let circle = Curve::Circle(Circle::new(Frame::WORLD, 1.0).unwrap());
    assert_eq!(
        Pipe::new(circle, 0.5, plane.clone()),
        Err(Error::Degenerate)
    );
    let p = on_plane();
    assert_eq!(
        Pipe::new(p.spine().clone(), f64::NAN, plane.clone()),
        Err(Error::NonFinite)
    );
    assert_eq!(
        Pipe::new(p.spine().clone(), 1e-9, plane.clone()),
        Err(Error::Degenerate)
    );
    // A radius past the spine's curvature folds the pipe.
    assert_eq!(
        Pipe::new(p.spine().clone(), 5.0, plane.clone()),
        Err(Error::Degenerate)
    );
    // A NURBS anchor.
    let (n, _) = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap())
        .to_nurbs([[0.0; 2]; 2])
        .unwrap();
    assert_eq!(
        Pipe::new(p.spine().clone(), R, Surface::Nurbs(Arc::new(n))),
        Err(Error::Degenerate)
    );
    // A spine that crosses the anchor.
    let low = Surface::Plane(Plane::new(
        Frame::new(
            Point3::new(0.0, 0.0, R),
            Vector3::new(0.0, 1.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
    ));
    assert_eq!(Pipe::new(p.spine().clone(), R, low), Err(Error::Degenerate));
    // A spine whose tangent runs along the anchor's normal somewhere.
    let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let up = NurbsCurve::new(
        k,
        vec![
            Point3::new(0.0, 0.0, 1.0),
            Point3::new(1.0, 0.0, 1.0),
            Point3::new(1.0, 0.0, 3.0),
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        Pipe::new(Curve::Nurbs(Arc::new(up)), 0.1, plane),
        Err(Error::Degenerate)
    );
}

#[test]
fn projections_find_the_nearest_point() {
    for p in both() {
        // Points off the surface along its normal come back to their foot.
        for (s, t) in grid(&p) {
            for d in [0.0, 0.01, -0.05] {
                let n = p.normal(s, t).unwrap().get();
                let x = p.point(s, t) + n * d;
                let f = p.project(x);
                assert!(
                    (f.distance - d.abs()).abs() <= 1e-12,
                    "{s} {t} {d}: {}",
                    f.distance
                );
                assert!(f.point.distance(p.point(s, t)) <= 1e-9, "{s} {t} {d}");
                assert_eq!(f.point, p.point(f.param.0, f.param.1));
            }
        }
        // Points anywhere are no farther than the nearest of a dense
        // sample of the surface.
        let b = p.bounds();
        let (lo, hi) = (b.min(), b.max());
        let samples: Vec<Point3> = {
            let [d0, d1] = p.spine().domain();
            let mut v = Vec::new();
            for i in 0..=400 {
                for j in 0..=60 {
                    let s = d0 + (d1 - d0) * f64::from(i) / 400.0;
                    let t = -PI + TAU * f64::from(j) / 60.0;
                    v.push(p.point(s, t));
                }
            }
            v
        };
        for k in 0..40 {
            let f = |a: f64, b: f64, i: u32| a + (b - a) * (f64::from(i * 37 % 101) / 100.0);
            let x = Point3::new(
                f(lo.x, hi.x, k),
                f(lo.y, hi.y, k + 13),
                f(lo.z, hi.z, k + 29),
            );
            let near = samples
                .iter()
                .map(|q| q.distance(x))
                .fold(f64::INFINITY, f64::min);
            let got = p.project(x);
            assert!(
                got.distance <= near + 1e-9,
                "{x:?}: {} > {near}",
                got.distance
            );
            assert!((got.point.distance(x) - got.distance).abs() <= 1e-15);
        }
    }
}

#[test]
fn bounds_and_moves() {
    for p in both() {
        let b = p.bounds().grow(1e-12);
        for (s, t) in grid(&p) {
            let x = p.point(s, t);
            assert!(
                x.x >= b.min().x && x.y >= b.min().y && x.z >= b.min().z,
                "{x:?}"
            );
            assert!(x.x <= b.max().x && x.y <= b.max().y && x.z <= b.max().z);
        }
        let r = Rigid::rotation(Vector3::new(1.0, 2.0, 3.0).normalize().unwrap(), 0.7)
            .then_after(&Rigid::translation(Vector3::new(1.0, -2.0, 0.5)));
        let m = p.transformed(&r);
        for (s, t) in grid(&p) {
            assert!(m.point(s, t).distance(r.apply_point(p.point(s, t))) <= 1e-12);
        }
    }
}

#[test]
fn derivative_bounds_hold() {
    for p in both() {
        let [d0, d1] = p.spine().domain();
        let h = (d1 - d0) / 7.0;
        for i in 0..7 {
            let rs = [d0 + h * f64::from(i), d0 + h * f64::from(i + 1)];
            for rt in [[-PI, PI], [-0.3, 0.4], [2.0, 3.0]] {
                let b = Surface::Pipe(Arc::new(p.clone())).derivative_bound([rs, rt]);
                assert!(b.duu.is_finite() && b.turn_u.is_finite(), "{rs:?} {rt:?}");
                for a in 0..=10 {
                    for c in 0..=10 {
                        let s = rs[0] + (rs[1] - rs[0]) * f64::from(a) / 10.0;
                        let t = rt[0] + (rt[1] - rt[0]) * f64::from(c) / 10.0;
                        let e = p.eval(s, t);
                        assert!(e.duu.length() <= b.duu, "duu at {s}, {t}");
                        assert!(e.duv.length() <= b.duv, "duv at {s}, {t}");
                        assert!(e.dvv.length() <= b.dvv, "dvv at {s}, {t}");
                        let k = 1e-6;
                        let turn = |a: Option<UnitVector3>, b: Option<UnitVector3>| {
                            (a.unwrap().get() - b.unwrap().get()).length() / (2.0 * k)
                        };
                        let tu = turn(p.normal(s + k, t), p.normal(s - k, t));
                        assert!(tu <= b.turn_u * (1.0 + 1e-6), "turn_u at {s}, {t}");
                        let tv = turn(p.normal(s, t + k), p.normal(s, t - k));
                        assert!(tv <= b.turn_v * (1.0 + 1e-6), "turn_v at {s}, {t}");
                    }
                }
            }
        }
        // Over a small box the bounds are close to the true values: the
        // frame's enclosures are tabulated over stretches of spine, each
        // narrow to an eighth of its size.
        let (s, t) = (d0 + (d1 - d0) * 0.3, FRAC_PI_2);
        let b = p.derivative_bound([[s, s + 1e-4], [t, t + 1e-4]]);
        assert!(
            b.dvv <= R * 1.01 && b.duu <= p.eval(s, t).duu.length() * 1.5 + 1e-9,
            "{b:?} {:?}",
            p.eval(s, t)
        );
    }
}

#[test]
fn pcurves_on_pipes() {
    // Round the cylinder, the ball touches it along an ellipse with the
    // spine's own parameter: the spine scaled by 3 / (3 + R) towards the
    // axis.
    let p = round_cylinder();
    let Curve::Ellipse(e) = p.spine() else {
        unreachable!()
    };
    let k = 3.0 / (3.0 + R);
    let f = e.frame();
    let major = f.x().get() * e.major();
    let squeezed = Vector3::new(major.x * k, major.y * k, major.z);
    let frame = Frame::new(f.origin(), squeezed.cross(f.y().get()), squeezed).unwrap();
    let contact = Curve::Ellipse(Ellipse::new(frame, squeezed.length(), e.minor() * k).unwrap());
    for i in 0..8 {
        let t = TAU * f64::from(i) / 8.0;
        assert!(contact.point(t).distance(p.point(t, 0.0)) <= 1e-13, "{t}");
    }
    let surface = Surface::Pipe(Arc::new(p.clone()));
    // An exact pcurve: the line θ = 0, by the spine's parameter.
    let (c2, map) = crate::pcurve::exact(&surface, &contact, [0.5, 2.0], 1e-8).unwrap();
    let q = c2.point(map[0] + map[1] * 1.3);
    assert!((q.x - 1.3).abs() <= 1e-9 && q.y.abs() <= 1e-9, "{q:?}");
    // A fitted one, certified through the pipe's first derivative bounds.
    let (fitted, bound) = crate::intersect::fit_pcurve(&surface, &contact, [0.5, 2.0]).unwrap();
    assert!(bound <= 10.0 * crate::tol::LINEAR_RESOLUTION, "{bound}");
    for i in 0..=20 {
        let t = 0.5 + 1.5 * f64::from(i) / 20.0;
        let uv = fitted.point(t);
        assert!(surface.point(uv.x, uv.y).distance(contact.point(t)) <= bound);
    }
    // First derivative bounds hold.
    let (su, sv) = p.first_bound([[0.5, 2.0], [-1.0, 1.0]]);
    for i in 0..=20 {
        for j in 0..=20 {
            let e = p.eval(
                0.5 + 1.5 * f64::from(i) / 20.0,
                -1.0 + 2.0 * f64::from(j) / 20.0,
            );
            assert!(e.du.length() <= su && e.dv.length() <= sv);
        }
    }
}

#[test]
fn nurbs_within_a_tolerance() {
    for (p, range) in [(on_plane(), [0.0, 2.0]), (round_cylinder(), [0.5, 3.0])] {
        let (n, [rs, rt], bound) = p.to_nurbs_within(range, 1e-6).unwrap();
        assert!(bound <= 1e-6, "{bound}");
        assert_eq!((rs, rt), (Reparam::Identity, Reparam::Angle));
        for i in 0..=24 {
            for j in 0..=24 {
                let s = range[0] + (range[1] - range[0]) * f64::from(i) / 24.0;
                let t = TAU * f64::from(j) / 24.0;
                let d = n.point(s, rt.to_nurbs(t)).distance(p.point(s, t));
                assert!(d <= bound, "{d} > {bound} at {s}, {t}");
            }
        }
        // Tighter takes more spans.
        let (fine, ..) = p.to_nurbs_within(range, 1e-8).unwrap();
        assert!(fine.counts()[0] > n.counts()[0]);
    }
    // Outside a clamped spine's domain.
    let p = on_plane();
    assert_eq!(
        p.to_nurbs_within([-1.0, 1.0], 1e-6).err(),
        Some(Error::Degenerate)
    );
}
