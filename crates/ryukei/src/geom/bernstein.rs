//! Polynomials in Bernstein form with interval coefficients, and their
//! certified real roots.
//!
//! A polynomial `p(s) = Σ c_i C(n, i) s^i (1 - s)^(n - i)` on `[0, 1]`
//! lies between its least and greatest coefficients (the convex hull
//! property), so interval coefficients bound it rigorously. Every
//! operation here rounds outward: the coefficients of the exact result lie
//! inside the computed intervals. [`Bernstein::roots`] isolates roots by
//! subdivision: a piece whose coefficients share one certified sign holds
//! none, and a piece whose derivative's coefficients do holds at most one,
//! decided by the signs at its ends. What can't be decided down to a
//! minimum width is reported as unresolved, never guessed.

use crate::interval::Interval;
use crate::predicates::Sign;

/// A polynomial on `[0, 1]` by its Bernstein coefficients.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bernstein {
    c: Vec<Interval>,
}

/// A root of a polynomial on `[0, 1]`, by the parameters bracketing it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Root {
    /// Exactly one root, a simple one, in `[lo, hi]`: the polynomial is
    /// monotone there with opposite certified signs at the ends. `rising`
    /// when it goes from negative to positive.
    Simple { lo: f64, hi: f64, rising: bool },
    /// A stretch where the sign can't be certified at the minimum width:
    /// any number of roots, a touch, or the polynomial vanishing. The
    /// caller decides by an explicit tolerance rule.
    Unresolved { lo: f64, hi: f64 },
}

impl Root {
    /// The bracket `[lo, hi]`.
    pub(crate) fn range(&self) -> [f64; 2] {
        match *self {
            Self::Simple { lo, hi, .. } | Self::Unresolved { lo, hi } => [lo, hi],
        }
    }
}

/// Where a piece is split while isolating roots: near the middle, but off
/// the dyadic fractions where symmetric problems put their roots.
const SPLIT: f64 = 0.499_023_437_5;

/// `C(n, k)` as an interval.
fn binom(n: usize, k: usize) -> Interval {
    let k = k.min(n - k);
    let mut r = Interval::point(1.0);
    for i in 0..k {
        r = (r * Interval::point((n - i) as f64))
            .checked_div(Interval::point((i + 1) as f64))
            .expect("a positive divisor");
    }
    r
}

/// The certified strict sign shared by every coefficient, if any.
fn common_sign(c: &[Interval]) -> Option<Sign> {
    let s = c[0].sign()?;
    (s != Sign::Zero && c.iter().all(|x| x.sign() == Some(s))).then_some(s)
}

/// A certified strict sign.
fn strict(x: Interval) -> Option<Sign> {
    x.sign().filter(|&s| s != Sign::Zero)
}

impl Bernstein {
    /// The polynomial with these coefficients. There must be at least one.
    pub(crate) fn new(c: Vec<Interval>) -> Self {
        assert!(!c.is_empty(), "a polynomial has a coefficient");
        Self { c }
    }

    /// The polynomial with these exact coefficients.
    pub(crate) fn from_f64(c: &[f64]) -> Self {
        Self::new(c.iter().map(|&x| Interval::point(x)).collect())
    }

    pub(crate) fn degree(&self) -> usize {
        self.c.len() - 1
    }

    pub(crate) fn coefficients(&self) -> &[Interval] {
        &self.c
    }

    /// An interval holding every value on `[0, 1]`.
    pub(crate) fn range(&self) -> Interval {
        self.c[1..].iter().fold(self.c[0], |a, &b| a.hull(b))
    }

    /// The same polynomial written with degree `n >= self.degree()`.
    pub(crate) fn elevated(&self, n: usize) -> Self {
        let mut c = self.c.clone();
        for m in self.degree()..n {
            let k = (m + 1) as f64;
            let mut next = Vec::with_capacity(m + 2);
            next.push(c[0]);
            for i in 1..=m {
                let a = Interval::point(i as f64)
                    .checked_div(Interval::point(k))
                    .expect("k > 0");
                next.push(a * c[i - 1] + (Interval::point(1.0) - a) * c[i]);
            }
            next.push(c[m]);
            c = next;
        }
        Self { c }
    }

