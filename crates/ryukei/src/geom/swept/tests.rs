use super::*;
use crate::geom::{Circle, Cylinder, Ellipse, Frame, KnotVector, Surface};
use core::f64::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

fn frame() -> Frame {
    Frame::new(
        Point3::new(1.0, 2.0, 3.0),
        Vector3::new(0.3, -0.2, 1.0),
        Vector3::new(1.0, 0.5, 0.0),
    )
    .unwrap()
}

fn nurbs(knots: KnotVector, points: Vec<Point3>, weights: Vec<f64>) -> Curve {
    Curve::Nurbs(Arc::new(
        NurbsCurve::new(knots, points, Some(weights)).unwrap(),
    ))
}

/// A planar cubic in `f`'s xy plane: a wave over two spans, with uneven
/// weights.
fn wave(f: &Frame) -> Curve {
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.5], &[4, 1, 4]).unwrap();
    let local = [(0.0, 0.0), (1.0, 2.0), (2.0, -1.0), (3.5, 1.5), (5.0, 0.0)];
    let points = local.iter().map(|&(a, b)| f.to_world(a, b, 0.0)).collect();
    nurbs(k, points, vec![1.0, 0.7, 1.6, 0.9, 1.0])
}

/// A closed periodic quadratic in `f`'s xy plane.
fn blob(f: &Frame) -> Curve {
    let k = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[1; 6]).unwrap();
    let local = [
        (2.0, 0.0),
        (1.0, 1.5),
        (-1.0, 2.0),
        (-2.0, 0.0),
        (0.0, -2.5),
    ];
    let points = local.iter().map(|&(a, b)| f.to_world(a, b, 0.0)).collect();
    nurbs(k, points, vec![1.0, 2.0, 0.5, 1.0, 1.5])
}

fn extrusion(curve: Curve, dir: Vector3) -> Surface {
    Surface::Extrusion(Arc::new(Extrusion::new(curve, dir).unwrap()))
}

/// Extrusions of every curve kind, straight and oblique.
fn extrusions() -> Vec<Surface> {
    let f = frame();
    let z = f.z().get();
    let slant = z + f.x().get() * 0.8 - f.y().get() * 0.3;
    let circle = Curve::Circle(Circle::new(f, 2.0).unwrap());
    let ellipse = Curve::Ellipse(Ellipse::new(f, 3.0, 1.25).unwrap());
    vec![
        extrusion(circle.clone(), z),
        extrusion(circle, slant),
        extrusion(ellipse.clone(), -z),
        extrusion(ellipse, slant),
        extrusion(wave(&f), z),
        extrusion(wave(&f), slant),
        extrusion(blob(&f), -slant),
    ]
}

/// Finite parameters spread over a surface's domain, `v` in `[-2, 2]`
/// where it is unbounded.
fn grid(s: &Surface, n: u32) -> Vec<(f64, f64)> {
    spread(s, n, 0.0)
}

/// As [`grid`], with the samples moved half a step in from the ends, so
/// they miss the knots of the curves here.
fn inner(s: &Surface, n: u32) -> Vec<(f64, f64)> {
    spread(s, n, 0.5)
}

fn spread(s: &Surface, n: u32, shift: f64) -> Vec<(f64, f64)> {
    let [du, dv] = s.domain();
    let finite = |[a, b]: [f64; 2]| {
        if a.is_finite() && b.is_finite() {
            [a, b]
        } else {
            [-2.0, 2.0]
        }
    };
    let ([u0, u1], [v0, v1]) = (finite(du), finite(dv));
    let mut out = Vec::new();
    for i in 0..=n {
        for j in 0..=n {
            let step = |k: u32| (f64::from(k) + shift) / (f64::from(n) + 2.0 * shift);
            out.push((u0 + (u1 - u0) * step(i), v0 + (v1 - v0) * step(j)));
        }
    }
    out
}

fn agree(a: Vector3, b: Vector3, tol: f64) -> bool {
    (a - b).length() <= tol * a.length().max(b.length()).max(1.0)
}

