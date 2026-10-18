//! Properties of swept surfaces over random generating curves: derivatives
//! agree with finite differences, the exact NURBS form agrees with
//! evaluation, projection is never beaten by brute force, and the
//! derivative bounds hold at sampled parameters.

use proptest::prelude::*;
use ryukei::geom::{
    Circle, Curve, Ellipse, Extrusion, Frame, NurbsCurve, Point3, Surface, Vector3,
};
use ryukei::tol::LINEAR_RESOLUTION;
use ryukei_testkit::Lcg;
use ryukei_testkit::nurbs::any_curve;
use std::sync::Arc;

/// Within `tol` of each other, relative to the larger size (at least 1).
fn agree(a: Vector3, b: Vector3, tol: f64) -> bool {
    (a - b).length() <= tol * a.length().max(b.length()).max(1.0)
}

fn frame(g: &mut Lcg) -> Frame {
    loop {
        let mut v = || {
            Vector3::new(
                g.next_f64(-1.0, 1.0),
                g.next_f64(-1.0, 1.0),
                g.next_f64(-1.0, 1.0),
            )
        };
        let (o, z, x) = (v() * 5.0, v(), v());
        if let Ok(f) = Frame::new(Point3::ORIGIN + o, z, x) {
            return f;
        }
    }
}

/// A random NURBS with its control points laid into `f`'s xy plane.
fn planar_nurbs(g: &mut Lcg, f: &Frame) -> Curve {
    let n = any_curve(g);
    let points = n
        .control_points()
        .map(|p| f.to_world(p.x, p.y, 0.0))
        .collect();
    let flat = NurbsCurve::new(n.knots().clone(), points, Some(n.weights().to_vec())).unwrap();
    Curve::Nurbs(Arc::new(flat))
}

/// A random planar curve: a circle, an ellipse or a NURBS, in `f`'s xy
/// plane.
fn planar_curve(g: &mut Lcg, f: &Frame) -> Curve {
    match g.next_u64() % 3 {
        0 => Curve::Circle(Circle::new(*f, g.next_f64(0.1, 5.0)).unwrap()),
        1 => {
            let a = g.next_f64(0.1, 5.0);
            Curve::Ellipse(Ellipse::new(*f, a, a * g.next_f64(0.05, 1.0)).unwrap())
        }
        _ => planar_nurbs(g, f),
    }
}

/// A random extrusion: a planar curve moved along its plane's normal one
/// time in four, otherwise along a direction leaning at least about 0.1
/// rad from the plane. `None` for a NURBS that is too straight.
fn extrusion(g: &mut Lcg) -> Option<Surface> {
    let f = frame(g);
    let c = planar_curve(g, &f);
    let dir = if g.next_u64().is_multiple_of(4) {
        f.z().get()
    } else {
        f.z().get() * g.next_f64(0.1, 1.0)
            + f.x().get() * g.next_f64(-1.0, 1.0)
            + f.y().get() * g.next_f64(-1.0, 1.0)
    };
    let e = Extrusion::new(c, dir).ok()?;
    Some(Surface::Extrusion(Arc::new(e)))
}

/// The finite part of a surface's domain: `v` in `[-3, 3]` where it is
/// unbounded.
fn finite_domain(s: &Surface) -> [[f64; 2]; 2] {
    s.domain().map(|[a, b]| {
        if a.is_finite() && b.is_finite() {
            [a, b]
        } else {
            [-3.0, 3.0]
        }
    })
}

fn param(g: &mut Lcg, [a, b]: [f64; 2]) -> f64 {
    g.next_f64(a, b)
}

/// The knots of a NURBS generating curve, to keep stencils off them.
fn knots(s: &Surface) -> Vec<f64> {
    let c = match s {
        Surface::Extrusion(e) => e.curve(),
        _ => return Vec::new(),
    };
    match c {
        Curve::Nurbs(n) => n.knots().knots().to_vec(),
        _ => Vec::new(),
    }
}

