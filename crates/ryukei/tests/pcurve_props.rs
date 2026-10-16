//! Pcurves behave like the curves in model space: derivatives agree with
//! finite differences, projection is never beaten by sampling, and exact
//! conversions agree with the analytic originals.

use proptest::prelude::*;
use ryukei::geom::{Circle2, Curve2, Ellipse2, Frame2, Line2, Point2, Vector2};
use ryukei_testkit::Lcg;
use ryukei_testkit::nurbs::any_curve2;
use std::sync::Arc;

/// Rounding allowance at these sizes.
const EPS: f64 = 1e-12;

fn coord() -> impl Strategy<Value = f64> {
    -10.0f64..10.0
}

fn point() -> impl Strategy<Value = Point2> {
    (coord(), coord()).prop_map(|(x, y)| Point2::new(x, y))
}

fn frame() -> impl Strategy<Value = Frame2> {
    (point(), point(), any::<bool>()).prop_filter_map("degenerate frame", |(o, x, ccw)| {
        Frame2::new(o, x.to_vector(), ccw).ok()
    })
}

fn curve() -> impl Strategy<Value = Curve2> {
    prop_oneof![
        (point(), point()).prop_filter_map("degenerate line", |(o, d)| {
            Line2::new(o, d.to_vector()).ok().map(Curve2::Line2)
        }),
        (frame(), 0.1f64..5.0).prop_map(|(f, r)| Curve2::Circle2(Circle2::new(f, r).unwrap())),
        (frame(), 0.1f64..5.0, 0.05f64..=1.0)
            .prop_map(|(f, a, k)| Curve2::Ellipse2(Ellipse2::new(f, a, a * k).unwrap())),
        any::<u64>().prop_map(|seed| Curve2::Nurbs2(Arc::new(any_curve2(&mut Lcg::new(seed))))),
    ]
}

fn agree(a: Vector2, b: Vector2, tol: f64) -> bool {
    (a - b).length() <= tol * a.length().max(b.length()).max(1.0)
}

/// A parameter inside the domain, or near `t` when the domain is
/// unbounded.
fn inside(c: &Curve2, t: f64) -> f64 {
    let [a, b] = c.domain();
    if a.is_finite() {
        a + (b - a) * (t + 10.0) / 20.0
    } else {
        t
    }
}

proptest! {
    #[test]
    fn derivatives_match_finite_differences(c in curve(), t in -10.0f64..10.0) {
        let t = inside(&c, t);
        let [lo, hi] = c.domain();
        let h = 1e-5;
        prop_assume!(t - 2.0 * h > lo && t + 2.0 * h < hi);
        if let Curve2::Nurbs2(n) = &c {
            prop_assume!(n.knots().knots().iter().all(|&k| (k - t).abs() > 2.0 * h));
        }
        let e = c.eval(t);
        let (a, m, b) = (c.point(t - h), c.point(t), c.point(t + h));
        prop_assert_eq!(e.point, m);
        prop_assert!(agree(e.d1, (b - a) / (2.0 * h), 1e-5), "{c:?} at {t}");
        prop_assert!(agree(e.d2, ((b - m) - (m - a)) / (h * h), 1e-2), "{c:?} at {t}");
    }

    #[test]
    fn projection_is_never_beaten(c in curve(), p in point()) {
        let foot = c.project(p);
        prop_assert_eq!(c.point(foot.param), foot.point);
        let [a, b] = match c.domain() {
            [a, _] if !a.is_finite() => [foot.param - 30.0, foot.param + 30.0],
            d => d,
        };
        for i in 0..=2000 {
            let t = a + (b - a) * f64::from(i) / 2000.0;
            prop_assert!(foot.distance <= c.point(t).distance(p) + EPS, "{c:?} {p:?} at {t}");
        }
    }

    #[test]
    fn conversions_agree(c in curve(), t in -10.0f64..10.0) {
        let t = inside(&c, t);
        let (n, r) = c.to_nurbs([t - 2.0, t + 2.0]).unwrap();
        let s = r.to_nurbs(t);
        prop_assert!(n.point(s).distance(c.point(t)) <= EPS, "{c:?} at {t}");
    }
}