fn derivatives_match_finite_differences(s: &Surface) {
    let h = 1e-5;
    for (u, v) in inner(s, 7) {
        let e = s.eval(u, v);
        let at = format!("{s:?} at ({u}, {v})");
        assert!(e.point.distance(s.point(u, v)) <= 1e-15, "{at}");
        let d = |du: f64, dv: f64| s.point(u + du, v + dv);
        let fu = (d(h, 0.0) - d(-h, 0.0)) / (2.0 * h);
        let fv = (d(0.0, h) - d(0.0, -h)) / (2.0 * h);
        assert!(agree(e.du, fu, 1e-7), "du {at}");
        assert!(agree(e.dv, fv, 1e-7), "dv {at}");
        let ev = |du: f64, dv: f64| s.eval(u + du, v + dv);
        let fuu = (ev(h, 0.0).du - ev(-h, 0.0).du) / (2.0 * h);
        let fuv = (ev(0.0, h).du - ev(0.0, -h).du) / (2.0 * h);
        let fvv = (ev(0.0, h).dv - ev(0.0, -h).dv) / (2.0 * h);
        assert!(agree(e.duu, fuu, 1e-6), "duu {at}");
        assert!(agree(e.duv, fuv, 1e-6), "duv {at}");
        assert!(agree(e.dvv, fvv, 1e-6), "dvv {at}");
    }
}

/// Where `du × dv` gives a normal, the surface's natural normal is that
/// one.
fn normal_is_natural(s: &Surface) {
    for (u, v) in grid(s, 9) {
        let n = s.normal(u, v).unwrap().get();
        assert!((n.length() - 1.0).abs() <= 1e-15);
        if let Some(m) = s.eval(u, v).normal() {
            assert!((n - m.get()).length() <= 1e-12, "{s:?} at ({u}, {v})");
        }
    }
}

fn nurbs_agrees(s: &Surface, ranges: [[f64; 2]; 2]) {
    let (n, [ru, rv]) = s.to_nurbs(ranges).unwrap();
    let [[u0, u1], [v0, v1]] = ranges;
    for i in 0..=24 {
        for j in 0..=24 {
            let u = u0 + (u1 - u0) * f64::from(i) / 24.0;
            let v = v0 + (v1 - v0) * f64::from(j) / 24.0;
            let (a, b) = (ru.to_nurbs(u), rv.to_nurbs(v));
            let d = n.point(a, b).distance(s.point(u, v));
            assert!(d <= 1e-12, "{s:?} at ({u}, {v}): {d}");
            if let (Some(m), Some(k)) = (s.eval(u, v).normal(), n.normal(a, b)) {
                assert!((k.get() - m.get()).length() <= 1e-9, "{s:?} at ({u}, {v})");
            }
        }
    }
}

/// The nearest point by brute force: the best of a dense grid over
/// `ranges`, polished by Newton's method on the squared distance.
fn brute_force(s: &Surface, p: Point3, [[u0, u1], [v0, v1]]: [[f64; 2]; 2]) -> f64 {
    let n = 160;
    let mut best = (f64::INFINITY, 0.0, 0.0);
    for i in 0..=n {
        for j in 0..=n {
            let u = u0 + (u1 - u0) * f64::from(i) / f64::from(n);
            let v = v0 + (v1 - v0) * f64::from(j) / f64::from(n);
            let d = s.point(u, v).distance(p);
            if d < best.0 {
                best = (d, u, v);
            }
        }
    }
    let (_, mut u, mut v) = best;
    for _ in 0..30 {
        let e = s.eval(u, v);
        let r = e.point - p;
        let (gu, gv) = (r.dot(e.du), r.dot(e.dv));
        let a = e.du.dot(e.du) + r.dot(e.duu);
        let b = e.du.dot(e.dv) + r.dot(e.duv);
        let c = e.dv.dot(e.dv) + r.dot(e.dvv);
        let det = a * c - b * b;
        if det <= 0.0 || a <= 0.0 {
            break;
        }
        let (nu, nv) = (u - (c * gu - b * gv) / det, v - (a * gv - b * gu) / det);
        if !(u0..=u1).contains(&nu) || !(v0..=v1).contains(&nv) {
            break;
        }
        (u, v) = (nu, nv);
    }
    best.0.min(s.point(u, v).distance(p))
}

