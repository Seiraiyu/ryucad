//! Every predicate must agree with the exact rational reference in
//! ryukei-testkit, especially on near-degenerate input where the
//! floating-point filter fails and the exact stage decides.

use proptest::prelude::*;
use ryukei::geom::{Point2, Point3};
use ryukei::predicates::{Sign, incircle, insphere, orient2d, orient3d};
use ryukei_testkit::exact;
use std::cmp::Ordering;

fn sign(o: Ordering) -> Sign {
    match o {
        Ordering::Less => Sign::Negative,
        Ordering::Equal => Sign::Zero,
        Ordering::Greater => Sign::Positive,
    }
}

/// Moves `x` by up to 8 ulps either way.
fn nudge(x: f64, ulps: i8) -> f64 {
    let mut y = x;
    for _ in 0..ulps.unsigned_abs() {
        y = if ulps > 0 { y.next_up() } else { y.next_down() };
    }
    y
}

fn coord() -> impl Strategy<Value = f64> {
    prop_oneof![
        -500.0f64..500.0,
        (-1000i32..1000).prop_map(|i| f64::from(i) * 0.5),
    ]
}

fn p2() -> impl Strategy<Value = Point2> {
    (coord(), coord()).prop_map(|(x, y)| Point2::new(x, y))
}

fn p3() -> impl Strategy<Value = Point3> {
    (coord(), coord(), coord()).prop_map(|(x, y, z)| Point3::new(x, y, z))
}

/// A point on the segment a-b (rounded), then nudged by a few ulps: almost
/// collinear with a and b.
fn near_line2(a: Point2, b: Point2, t: f64, n: (i8, i8)) -> Point2 {
    let p = a.lerp(b, t);
    Point2::new(nudge(p.x, n.0), nudge(p.y, n.1))
}

fn near_plane3(a: Point3, b: Point3, c: Point3, s: f64, t: f64, n: (i8, i8, i8)) -> Point3 {
    let p = a + (b - a) * s + (c - a) * t;
    Point3::new(nudge(p.x, n.0), nudge(p.y, n.1), nudge(p.z, n.2))
}

/// `x * 2^-k`, as two exact power-of-two steps so the factor never
/// underflows on its own.
fn pow2(x: f64, k: i32) -> f64 {
    let step = |e: i32| f64::from_bits(((1023 - e) as u64) << 52);
    x * step(k.min(1022)) * step((k - 1022).max(0))
}

fn ulps() -> impl Strategy<Value = i8> {
    -8i8..=8
}

fn f2(p: Point2) -> [f64; 2] {
    [p.x, p.y]
}

