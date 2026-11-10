//! Interval arithmetic with portable outward rounding.
//!
//! Each bound is computed in the default round-to-nearest mode and then
//! widened by one ulp with `next_down` / `next_up`. Round-to-nearest is off
//! by at most half an ulp, so the true result always lies inside. This never
//! touches the floating-point control registers, so it behaves the same on
//! every platform and inside any thread.
//!
//! Intervals certify signs: [`Interval::sign`] answers only when the whole
//! interval is on one side of zero, and returns `None` (uncertain) otherwise.
//! An uncertain caller must escalate to exact arithmetic or apply an explicit
//! tolerance rule, never guess.

use crate::math;
use crate::predicates::Sign;
use core::ops::{Add, Mul, Neg, Sub};

/// A closed interval `[lo, hi]` known to contain the true value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    lo: f64,
    hi: f64,
}

impl Interval {
    /// The interval holding exactly `x`.
    #[inline]
    pub fn point(x: f64) -> Self {
        Self { lo: x, hi: x }
    }

    /// `[lo, hi]`, or `None` unless `lo <= hi` (so never NaN).
    #[inline]
    pub fn new(lo: f64, hi: f64) -> Option<Self> {
        (lo <= hi).then_some(Self { lo, hi })
    }

    #[inline]
    pub fn lo(self) -> f64 {
        self.lo
    }

    #[inline]
    pub fn hi(self) -> f64 {
        self.hi
    }

    pub fn contains(self, x: f64) -> bool {
        self.lo <= x && x <= self.hi
    }

    /// Widen round-to-nearest bounds by one ulp each way.
    #[inline]
    fn outward(lo: f64, hi: f64) -> Self {
        Self {
            lo: down(lo),
            hi: up(hi),
        }
    }

    /// The smallest interval holding both.
    #[inline]
    pub fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// The largest magnitude of any value inside: `max(|lo|, |hi|)`.
    #[inline]
    pub fn mag(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }

    /// `x²` for every `x` inside, tighter than `self * self` when the
    /// interval straddles zero.
    #[inline]
    pub fn square(self) -> Self {
        if self.lo >= 0.0 {
            Self::outward(self.lo * self.lo, self.hi * self.hi)
        } else if self.hi <= 0.0 {
            Self::outward(self.hi * self.hi, self.lo * self.lo)
        } else {
            let m = self.mag();
            Self {
                lo: 0.0,
                hi: up(m * m),
            }
        }
    }

    /// The certified sign, or `None` when the interval contains zero.
    pub fn sign(self) -> Option<Sign> {
        if self.lo > 0.0 {
            Some(Sign::Positive)
        } else if self.hi < 0.0 {
            Some(Sign::Negative)
        } else if self.lo == 0.0 && self.hi == 0.0 {
            Some(Sign::Zero)
        } else {
            None
        }
    }

    /// `None` when `rhs` contains zero.
    pub fn checked_div(self, rhs: Self) -> Option<Self> {
        if rhs.contains(0.0) {
            return None;
        }
        let q = [
            self.lo / rhs.lo,
            self.lo / rhs.hi,
            self.hi / rhs.lo,
            self.hi / rhs.hi,
        ];
        Some(Self::outward(min4(q), max4(q)))
    }

    /// `None` when the interval is entirely negative. A partly negative
    /// interval is clamped to its non-negative part first.
    pub fn sqrt(self) -> Option<Self> {
        if self.hi < 0.0 {
            return None;
        }
        let lo = down(math::sqrt(self.lo.max(0.0))).max(0.0);
        Some(Self {
            lo,
            hi: up(math::sqrt(self.hi)),
        })
    }
}

/// Bounds closer to zero than this are widened to it (or to zero, on the
/// side that keeps their sign), so interval arithmetic never makes a
/// subnormal number. A bound of exactly zero widened by one ulp was one
/// (`0.0.next_down()` is `-5e-324`), and every product of it was another:
/// on CPUs that handle subnormals in microcode (Intel before Ice Lake, AMD
/// before Zen) each such operation costs about a hundred cycles, and the
/// self-intersection check of a NIST body ran twice as long on a Xeon
/// E5-2650 as with subnormals flushed (#301). `TINY` is `2^-511`, so its
/// square is still normal, as is its product with anything above
/// `2^-511`. The widening only grows intervals, so they still hold the true
/// value; a sign is lost only for values within `2^-511` (about `1.5e-154`)
/// of zero, far below any tolerance. Every bound outside that range is
/// the same bits as one ulp's widening gives.
pub(crate) const TINY: f64 = f64::from_bits(0x2000_0000_0000_0000);