fn projection_is_nearest(s: &Surface, p: Point3, ranges: [[f64; 2]; 2]) {
    let foot = s.project(p);
    let (u, v) = foot.param;
    assert_eq!(foot.point, s.point(u, v), "{s:?} {p:?}");
    assert_eq!(foot.distance, foot.point.distance(p));
    assert_eq!(s.distance(p), foot.distance);
    let brute = brute_force(s, p, ranges);
    assert!(
        foot.distance <= brute + LINEAR_RESOLUTION,
        "{s:?} {p:?}: {} vs {brute}",
        foot.distance
    );
}

/// Checks a bound against the derivatives on a grid of the box, and the
/// normal's turning against finite differences.
fn bound_holds(s: &Surface, [ru, rv]: [[f64; 2]; 2]) {
    let b = s.derivative_bound([ru, rv]);
    for i in 0..=30 {
        for j in 0..=30 {
            let u = ru[0] + (ru[1] - ru[0]) * f64::from(i) / 30.0;
            let v = rv[0] + (rv[1] - rv[0]) * f64::from(j) / 30.0;
            let e = s.eval(u, v);
            let at = format!("{s:?} at ({u}, {v}): {b:?}");
            assert!(e.duu.length() <= b.duu, "duu {at}");
            assert!(e.duv.length() <= b.duv, "duv {at}");
            assert!(e.dvv.length() <= b.dvv, "dvv {at}");
            let Some(n) = s.normal(u, v) else { continue };
            let h = 1e-7;
            let turn = |du: f64, dv: f64| {
                let (a, c) = (u + du, v + dv);
                if a > ru[1] || c > rv[1] {
                    return 0.0;
                }
                s.normal(a, c)
                    .map_or(0.0, |m| (m.get() - n.get()).length() / h)
            };
            assert!(
                turn(h, 0.0) <= b.turn_u * (1.0 + 1e-4) + 1e-6,
                "turn_u {at}"
            );
            assert!(
                turn(0.0, h) <= b.turn_v * (1.0 + 1e-4) + 1e-6,
                "turn_v {at}"
            );
        }
    }
}

fn transform_round_trips(s: &Surface) {
    let axis = Vector3::new(0.3, 1.0, -0.4).normalize().unwrap();
    let r =
        Rigid::translation(Vector3::new(3.0, -1.0, 2.0)).then_after(&Rigid::rotation(axis, 0.9));
    let t = s.transformed(&r);
    let back = t.transformed(&r.inverse());
    for (u, v) in grid(s, 6) {
        let p = s.point(u, v);
        assert!(t.point(u, v).distance(r.apply_point(p)) <= 1e-12);
        assert!(back.point(u, v).distance(p) <= 1e-12);
        let n = s.normal(u, v).unwrap();
        let m = t.normal(u, v).unwrap();
        assert!((m.get() - r.apply_vector(n.get())).length() <= 1e-12);
        // A point off the surface projects to the same place.
        let off = p + n.get() * 0.01;
        let (a, b) = s.project(off).param;
        let (c, d) = t.project(r.apply_point(off)).param;
        let gap = s.point(a, b).distance(s.point(c, d));
        assert!(gap <= 1e-9, "{s:?} at ({u}, {v}): {gap}");
    }
}

