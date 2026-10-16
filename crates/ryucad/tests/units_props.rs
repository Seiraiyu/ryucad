//! Quantity parsing against an exact rational reference.

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::Signed;
use proptest::prelude::*;
use ryucad::units::{Quantity, Unit};
use ryukei_testkit::exact::rational;

/// The exact value of a decimal written as digits, a point position and
/// an exponent.
fn exact_decimal(mantissa: u64, scale: i32) -> BigRational {
    let ten = BigRational::from_integer(BigInt::from(10));
    let mut r = BigRational::from_integer(BigInt::from(mantissa));
    for _ in 0..scale.unsigned_abs() {
        r = if scale > 0 { &r * &ten } else { &r / &ten };
    }
    r
}

fn exact_factor(u: Unit) -> BigRational {
    let (n, d): (i64, i64) = match u {
        Unit::M => (1, 1),
        Unit::Cm => (1, 100),
        Unit::Mm => (1, 1000),
        Unit::Um => (1, 1_000_000),
        Unit::In => (254, 10_000),
        Unit::Ft => (3048, 10_000),
    };
    BigRational::new(BigInt::from(n), BigInt::from(d))
}

proptest! {
    #[test]
    fn the_text_is_stable(mantissa in 0u64..10_000_000_000, scale in -12i32..6, u in 0usize..6) {
        let unit = Unit::ALL[u];
        let s = format!("{mantissa}e{scale} {unit}");
        let a = Quantity::parse(&s, Unit::Mm).unwrap();
        let b = Quantity::parse(&a.text(), Unit::M).unwrap();
        prop_assert_eq!(a.text(), b.text());
        prop_assert_eq!(a.meters().to_bits(), b.meters().to_bits());
    }

    #[test]
    fn meters_are_within_two_roundings(mantissa in 1u64..10_000_000_000, scale in -12i32..6, u in 0usize..6) {
        let unit = Unit::ALL[u];
        let s = format!("{mantissa}e{scale} {unit}");
        let got = Quantity::parse(&s, Unit::Mm).unwrap().meters();
        let exact = exact_decimal(mantissa, scale) * exact_factor(unit);
        // Parsing and the factor each round once, and the product once more:
        // a relative error below 3 · 2⁻⁵³.
        let err = (rational(got) - &exact).abs();
        let bound = &exact * rational(3.0 * f64::EPSILON / 2.0);
        prop_assert!(err <= bound, "{s}: {got:e}");
    }
}