    pub(crate) fn add(&self, other: &Self) -> Self {
        let n = self.degree().max(other.degree());
        let (a, b) = (self.elevated(n), other.elevated(n));
        Self::new(a.c.iter().zip(&b.c).map(|(&x, &y)| x + y).collect())
    }

    pub(crate) fn sub(&self, other: &Self) -> Self {
        self.add(&other.scale(Interval::point(-1.0)))
    }

    pub(crate) fn scale(&self, k: Interval) -> Self {
        Self::new(self.c.iter().map(|&x| x * k).collect())
    }

    pub(crate) fn mul(&self, other: &Self) -> Self {
        let (m, n) = (self.degree(), other.degree());
        let mut c = vec![Interval::point(0.0); m + n + 1];
        let total: Vec<Interval> = (0..=m + n).map(|k| binom(m + n, k)).collect();
        for (i, &a) in self.c.iter().enumerate() {
            let bi = binom(m, i);
            for (j, &b) in other.c.iter().enumerate() {
                let k = (bi * binom(n, j))
                    .checked_div(total[i + j])
                    .expect("binomials are positive");
                c[i + j] = c[i + j] + a * b * k;
            }
        }
        Self::new(c)
    }

    /// The derivative with respect to `s`.
    pub(crate) fn deriv(&self) -> Self {
        let n = self.degree();
        if n == 0 {
            return Self::from_f64(&[0.0]);
        }
        let k = Interval::point(n as f64);
        Self::new(self.c.windows(2).map(|w| (w[1] - w[0]) * k).collect())
    }

    /// The value at `s`, by de Casteljau's algorithm.
    pub(crate) fn eval(&self, s: f64) -> Interval {
        let (s, t) = (
            Interval::point(s),
            Interval::point(1.0) - Interval::point(s),
        );
        let mut q = self.c.clone();
        for k in 1..q.len() {
            for i in 0..q.len() - k {
                q[i] = q[i] * t + q[i + 1] * s;
            }
        }
        q[0]
    }

    /// The pieces on `[0, s]` and `[s, 1]`, each reparameterized to
    /// `[0, 1]`.
    pub(crate) fn split(&self, s: f64) -> (Self, Self) {
        let (si, ti) = (
            Interval::point(s),
            Interval::point(1.0) - Interval::point(s),
        );
        let n = self.c.len();
        let mut q = self.c.clone();
        let mut left = Vec::with_capacity(n);
        let mut right = Vec::with_capacity(n);
        left.push(q[0]);
        right.push(q[n - 1]);
        for k in 1..n {
            for i in 0..n - k {
                q[i] = q[i] * ti + q[i + 1] * si;
            }
            left.push(q[0]);
            right.push(q[n - 1 - k]);
        }
        right.reverse();
        (Self::new(left), Self::new(right))
    }

    /// The piece on `[a, b]`, reparameterized to `[0, 1]`.
    pub(crate) fn part(&self, a: f64, b: f64) -> Self {
        let right = if a > 0.0 {
            self.split(a).1
        } else {
            self.clone()
        };
        if b >= 1.0 {
            return right;
        }
        let s = (b - a) / (1.0 - a);
        right.split(s).0
    }

    /// The roots in `[0, 1]`, in increasing order. Unresolved stretches
    /// are at most about `min_width` wide (more where they merge), and
    /// simple roots are bracketed to `min_width` or until rounding can't
    /// sign the middle of the bracket.
    pub(crate) fn roots(&self, min_width: f64) -> Vec<Root> {
        let mut out = Vec::new();
        self.isolate(0.0, 1.0, min_width, &mut out);
        let mut merged: Vec<Root> = Vec::with_capacity(out.len());
        for r in out {
            if let (Some(Root::Unresolved { hi, .. }), Root::Unresolved { lo: l2, hi: h2 }) =
                (merged.last_mut(), r)
                && l2 <= *hi
            {
                *hi = hi.max(h2);
                continue;
            }
            merged.push(r);
        }
        merged
            .into_iter()
            .map(|r| match r {
                Root::Unresolved { lo, hi } => {
                    self.simple_after_all(lo, hi, min_width).unwrap_or(r)
                }
                simple => simple,
            })
            .collect()
    }