#[test]
fn extrusion_rejects_bad_input() {
    let f = frame();
    let circle = Curve::Circle(Circle::new(f, 2.0).unwrap());
    let z = f.z().get();
    let new = |c: &Curve, d: Vector3| Extrusion::new(c.clone(), d).map(|_| ());
    assert_eq!(new(&circle, Vector3::ZERO), Err(Error::Degenerate));
    assert_eq!(
        new(&circle, Vector3::new(f64::NAN, 0.0, 1.0)),
        Err(Error::NonFinite)
    );
    let line = Curve::Line(crate::geom::Line::new(Point3::ORIGIN, z).unwrap());
    assert_eq!(
        new(&line, Vector3::new(1.0, 0.0, 0.0)),
        Err(Error::Degenerate)
    );
    // In the circle's plane, and within the angular resolution of it.
    assert_eq!(new(&circle, f.x().get()), Err(Error::Degenerate));
    assert_eq!(
        new(&circle, f.x().get() + z * 0.5e-11),
        Err(Error::Degenerate)
    );
    assert!(new(&circle, f.x().get() + z * 1e-9).is_ok());
    // A straight NURBS, and one that leaves its plane.
    let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let along = |t: f64| f.to_world(t, 2.0 * t, 0.0);
    let straight = nurbs(
        k.clone(),
        vec![along(0.0), along(1.0), along(3.0)],
        vec![1.0; 3],
    );
    assert_eq!(new(&straight, z), Err(Error::Degenerate));
    let bent = |lift: f64| {
        let pts = vec![
            f.to_world(0.0, 0.0, 0.0),
            f.to_world(1.0, 1.0, lift),
            f.to_world(2.0, 0.0, 0.0),
            f.to_world(1.0, -1.0, 0.0),
        ];
        let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
        nurbs(k, pts, vec![1.0; 4])
    };
    assert!(new(&bent(0.0), z).is_ok());
    assert!(new(&bent(1e-9), z).is_ok());
    assert_eq!(new(&bent(1e-6), z), Err(Error::Degenerate));
    // An S whose control polygon's signed area cancels is still planar.
    let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
    let s = [(0.0, 0.0), (1.0, 1.0), (2.0, -1.0), (3.0, 0.0)];
    let s = nurbs(
        k,
        s.iter().map(|&(a, b)| f.to_world(a, b, 0.0)).collect(),
        vec![1.0; 4],
    );
    let e = Extrusion::new(s, z).unwrap();
    assert!((e.normal().get().dot(z).abs() - 1.0).abs() <= 1e-15);
}

#[test]
fn extrusion_accessors_and_equality() {
    let f = frame();
    let circle = Curve::Circle(Circle::new(f, 2.0).unwrap());
    let e = Extrusion::new(circle.clone(), f.z().get() * 3.0).unwrap();
    assert_eq!(e.curve(), &circle);
    assert!((e.dir().get() - f.z().get()).length() <= 1e-15);
    assert_eq!(e.normal(), f.z());
    let tilted = f.z().get() + f.x().get();
    let slant = Extrusion::new(circle.clone(), tilted).unwrap();
    assert!(matches!(slant.flat, Flat::Nurbs { .. }));
    assert_eq!(slant, Extrusion::new(circle, tilted).unwrap());
    assert_ne!(slant, e);
    let s = Surface::Extrusion(Arc::new(e));
    assert_eq!(s.frame(), None);
    assert_eq!(s.domain(), [[0.0, TAU], [f64::NEG_INFINITY, f64::INFINITY]]);
    assert_eq!(s.periods(), (Some(TAU), None));
    assert_eq!(s.closed_genus(), None);
    assert_eq!(s.bounds(), crate::tol::size_box());
    let w = extrusion(wave(&f), f.z().get());
    assert_eq!(w.domain()[0], [0.0, 2.5]);
    assert_eq!(w.periods(), (None, None));
}

#[test]
fn extrusion_derivatives_and_normals() {
    for s in extrusions() {
        derivatives_match_finite_differences(&s);
        normal_is_natural(&s);
    }
}

#[test]
fn an_extruded_circle_is_a_cylinder() {
    let f = frame();
    let cyl = Surface::Cylinder(Cylinder::new(f, 2.0).unwrap());
    let ext = extrusion(Curve::Circle(Circle::new(f, 2.0).unwrap()), f.z().get());
    for (u, v) in grid(&cyl, 8) {
        assert!(cyl.point(u, v).distance(ext.point(u, v)) <= 1e-14);
        let (a, b) = (cyl.normal(u, v).unwrap(), ext.normal(u, v).unwrap());
        assert!((a.get() - b.get()).length() <= 1e-15, "at {u}");
    }
    for p in [Point3::new(4.0, 0.0, 1.0), Point3::new(-1.0, 2.5, 7.0)] {
        assert!((cyl.distance(p) - ext.distance(p)).abs() <= 1e-14);
        let (a, b) = (cyl.project(p).param, ext.project(p).param);
        assert!((a.0 - b.0).abs() <= 1e-14 && (a.1 - b.1).abs() <= 1e-14);
    }
}

#[test]
fn extrusion_converts_exactly() {
    for s in extrusions() {
        let [du, _] = s.domain();
        nurbs_agrees(&s, [du, [-1.5, 2.0]]);
    }
}