fn a3(p: Point3) -> [f64; 3] {
    p.to_array()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn orient2d_random(a in p2(), b in p2(), c in p2()) {
        prop_assert_eq!(orient2d(a, b, c), sign(exact::orient2d(f2(a), f2(b), f2(c))));
    }

    #[test]
    fn orient2d_near_collinear(a in p2(), b in p2(), t in -2.0f64..3.0, n in (ulps(), ulps())) {
        let c = near_line2(a, b, t, n);
        prop_assert_eq!(orient2d(a, b, c), sign(exact::orient2d(f2(a), f2(b), f2(c))));
    }

    #[test]
    fn orient3d_random(a in p3(), b in p3(), c in p3(), d in p3()) {
        prop_assert_eq!(orient3d(a, b, c, d), sign(exact::orient3d(a3(a), a3(b), a3(c), a3(d))));
    }

    #[test]
    fn orient3d_near_coplanar(
        a in p3(), b in p3(), c in p3(), s in -2.0f64..3.0, t in -2.0f64..3.0,
        n in (ulps(), ulps(), ulps()),
    ) {
        let d = near_plane3(a, b, c, s, t, n);
        prop_assert_eq!(orient3d(a, b, c, d), sign(exact::orient3d(a3(a), a3(b), a3(c), a3(d))));
    }

    #[test]
    fn incircle_random(a in p2(), b in p2(), c in p2(), d in p2()) {
        prop_assert_eq!(
            incircle(a, b, c, d),
            sign(exact::incircle(f2(a), f2(b), f2(c), f2(d)))
        );
    }

    #[test]
    fn incircle_near_cocircular(
        cx in coord(), cy in coord(), r in 0.001f64..500.0,
        angles in [0.0f64..6.3, 0.0f64..6.3, 0.0f64..6.3, 0.0f64..6.3],
        n in (ulps(), ulps()),
    ) {
        let on = |t: f64| Point2::new(
            cx + r * ryukei::math::cos(t),
            cy + r * ryukei::math::sin(t),
        );
        let [a, b, c, d] = angles.map(on);
        let d = Point2::new(nudge(d.x, n.0), nudge(d.y, n.1));
        prop_assert_eq!(
            incircle(a, b, c, d),
            sign(exact::incircle(f2(a), f2(b), f2(c), f2(d)))
        );
    }

    #[test]
    fn insphere_random(a in p3(), b in p3(), c in p3(), d in p3(), e in p3()) {
        prop_assert_eq!(
            insphere(a, b, c, d, e),
            sign(exact::insphere(a3(a), a3(b), a3(c), a3(d), a3(e)))
        );
    }

    #[test]
    fn insphere_near_cospherical(
        r in 0.001f64..500.0,
        angles in proptest::array::uniform5((0.0f64..6.3, -1.5f64..1.5)),
        n in (ulps(), ulps(), ulps()),
    ) {
        let on = |(t, u): (f64, f64)| {
            let (st, ct) = ryukei::math::sin_cos(t);
            let (su, cu) = ryukei::math::sin_cos(u);
            Point3::new(r * ct * cu, r * st * cu, r * su)
        };
        let [a, b, c, d, e] = angles.map(on);
        let e = Point3::new(nudge(e.x, n.0), nudge(e.y, n.1), nudge(e.z, n.2));
        prop_assert_eq!(
            insphere(a, b, c, d, e),
            sign(exact::insphere(a3(a), a3(b), a3(c), a3(d), a3(e)))
        );
    }

    #[test]
    fn tiny_coordinates(
        a in p3(), b in p3(), c in p3(), d in p3(), s in -2.0f64..3.0, t in -2.0f64..3.0,
        k in 90i32..1074, n in (ulps(), ulps(), ulps()),
    ) {
        // Scaled by 2^-k (exact until subnormal), so the predicates must take
        // the big-integer path; e is nearly coplanar with a, b, c.
        let tiny = |p: Point3| Point3::new(pow2(p.x, k), pow2(p.y, k), pow2(p.z, k));
        let (a, b, c, d) = (tiny(a), tiny(b), tiny(c), tiny(d));
        let e = near_plane3(a, b, c, s, t, n);
        let flat = |p: Point3| Point2::new(p.x, p.y);
        let (a2, b2, c2, e2) = (flat(a), flat(b), flat(c), flat(e));
        prop_assert_eq!(orient2d(a2, b2, e2), sign(exact::orient2d(f2(a2), f2(b2), f2(e2))));
        prop_assert_eq!(orient3d(a, b, c, e), sign(exact::orient3d(a3(a), a3(b), a3(c), a3(e))));
        prop_assert_eq!(
            incircle(a2, b2, c2, e2),
            sign(exact::incircle(f2(a2), f2(b2), f2(c2), f2(e2)))
        );
        prop_assert_eq!(
            insphere(a, b, c, d, e),
            sign(exact::insphere(a3(a), a3(b), a3(c), a3(d), a3(e)))
        );
    }

    #[test]
    fn exact_zero_on_integer_grid(a in p2(), k in -4i32..4) {
        // a, a + v, a + k v with half-integer inputs: exactly collinear.
        let v = Point2::new(1.5, -2.5) - Point2::ORIGIN;
        let b = a + v;
        let c = a + v * f64::from(k);
        prop_assume!(c != a && c != b);
        prop_assert_eq!(orient2d(a, b, c), sign(exact::orient2d(f2(a), f2(b), f2(c))));
    }
}
