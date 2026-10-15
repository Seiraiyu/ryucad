//! Interval results must contain the exact rational result.

use proptest::prelude::*;
use ryukei::interval::Interval;
use ryukei_testkit::exact::{Rational, rational, within};

fn bound() -> impl Strategy<Value = f64> {
    prop_oneof![-1.0e6f64..1.0e6, -1.0e-6f64..1.0e-6]
}

/// An interval and an exact point inside it.
fn interval_and_point() -> impl Strategy<Value = (Interval, f64)> {
    (bound(), bound(), 0.0f64..=1.0).prop_map(|(a, b, t)| {
        let (lo, hi) = (a.min(b), a.max(b));
        let x = (lo + (hi - lo) * t).clamp(lo, hi);
        (Interval::new(lo, hi).unwrap(), x)
    })
}

fn holds(i: Interval, r: &Rational) -> bool {
    within(i.lo(), i.hi(), r)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn add_contains((a, x) in interval_and_point(), (b, y) in interval_and_point()) {
        prop_assert!(holds(a + b, &(rational(x) + rational(y))));
    }

    #[test]
    fn sub_contains((a, x) in interval_and_point(), (b, y) in interval_and_point()) {
        prop_assert!(holds(a - b, &(rational(x) - rational(y))));
    }

    #[test]
    fn mul_contains((a, x) in interval_and_point(), (b, y) in interval_and_point()) {
        prop_assert!(holds(a * b, &(rational(x) * rational(y))));
    }

    #[test]
    fn div_contains((a, x) in interval_and_point(), (b, y) in interval_and_point()) {
        if let Some(q) = a.checked_div(b) {
            prop_assert!(holds(q, &(rational(x) / rational(y))));
        } else {
            prop_assert!(b.contains(0.0));
        }
    }

    #[test]
    fn sqrt_contains(a in 0.0f64..1.0e6, b in 0.0f64..1.0e6) {
        let (lo, hi) = (a.min(b), a.max(b));
        let r = Interval::new(lo, hi).unwrap().sqrt().unwrap();
        // sqrt(lo) >= r.lo  <=>  lo >= r.lo^2 (both sides non-negative).
        prop_assert!(rational(r.lo()) * rational(r.lo()) <= rational(lo));
        prop_assert!(rational(r.hi()) * rational(r.hi()) >= rational(hi));
    }

    #[test]
    fn certified_sign_is_true_sign((a, x) in interval_and_point(), (b, y) in interval_and_point()) {
        use ryukei::predicates::Sign;
        let exact = rational(x) * rational(y) - rational(y) * rational(y);
        let i = a * b - b * b;
        match i.sign() {
            Some(Sign::Positive) => prop_assert!(exact > rational(0.0)),
            Some(Sign::Negative) => prop_assert!(exact < rational(0.0)),
            Some(Sign::Zero) => prop_assert!(exact == rational(0.0)),
            None => {}
        }
    }
}