#[test]
fn extrusion_projects_to_the_nearest_point() {
    let points = [
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(4.0, -1.0, 3.5),
        Point3::new(1.0, 2.0, 3.0),
        Point3::new(-2.0, 5.0, 1.0),
        Point3::new(6.0, 3.0, -2.0),
    ];
    for s in extrusions() {
        let [du, _] = s.domain();
        for &p in &points {
            let (_, v) = s.project(p).param;
            projection_is_nearest(&s, p, [du, [v - 6.0, v + 6.0]]);
        }
        // Points on the surface project onto themselves.
        for (u, v) in grid(&s, 5) {
            assert!(s.distance(s.point(u, v)) <= 1e-12, "{s:?} at ({u}, {v})");
        }
    }
}

#[test]
fn extrusion_bounds_hold() {
    for s in extrusions() {
        let [[u0, u1], _] = s.domain();
        bound_holds(&s, [[u0, u1], [-1.0, 1.0]]);
        let mid = (u0 + u1) / 2.0;
        bound_holds(&s, [[mid, mid + 0.2], [0.3, 0.4]]);
    }
    // An oblique direction turns the normal faster than the curve turns.
    let f = frame();
    let circle = Curve::Circle(Circle::new(f, 2.0).unwrap());
    let s = extrusion(circle, f.z().get() * 0.5 + f.x().get());
    let b = s.derivative_bound([[0.0, 1.0], [0.0, 1.0]]);
    assert!(b.turn_u > 2.0 && b.turn_v == 0.0, "{b:?}");
    assert_eq!((b.duv, b.dvv), (0.0, 0.0));
}

#[test]
fn extrusion_transforms() {
    for s in extrusions() {
        transform_round_trips(&s);
    }
}

#[test]
fn extrusion_along_an_oblique_direction() {
    // A unit circle in the world xy plane extruded along x + z.
    let circle = Curve::Circle(Circle::new(Frame::WORLD, 1.0).unwrap());
    let s = extrusion(circle, Vector3::new(1.0, 0.0, 1.0));
    let h = core::f64::consts::FRAC_1_SQRT_2;
    let p = s.point(FRAC_PI_2, 1.0);
    assert!(p.distance(Point3::new(h, 1.0, h)) <= 1e-15);
    // A point a little outside the start of the circle, moved up the
    // direction: its foot is at u = 0, v = 2.
    let foot = s.project(Point3::new(1.1 + 2.0 * h, 0.0, 2.0 * h));
    let (u, v) = foot.param;
    assert!(u.abs() <= 1e-9 || (u - TAU).abs() <= 1e-9, "{u}");
    assert!((v - 2.0 - 0.1 * h).abs() <= 1e-9, "{v}");
}

/// A curve in `f`'s xz half-plane through `(ρ, h)` control values.
fn meridian(f: &Frame, knots: KnotVector, local: &[(f64, f64)], weights: Vec<f64>) -> Curve {
    let points = local.iter().map(|&(r, h)| f.to_world(r, 0.0, h)).collect();
    nurbs(knots, points, weights)
}

/// A straight segment from `a` to `b` over `[t0, t1]`, as a degree 1
/// NURBS in `f`'s xz plane.
fn segment(f: &Frame, a: (f64, f64), b: (f64, f64), [t0, t1]: [f64; 2]) -> Curve {
    let k = KnotVector::clamped(1, &[t0, t1], &[2, 2]).unwrap();
    meridian(f, k, &[a, b], vec![1.0; 2])
}

/// The NURBS semicircle of radius `r` from the south pole to the north,
/// over `[-π/2, π/2]`: the sphere's own profile.
fn semicircle(f: &Frame, r: f64) -> Curve {
    let k = KnotVector::clamped(2, &[-FRAC_PI_2, 0.0, FRAC_PI_2], &[3, 2, 3]).unwrap();
    let h = core::f64::consts::FRAC_1_SQRT_2;
    let local = [(0.0, -r), (r, -r), (r, 0.0), (r, r), (0.0, r)];
    meridian(f, k, &local, vec![1.0, h, 1.0, h, 1.0])
}

