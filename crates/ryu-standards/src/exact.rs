//! Exact integer arithmetic for the formulas: products of powers
//! compared exactly, integer square roots, and ISO 3's R40 series.
//! Nothing here rounds but where a caller says so.

use std::cmp::Ordering;

/// An unsigned integer of any size, little-endian in 32-bit limbs, with
/// no zero limb on top: enough to compare `p^61` exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Big(Vec<u32>);

impl Big {
    pub(crate) fn from_u128(mut n: u128) -> Big {
        let mut v = Vec::new();
        while n > 0 {
            v.push(n as u32);
            n >>= 32;
        }
        Big(v)
    }

    pub(crate) fn mul(&self, o: &Big) -> Big {
        if self.0.is_empty() || o.0.is_empty() {
            return Big(Vec::new());
        }
        let mut out = vec![0u32; self.0.len() + o.0.len()];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.0.iter().enumerate() {
                // (2^32 - 1)^2 + 2 (2^32 - 1) = 2^64 - 1: no overflow.
                let t = u64::from(a) * u64::from(b) + u64::from(out[i + j]) + carry;
                out[i + j] = t as u32;
                carry = t >> 32;
            }
            let mut k = i + o.0.len();
            while carry > 0 {
                let t = u64::from(out[k]) + carry;
                out[k] = t as u32;
                carry = t >> 32;
                k += 1;
            }
        }
        while out.last() == Some(&0) {
            out.pop();
        }
        Big(out)
    }

    /// `self + other`.
    pub(crate) fn add(&self, other: &Big) -> Big {
        let n = self.0.len().max(other.0.len());
        let mut out = Vec::with_capacity(n + 1);
        let mut carry = 0u64;
        for i in 0..n {
            let s = u64::from(*self.0.get(i).unwrap_or(&0))
                + u64::from(*other.0.get(i).unwrap_or(&0))
                + carry;
            out.push(s as u32);
            carry = s >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        Big::trimmed(out)
    }

    /// `self - other`; `other` is no greater.
    pub(crate) fn sub(&self, other: &Big) -> Big {
        debug_assert!(*other <= *self);
        let mut out = Vec::with_capacity(self.0.len());
        let mut borrow = 0i64;
        for i in 0..self.0.len() {
            let mut d = i64::from(self.0[i]) - i64::from(*other.0.get(i).unwrap_or(&0)) - borrow;
            borrow = i64::from(d < 0);
            if d < 0 {
                d += 1 << 32;
            }
            out.push(d as u32);
        }
        Big::trimmed(out)
    }

    fn trimmed(mut v: Vec<u32>) -> Big {
        while v.last() == Some(&0) {
            v.pop();
        }
        Big(v)
    }

    pub(crate) fn pow(&self, mut e: u32) -> Big {
        let mut base = self.clone();
        let mut acc = Big::from_u128(1);
        while e > 0 {
            if e & 1 == 1 {
                acc = acc.mul(&base);
            }
            e >>= 1;
            if e > 0 {
                base = base.mul(&base);
            }
        }
        acc
    }
}

