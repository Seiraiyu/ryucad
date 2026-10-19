//! Polynomials in two variables, in tensor-product Bernstein form with
//! interval coefficients, on `[0, 1]²`.
//!
//! Like [`super::bernstein`], the polynomial lies between its least and
//! greatest coefficients, every operation rounds outward, and a row or
//! column of coefficients restricted to an edge is the edge's univariate
//! polynomial exactly.

use super::bernstein::Bernstein;
use crate::interval::Interval;

/// `Σ c[i][j] B_i^m(s) B_j^n(t)`, stored row-major: `c[i (n + 1) + j]`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bernstein2 {
    m: usize,
    n: usize,
    c: Vec<Interval>,
}

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

/// The factors `C(a, i) C(b, j) / C(a + b, i + j)` a product's terms carry.
fn ratios(a: usize, b: usize) -> Vec<Vec<Interval>> {
    (0..=a)
        .map(|i| {
            (0..=b)
                .map(|j| {
                    (binom(a, i) * binom(b, j))
                        .checked_div(binom(a + b, i + j))
                        .expect("binomials are positive")
                })
                .collect()
        })
        .collect()
}

/// De Casteljau on a strided sequence of coefficients: the left and right
/// pieces at `s`.
fn split_line(q: &mut [Interval], s: f64) -> (Vec<Interval>, Vec<Interval>) {
    let (si, ti) = (
        Interval::point(s),
        Interval::point(1.0) - Interval::point(s),
    );
    let n = q.len();
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
    (left, right)
}

impl Bernstein2 {
    /// Degrees `m` in `s` and `n` in `t`, coefficients row-major.
    pub(crate) fn new(m: usize, n: usize, c: Vec<Interval>) -> Self {
        assert_eq!(c.len(), (m + 1) * (n + 1), "a full grid of coefficients");
        Self { m, n, c }
    }

    fn at(&self, i: usize, j: usize) -> Interval {
        self.c[i * (self.n + 1) + j]
    }

    /// An interval holding every value on the square.
    pub(crate) fn range(&self) -> Interval {
        self.c[1..].iter().fold(self.c[0], |a, &b| a.hull(b))
    }

    /// The same polynomial written with degrees `[m, n]`, each at least
    /// its own.
    pub(crate) fn elevated(&self, m: usize, n: usize) -> Self {
        let rows: Vec<Bernstein> = (0..=self.m)
            .map(|i| Bernstein::new((0..=self.n).map(|j| self.at(i, j)).collect()).elevated(n))
            .collect();
        let mut c = vec![Interval::point(0.0); (m + 1) * (n + 1)];
        for j in 0..=n {
            let col =
                Bernstein::new(rows.iter().map(|r| r.coefficients()[j]).collect()).elevated(m);
            for (i, &x) in col.coefficients().iter().enumerate() {
                c[i * (n + 1) + j] = x;
            }
        }
        Self::new(m, n, c)
    }

    pub(crate) fn add(&self, other: &Self) -> Self {
        let (m, n) = (self.m.max(other.m), self.n.max(other.n));
        let (a, b) = (self.elevated(m, n), other.elevated(m, n));
        Self::new(m, n, a.c.iter().zip(&b.c).map(|(&x, &y)| x + y).collect())
    }

    pub(crate) fn sub(&self, other: &Self) -> Self {
        self.add(&other.scale(Interval::point(-1.0)))
    }

    pub(crate) fn scale(&self, k: Interval) -> Self {
        Self::new(self.m, self.n, self.c.iter().map(|&x| x * k).collect())
    }

    pub(crate) fn mul(&self, other: &Self) -> Self {
        let (m, n) = (self.m + other.m, self.n + other.n);
        let (rs, rt) = (ratios(self.m, other.m), ratios(self.n, other.n));
        let mut c = vec![Interval::point(0.0); (m + 1) * (n + 1)];
        for (i1, rs1) in rs.iter().enumerate() {
            for (j1, rt1) in rt.iter().enumerate() {
                let a = self.at(i1, j1);
                for (i2, &ks) in rs1.iter().enumerate() {
                    for (j2, &kt) in rt1.iter().enumerate() {
                        let idx = (i1 + i2) * (n + 1) + j1 + j2;
                        c[idx] = c[idx] + a * other.at(i2, j2) * (ks * kt);
                    }
                }
            }
        }
        Self::new(m, n, c)
    }

    /// The derivative with respect to `s`.
    pub(crate) fn deriv_s(&self) -> Self {
        if self.m == 0 {
            return Self::new(0, self.n, vec![Interval::point(0.0); self.n + 1]);
        }
        let k = Interval::point(self.m as f64);
        let mut c = Vec::with_capacity(self.m * (self.n + 1));
        for i in 0..self.m {
            for j in 0..=self.n {
                c.push((self.at(i + 1, j) - self.at(i, j)) * k);
            }
        }
        Self::new(self.m - 1, self.n, c)
    }

    /// The derivative with respect to `t`.
    pub(crate) fn deriv_t(&self) -> Self {
        if self.n == 0 {
            return Self::new(self.m, 0, vec![Interval::point(0.0); self.m + 1]);
        }
        let k = Interval::point(self.n as f64);
        let mut c = Vec::with_capacity((self.m + 1) * self.n);
        for i in 0..=self.m {
            for j in 0..self.n {
                c.push((self.at(i, j + 1) - self.at(i, j)) * k);
            }
        }
        Self::new(self.m, self.n - 1, c)
    }

