//! Deterministic scalar math: the only place kernel code gets these functions.
//!
//! `sqrt` is correctly rounded by IEEE 754 on every platform, so it uses the
//! hardware instruction. Everything else delegates to the pure-Rust `libm`
//! crate, which compiles to the same operations on every target. Swapping in
//! a correctly rounded implementation later only touches this file.
//!
//! The std versions (`f64::sin` etc.) are banned workspace-wide by
//! `clippy.toml`; they may lower to platform libm calls that differ by OS.

#[inline]
pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// Fused multiply-add `a * b + c` with a single rounding. The only FMA in
/// the kernel: Rust never contracts `a * b + c` on its own.
#[inline]
pub fn fma(a: f64, b: f64, c: f64) -> f64 {
    libm::fma(a, b, c)
}

#[inline]
pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}

#[inline]
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}

/// `(sin(x), cos(x))`.
#[inline]
pub fn sin_cos(x: f64) -> (f64, f64) {
    libm::sincos(x)
}

#[inline]
pub fn tan(x: f64) -> f64 {
    libm::tan(x)
}

#[inline]
pub fn asin(x: f64) -> f64 {
    libm::asin(x)
}

#[inline]
pub fn acos(x: f64) -> f64 {
    libm::acos(x)
}

#[inline]
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

#[inline]
pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}

#[inline]
pub fn ln(x: f64) -> f64 {
    libm::log(x)
}

#[inline]
pub fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

#[inline]
pub fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

#[inline]
pub fn cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 4.0 * f64::EPSILON * b.abs().max(1.0)
    }

    #[test]
    fn exact_cases() {
        assert_eq!(sqrt(4.0), 2.0);
        assert_eq!(fma(2.0, 3.0, 1.0), 7.0);
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(cbrt(27.0), 3.0);
    }

    #[test]
    fn fma_rounds_once() {
        // (1 + 2^-30)^2 - 1 needs the unrounded product to keep the 2^-60 term.
        let e = 1.0 / (1u64 << 30) as f64;
        let a = 1.0 + e;
        assert_eq!(fma(a, a, -1.0), 2.0 * e + e * e);
    }

    #[test]
    fn known_values() {
        assert!(close(sin(FRAC_PI_2), 1.0));
        assert!(close(cos(PI), -1.0));
        assert!(close(tan(FRAC_PI_4), 1.0));
        assert!(close(asin(1.0), FRAC_PI_2));
        assert!(close(acos(-1.0), PI));
        assert!(close(atan2(1.0, 1.0), FRAC_PI_4));
        let (s, c) = sin_cos(0.5);
        assert_eq!(s, sin(0.5));
        assert_eq!(c, cos(0.5));
    }

    // The platform functions are the reference here, so the ban is lifted.
    #[allow(clippy::disallowed_methods)]
    mod against_std {
        use super::*;
        use proptest::prelude::*;

        fn within_ulps(a: f64, b: f64, ulps: u64) -> bool {
            if a == b {
                return true;
            }
            let (ia, ib) = (a.to_bits() as i64, b.to_bits() as i64);
            a.signum() == b.signum() && ia.abs_diff(ib) <= ulps
        }

        proptest! {
            #[test]
            fn trig_matches_std(x in -1.0e3f64..1.0e3) {
                prop_assert!(within_ulps(sin(x), x.sin(), 2));
                prop_assert!(within_ulps(cos(x), x.cos(), 2));
            }

            #[test]
            fn exp_ln_match_std(x in -700.0f64..700.0) {
                prop_assert!(within_ulps(exp(x), x.exp(), 2));
                let y = x.abs() + 1e-300;
                prop_assert!(within_ulps(ln(y), y.ln(), 2));
            }

            #[test]
            fn atan2_matches_std(y in -1.0e3f64..1.0e3, x in -1.0e3f64..1.0e3) {
                prop_assert!(within_ulps(atan2(y, x), y.atan2(x), 2));
            }
        }
    }
}
