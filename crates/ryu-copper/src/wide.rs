//! Wide integers for the exact predicates: sign and magnitude, the
//! magnitude in up to [`LIMBS`] 64-bit limbs (2048 bits), with only the
//! limbs in use touched, so small numbers stay cheap.
//!
//! Add, subtract, multiply and compare; no division. Every predicate in
//! [`crate::measure`] derives its bit budget in its documentation and stays
//! within 2048 bits for coordinates within ±2^40 nm; a result past the
//! budget panics as a broken invariant, never wraps.

use std::cmp::Ordering;
use std::ops::{Add, Mul, Neg, Sub};

/// The most limbs a magnitude holds.
pub const LIMBS: usize = 32;

/// A signed integer of up to 2048 bits.
#[derive(Clone, Copy)]
pub struct Int {
    neg: bool,
    len: usize,
    mag: [u64; LIMBS],
}

impl Int {
    pub const ZERO: Int = Int {
        neg: false,
        len: 0,
        mag: [0; LIMBS],
    };

    pub fn from_i128(v: i128) -> Int {
        let mut out = Int::ZERO;
        let m = v.unsigned_abs();
        out.mag[0] = m as u64;
        out.mag[1] = (m >> 64) as u64;
        out.len = 2;
        out.neg = v < 0;
        out.trim();
        out
    }

    fn trim(&mut self) {
        while self.len > 0 && self.mag[self.len - 1] == 0 {
            self.len -= 1;
        }
        if self.len == 0 {
            self.neg = false;
        }
    }

    pub fn is_zero(&self) -> bool {
        self.len == 0
    }

