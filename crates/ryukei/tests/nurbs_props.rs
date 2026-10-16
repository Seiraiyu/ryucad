//! Properties of random NURBS: derivatives agree with finite differences,
//! refinement, clamping and splitting leave the shape unchanged, rigid
//! transforms commute with evaluation, periodic splines repeat, the hull
//! bounds hold every point, and projection is never beaten by brute force.

use proptest::prelude::*;
use ryukei::geom::{Direction, NurbsCurve, NurbsSurface, Point3, Rigid, Vector3};
use ryukei_testkit::Lcg;
use ryukei_testkit::nurbs::{any_curve, any_surface, param};

/// Within `tol` of each other, relative to the larger size (at least 1).
fn agree(a: Vector3, b: Vector3, tol: f64) -> bool {
    (a - b).length() <= tol * a.length().max(b.length()).max(1.0)
}

fn near(a: Point3, b: Point3) -> bool {
    a.distance(b) <= 1e-12
}

/// `a` to `b` at `s` in `[0, 1]`, exactly `a` and `b` at the ends: `a + (b -
/// a) * s` can land an ulp past `b`, where a periodic curve has wrapped to its
/// start (equal to its end only up to rounding).
fn lerp(a: f64, b: f64, s: f64) -> f64 {
    (1.0 - s) * a + s * b
}

/// The curves agree at 41 samples of `[a, b]`.
fn same_curve(c: &NurbsCurve, d: &NurbsCurve, [a, b]: [f64; 2]) -> bool {
    (0..=40).all(|i| {
        let t = lerp(a, b, f64::from(i) / 40.0);
        near(c.point(t), d.point(t))
    })
}

/// The surfaces agree on an 11 × 11 grid over `[u0, u1] × [v0, v1]`.
fn same_surface(a: &NurbsSurface, b: &NurbsSurface, [[u0, u1], [v0, v1]]: [[f64; 2]; 2]) -> bool {
    (0..=10).all(|i| {
        (0..=10).all(|j| {
            let u = lerp(u0, u1, f64::from(i) / 10.0);
            let v = lerp(v0, v1, f64::from(j) / 10.0);
            near(a.point(u, v), b.point(u, v))
        })
    })
}

fn rigid(g: &mut Lcg) -> Rigid {
    let axis = Vector3::new(g.next_f64(-1.0, 1.0), g.next_f64(-1.0, 1.0), 1.0);
    let shift = Vector3::new(
        g.next_f64(-9.0, 9.0),
        g.next_f64(-9.0, 9.0),
        g.next_f64(-9.0, 9.0),
    );
    Rigid::translation(shift).then_after(&Rigid::rotation(
        axis.normalize().unwrap(),
        g.next_f64(-3.0, 3.0),
    ))
}

