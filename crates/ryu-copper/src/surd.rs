//! Exact signs of numbers with square roots in them: `a + b√x` and
//! `a + b√x + c√y + d√(xy)` for integers `a, b, c, d` and `x, y ≥ 0`.
//!
//! Every distance between segments and arcs, compared with a threshold,
//! comes down to one of these (see [`crate::measure`]). The sign is found
//! by squaring: `a + b√x` with `a` and `b` of different signs has the sign
//! of `a` exactly when `a² > b²x`. With two roots, `U + V√y` (where
//! `U = a + b√x`, `V = c + d√x`) is decided from the signs of `U`, `V` and
//! `U² - yV²`, itself an `a' + b'√x`. Nothing is rounded, so equality is
//! found exactly too.

use std::cmp::Ordering;

use crate::wide::Int;

fn neg(o: Ordering) -> Ordering {
    o.reverse()
}

/// The sign of `a + b√x`, `x ≥ 0`.
pub fn sign1(a: Int, b: Int, x: Int) -> Ordering {
    debug_assert!(x.sign() != Ordering::Less);
    let sa = a.sign();
    let sb = if x.is_zero() {
        Ordering::Equal
    } else {
        b.sign()
    };
    if sb == Ordering::Equal || sa == sb {
        return if sa == Ordering::Equal { sb } else { sa };
    }
    if sa == Ordering::Equal {
        return sb;
    }
    // Opposite signs: a dominates when a² > b²x.
    let t = (a.square() - b.square() * x).sign();
    if sa == Ordering::Greater { t } else { neg(t) }
}

/// The sign of `a + b√x + c√y + d√(xy)`, `x, y ≥ 0`.
pub fn sign2(a: Int, b: Int, c: Int, d: Int, x: Int, y: Int) -> Ordering {
    debug_assert!(y.sign() != Ordering::Less);
    if y.is_zero() || (c.is_zero() && d.is_zero()) {
        return sign1(a, b, x);
    }
    let su = sign1(a, b, x);
    let sv = sign1(c, d, x);
    if sv == Ordering::Equal {
        return su;
    }
    if su == Ordering::Equal || su == sv {
        return sv;
    }
    // U and V√y have opposite signs: U dominates when U² > yV².
    // U² - yV² = (a² + b²x - y(c² + d²x)) + (2ab - 2ycd)√x.
    let two = Int::from(2i128);
    let p = a.square() + b.square() * x - y * (c.square() + d.square() * x);
    let q = two * (a * b - y * c * d);
    let t = sign1(p, q, x);
    if su == Ordering::Greater { t } else { neg(t) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(v: i128) -> Int {
        Int::from(v)
    }

    fn f(v: i128) -> f64 {
        v as f64
    }

    #[test]
    fn one_root_against_floats() {
        // Values far from zero, where f64 can't be wrong.
        for a in -20..=20i128 {
            for b in -20..=20i128 {
                for x in [0i128, 1, 2, 3, 5, 8, 50] {
                    let v = f(a) + f(b) * f(x).sqrt();
                    let want = if v.abs() < 1e-9 {
                        Ordering::Equal
                    } else if v > 0.0 {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    };
                    assert_eq!(sign1(i(a), i(b), i(x)), want, "{a} + {b}√{x}");
                }
            }
        }
    }

    #[test]
    fn exact_zeros() {
        // 3 - √9, 6 - 2√9, -4 + √16.
        assert_eq!(sign1(i(3), i(-1), i(9)), Ordering::Equal);
        assert_eq!(sign1(i(6), i(-2), i(9)), Ordering::Equal);
        assert_eq!(sign1(i(-4), i(1), i(16)), Ordering::Equal);
        // √2 + √8 - 3√2 = 0: a=0, b=1, c=1, d=0, x=2, y=8, then -3√2 as b.
        assert_eq!(sign2(i(0), i(-2), i(1), i(0), i(2), i(8)), Ordering::Equal);
        // 1 + √2 - √3 - √6 / ... : √6 = √(2·3).
        // (1 + √2)(1 - √3) = 1 + √2 - √3 - √6 < 0.
        assert_eq!(sign2(i(1), i(1), i(-1), i(-1), i(2), i(3)), Ordering::Less);
    }

    #[test]
    fn two_roots_against_floats() {
        let xs = [0i128, 2, 3, 7, 9];
        let mut g = 1u64;
        let mut next = || {
            g = g
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((g >> 33) % 41) as i128 - 20
        };
        for _ in 0..20_000 {
            let (a, b, c, d) = (next(), next(), next(), next());
            let x = xs[(next() + 20) as usize % xs.len()];
            let y = xs[(next() + 20) as usize % xs.len()];
            let v = f(a) + f(b) * f(x).sqrt() + f(c) * f(y).sqrt() + f(d) * f(x * y).sqrt();
            if v.abs() < 1e-6 {
                continue;
            }
            let want = if v > 0.0 {
                Ordering::Greater
            } else {
                Ordering::Less
            };
            assert_eq!(
                sign2(i(a), i(b), i(c), i(d), i(x), i(y)),
                want,
                "{a} {b} {c} {d} {x} {y}"
            );
        }
    }

    #[test]
    fn near_misses_are_decided() {
        // √(10^24 + 1) - 10^12 is about 5e-13: positive, though f64 says 0.
        let big = 1_000_000_000_000i128;
        assert_eq!(sign1(i(-big), i(1), i(big * big + 1)), Ordering::Greater);
        assert_eq!(sign1(i(-big), i(1), i(big * big - 1)), Ordering::Less);
        assert_eq!(sign1(i(-big), i(1), i(big * big)), Ordering::Equal);
    }
}
