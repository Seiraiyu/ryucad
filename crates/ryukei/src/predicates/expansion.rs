//! Exact floating-point expansion arithmetic (Shewchuk 1997).
//!
//! An expansion is a sum of f64 components, nonoverlapping and ordered by
//! increasing magnitude, that represents a real number exactly. Every
//! operation here is error-free, assuming no overflow or underflow. Only
//! plain `+ - *` are used, so results are identical on every platform.

/// `a + b == x + y` exactly, with `x = fl(a + b)`.
#[inline]
pub(crate) fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bv = x - a;
    let av = x - bv;
    (x, (a - av) + (b - bv))
}

/// `a - b == x + y` exactly, with `x = fl(a - b)`.
#[inline]
pub(crate) fn two_diff(a: f64, b: f64) -> (f64, f64) {
    let x = a - b;
    let bv = a - x;
    let av = x + bv;
    (x, (a - av) + (bv - b))
}

/// Dekker's split: `a == hi + lo`, each with at most 26 significant bits.
#[inline]
fn split(a: f64) -> (f64, f64) {
    const SPLITTER: f64 = 134_217_729.0; // 2^27 + 1
    let c = SPLITTER * a;
    let hi = c - (c - a);
    (hi, a - hi)
}

/// `a * b == x + y` exactly, with `x = fl(a * b)`. Uses Dekker splitting
/// rather than FMA, so it needs no hardware or library support.
#[inline]
pub(crate) fn two_product(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let (ahi, alo) = split(a);
    let (bhi, blo) = split(b);
    let err1 = x - ahi * bhi;
    let err2 = err1 - alo * bhi;
    let err3 = err2 - ahi * blo;
    (x, alo * blo - err3)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Expansion(Vec<f64>);

impl Expansion {
    #[cfg(test)]
    fn from_f64(a: f64) -> Self {
        Self::from_components(&[a])
    }

    /// `a - b`, exactly.
    pub(crate) fn diff(a: f64, b: f64) -> Self {
        let (x, y) = two_diff(a, b);
        Self::from_components(&[y, x])
    }

    fn from_components(c: &[f64]) -> Self {
        Self(c.iter().copied().filter(|&x| x != 0.0).collect())
    }

    /// Adds one f64 (Shewchuk's GROW-EXPANSION with zero elimination).
    fn grow(&self, b: f64) -> Self {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut q = b;
        for &e in &self.0 {
            let (sum, err) = two_sum(q, e);
            if err != 0.0 {
                out.push(err);
            }
            q = sum;
        }
        if q != 0.0 {
            out.push(q);
        }
        Self(out)
    }

    pub(crate) fn add(&self, other: &Self) -> Self {
        other.0.iter().fold(self.clone(), |acc, &f| acc.grow(f))
    }

    pub(crate) fn neg(&self) -> Self {
        Self(self.0.iter().map(|x| -x).collect())
    }

    pub(crate) fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }

    /// Multiplies by one f64 (Shewchuk's SCALE-EXPANSION with zero
    /// elimination).
    fn scale(&self, b: f64) -> Self {
        let mut out = Vec::with_capacity(2 * self.0.len());
        let Some((&first, rest)) = self.0.split_first() else {
            return Self::default();
        };
        let (mut q, h) = two_product(first, b);
        if h != 0.0 {
            out.push(h);
        }
        for &e in rest {
            let (p1, p0) = two_product(e, b);
            let (sum, h) = two_sum(q, p0);
            if h != 0.0 {
                out.push(h);
            }
            let (nq, h) = two_sum(p1, sum);
            if h != 0.0 {
                out.push(h);
            }
            q = nq;
        }
        if q != 0.0 {
            out.push(q);
        }
        Self(out)
    }

    pub(crate) fn mul(&self, other: &Self) -> Self {
        other
            .0
            .iter()
            .fold(Self::default(), |acc, &f| acc.add(&self.scale(f)))
    }

    /// The sign of the exact value: that of its largest component.
    pub(crate) fn signum(&self) -> f64 {
        match self.0.last() {
            None => 0.0,
            Some(x) => x.signum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sum_is_exact() {
        let (x, y) = two_sum(1.0, 1e-20);
        assert_eq!((x, y), (1.0, 1e-20));
        let (x, y) = two_diff(1.0, 1e-20);
        assert_eq!((x, y), (1.0, -1e-20));
    }

    #[test]
    fn two_product_is_exact() {
        // (1 + 2^-30)^2 = 1 + 2^-29 + 2^-60; the last term is the error.
        let e = 1.0 / (1u64 << 30) as f64;
        let (x, y) = two_product(1.0 + e, 1.0 + e);
        assert_eq!(x, 1.0 + 2.0 * e);
        assert_eq!(y, e * e);
    }

    #[test]
    fn cancellation_keeps_the_tiny_part() {
        // (1 + 1e-20) - 1 is 1e-20 exactly, not 0.
        let a = Expansion::from_f64(1.0).add(&Expansion::from_f64(1e-20));
        let d = a.sub(&Expansion::from_f64(1.0));
        assert_eq!(d, Expansion::from_f64(1e-20));
        assert_eq!(d.signum(), 1.0);
    }

    #[test]
    fn product_and_sign() {
        let a = Expansion::diff(1.0, 1e-20); // 1 - 1e-20
        let sq = a.mul(&a); // 1 - 2e-20 + 1e-40
        let r = sq.sub(&Expansion::from_f64(1.0));
        assert_eq!(r.signum(), -1.0);
        assert_eq!(Expansion::default().signum(), 0.0);
        assert_eq!(Expansion::diff(3.0, 3.0), Expansion::default());
    }
}