fn knot_free(knots: &[f64], t: f64, h: f64) -> bool {
    knots.iter().all(|&k| (k - t).abs() > 2.0 * h)
}

fn derivatives_hold(s: &Surface, g: &mut Lcg, along_u: bool) -> Result<(), TestCaseError> {
    let [ru, rv] = finite_domain(s);
    let (u, v) = (param(g, ru), param(g, rv));
    let k = knots(s);
    let h = 1e-5;
    let t = if along_u { u } else { v };
    prop_assume!(knot_free(&k, t, h));
    let e = s.eval(u, v);
    prop_assert_eq!(e.point, s.point(u, v));
    let ev = |du: f64, dv: f64| s.eval(u + du, v + dv);
    let d = |du: f64, dv: f64| s.point(u + du, v + dv);
    let fu = (d(h, 0.0) - d(-h, 0.0)) / (2.0 * h);
    let fv = (d(0.0, h) - d(0.0, -h)) / (2.0 * h);
    let fuu = (ev(h, 0.0).du - ev(-h, 0.0).du) / (2.0 * h);
    let fuv = (ev(0.0, h).du - ev(0.0, -h).du) / (2.0 * h);
    let fvv = (ev(0.0, h).dv - ev(0.0, -h).dv) / (2.0 * h);
    prop_assert!(agree(e.du, fu, 1e-5), "du {:?} {:?}", e, s);
    prop_assert!(agree(e.dv, fv, 1e-5), "dv {:?} {:?}", e, s);
    prop_assert!(agree(e.duu, fuu, 1e-3), "duu {:?}", e);
    prop_assert!(agree(e.duv, fuv, 1e-3), "duv {:?}", e);
    prop_assert!(agree(e.dvv, fvv, 1e-3), "dvv {:?}", e);
    // The natural normal is along du × dv wherever that is well defined.
    if let (Some(n), Some(m)) = (s.normal(u, v), e.normal())
        && e.du.length() > 1e-3
        && e.dv.length() > 1e-3
    {
        prop_assert!((n.get() - m.get()).length() <= 1e-9, "{:?} {:?}", n, m);
    }
    Ok(())
}

fn nurbs_agrees(s: &Surface, g: &mut Lcg) -> Result<(), TestCaseError> {
    let [ru, rv] = finite_domain(s);
    let (n, [pu, pv]) = s.to_nurbs([ru, rv]).unwrap();
    for _ in 0..20 {
        let (u, v) = (param(g, ru), param(g, rv));
        let d = n
            .point(pu.to_nurbs(u), pv.to_nurbs(v))
            .distance(s.point(u, v));
        prop_assert!(d <= 1e-11, "{:?} at ({}, {}): {}", s, u, v, d);
    }
    Ok(())
}

/// No sampled point is nearer than the projection, and Newton's method
/// from the best sample doesn't beat it.
fn projection_holds(s: &Surface, p: Point3, ranges: [[f64; 2]; 2]) -> Result<(), TestCaseError> {
    let foot = s.project(p);
    let (u, v) = foot.param;
    prop_assert_eq!(foot.point, s.point(u, v));
    prop_assert_eq!(foot.distance, foot.point.distance(p));
    let [[u0, u1], [v0, v1]] = ranges;
    let (nu, nv) = (240, 160);
    let mut best = (f64::INFINITY, 0.0, 0.0);
    for i in 0..=nu {
        for j in 0..=nv {
            let a = u0 + (u1 - u0) * f64::from(i) / f64::from(nu);
            let b = v0 + (v1 - v0) * f64::from(j) / f64::from(nv);
            let d = s.point(a, b).distance(p);
            if d < best.0 {
                best = (d, a, b);
            }
        }
    }
    let (mut a, mut b) = (best.1, best.2);
    for _ in 0..30 {
        let e = s.eval(a, b);
        let r = e.point - p;
        let (gu, gv) = (r.dot(e.du), r.dot(e.dv));
        let h11 = e.du.dot(e.du) + r.dot(e.duu);
        let h12 = e.du.dot(e.dv) + r.dot(e.duv);
        let h22 = e.dv.dot(e.dv) + r.dot(e.dvv);
        let det = h11 * h22 - h12 * h12;
        if det <= 0.0 || h11 <= 0.0 {
            break;
        }
        let (na, nb) = (
            a - (h22 * gu - h12 * gv) / det,
            b - (h11 * gv - h12 * gu) / det,
        );
        if !(u0..=u1).contains(&na) || !(v0..=v1).contains(&nb) {
            break;
        }
        (a, b) = (na, nb);
    }
    let brute = best.0.min(s.point(a, b).distance(p));
    prop_assert!(
        foot.distance <= brute + LINEAR_RESOLUTION,
        "{:?} {:?}: {} vs {}",
        s,
        p,
        foot.distance,
        brute
    );
    Ok(())
}