    /// -1, 0 or 1 as an ordering against zero.
    pub fn sign(&self) -> Ordering {
        if self.len == 0 {
            Ordering::Equal
        } else if self.neg {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }

    /// The number of bits the magnitude needs.
    pub fn bits(&self) -> u32 {
        if self.len == 0 {
            0
        } else {
            64 * self.len as u32 - self.mag[self.len - 1].leading_zeros()
        }
    }

    /// The value as an `i128`, when it fits.
    pub fn to_i128(&self) -> Option<i128> {
        if self.len > 2 {
            return None;
        }
        let m = u128::from(self.mag[0]) | (u128::from(self.mag[1]) << 64);
        if self.neg {
            if m > i128::MAX as u128 + 1 {
                None
            } else {
                Some((m as i128).wrapping_neg())
            }
        } else {
            i128::try_from(m).ok()
        }
    }

    /// The value rounded to `f64` limb by limb: deterministic, for
    /// estimates only.
    pub fn to_f64(&self) -> f64 {
        let mut v = 0.0;
        for k in (0..self.len).rev() {
            v = v * 18_446_744_073_709_551_616.0 + self.mag[k] as f64;
        }
        if self.neg { -v } else { v }
    }

    pub fn square(self) -> Int {
        self * self
    }
}

fn cmp_mag(a: &Int, b: &Int) -> Ordering {
    a.len.cmp(&b.len).then_with(|| {
        for i in (0..a.len).rev() {
            match a.mag[i].cmp(&b.mag[i]) {
                Ordering::Equal => {}
                o => return o,
            }
        }
        Ordering::Equal
    })
}

const BUDGET: &str = "a wide integer passed its 2048-bit budget";

/// `|a| + |b|`, with `a`'s sign.
fn add_mag(a: &Int, b: &Int) -> Int {
    let mut out = Int::ZERO;
    let n = a.len.max(b.len);
    let mut carry = 0u64;
    for i in 0..n {
        let (s1, c1) = a.mag[i].overflowing_add(b.mag[i]);
        let (s2, c2) = s1.overflowing_add(carry);
        out.mag[i] = s2;
        carry = u64::from(c1) + u64::from(c2);
    }
    out.len = n;
    if carry != 0 {
        assert!(n < LIMBS, "{BUDGET}");
        out.mag[n] = carry;
        out.len = n + 1;
    }
    out.neg = a.neg;
    out.trim();
    out
}

/// `|a| - |b|` for `|a| >= |b|`, with `a`'s sign.
fn sub_mag(a: &Int, b: &Int) -> Int {
    let mut out = Int::ZERO;
    let mut borrow = 0u64;
    for i in 0..a.len {
        let (d1, b1) = a.mag[i].overflowing_sub(b.mag[i]);
        let (d2, b2) = d1.overflowing_sub(borrow);
        out.mag[i] = d2;
        borrow = u64::from(b1) + u64::from(b2);
    }
    out.len = a.len;
    out.neg = a.neg;
    out.trim();
    out
}

impl Add for Int {
    type Output = Int;
    fn add(self, b: Int) -> Int {
        if self.neg == b.neg || b.is_zero() || self.is_zero() {
            if self.is_zero() {
                return b;
            }
            return add_mag(&self, &b);
        }
        match cmp_mag(&self, &b) {
            Ordering::Less => sub_mag(&b, &self),
            Ordering::Equal => Int::ZERO,
            Ordering::Greater => sub_mag(&self, &b),
        }
    }
}

impl Neg for Int {
    type Output = Int;
    fn neg(mut self) -> Int {
        if self.len != 0 {
            self.neg = !self.neg;
        }
        self
    }
}

impl Sub for Int {
    type Output = Int;
    fn sub(self, b: Int) -> Int {
        self + (-b)
    }
}

impl Mul for Int {
    type Output = Int;
    fn mul(self, b: Int) -> Int {
        if self.is_zero() || b.is_zero() {
            return Int::ZERO;
        }
        let n = self.len + b.len;
        assert!(n <= LIMBS + 1, "{BUDGET}");
        let mut acc = [0u64; LIMBS + 1];
        for i in 0..self.len {
            let mut carry = 0u128;
            let x = u128::from(self.mag[i]);
            for j in 0..b.len {
                let t = x * u128::from(b.mag[j]) + u128::from(acc[i + j]) + carry;
                acc[i + j] = t as u64;
                carry = t >> 64;
            }
            let mut k = i + b.len;
            while carry != 0 {
                let t = u128::from(acc[k]) + carry;
                acc[k] = t as u64;
                carry = t >> 64;
                k += 1;
            }
        }
        let mut len = n;
        while len > 0 && acc[len - 1] == 0 {
            len -= 1;
        }
        assert!(len <= LIMBS, "{BUDGET}");
        let mut out = Int::ZERO;
        out.mag[..len].copy_from_slice(&acc[..len]);
        out.len = len;
        out.neg = self.neg != b.neg;
        out
    }
}

impl From<i128> for Int {
    fn from(v: i128) -> Int {
        Int::from_i128(v)
    }
}

impl From<i64> for Int {
    fn from(v: i64) -> Int {
        Int::from_i128(i128::from(v))
    }
}

impl PartialEq for Int {
    fn eq(&self, o: &Int) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Int {}

impl PartialOrd for Int {
    fn partial_cmp(&self, o: &Int) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Int {
    fn cmp(&self, o: &Int) -> Ordering {
        match (self.sign(), o.sign()) {
            (a, b) if a != b => a.cmp(&b),
            (Ordering::Equal, _) => Ordering::Equal,
            (Ordering::Greater, _) => cmp_mag(self, o),
            _ => cmp_mag(o, self),
        }
    }
}

impl std::fmt::Debug for Int {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(v) = self.to_i128() {
            return write!(f, "{v}");
        }
        write!(f, "{}0x", if self.neg { "-" } else { "" })?;
        for i in (0..self.len).rev() {
            write!(f, "{:016x}", self.mag[i])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(v: i128) -> Int {
        Int::from(v)
    }

    #[test]
    fn small_values_agree_with_i128() {
        let vals = [
            0i128,
            1,
            -1,
            7,
            -13,
            1 << 62,
            -(1 << 63),
            (1 << 64) - 1,
            -(1 << 64),
            (1 << 100) + 12345,
            -(1 << 90) - 777,
        ];
        for &a in &vals {
            assert_eq!(i(a).to_i128(), Some(a));
            assert_eq!((-i(a)).to_i128(), Some(-a));
            for &b in &vals {
                assert_eq!((i(a) + i(b)).to_i128(), Some(a + b), "{a} + {b}");
                assert_eq!((i(a) - i(b)).to_i128(), Some(a - b), "{a} - {b}");
                assert_eq!(i(a).cmp(&i(b)), a.cmp(&b), "{a} <> {b}");
                if a.unsigned_abs() < 1 << 62 && b.unsigned_abs() < 1 << 62 {
                    assert_eq!((i(a) * i(b)).to_i128(), Some(a * b), "{a} * {b}");
                }
            }
        }
        assert_eq!(i(i128::MIN).to_i128(), Some(i128::MIN));
        assert_eq!(i(i128::MAX).to_i128(), Some(i128::MAX));
        assert_eq!((i(i128::MAX) + i(1)).to_i128(), None);
    }

    #[test]
    fn products_past_i128_are_exact() {
        // (2^100 + 1)^2 = 2^200 + 2^101 + 1.
        let a = i((1 << 100) + 1);
        let sq = a.square();
        assert_eq!(sq.bits(), 201);
        let back = sq - i(1) - i(1 << 101);
        // 2^200 = (2^100)^2.
        assert_eq!(back, i(1 << 100) * i(1 << 100));
        assert_eq!((-a * a).sign(), Ordering::Less);
        assert!(-a * a < Int::ZERO);
        assert!(a * a > a);
    }

    #[test]
    fn the_budget_holds_2047_bits() {
        // (2^127 - 1)^16 has 2032 bits and fits.
        let x = i(i128::MAX);
        let x2 = x * x;
        let x4 = x2 * x2;
        let x8 = x4 * x4;
        let x16 = x8 * x8;
        assert_eq!(x16.bits(), 2032);
        assert_eq!((x16 - x16), Int::ZERO);
        assert_eq!((x16 + x16).bits(), 2033);
    }

    #[test]
    #[should_panic(expected = "2048-bit budget")]
    fn past_the_budget_panics() {
        let x = i(i128::MAX);
        let x16 = {
            let x2 = x * x;
            let x4 = x2 * x2;
            let x8 = x4 * x4;
            x8 * x8
        };
        let _ = x16 * x16;
    }
}