proptest! {
    #[test]
    fn curve_derivatives_match_finite_differences(seed: u64) {
        let mut g = Lcg::new(seed);
        let c = any_curve(&mut g);
        let t = param(&mut g, c.domain());
        let h = 1e-5;
        let e = c.eval(t);
        let (a, m, b) = (c.point(t - h), c.point(t), c.point(t + h));
        prop_assert_eq!(e.point, m);
        // A knot may sit inside the stencil, where the curve is only C^0
        // from one side; skip those.
        let k = c.knots().knots();
        prop_assume!(k.iter().all(|&u| (u - t).abs() > 2.0 * h));
        prop_assert!(agree(e.d1, (b - a) / (2.0 * h), 1e-5), "{e:?}");
        prop_assert!(agree(e.d2, ((b - m) - (m - a)) / (h * h), 1e-2), "{e:?}");
    }

    #[test]
    fn surface_derivatives_match_finite_differences(seed: u64) {
        let mut g = Lcg::new(seed);
        let s = any_surface(&mut g);
        let [du, dv] = s.domain();
        let (u, v) = (param(&mut g, du), param(&mut g, dv));
        let h = 1e-5;
        let [ku, kv] = s.knots();
        prop_assume!(ku.knots().iter().all(|&k| (k - u).abs() > 2.0 * h));
        prop_assume!(kv.knots().iter().all(|&k| (k - v).abs() > 2.0 * h));
        let e = s.eval(u, v);
        let p = |a: f64, b: f64| s.point(u + a, v + b);
        let m = p(0.0, 0.0);
        prop_assert!(agree(e.du, (p(h, 0.0) - p(-h, 0.0)) / (2.0 * h), 1e-5));
        prop_assert!(agree(e.dv, (p(0.0, h) - p(0.0, -h)) / (2.0 * h), 1e-5));
        let second = |a: Point3, b: Point3| ((a - m) + (b - m)) / (h * h);
        prop_assert!(agree(e.duu, second(p(h, 0.0), p(-h, 0.0)), 1e-2));
        prop_assert!(agree(e.dvv, second(p(0.0, h), p(0.0, -h)), 1e-2));
        let cross = ((p(h, h) - p(h, -h)) - (p(-h, h) - p(-h, -h))) / (4.0 * h * h);
        prop_assert!(agree(e.duv, cross, 1e-2));
    }

    #[test]
    fn curve_refinement_clamping_and_splitting_are_invisible(seed: u64) {
        let mut g = Lcg::new(seed);
        let c = any_curve(&mut g);
        let d = c.domain();
        prop_assert!(same_curve(&c, &c.to_clamped(), d));
        let extra: Vec<f64> = (0..3).map(|_| param(&mut g, d)).collect();
        if let Ok(r) = c.refine(&extra) {
            prop_assert!(same_curve(&c, &r, d));
        }
        let t = param(&mut g, [d[0] + 0.01, d[1] - 0.01]);
        let (a, b) = c.split(t).unwrap();
        prop_assert!(same_curve(&c, &a, [d[0], t]));
        prop_assert!(same_curve(&c, &b, [t, d[1]]));
    }

    #[test]
    fn surface_refinement_clamping_and_splitting_are_invisible(seed: u64) {
        let mut g = Lcg::new(seed);
        let s = any_surface(&mut g);
        let [du, dv] = s.domain();
        prop_assert!(same_surface(&s, &s.to_clamped(), [du, dv]));
        let extra: Vec<f64> = (0..2).map(|_| param(&mut g, dv)).collect();
        if let Ok(r) = s.refine(Direction::V, &extra) {
            prop_assert!(same_surface(&s, &r, [du, dv]));
        }
        let t = param(&mut g, [du[0] + 0.01, du[1] - 0.01]);
        let (a, b) = s.split(Direction::U, t).unwrap();
        prop_assert!(same_surface(&s, &a, [[du[0], t], dv]));
        prop_assert!(same_surface(&s, &b, [[t, du[1]], dv]));
    }

    #[test]
    fn rigid_transforms_commute_with_evaluation(seed: u64) {
        let mut g = Lcg::new(seed);
        let r = rigid(&mut g);
        let c = any_curve(&mut g);
        let t = param(&mut g, c.domain());
        let (a, b) = (c.transformed(&r).eval(t), c.eval(t));
        prop_assert!(near(a.point, r.apply_point(b.point)));
        prop_assert!(agree(a.d1, r.apply_vector(b.d1), 1e-12));
        prop_assert!(agree(a.d2, r.apply_vector(b.d2), 1e-12));
        let s = any_surface(&mut g);
        let [du, dv] = s.domain();
        let (u, v) = (param(&mut g, du), param(&mut g, dv));
        let (a, b) = (s.transformed(&r).eval(u, v), s.eval(u, v));
        prop_assert!(near(a.point, r.apply_point(b.point)));
        prop_assert!(agree(a.duv, r.apply_vector(b.duv), 1e-12));
    }

    #[test]
    fn periodic_splines_repeat(seed: u64) {
        let mut g = Lcg::new(seed);
        let c = any_curve(&mut g);
        if let Some(p) = c.period() {
            let t = param(&mut g, c.domain());
            prop_assert!(near(c.point(t), c.point(t + p)));
            prop_assert!(near(c.point(t), c.point(t - 3.0 * p)));
        }
    }

    #[test]
    fn bounds_hold_every_point(seed: u64) {
        let mut g = Lcg::new(seed);
        let c = any_curve(&mut g);
        let b = c.bounds(c.domain());
        let tol = ryukei::tol::Tolerance::RESOLUTION;
        for _ in 0..50 {
            prop_assert!(b.contains_with_tol(c.point(param(&mut g, c.domain())), tol));
        }
        let s = any_surface(&mut g);
        let b = s.bounds();
        let [du, dv] = s.domain();
        for _ in 0..50 {
            let (u, v) = (param(&mut g, du), param(&mut g, dv));
            prop_assert!(b.contains_with_tol(s.point(u, v), tol));
        }
    }
}

