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
    pub fn point(x: f64) -> Self {
        Self { lo: x, hi: x }
    }

    /// `[lo, hi]`, or `None` unless `lo <= hi` (so never NaN).
    pub fn new(lo: f64, hi: f64) -> Option<Self> {
        (lo <= hi).then_some(Self { lo, hi })
    }

    pub fn lo(self) -> f64 {
        self.lo
    }

    pub fn hi(self) -> f64 {
        self.hi
    }

    pub fn contains(self, x: f64) -> bool {
        self.lo <= x && x <= self.hi
    }

    /// Widen round-to-nearest bounds by one ulp each way.
    fn outward(lo: f64, hi: f64) -> Self {
        Self {
            lo: down(lo),
            hi: up(hi),
        }
    }

    /// The smallest interval holding both.
    pub fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// The largest magnitude of any value inside: `max(|lo|, |hi|)`.
    pub fn mag(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }

    /// `x²` for every `x` inside, tighter than `self * self` when the
    /// interval straddles zero.
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
fn down(x: f64) -> f64 {
    if x > -TINY && x < TINY {
        if x > 0.0 { 0.0 } else { -TINY }
    } else {
        x.next_down()
    }
}

/// The next float up from a round-to-nearest upper bound, kept out of the
/// subnormal range, as [`down`] is.
fn up(x: f64) -> f64 {
    if x > -TINY && x < TINY {
        if x < 0.0 { 0.0 } else { TINY }
    } else {
        x.next_up()
    }
}

fn min4(v: [f64; 4]) -> f64 {
    v[0].min(v[1]).min(v[2]).min(v[3])
}

fn max4(v: [f64; 4]) -> f64 {
    v[0].max(v[1]).max(v[2]).max(v[3])
}

impl Add for Interval {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::outward(self.lo + rhs.lo, self.hi + rhs.hi)
    }
}

impl Sub for Interval {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::outward(self.lo - rhs.hi, self.hi - rhs.lo)
    }
}

impl Neg for Interval {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            lo: -self.hi,
            hi: -self.lo,
        }
    }
}

impl Mul for Interval {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
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
}
