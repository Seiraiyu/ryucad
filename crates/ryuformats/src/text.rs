//! Numbers as text: the shortest decimal that reads back to the same
//! bits, never in exponent form, and `-0` as `0`.

use std::fmt::Write as _;

/// Appends `x` to `out` as the shortest decimal that reads back to the
/// same `f32`. `x` must be finite.
pub fn push_f32(out: &mut String, x: f32) {
    debug_assert!(x.is_finite());
    if x == 0.0 {
        out.push('0');
    } else {
        let _ = write!(out, "{x}");
    }
}

/// Appends `x` to `out` as the shortest decimal that reads back to the
/// same `f64`. `x` must be finite.
pub fn push_f64(out: &mut String, x: f64) {
    debug_assert!(x.is_finite());
    if x == 0.0 {
        out.push('0');
    } else {
        let _ = write!(out, "{x}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_text(x: f32) -> String {
        let mut s = String::new();
        push_f32(&mut s, x);
        s
    }

    fn f64_text(x: f64) -> String {
        let mut s = String::new();
        push_f64(&mut s, x);
        s
    }

    #[test]
    fn numbers_read_back_bit_for_bit() {
        assert_eq!(f32_text(0.0), "0");
        assert_eq!(f32_text(-0.0), "0");
        assert_eq!(f32_text(1.0), "1");
        assert_eq!(f32_text(-2.5), "-2.5");
        assert_eq!(f32_text(0.1), "0.1");
        assert_eq!(f32_text(1e-7), "0.0000001");
        assert_eq!(f64_text(0.1), "0.1");
        assert_eq!(f64_text(-0.0), "0");
        assert_eq!(f64_text(1.0 / 3.0), "0.3333333333333333");
        for x in [f32::MIN_POSITIVE, f32::MAX, 123.456, -9.99e-3, 7e20] {
            assert_eq!(f32_text(x).parse::<f32>().unwrap().to_bits(), x.to_bits());
        }
        for x in [
            f64::MIN_POSITIVE,
            f64::MAX,
            1e-300,
            -4.4e17,
            0.30000000000000004,
        ] {
            assert_eq!(f64_text(x).parse::<f64>().unwrap().to_bits(), x.to_bits());
            assert!(!f64_text(x).contains('e'));
        }
    }
}