/// A circle of radius `r` about `(big, 0)` in `f`'s xz plane, running
/// from `+x` towards `+z` as a torus's tube does.
fn tube(f: &Frame, big: f64, r: f64) -> Curve {
    let at = Frame::new(f.to_world(big, 0.0, 0.0), -f.y().get(), f.x().get()).unwrap();
    Curve::Circle(Circle::new(at, r).unwrap())
}

fn revolution(f: Frame, curve: Curve) -> Surface {
    Surface::Revolution(Arc::new(Revolution::new(f, curve).unwrap()))
}

/// Revolutions of every curve kind, with poles at none, one or both
/// ends.
fn revolutions() -> Vec<Surface> {
    let f = frame();
    let k = KnotVector::clamped(3, &[0.0, 1.0, 2.5], &[4, 1, 4]).unwrap();
    let wave = [(1.0, 0.0), (3.0, 1.0), (0.5, 2.0), (2.0, 3.5), (1.0, 5.0)];
    let k2 = KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 1, 3]).unwrap();
    let dome = [(0.0, 0.0), (2.0, 0.0), (2.0, 1.5), (1.0, 2.0)];
    let kp = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[1; 6]).unwrap();
    let blob = [(4.0, 0.0), (3.0, 1.5), (1.5, 2.0), (1.0, 0.0), (2.5, -2.5)];
    let ellipse = Frame::new(f.to_world(3.0, 0.0, 1.0), -f.y().get(), f.z().get()).unwrap();
    vec![
        revolution(f, tube(&f, 3.0, 1.0)),
        revolution(f, Curve::Ellipse(Ellipse::new(ellipse, 1.5, 0.75).unwrap())),
        revolution(f, segment(&f, (2.0, -1.0), (2.0, 3.0), [-1.0, 3.0])),
        revolution(f, segment(&f, (0.0, -2.0), (3.0, 1.0), [0.0, 2.0])),
        revolution(f, semicircle(&f, 2.0)),
        revolution(f, meridian(&f, k, &wave, vec![1.0, 0.7, 1.6, 0.9, 1.0])),
        revolution(f, meridian(&f, k2, &dome, vec![1.0, 0.8, 1.2, 1.0])),
        revolution(f, meridian(&f, kp, &blob, vec![1.0, 2.0, 0.5, 1.0, 1.5])),
    ]
}

#[test]
fn revolution_rejects_bad_input() {
    let f = frame();
    let new = |c: Curve| Revolution::new(f, c).map(|_| ());
    let along = crate::geom::Line::new(f.origin(), f.z().get()).unwrap();
    assert_eq!(new(Curve::Line(along)), Err(Error::Degenerate));
    // Circles: off the plane, touching the axis, crossing it.
    let off = Frame::new(f.to_world(3.0, 1e-6, 0.0), -f.y().get(), f.x().get()).unwrap();
    assert_eq!(
        new(Curve::Circle(Circle::new(off, 1.0).unwrap())),
        Err(Error::Degenerate)
    );
    let tilted = Frame::new(f.to_world(3.0, 0.0, 0.0), f.z().get(), f.x().get()).unwrap();
    assert_eq!(
        new(Curve::Circle(Circle::new(tilted, 1.0).unwrap())),
        Err(Error::Degenerate)
    );
    assert_eq!(new(tube(&f, 1.0, 1.0)), Err(Error::Degenerate));
    assert_eq!(new(tube(&f, 1.0, 1.5)), Err(Error::Degenerate));
    assert!(new(tube(&f, 1.0 + 1e-6, 1.0)).is_ok());
    // NURBS: a control point off the plane or behind the axis.
    let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let lifted = vec![
        f.to_world(1.0, 0.0, 0.0),
        f.to_world(2.0, 1e-6, 1.0),
        f.to_world(1.0, 0.0, 2.0),
    ];
    assert_eq!(
        new(nurbs(k.clone(), lifted, vec![1.0; 3])),
        Err(Error::Degenerate)
    );
    let behind = [(1.0, 0.0), (-1e-6, 1.0), (1.0, 2.0)];
    assert_eq!(
        new(meridian(&f, k.clone(), &behind, vec![1.0; 3])),
        Err(Error::Degenerate)
    );
    // A control point on the axis is fine when the curve keeps clear.
    let clear = [(1.0, 0.0), (0.0, 1.0), (1.0, 2.0)];
    assert!(new(meridian(&f, k.clone(), &clear, vec![1.0; 3])).is_ok());
    // A touch inside the domain: a corner of a polyline on the axis.
    let k1 = KnotVector::clamped(1, &[0.0, 1.0, 2.0], &[2, 1, 2]).unwrap();
    assert_eq!(
        new(meridian(&f, k1, &clear, vec![1.0; 3])),
        Err(Error::Degenerate)
    );
    // A segment lying along the axis, and one leaving it along the axis.
    let axial = segment(&f, (0.0, 0.0), (0.0, 1.0), [0.0, 1.0]);
    assert_eq!(new(axial), Err(Error::Degenerate));
    let grazing = [(0.0, 0.0), (0.0, 1.0), (1.0, 2.0)];
    assert_eq!(
        new(meridian(&f, k, &grazing, vec![1.0; 3])),
        Err(Error::Degenerate)
    );
    // Almost touching inside: just inside resolution, and clear of it.
    let k3 = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
    let dip = |d: f64| [(2.0 * d, 0.0), (0.0, 1.0), (2.0 * d, 2.0)];
    assert_eq!(
        new(meridian(&f, k3.clone(), &dip(0.6e-8), vec![1.0; 3])),
        Err(Error::Degenerate)
    );
    assert!(new(meridian(&f, k3, &dip(1e-6), vec![1.0; 3])).is_ok());
}

