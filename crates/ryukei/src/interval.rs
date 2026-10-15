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
            lo: lo.next_down(),
            hi: hi.next_up(),
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
        let lo = math::sqrt(self.lo.max(0.0)).next_down().max(0.0);
        Some(Self {
            lo,
            hi: math::sqrt(self.hi).next_up(),
        })
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
    fn sign_is_certified_or_uncertain() {
        assert_eq!(iv(1e-300, 1.0).sign(), Some(Sign::Positive));
        assert_eq!(iv(-1.0, -1e-300).sign(), Some(Sign::Negative));
        assert_eq!(Interval::point(0.0).sign(), Some(Sign::Zero));
        assert_eq!(iv(-1e-300, 1e-300).sign(), None);
        // Widening makes even an exact zero result uncertain.
        assert_eq!((Interval::point(1.0) - Interval::point(1.0)).sign(), None);
    }
}