/// The next float down from a round-to-nearest lower bound, kept out of
/// the subnormal range. The input is judged, not the step, so no subnormal
/// is made even to be compared: for `|x| < TINY`, `next_down(x) >= -TINY`,
/// and `next_down(x) > 0` when `x > 0`.
///
/// Judged on the bits, as [`up`] is: the same floats as the rule says for
/// every input (`down_by_rule` in the tests), at a fraction of
/// `next_down`'s general cases (#308: these two were a quarter of a
/// spline mesh's time).
#[inline]
fn down(x: f64) -> f64 {
    let b = x.to_bits();
    let a = b & !SIGN;
    if a < TINY_BITS {
        // |x| < TINY: 0 for x > 0, else -TINY (-0 and 0 included).
        if b == a && a != 0 { 0.0 } else { -TINY }
    } else if a > INF_BITS || b == NEG_INF_BITS {
        // NaN, and -inf, step nowhere.
        x
    } else if b == a {
        // Positive (+inf to MAX): one step towards zero.
        f64::from_bits(b - 1)
    } else {
        // Negative: one step away from zero.
        f64::from_bits(b + 1)
    }
}

/// The next float up from a round-to-nearest upper bound, kept out of the
/// subnormal range, as [`down`] is.
///
/// Judged on the bits, as [`down`] is.
#[inline]
fn up(x: f64) -> f64 {
    let b = x.to_bits();
    let a = b & !SIGN;
    if a < TINY_BITS {
        // |x| < TINY: 0 for x < 0, else TINY (-0 and 0 included).
        if b != a && a != 0 { 0.0 } else { TINY }
    } else if a > INF_BITS || b == INF_BITS {
        // NaN, and +inf, step nowhere.
        x
    } else if b == a {
        // Positive: one step away from zero.
        f64::from_bits(b + 1)
    } else {
        // Negative (-inf to -MAX): one step towards zero.
        f64::from_bits(b - 1)
    }
}

const SIGN: u64 = 1 << 63;
const TINY_BITS: u64 = TINY.to_bits();
const INF_BITS: u64 = f64::INFINITY.to_bits();
const NEG_INF_BITS: u64 = f64::NEG_INFINITY.to_bits();

/// `(x + y).hi()` for intervals whose upper ends are `a` and `b`, by the
/// rule [`Add`] applies: the round-to-nearest sum stepped up by [`up`].
/// For bounds that are sums and products of non-negative terms, where only
/// the upper ends are wanted, this is the same float without the lower
/// ends' work (#308).
#[inline]
pub(crate) fn add_up(a: f64, b: f64) -> f64 {
    up(a + b)
}

/// `(x * y).hi()` for intervals whose upper ends are `a` and `b`, each
/// either a point at least zero or made from such points by [`add_up`] and
/// [`mul_up`] (so its lower end is at least `-TINY` and its upper end at
/// least `TINY` when its lower end is below zero): [`Mul`] then takes the
/// product of the upper ends, stepped up by [`up`], whatever their signs'
/// cases, and the same float for infinities and NaN.
#[inline]
pub(crate) fn mul_up(a: f64, b: f64) -> f64 {
    up(a * b)
}

#[inline]
fn min4(v: [f64; 4]) -> f64 {
    v[0].min(v[1]).min(v[2]).min(v[3])
}

#[inline]
fn max4(v: [f64; 4]) -> f64 {
    v[0].max(v[1]).max(v[2]).max(v[3])
}

impl Add for Interval {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::outward(self.lo + rhs.lo, self.hi + rhs.hi)
    }
}

impl Sub for Interval {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::outward(self.lo - rhs.hi, self.hi - rhs.lo)
    }
}

impl Neg for Interval {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            lo: -self.hi,
            hi: -self.lo,
        }
    }
}

