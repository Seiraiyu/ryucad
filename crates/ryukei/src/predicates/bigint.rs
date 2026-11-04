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

/// The largest a coordinate may be scaled to by [`rescale`]: `2^100`, so
/// a product of five stays far below overflow, as the size box keeps
/// unscaled coordinates. (`rescale` keeps the largest exponent under 100.)
#[cfg(test)]
const ROOMY: f64 = 1_267_650_600_228_229_401_496_703_205_376.0; // 2^100

/// `floor(log2(|x|))` for a finite non-zero `x`, subnormals included.
fn exponent(x: f64) -> i32 {
    let bits = x.abs().to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32;
    if e == 0 {
        // A subnormal: its leading bit's place in the fraction.
        -1023 - (bits.leading_zeros() as i32 - 12)
    } else {
        e - 1023
    }
}

/// `x × 2^k`, exactly, for `k >= 0` and a result no larger than `2^100`
/// in magnitude (a power of two times a float is exact unless it
/// overflows or underflows, and scaling up never underflows).
pub(crate) fn scale2(x: f64, k: i32) -> f64 {
    let mut x = x;
    let mut k = k;
    while k > 0 {
        let step = k.min(1000);
        x *= f64::from_bits(((1023 + step) as u64) << 52);
        k -= step;
    }
    x
}

/// The power of two to scale `coords` by so that every one is zero or
/// between [`TINY`] and `2^100` in magnitude, for coordinates that
/// aren't [`expansion_safe`] as they are: then the predicates, being
/// homogeneous, give the same sign on the scaled coordinates through the
/// floating-point filter and expansions, rather than big integers scaled
/// by `2^1074` (thousands of bits a product). `None` where the
/// coordinates span more than `2^199`, which no scale fits.
pub(crate) fn rescale(coords: &[f64]) -> Option<i32> {
    let nonzero = coords.iter().copied().filter(|&x| x != 0.0);
    let lo = nonzero.clone().map(exponent).min()?;
    let hi = nonzero.map(exponent).max()?;
    // lo scaled to [2^-100, 2^-99); hi then at most 2^(hi - lo - 99).
    let k = -100 - lo;
    (k > 0 && hi + k < 100).then_some(k)
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
    fn rescaling_brings_tiny_coordinates_into_range() {
        assert_eq!(exponent(1.0), 0);
        assert_eq!(exponent(-0.75), -1);
        assert_eq!(exponent(f64::from_bits(1)), -1074);
        assert_eq!(exponent(f64::MIN_POSITIVE), -1022);
        assert_eq!(exponent(f64::MIN_POSITIVE / 2.0), -1023);
        for coords in [
            vec![1.0, 1e-40, 0.0, -3.5],
            vec![1e-300, 2e-300, -5e-310],
            vec![f64::from_bits(1), 1e-300],
            vec![TINY / 3.0, 1e20],
        ] {
            let k = rescale(&coords).unwrap();
            let scaled: Vec<f64> = coords.iter().map(|&x| scale2(x, k)).collect();
            assert!(expansion_safe(&scaled), "{coords:?} by 2^{k}");
            assert!(scaled.iter().all(|x| x.abs() <= ROOMY));
            for (x, y) in coords.iter().zip(&scaled) {
                // Exact: scaling back gives the coordinate.
                let mut z = *y;
                for _ in 0..k {
                    z /= 2.0;
                }
                assert_eq!(z.to_bits(), x.to_bits());
            }
        }
        // A span no scale fits stays with big integers.
        assert_eq!(rescale(&[1e-200, 1e10]), None);
        assert_eq!(rescale(&[0.0, 0.0]), None);
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