    /// Isolates the roots of this polynomial's piece on `[a, b]` (whose
    /// coefficients `self` holds after reparameterizing).
    fn isolate(&self, a: f64, b: f64, w: f64, out: &mut Vec<Root>) {
        if common_sign(&self.c).is_some() {
            return;
        }
        // Every piece of it would have coefficients holding zero too:
        // nothing here can be signed.
        if self.c.iter().all(|x| x.contains(0.0)) {
            out.push(Root::Unresolved { lo: a, hi: b });
            return;
        }
        if self.degree() > 0
            && let Some(slope) = common_sign(&self.deriv().c)
        {
            self.monotone(a, b, slope, w, out);
            return;
        }
        if b - a <= w || self.degree() == 0 {
            out.push(Root::Unresolved { lo: a, hi: b });
            return;
        }
        let m = a + (b - a) * SPLIT;
        let (left, right) = self.split(SPLIT);
        left.isolate(a, m, w, out);
        right.isolate(m, b, w, out);
    }

    /// The root of a monotone piece on `[a, b]`, if it has one.
    fn monotone(&self, a: f64, b: f64, slope: Sign, w: f64, out: &mut Vec<Root>) {
        let rising = slope == Sign::Positive;
        let n = self.degree();
        match (strict(self.c[0]), strict(self.c[n])) {
            (Some(s0), Some(s1)) if s0 == s1 => {}
            (Some(_), Some(_)) => out.push(self.refine(a, b, rising, w)),
            (None, None) => out.push(Root::Unresolved { lo: a, hi: b }),
            // One end can't be signed: the root, if any, is near it. Narrow
            // the stretch from the other end while its sign is certified.
            (None, Some(s1)) => {
                let (mut lo, mut hi) = (0.0, 1.0);
                while (hi - lo) * (b - a) > w {
                    let m = (lo + hi) / 2.0;
                    match strict(self.eval(m)) {
                        Some(s) if s == s1 => hi = m,
                        Some(_) => {
                            let r = self.part(m, hi).refine(
                                a + (b - a) * m,
                                a + (b - a) * hi,
                                rising,
                                w,
                            );
                            out.push(r);
                            return;
                        }
                        None => lo = m,
                    }
                }
                out.push(Root::Unresolved {
                    lo: a,
                    hi: a + (b - a) * hi,
                });
            }
            (Some(s0), None) => {
                let (mut lo, mut hi) = (0.0, 1.0);
                while (hi - lo) * (b - a) > w {
                    let m = (lo + hi) / 2.0;
                    match strict(self.eval(m)) {
                        Some(s) if s == s0 => lo = m,
                        Some(_) => {
                            let r = self.part(lo, m).refine(
                                a + (b - a) * lo,
                                a + (b - a) * m,
                                rising,
                                w,
                            );
                            out.push(r);
                            return;
                        }
                        None => hi = m,
                    }
                }
                out.push(Root::Unresolved {
                    lo: a + (b - a) * lo,
                    hi: b,
                });
            }
        }
    }

    /// Brackets the one root of a monotone piece on `[a, b]` whose ends
    /// have opposite certified signs, by bisection.
    fn refine(&self, a: f64, b: f64, rising: bool, w: f64) -> Root {
        let below = if rising {
            Sign::Negative
        } else {
            Sign::Positive
        };
        let (mut lo, mut hi) = (0.0, 1.0);
        while (hi - lo) * (b - a) > w {
            let m = (lo + hi) / 2.0;
            match strict(self.eval(m)) {
                Some(s) if s == below => lo = m,
                Some(_) => hi = m,
                None => break,
            }
        }
        Root::Simple {
            lo: a + (b - a) * lo,
            hi: a + (b - a) * hi,
            rising,
        }
    }