impl Mul for Interval {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        let (a, b) = (self, rhs);
        if a.lo.is_finite() && a.hi.is_finite() && b.lo.is_finite() && b.hi.is_finite() {
            // By the bounds' signs, the two products the least and the
            // greatest of all four are: rounding to nearest is monotone,
            // so with finite bounds (no product NaN) these are the same
            // floats `min4` and `max4` pick, but for the sign of a zero,
            // which `outward` takes to the same bound either way.
            let (lo, hi) = if a.lo >= 0.0 {
                if b.lo >= 0.0 {
                    (a.lo * b.lo, a.hi * b.hi)
                } else if b.hi <= 0.0 {
                    (a.hi * b.lo, a.lo * b.hi)
                } else {
                    (a.hi * b.lo, a.hi * b.hi)
                }
            } else if a.hi <= 0.0 {
                if b.lo >= 0.0 {
                    (a.lo * b.hi, a.hi * b.lo)
                } else if b.hi <= 0.0 {
                    (a.hi * b.hi, a.lo * b.lo)
                } else {
                    (a.lo * b.hi, a.lo * b.lo)
                }
            } else if b.lo >= 0.0 {
                (a.lo * b.hi, a.hi * b.hi)
            } else if b.hi <= 0.0 {
                (a.hi * b.lo, a.lo * b.lo)
            } else {
                (
                    (a.lo * b.hi).min(a.hi * b.lo),
                    (a.lo * b.lo).max(a.hi * b.hi),
                )
            };
            return Self::outward(lo, hi);
        }
        let p = [
            self.lo * rhs.lo,
            self.lo * rhs.hi,
            self.hi * rhs.lo,
            self.hi * rhs.hi,
        ];
        Self::outward(min4(p), max4(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down_by_rule(x: f64) -> f64 {
        if x > -TINY && x < TINY {
            if x > 0.0 { 0.0 } else { -TINY }
        } else {
            x.next_down()
        }
    }

    fn up_by_rule(x: f64) -> f64 {
        if x > -TINY && x < TINY {
            if x < 0.0 { 0.0 } else { TINY }
        } else {
            x.next_up()
        }
    }

    /// `up` and `down` on the bits are the rule's floats, NaN for NaN: at
    /// every special value, every float near the edges they judge, and
    /// two million drawn from all bit patterns (#308).
    #[test]
    fn steps_on_the_bits_follow_the_rule() {
        let same = |a: f64, b: f64| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        let check = |x: f64| {
            assert!(
                same(up(x), up_by_rule(x)),
                "up({x:e}): {:e} against {:e}",
                up(x),
                up_by_rule(x)
            );
            assert!(
                same(down(x), down_by_rule(x)),
                "down({x:e}): {:e} against {:e}",
                down(x),
                down_by_rule(x)
            );
        };
        let edges = [0.0, TINY, f64::MIN_POSITIVE, f64::MAX, f64::INFINITY, 1.0];
        for e in edges {
            let mut x = e;
            for _ in 0..4 {
                x = x.next_down();
            }
            for _ in 0..9 {
                for v in [x, -x] {
                    check(v);
                }
                x = x.next_up();
            }
        }
        for x in [
            f64::NAN,
            -f64::NAN,
            f64::from_bits(0x7fff_ffff_ffff_ffff),
            f64::from_bits(0xfff0_0000_0000_0001),
        ] {
            check(x);
        }
        let mut b: u64 = 0x308_0308;
        for _ in 0..2_000_000 {
            b ^= b << 13;
            b ^= b >> 7;
            b ^= b << 17;
            check(f64::from_bits(b));
        }
    }

    fn iv(lo: f64, hi: f64) -> Interval {
        Interval::new(lo, hi).unwrap()
    }

    #[test]
    fn construction() {
        assert!(Interval::new(2.0, 1.0).is_none());
        assert!(Interval::new(f64::NAN, 1.0).is_none());
        assert_eq!(Interval::point(3.0).lo(), 3.0);
        assert!(iv(1.0, 2.0).contains(1.5));
        assert!(!iv(1.0, 2.0).contains(2.5));
    }

    #[test]
    fn arithmetic_contains_the_true_result() {
        // 0.1 + 0.2 is not 0.3 in f64, but the interval holds the real sum.
        let s = Interval::point(0.1) + Interval::point(0.2);
        assert!(s.contains(0.1 + 0.2));
        assert!(s.lo() < s.hi());
        assert!((iv(1.0, 2.0) - iv(0.5, 3.0)).contains(-2.0));
        let m = iv(-2.0, 3.0) * iv(-1.0, 4.0);
        assert!(m.lo() <= -8.0 && m.hi() >= 12.0);
        assert_eq!(-iv(1.0, 2.0), iv(-2.0, -1.0));
    }

    #[test]
    fn division() {
        assert!(iv(1.0, 2.0).checked_div(iv(-1.0, 1.0)).is_none());
        let q = iv(1.0, 2.0).checked_div(iv(4.0, 8.0)).unwrap();
        assert!(q.lo() <= 0.125 && q.hi() >= 0.5);
    }

    #[test]
    fn square_root() {
        assert!(iv(-2.0, -1.0).sqrt().is_none());
        let r = iv(-1.0, 4.0).sqrt().unwrap();
        assert_eq!(r.lo(), 0.0);
        assert!(r.contains(2.0));
        let two = Interval::point(2.0).sqrt().unwrap();
        assert!(two.lo() * two.lo() <= 2.0 && two.hi() * two.hi() >= 2.0);
    }

    #[test]
    fn hull_magnitude_and_square() {
        assert_eq!(iv(1.0, 2.0).hull(iv(-1.0, 0.5)), iv(-1.0, 2.0));
        assert_eq!(iv(-3.0, 2.0).mag(), 3.0);
        let s = iv(-3.0, 2.0).square();
        assert!(s.lo() == 0.0 && s.hi() >= 9.0);
        let s = iv(-3.0, -2.0).square();
        assert!(s.contains(4.0) && s.contains(9.0) && s.lo() > 3.9);
        assert!(iv(2.0, 3.0).square().contains(9.0));
    }

    #[test]
    fn sign_is_certified_or_uncertain() {
        assert_eq!(iv(1e-300, 1.0).sign(), Some(Sign::Positive));
        assert_eq!(iv(-1.0, -1e-300).sign(), Some(Sign::Negative));
        assert_eq!(Interval::point(0.0).sign(), Some(Sign::Zero));
        assert_eq!(iv(-1e-300, 1e-300).sign(), None);
        // Widening makes even an exact zero result uncertain.
        assert_eq!((Interval::point(1.0) - Interval::point(1.0)).sign(), None);
    }

    /// No operation makes a subnormal bound (#301): exact zeros, products
    /// of them, cancellations and tiny results all widen to `±TINY` or
    /// zero instead, and still hold the true value.
    #[test]
    fn no_bound_is_subnormal() {
        let tiny_bound = |x: Interval| {
            assert!(!x.lo.is_subnormal() && !x.hi.is_subnormal(), "{x:?}");
            x
        };
        let zero = Interval::point(0.0);
        let one = Interval::point(1.0);
        let z = tiny_bound(zero * one);
        assert!(z.contains(0.0) && z.lo() == -TINY && z.hi() == TINY);
        tiny_bound(one - one);
        tiny_bound(z * z);
        tiny_bound(z * iv(-3.0, 0.5));
        tiny_bound(zero + zero);
        tiny_bound(iv(-1e-300, 1e-300) * iv(1e-20, 1.0));
        let small = tiny_bound(iv(1e-100, 2e-100) * iv(1e-100, 1e-60));
        // A non-negative result keeps its lower bound non-negative.
        assert_eq!(small.lo(), 0.0);
        assert!(small.hi() >= 2e-160);
        let neg = tiny_bound(iv(-2e-100, -1e-100) * iv(1e-100, 1e-60));
        assert_eq!(neg.hi(), 0.0);
        tiny_bound(iv(0.0, 1e-300).square());
        tiny_bound(iv(0.0, 1e-310).sqrt().unwrap());
        tiny_bound(iv(1e-300, 1e-299).checked_div(iv(1e10, 1e11)).unwrap());
        // Away from zero, the bounds are one ulp out, as before.
        let x = iv(1.0, 2.0) * iv(3.0, 5.0);
        assert_eq!((x.lo(), x.hi()), (3.0f64.next_down(), 10.0f64.next_up()));
    }

    /// The product by the bounds' signs gives the bits the four products'
    /// least and greatest give, over bounds of every sign, zeros of both
    /// signs, tiny and huge magnitudes, and infinities.
    #[test]
    fn products_by_sign_match_all_four() {
        let values = [
            0.0,
            -0.0,
            1e-300,
            -1e-300,
            TINY,
            -TINY,
            5e-324,
            -5e-324,
            0.3,
            -0.3,
            1.0,
            -1.0,
            7.25,
            -7.25,
            1e200,
            -1e200,
            f64::MAX,
            -f64::MAX,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut more = Vec::new();
        for _ in 0..200 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let x = f64::from_bits(seed);
            if x.is_finite() {
                more.push(x);
            }
        }
        let all: Vec<f64> = values.iter().copied().chain(more).collect();
        let mut intervals = Vec::new();
        for &a in &all {
            for &b in &all {
                if let Some(i) = Interval::new(a.min(b), a.max(b)) {
                    intervals.push(i);
                }
            }
        }
        let old = |x: Interval, y: Interval| {
            let p = [x.lo * y.lo, x.lo * y.hi, x.hi * y.lo, x.hi * y.hi];
            Interval::outward(min4(p), max4(p))
        };
        let mut n = 0usize;
        for (i, &x) in intervals.iter().enumerate().step_by(7) {
            for &y in intervals.iter().skip(i % 11).step_by(13) {
                let (a, b) = (x * y, old(x, y));
                assert!(
                    a.lo.to_bits() == b.lo.to_bits() && a.hi.to_bits() == b.hi.to_bits(),
                    "{x:?} * {y:?}: {a:?} against {b:?}"
                );
                n += 1;
            }
        }
        assert!(n > 100_000, "only {n} products");
    }
}