impl Ord for Big {
    fn cmp(&self, o: &Big) -> Ordering {
        self.0
            .len()
            .cmp(&o.0.len())
            .then_with(|| self.0.iter().rev().cmp(o.0.iter().rev()))
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, o: &Big) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// The product of `base^exp` over `terms`.
pub(crate) fn prod(terms: &[(u128, u32)]) -> Big {
    terms.iter().fold(Big::from_u128(1), |acc, &(b, e)| {
        acc.mul(&Big::from_u128(b).pow(e))
    })
}

/// `floor(sqrt(n))`.
pub(crate) fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    // Start at or above the root; Newton's steps then fall to it.
    let mut x = 1u128 << (128 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// ISO 3's R40 preferred numbers in a decade, times 100.
const R40: [u128; 40] = [
    100, 106, 112, 118, 125, 132, 140, 150, 160, 170, 180, 190, 200, 212, 224, 236, 250, 265, 280,
    300, 315, 335, 355, 375, 400, 425, 450, 475, 500, 530, 560, 600, 630, 670, 710, 750, 800, 850,
    900, 950,
];

/// The R40 number nearest a value between 1 µm and 10 mm, in hundredths
/// of a micrometre, given `at_least(n, d)`: whether the value in
/// micrometres is at least `n / d`. Nearest is by difference; a value
/// exactly halfway goes up.
pub(crate) fn nearest_r40(at_least: impl Fn(u128, u128) -> bool) -> u64 {
    // Each candidate c is c / 100 µm.
    let candidates: Vec<u128> = (0..4u32)
        .flat_map(|k| R40.iter().map(move |r| r * 10u128.pow(k)))
        .chain([1_000_000])
        .collect();
    let above = candidates
        .iter()
        .position(|&c| !at_least(c, 100))
        .unwrap_or(candidates.len() - 1);
    if above == 0 {
        return candidates[0] as u64;
    }
    let (lo, hi) = (candidates[above - 1], candidates[above]);
    // At least the midpoint (lo + hi) / 200 µm: the upper one.
    if at_least(lo + hi, 200) {
        hi as u64
    } else {
        lo as u64
    }
}

/// Hundredths of a micrometre to whole micrometres, halves up.
pub(crate) fn whole_um(centi: u64) -> u64 {
    (centi + 50) / 100
}

/// `n / d` rounded to the nearest whole number, halves up; `d > 0`.
pub(crate) fn round_div(n: i128, d: i128) -> i128 {
    (2 * n + d).div_euclid(2 * d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_compare_exactly() {
        // 2^100 against 10^30: 1.27e30 > 1e30.
        assert!(prod(&[(2, 100)]) > prod(&[(10, 30)]));
        // 3^40 = 12157665459056928801 exactly.
        assert_eq!(prod(&[(3, 40)]), Big::from_u128(12_157_665_459_056_928_801));
        assert_eq!(prod(&[(7, 0)]), Big::from_u128(1));
        assert_eq!(prod(&[(0, 3), (5, 2)]), Big::from_u128(0));
        // A product past u128: (2^64)^3 = 2^192 = (2^96)^2.
        assert_eq!(prod(&[(1 << 64, 3)]), prod(&[(1 << 96, 2)]));
    }

    #[test]
    fn square_roots_are_floors() {
        for n in [0u128, 1, 2, 3, 4, 15, 16, 17, 99, 100, u128::from(u64::MAX)] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n}");
        }
        assert_eq!(isqrt(75 * 1_000_000u128.pow(2)), 8_660_254);
    }

    #[test]
    fn r40_rounds_to_the_nearest_preferred_number() {
        // 230 µm lies between 224 and 236, nearer 236 (ISO 965-1's
        // T_D1(6) at 1 mm is printed 236).
        let at_least = |v: u128| move |n: u128, d: u128| v * d >= n;
        assert_eq!(nearest_r40(at_least(230)), 23_600);
        assert_eq!(nearest_r40(at_least(37)), 3_750);
        assert_eq!(nearest_r40(at_least(60)), 6_000);
        assert_eq!(whole_um(3_750), 38);
        assert_eq!(whole_um(23_600), 236);
    }

    #[test]
    fn halves_round_up() {
        assert_eq!(round_div(5, 2), 3);
        assert_eq!(round_div(-5, 2), -2);
        assert_eq!(round_div(4_917_468, 1_000), 4_917);
    }

    #[test]
    fn big_sums_differences_and_products() {
        let a = prod(&[(10, 30)]);
        let b = prod(&[(7, 20)]);
        assert_eq!(a.add(&b).sub(&b), a);
        assert_eq!(a.mul(&b), prod(&[(10, 30), (7, 20)]));
        assert!(a.add(&b) > a);
    }
}