/// The bound over a random box holds at sampled parameters in it.
fn bound_holds(s: &Surface, g: &mut Lcg) -> Result<(), TestCaseError> {
    let [ru, rv] = finite_domain(s);
    let box_in = |g: &mut Lcg, [a, b]: [f64; 2]| {
        let (x, y) = (param(g, [a, b]), param(g, [a, b]));
        [x.min(y), x.max(y)]
    };
    let (bu, bv) = (box_in(g, ru), box_in(g, rv));
    let b = s.derivative_bound([bu, bv]);
    for _ in 0..40 {
        let (u, v) = (param(g, bu), param(g, bv));
        let e = s.eval(u, v);
        prop_assert!(e.duu.length() <= b.duu, "duu {:?} {:?}", e, b);
        prop_assert!(e.duv.length() <= b.duv, "duv {:?} {:?}", e, b);
        prop_assert!(e.dvv.length() <= b.dvv, "dvv {:?} {:?}", e, b);
        let Some(n) = s.normal(u, v) else { continue };
        let h = 1e-7;
        let turn = |du: f64, dv: f64| {
            s.normal(u + du, v + dv)
                .map_or(0.0, |m| (m.get() - n.get()).length() / h)
        };
        if u + h <= bu[1] {
            prop_assert!(
                turn(h, 0.0) <= b.turn_u * (1.0 + 1e-3) + 1e-5,
                "turn_u {:?}",
                b
            );
        }
        if v + h <= bv[1] {
            prop_assert!(
                turn(0.0, h) <= b.turn_v * (1.0 + 1e-3) + 1e-5,
                "turn_v {:?}",
                b
            );
        }
    }
    Ok(())
}

fn point(g: &mut Lcg) -> Point3 {
    Point3::new(
        g.next_f64(-9.0, 9.0),
        g.next_f64(-9.0, 9.0),
        g.next_f64(-9.0, 9.0),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn extrusion_derivatives_match_finite_differences(seed: u64, along_u: bool) {
        let mut g = Lcg::new(seed);
        let s = extrusion(&mut g);
        prop_assume!(s.is_some());
        let s = s.unwrap();
        derivatives_hold(&s, &mut g, along_u)?;
    }

    #[test]
    fn extrusion_nurbs_agrees_with_evaluation(seed: u64) {
        let mut g = Lcg::new(seed);
        let s = extrusion(&mut g);
        prop_assume!(s.is_some());
        let s = s.unwrap();
        nurbs_agrees(&s, &mut g)?;
    }

    #[test]
    fn extrusion_bounds_hold(seed: u64) {
        let mut g = Lcg::new(seed);
        let s = extrusion(&mut g);
        prop_assume!(s.is_some());
        let s = s.unwrap();
        bound_holds(&s, &mut g)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn extrusion_projection_is_never_beaten(seed: u64) {
        let mut g = Lcg::new(seed);
        let s = extrusion(&mut g);
        prop_assume!(s.is_some());
        let s = s.unwrap();
        let p = point(&mut g);
        // Every point of the curve, and p, is within 17 of the origin, so the
        // nearest point's v is within 40 of 0.
        let [ru, _] = finite_domain(&s);
        projection_holds(&s, p, [ru, [-40.0, 40.0]])?;
    }
}