#[test]
fn revolution_poles_and_genus() {
    let f = frame();
    let s = revolutions();
    let poles = |s: &Surface| match s {
        Surface::Revolution(r) => r.poles(),
        _ => unreachable!(),
    };
    let genus: Vec<_> = s.iter().map(Surface::closed_genus).collect();
    assert_eq!(
        genus,
        [Some(1), Some(1), None, None, Some(0), None, None, Some(1)]
    );
    assert_eq!(poles(&s[0]), [false, false]);
    assert_eq!(poles(&s[3]), [true, false]);
    assert_eq!(poles(&s[4]), [true, true]);
    assert_eq!(poles(&s[6]), [true, false]);
    for s in &s {
        assert_eq!(s.frame(), None);
        assert_eq!(s.periods().0, Some(TAU));
        assert_eq!(s.domain()[0], [0.0, TAU]);
    }
    assert_eq!(s[0].periods(), (Some(TAU), Some(TAU)));
    assert_eq!(s[4].domain()[1], [-FRAC_PI_2, FRAC_PI_2]);
    // The revolution keeps its frame and curve; equality is theirs.
    let r = Revolution::new(f, semicircle(&f, 2.0)).unwrap();
    assert_eq!(r.frame(), &f);
    assert_eq!(r.curve(), &semicircle(&f, 2.0));
    assert_eq!(r, Revolution::new(f, semicircle(&f, 2.0)).unwrap());
}

#[test]
fn revolution_bounds_hold_every_point() {
    for s in revolutions() {
        let b = s.bounds();
        for (u, v) in grid(&s, 24) {
            let p = s.point(u, v);
            assert!(b.contains_with_tol(p, crate::tol::Tolerance::RESOLUTION));
        }
    }
    // A sphere's box is the sphere's, here.
    let f = Frame::WORLD;
    let b = revolution(f, semicircle(&f, 2.0)).bounds();
    let sphere = Surface::Sphere(crate::geom::Sphere::new(f, 2.0).unwrap()).bounds();
    assert!(b.min().distance(sphere.min()) <= 1e-15 && b.max().distance(sphere.max()) <= 1e-15);
}

#[test]
fn revolution_derivatives_and_normals() {
    for s in revolutions() {
        derivatives_match_finite_differences(&s);
        normal_is_natural(&s);
    }
}

/// The revolution agrees with an analytic surface at the same `u` and the
/// curve's parameter for its `v` (through `reparam`), in points and
/// normals.
fn same_as(rev: &Surface, analytic: &Surface, reparam: Reparam, vs: [f64; 2]) {
    for i in 0..=12 {
        for j in 0..=12 {
            let u = TAU * f64::from(i) / 12.0;
            let v = vs[0] + (vs[1] - vs[0]) * f64::from(j) / 12.0;
            let w = reparam.to_nurbs(v);
            let at = format!("{analytic:?} at ({u}, {v})");
            assert!(
                rev.point(u, w).distance(analytic.point(u, v)) <= 1e-14,
                "{at}"
            );
            let (a, b) = (rev.normal(u, w).unwrap(), analytic.normal(u, v).unwrap());
            assert!((a.get() - b.get()).length() <= 1e-12, "{at}");
        }
    }
}