/// A query point in a box a little larger than the random control nets.
fn query(g: &mut Lcg) -> Point3 {
    Point3::new(
        g.next_f64(-8.0, 8.0),
        g.next_f64(-8.0, 8.0),
        g.next_f64(-8.0, 8.0),
    )
}

/// The projection of a random point onto a random curve is a point of the
/// curve at its distance, inside the domain, among the local minima, and
/// no sample of the curve is nearer.
fn curve_projection(seed: u64) -> Result<(), TestCaseError> {
    let mut g = Lcg::new(seed);
    let c = any_curve(&mut g);
    let p = query(&mut g);
    let foot = c.project(p);
    prop_assert_eq!(c.point(foot.param), foot.point);
    prop_assert!((foot.point.distance(p) - foot.distance).abs() <= 1e-15);
    let [a, b] = c.domain();
    prop_assert!(a <= foot.param && foot.param <= b);
    let n = 4000;
    for i in 0..=n {
        let t = a + (b - a) * f64::from(i) / f64::from(n);
        prop_assert!(
            foot.distance <= c.point(t).distance(p) + 1e-12,
            "beaten at {}",
            t
        );
    }
    let minima = c.local_minima(p);
    prop_assert!(minima.iter().any(|m| m.param == foot.param));
    Ok(())
}

/// The same for surfaces, against an 81 × 81 grid of samples.
fn surface_projection(seed: u64) -> Result<(), TestCaseError> {
    let mut g = Lcg::new(seed);
    let s = any_surface(&mut g);
    let p = query(&mut g);
    let foot = s.project(p);
    let (u, v) = foot.param;
    prop_assert_eq!(s.point(u, v), foot.point);
    let [[u0, u1], [v0, v1]] = s.domain();
    prop_assert!(u0 <= u && u <= u1 && v0 <= v && v <= v1);
    let n = 80;
    for i in 0..=n {
        for j in 0..=n {
            let a = u0 + (u1 - u0) * f64::from(i) / f64::from(n);
            let b = v0 + (v1 - v0) * f64::from(j) / f64::from(n);
            prop_assert!(
                foot.distance <= s.point(a, b).distance(p) + 1e-12,
                "beaten at ({}, {})",
                a,
                b
            );
        }
    }
    let minima = s.local_minima(p);
    prop_assert!(minima.iter().any(|m| m.param == foot.param));
    Ok(())
}

proptest! {
    #[test]
    fn curve_projection_is_global(seed: u64) {
        curve_projection(seed)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn surface_projection_is_global(seed: u64) {
        surface_projection(seed)?;
    }
}

/// Seeds that once found a nearest point the search missed: on a kink of
/// a curve, on a crease of a surface, and on creases of periodic splines.
#[test]
fn projection_regressions() {
    for seed in [4032838485436544614, 4893023576973524366] {
        curve_projection(seed).unwrap();
    }
    for seed in [282258199205089042, 2380777889367967749] {
        surface_projection(seed).unwrap();
    }
}