    /// A stretch left unresolved that, widened a little, is a monotone
    /// piece with opposite certified signs at its ends: a simple root
    /// that sat on a split point.
    fn simple_after_all(&self, lo: f64, hi: f64, w: f64) -> Option<Root> {
        let pad = (hi - lo).max(w);
        let (a, b) = ((lo - pad).max(0.0), (hi + pad).min(1.0));
        let p = self.part(a, b);
        let slope = common_sign(&p.deriv().c)?;
        let n = p.degree();
        let (s0, s1) = (strict(p.c[0])?, strict(p.c[n])?);
        (s0 != s1).then(|| p.refine(a, b, slope == Sign::Positive, w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s - r` in Bernstein form: coefficients `-r` and `1 - r`.
    fn linear(r: f64) -> Bernstein {
        Bernstein::from_f64(&[-r, 1.0 - r])
    }

    fn product(roots: &[f64]) -> Bernstein {
        roots
            .iter()
            .fold(Bernstein::from_f64(&[1.0]), |p, &r| p.mul(&linear(r)))
    }

    /// Every root simple, holding the expected one in a bracket narrower
    /// than `width`.
    fn brackets(p: &Bernstein, expected: &[f64], width: f64) {
        let found = p.roots(1e-14);
        assert_eq!(found.len(), expected.len(), "{found:?}");
        for (r, &x) in found.iter().zip(expected) {
            let Root::Simple { lo, hi, .. } = *r else {
                panic!("unresolved {r:?}")
            };
            assert!(lo <= x && x <= hi && hi - lo <= width, "{x} in {lo}..{hi}");
        }
    }

    #[test]
    fn arithmetic_keeps_the_values() {
        let p = product(&[0.2, 0.7]);
        for s in [0.0, 0.25, 0.5, 0.9, 1.0] {
            let exact = (s - 0.2) * (s - 0.7);
            assert!(p.eval(s).contains(exact), "{s}");
            let e = p.elevated(5);
            assert!(e.eval(s).contains(exact));
            let d = p.deriv();
            assert!(d.eval(s).contains(2.0 * s - 0.9));
            let q = p.add(&linear(0.5)).sub(&linear(0.5));
            assert!(q.eval(s).lo() <= exact && q.eval(s).hi() >= exact);
        }
        let (l, r) = p.split(0.3);
        assert!(l.eval(0.5).contains((0.15 - 0.2) * (0.15 - 0.7)));
        assert!(r.eval(0.5).contains((0.65 - 0.2) * (0.65 - 0.7)));
        let m = p.part(0.2, 0.6);
        assert!(m.eval(0.5).contains((0.4 - 0.2) * (0.4 - 0.7)));
        assert!(p.range().contains(-0.25 + 0.01) && p.range().contains(0.14));
    }

    #[test]
    fn simple_roots_are_bracketed() {
        brackets(&product(&[0.3, 0.7]), &[0.3, 0.7], 1e-13);
        let r = product(&[0.3, 0.7]).roots(1e-14);
        assert!(matches!(r[0], Root::Simple { rising: false, .. }));
        assert!(matches!(r[1], Root::Simple { rising: true, .. }));
        // Five close roots, and roots on the dyadic points a plain bisection
        // would split at.
        let roots = [0.1, 0.1001, 0.25, 0.5, 0.75];
        brackets(&product(&roots), &roots, 1e-10);
        assert!(product(&[1.5, -0.2]).roots(1e-14).is_empty());
    }

    #[test]
    fn what_rounding_cannot_decide_is_unresolved() {
        // A double root: the polynomial touches zero without crossing.
        let r = product(&[0.4, 0.4]).roots(1e-12);
        assert_eq!(r.len(), 1);
        let [lo, hi] = r[0].range();
        assert!(matches!(r[0], Root::Unresolved { .. }));
        assert!(lo <= 0.4 && 0.4 <= hi && hi - lo < 1e-5, "{lo} {hi}");
        // A root at an end.
        let r = product(&[0.0, 0.6]).roots(1e-14);
        assert_eq!(r.len(), 2);
        assert!(r[0].range()[0] == 0.0 && r[0].range()[1] < 1e-10);
        assert!(matches!(r[1], Root::Simple { .. }));
        // The zero polynomial vanishes everywhere.
        let z = Bernstein::from_f64(&[0.0, 0.0, 0.0]);
        assert_eq!(z.roots(1e-3), [Root::Unresolved { lo: 0.0, hi: 1.0 }]);
    }

    #[test]
    fn high_degree_products() {
        let roots: Vec<f64> = (1..=12).map(|i| f64::from(i) / 13.0).collect();
        brackets(&product(&roots), &roots, 1e-8);
    }
}
