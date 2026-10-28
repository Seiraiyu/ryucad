//! Gauss–Legendre rules on `[0, 1]`, and the collapsed (Duffy) product
//! rule on a triangle built from them. Nodes come from Newton's method on
//! the Legendre recurrence with `math::cos` starting points, so they are
//! the same bits everywhere.

use crate::math;
use core::f64::consts::PI;
use std::sync::OnceLock;

/// The orders measurement uses: each step doubles the last.
pub(crate) const ORDERS: [usize; 3] = [4, 8, 16];

/// The `n`-point rule on `[0, 1]`: `(node, weight)` in increasing node
/// order; the weights sum to 1.
pub(crate) fn rule(n: usize) -> &'static [(f64, f64)] {
    static RULES: OnceLock<Vec<Vec<(f64, f64)>>> = OnceLock::new();
    let rules = RULES.get_or_init(|| ORDERS.iter().map(|&n| compute(n)).collect());
    let k = ORDERS
        .iter()
        .position(|&m| m == n)
        .expect("one of the measurement orders");
    &rules[k]
}

/// `P_n(x)` and `P_n'(x)` by the three-term recurrence.
fn legendre(n: usize, x: f64) -> (f64, f64) {
    let (mut p0, mut p1) = (1.0, x);
    for k in 2..=n {
        let k = k as f64;
        let p2 = ((2.0 * k - 1.0) * x * p1 - (k - 1.0) * p0) / k;
        p0 = p1;
        p1 = p2;
    }
    let dp = n as f64 * (x * p1 - p0) / (x * x - 1.0);
    (p1, dp)
}

fn compute(n: usize) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // The i-th root from the top, refined by Newton's method.
        let mut x = math::cos(PI * (i as f64 + 0.75) / (n as f64 + 0.5));
        for _ in 0..100 {
            let (p, dp) = legendre(n, x);
            let dx = p / dp;
            x -= dx;
            if dx.abs() <= 1e-16 {
                break;
            }
        }
        let (_, dp) = legendre(n, x);
        let w = 2.0 / ((1.0 - x * x) * dp * dp);
        out.push(((1.0 - x) / 2.0, w / 2.0));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// The `n × n` collapsed rule on the unit triangle `{(a, b): a, b ≥ 0,
/// a + b ≤ 1}`: `(a, b, weight)`, the weights summing to its area, 1/2.
pub(crate) fn triangle(n: usize) -> &'static [(f64, f64, f64)] {
    static RULES: OnceLock<Vec<Vec<(f64, f64, f64)>>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        ORDERS
            .iter()
            .map(|&n| {
                let r = rule(n);
                let mut out = Vec::with_capacity(n * n);
                for &(t, wt) in r {
                    for &(s, ws) in r {
                        out.push((s * (1.0 - t), t, ws * wt * (1.0 - t)));
                    }
                }
                out
            })
            .collect()
    });
    let k = ORDERS
        .iter()
        .position(|&m| m == n)
        .expect("one of the measurement orders");
    &rules[k]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pow(x: f64, k: u32) -> f64 {
        (0..k).fold(1.0, |p, _| p * x)
    }

    #[test]
    fn rules_integrate_polynomials_exactly() {
        for n in ORDERS {
            let r = rule(n);
            assert_eq!(r.len(), n);
            let sum: f64 = r.iter().map(|&(_, w)| w).sum();
            assert!((sum - 1.0).abs() < 1e-14, "{n}: {sum}");
            // x^k integrates to 1/(k+1) for k < 2n.
            for k in 0..2 * n {
                let got: f64 = r.iter().map(|&(x, w)| w * pow(x, k as u32)).sum();
                assert!((got - 1.0 / (k as f64 + 1.0)).abs() < 1e-13, "{n}, x^{k}");
            }
            assert!(r.windows(2).all(|p| p[0].0 < p[1].0));
        }
    }

    #[test]
    fn triangle_rules_integrate_monomials() {
        // ∫ a^i b^j over the unit triangle is i! j! / (i + j + 2)!.
        let fact = |k: u32| (1..=k).map(f64::from).product::<f64>();
        for n in ORDERS {
            let t = triangle(n);
            assert_eq!(t.len(), n * n);
            for i in 0..4u32 {
                for j in 0..4u32 {
                    let got: f64 = t.iter().map(|&(a, b, w)| w * pow(a, i) * pow(b, j)).sum();
                    let want = fact(i) * fact(j) / fact(i + j + 2);
                    assert!((got - want).abs() < 1e-14, "{n}: a^{i} b^{j}");
                }
            }
        }
    }

    #[test]
    fn rules_are_the_same_bits_each_time() {
        assert_eq!(rule(8), compute(8).as_slice());
    }
}