    /// The pieces on `s` in `[0, s]` and `[s, 1]`, each reparameterized to
    /// `[0, 1]`.
    pub(crate) fn split_s(&self, s: f64) -> (Self, Self) {
        let mut l = vec![Interval::point(0.0); self.c.len()];
        let mut r = l.clone();
        for j in 0..=self.n {
            let mut col: Vec<Interval> = (0..=self.m).map(|i| self.at(i, j)).collect();
            let (a, b) = split_line(&mut col, s);
            for i in 0..=self.m {
                l[i * (self.n + 1) + j] = a[i];
                r[i * (self.n + 1) + j] = b[i];
            }
        }
        (Self::new(self.m, self.n, l), Self::new(self.m, self.n, r))
    }

    /// The pieces on `t` in `[0, t]` and `[t, 1]`.
    pub(crate) fn split_t(&self, t: f64) -> (Self, Self) {
        let mut l = Vec::with_capacity(self.c.len());
        let mut r = Vec::with_capacity(self.c.len());
        for i in 0..=self.m {
            let mut row: Vec<Interval> = (0..=self.n).map(|j| self.at(i, j)).collect();
            let (a, b) = split_line(&mut row, t);
            l.extend(a);
            r.extend(b);
        }
        (Self::new(self.m, self.n, l), Self::new(self.m, self.n, r))
    }

    /// The univariate polynomial in `t` along the line `s = s0`.
    pub(crate) fn at_s(&self, s0: f64) -> Bernstein {
        let row = |i: usize| -> Vec<Interval> { (0..=self.n).map(|j| self.at(i, j)).collect() };
        if s0 <= 0.0 {
            return Bernstein::new(row(0));
        }
        if s0 >= 1.0 {
            return Bernstein::new(row(self.m));
        }
        let c = (0..=self.n)
            .map(|j| Bernstein::new((0..=self.m).map(|i| self.at(i, j)).collect()).eval(s0))
            .collect();
        Bernstein::new(c)
    }

    /// The univariate polynomial in `s` along the line `t = t0`.
    pub(crate) fn at_t(&self, t0: f64) -> Bernstein {
        let col = |j: usize| -> Vec<Interval> { (0..=self.m).map(|i| self.at(i, j)).collect() };
        if t0 <= 0.0 {
            return Bernstein::new(col(0));
        }
        if t0 >= 1.0 {
            return Bernstein::new(col(self.n));
        }
        let c = (0..=self.m)
            .map(|i| Bernstein::new((0..=self.n).map(|j| self.at(i, j)).collect()).eval(t0))
            .collect();
        Bernstein::new(c)
    }

    /// The value at `(s, t)`.
    pub(crate) fn eval(&self, s: f64, t: f64) -> Interval {
        self.at_s(s).eval(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s - a` and `t - b` as bivariate polynomials.
    fn lin_s(a: f64) -> Bernstein2 {
        Bernstein2::new(1, 0, vec![Interval::point(-a), Interval::point(1.0 - a)])
    }

    fn lin_t(b: f64) -> Bernstein2 {
        Bernstein2::new(0, 1, vec![Interval::point(-b), Interval::point(1.0 - b)])
    }

    /// `(s - 0.3)² + (t - 0.6)² - 0.04`: a circle of radius 0.2.
    fn circle() -> Bernstein2 {
        let (x, y) = (lin_s(0.3), lin_t(0.6));
        x.mul(&x)
            .add(&y.mul(&y))
            .sub(&Bernstein2::new(0, 0, vec![Interval::point(0.04)]))
    }

    fn exact(s: f64, t: f64) -> f64 {
        (s - 0.3) * (s - 0.3) + (t - 0.6) * (t - 0.6) - 0.04
    }

    #[test]
    fn values_and_derivatives() {
        let p = circle();
        assert_eq!((p.m, p.n), (2, 2));
        for (s, t) in [(0.0, 0.0), (0.3, 0.6), (0.9, 0.1), (1.0, 1.0)] {
            assert!(p.eval(s, t).contains(exact(s, t)), "{s} {t}");
            assert!(p.deriv_s().eval(s, t).contains(2.0 * (s - 0.3)));
            assert!(p.deriv_t().eval(s, t).contains(2.0 * (t - 0.6)));
        }
        let (l, r) = p.split_s(0.4);
        assert!(l.eval(0.5, 0.5).contains(exact(0.2, 0.5)));
        assert!(r.eval(0.5, 0.5).contains(exact(0.7, 0.5)));
        let (d, u) = p.split_t(0.25);
        assert!(d.eval(0.5, 0.5).contains(exact(0.5, 0.125)));
        assert!(u.eval(0.5, 0.5).contains(exact(0.5, 0.625)));
        let e = p.elevated(3, 4);
        assert!(e.eval(0.7, 0.2).contains(exact(0.7, 0.2)));
    }

    #[test]
    fn edges_are_univariate() {
        let p = circle();
        // Along s = 0.3 the circle crosses at t = 0.4 and 0.8.
        let line = p.at_s(0.3);
        let roots: Vec<f64> = line
            .roots(1e-12)
            .iter()
            .map(|r| (r.range()[0] + r.range()[1]) / 2.0)
            .collect();
        assert_eq!(roots.len(), 2);
        assert!((roots[0] - 0.4).abs() < 1e-9 && (roots[1] - 0.8).abs() < 1e-9);
        assert!(p.at_t(0.0).eval(0.5).contains(exact(0.5, 0.0)));
        assert!(p.at_s(1.0).eval(0.5).contains(exact(1.0, 0.5)));
        // Far from the circle the range excludes zero.
        let (corner, _) = p.split_s(0.05);
        let (corner, _) = corner.split_t(0.1);
        assert!(corner.range().lo() > 0.0);
    }
}
