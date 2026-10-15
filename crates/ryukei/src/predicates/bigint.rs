//! Exact big-integer arithmetic for the rare inputs where expansion
//! arithmetic could underflow (coordinates below [`TINY`] in magnitude).
//!
//! Every finite f64 is an integer multiple of 2^-1074, so scaling every
//! coordinate by 2^1074 turns it into an exact integer. The predicates are
//! homogeneous polynomials, so a common positive scale leaves their sign
//! unchanged.

use num_bigint::BigInt;
use num_bigint::Sign as BigSign;

/// Coordinates at least this large in magnitude (or zero) are multiples of
/// 2^-152, so degree-5 products stay above 2^-760: no underflow in the
/// expansion stage. Anything smaller goes through [`Fixed`].
pub(crate) const TINY: f64 = 7.888_609_052_210_118e-31; // 2^-100

/// True when expansion arithmetic is exact for all of `coords`.
pub(crate) fn expansion_safe(coords: &[f64]) -> bool {
    coords.iter().all(|&x| x == 0.0 || x.abs() >= TINY)
}

/// A real number `n * 2^-1074`, held exactly as the integer `n`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fixed(BigInt);

impl Fixed {
    pub(crate) fn from_f64(x: f64) -> Self {
        debug_assert!(x.is_finite());
        let bits = x.to_bits();
        let negative = bits >> 63 == 1;
        let exp = ((bits >> 52) & 0x7ff) as i64;
        let frac = bits & ((1u64 << 52) - 1);
        // x = mant * 2^(shift - 1074).
        let (mant, shift) = if exp == 0 {
            (frac, 0)
        } else {
            (frac | (1u64 << 52), exp - 1)
        };
        let n = BigInt::from(mant) << shift;
        Self(if negative { -n } else { n })
    }

    pub(crate) fn add(&self, other: &Self) -> Self {
        Self(&self.0 + &other.0)
    }

    pub(crate) fn sub(&self, other: &Self) -> Self {
        Self(&self.0 - &other.0)
    }

    /// The product, rescaled by another 2^1074. All terms of one
    /// determinant have the same degree, so the extra factor is common to
    /// them and the sign is unaffected.
    pub(crate) fn mul(&self, other: &Self) -> Self {
        Self(&self.0 * &other.0)
    }

    pub(crate) fn signum(&self) -> f64 {
        match self.0.sign() {
            BigSign::Minus => -1.0,
            BigSign::NoSign => 0.0,
            BigSign::Plus => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_is_two_to_minus_100() {
        assert_eq!(TINY, 1.0 / (1u128 << 100) as f64);
        assert!(expansion_safe(&[0.0, 1.0, -TINY]));
        assert!(!expansion_safe(&[1.0, TINY / 2.0]));
        assert!(!expansion_safe(&[f64::from_bits(1)]));
    }

    #[test]
    fn exact_conversion() {
        assert_eq!(Fixed::from_f64(0.0).signum(), 0.0);
        assert_eq!(Fixed::from_f64(-0.0).signum(), 0.0);
        let one = Fixed::from_f64(1.0);
        assert_eq!(one.0, BigInt::from(1) << 1074u32);
        let smallest = Fixed::from_f64(f64::from_bits(1));
        assert_eq!(smallest.0, BigInt::from(1));
        assert_eq!(Fixed::from_f64(-1.5).0, -(BigInt::from(3) << 1073u32));
    }

    #[test]
    fn tiny_differences_survive() {
        // (a - b) * (a - b) for subnormal neighbours is 2^-2148: underflows
        // in f64, but is exactly positive here.
        let a = Fixed::from_f64(f64::from_bits(2));
        let b = Fixed::from_f64(f64::from_bits(1));
        let d = a.sub(&b);
        assert_eq!(d.mul(&d).signum(), 1.0);
        assert_eq!(d.mul(&d).sub(&d.mul(&d)).signum(), 0.0);
        assert_eq!(a.add(&b).signum(), 1.0);
    }
}