#[test]
fn revolutions_of_analytic_profiles() {
    use crate::geom::{Cone, Sphere, Torus};
    let f = frame();
    let cyl = Surface::Cylinder(Cylinder::new(f, 2.0).unwrap());
    let line = revolution(f, segment(&f, (2.0, -1.0), (2.0, 3.0), [-1.0, 3.0]));
    same_as(&line, &cyl, Reparam::Identity, [-1.0, 3.0]);
    // A cone of radius 1.5 and half-angle α has its apex at v = -1.5 /
    // tan α; its generator rises by 1 in v as it widens by tan α.
    let alpha: f64 = 0.6;
    let cone = Cone::new(f, 1.5, alpha).unwrap();
    let (apex, slope) = (cone.apex_v(), math::tan(alpha));
    let generator = segment(&f, (0.0, apex), (1.5 + 2.0 * slope, 2.0), [apex, 2.0]);
    same_as(
        &revolution(f, generator),
        &Surface::Cone(cone),
        Reparam::Identity,
        [apex, 2.0],
    );
    let sphere = Surface::Sphere(Sphere::new(f, 2.0).unwrap());
    let semi = revolution(f, semicircle(&f, 2.0));
    same_as(&semi, &sphere, Reparam::Angle, [-FRAC_PI_2, FRAC_PI_2]);
    let torus = Surface::Torus(Torus::new(f, 3.0, 1.0).unwrap());
    same_as(
        &revolution(f, tube(&f, 3.0, 1.0)),
        &torus,
        Reparam::Identity,
        [0.0, TAU],
    );
    // Projections agree with the closed forms (within the line's heights).
    for p in [
        f.to_world(3.0, 1.0, 0.5),
        f.to_world(-0.5, 0.2, 1.5),
        f.origin(),
    ] {
        for (rev, s) in [(&line, &cyl), (&semi, &sphere)] {
            assert!((rev.distance(p) - s.distance(p)).abs() <= 1e-12, "{s:?}");
        }
    }
}

#[test]
fn revolution_converts_exactly() {
    for s in revolutions() {
        nurbs_agrees(&s, s.domain());
    }
}

#[test]
fn revolution_projects_to_the_nearest_point() {
    let f = frame();
    let points = [
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(4.0, -1.0, 3.5),
        f.origin(),
        f.to_world(0.0, 0.0, 1.0),
        Point3::new(-2.0, 5.0, 1.0),
        Point3::new(6.0, 3.0, -2.0),
    ];
    for s in revolutions() {
        for &p in &points {
            projection_is_nearest(&s, p, s.domain());
        }
        for (u, v) in grid(&s, 5) {
            assert!(s.distance(s.point(u, v)) <= 1e-12, "{s:?} at ({u}, {v})");
        }
    }
}

#[test]
fn revolution_bounds_of_derivatives_hold() {
    for s in revolutions() {
        let [ru, rv] = s.domain();
        bound_holds(&s, [ru, rv]);
        let mid = (rv[0] + rv[1]) / 2.0;
        bound_holds(&s, [[1.0, 1.3], [mid, mid + 0.1]]);
        // Boxes past a clamped domain's ends are cut to it.
        bound_holds(&s, [[1.0, 1.3], [rv[0] - 0.5, rv[0] + 0.1]]);
    }
    // A kink: slow before the double knot, fast after it, so |C'| jumps
    // there and the speed bound must not cross it.
    let f = frame();
    let k = KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 2, 3]).unwrap();
    let kinked = [(1.0, 0.0), (1.0, 0.1), (1.1, 0.2), (4.0, 3.0), (1.0, 6.0)];
    let s = revolution(f, meridian(&f, k, &kinked, vec![1.0; 5]));
    bound_holds(&s, [[0.0, 1.0], [0.3, 1.2]]);
}

#[test]
fn revolution_transforms() {
    for s in revolutions() {
        transform_round_trips(&s);
    }
}
