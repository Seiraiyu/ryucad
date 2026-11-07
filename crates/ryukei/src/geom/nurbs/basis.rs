//! B-spline basis functions and their first two derivatives (The NURBS
//! Book, algorithm A2.3), in fixed-size stack buffers.

use super::{KnotVector, MAX_DEGREE};

const W: usize = MAX_DEGREE + 1;

/// The tables [`KnotVector::basis`] takes for degrees below it.
const SMALL: usize = 8;

/// The `degree + 1` basis functions that are nonzero on one knot span, and
/// their first two derivatives, at one parameter.
#[derive(Clone, Copy, Debug)]
pub struct Basis {
    span: usize,
    degree: usize,
    ders: [[f64; W]; 3],
}

impl Basis {
    /// The flat index of the knot span. The functions are those of control
    /// points `span - degree ..= span` in the flat numbering.
    pub fn span(&self) -> usize {
        self.span
    }

    /// The `k`-th derivatives (`k <= 2`) of the nonzero functions, in
    /// order.
    pub fn values(&self, k: usize) -> &[f64] {
        &self.ders[k][..=self.degree]
    }
}

impl KnotVector {
    /// The nonzero basis functions at `t` and their derivatives. A periodic
    /// parameter is wrapped into the domain first; a clamped one outside
    /// the domain extrapolates the end span.
    pub fn basis(&self, t: f64) -> Basis {
        // The same arithmetic in either size: degrees up to 7 (most of
        // what files hold) take tables of 8, not clearing the largest
        // degree's 26 by 26 on every call (#308: about a tenth of mass's
        // time on KiCad's Smolhaj model).
        self.basis_to(t, 2)
    }

    /// [`Self::basis`] up to the `order`-th derivatives (at most 2), the
    /// same bits for those: each order's sums run alone, after the lower
    /// ones'. Higher orders read as zero. For callers that want the values
    /// alone, or with first derivatives (#308).
    pub(crate) fn basis_to(&self, t: f64, order: usize) -> Basis {
        if self.degree() < SMALL {
            self.basis_in::<SMALL>(t, order)
        } else {
            self.basis_in::<W>(t, order)
        }
    }

    /// [`Self::basis_to`] with tables of `N`, more than the degree.
    fn basis_in<const N: usize>(&self, t: f64, order: usize) -> Basis {
        let t = self.wrap(t);
        let span = self.span(t);
        let (u, p) = (self.flat(), self.degree());
        // ndu: basis functions in the upper triangle, knot differences in
        // the lower.
        let mut ndu = [[0.0; N]; N];
        let (mut left, mut right) = ([0.0; N], [0.0; N]);
        ndu[0][0] = 1.0;
        for j in 1..=p {
            left[j] = t - u[span + 1 - j];
            right[j] = u[span + j] - t;
            let mut saved = 0.0;
            for r in 0..j {
                ndu[j][r] = right[r + 1] + left[j - r];
                let temp = ndu[r][j - 1] / ndu[j][r];
                ndu[r][j] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            ndu[j][j] = saved;
        }
        let mut ders = [[0.0; W]; 3];
        for j in 0..=p {
            ders[0][j] = ndu[j][p];
        }
        let n = p.min(order.min(2));
        let mut a = [[0.0; N]; 2];
        for r in 0..=p {
            let (mut s1, mut s2) = (0, 1);
            a[0][0] = 1.0;
            for k in 1..=n {
                let mut d = 0.0;
                let rk = r as isize - k as isize;
                let pk = p - k;
                if r >= k {
                    a[s2][0] = a[s1][0] / ndu[pk + 1][rk as usize];
                    d = a[s2][0] * ndu[rk as usize][pk];
                }
                let j1 = if rk >= -1 { 1 } else { (-rk) as usize };
                let j2 = if r as isize - 1 <= pk as isize {
                    k - 1
                } else {
                    p - r
                };
                for j in j1..=j2 {
                    let col = (rk + j as isize) as usize;
                    a[s2][j] = (a[s1][j] - a[s1][j - 1]) / ndu[pk + 1][col];
                    d += a[s2][j] * ndu[col][pk];
                }
                if r <= pk {
                    a[s2][k] = -a[s1][k - 1] / ndu[pk + 1][r];
                    d += a[s2][k] * ndu[r][pk];
                }
                ders[k][r] = d;
                (s1, s2) = (s2, s1);
            }
        }
        let mut factor = p as f64;
        for (k, row) in ders.iter_mut().enumerate().take(n + 1).skip(1) {
            for d in &mut row[..=p] {
                *d *= factor;
            }
            factor *= (p - k) as f64;
        }
        Basis {
            span,
            degree: p,
            ders,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::KnotVector;

    /// The NURBS Book, examples 2.3 and 2.4: U = {0,0,0,1,2,3,4,4,5,5,5}.
    fn book() -> KnotVector {
        KnotVector::clamped(2, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[3, 1, 1, 1, 2, 3]).unwrap()
    }

    /// The small tables give the largest's bits (#308): degrees 1 to 7,
    /// uneven knots, parameters across the domain and on knots.
    #[test]
    fn small_tables_give_the_same_bits() {
        let mut x: u64 = 0x308;
        let mut unit = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f64 / (1u64 << 53) as f64
        };
        for p in 1..super::SMALL {
            let mut ks = vec![0.0];
            for _ in 0..6 {
                let last = *ks.last().unwrap();
                ks.push(last + 0.01 + unit());
            }
            let mut mults = vec![1; ks.len()];
            mults[0] = p + 1;
            *mults.last_mut().unwrap() = p + 1;
            mults[3] = p.min(2);
            let k = KnotVector::clamped(p, &ks, &mults).unwrap();
            let [a, b] = k.domain();
            let mut ts: Vec<f64> = (0..400).map(|_| a + (b - a) * unit()).collect();
            ts.extend(ks.iter().copied());
            for t in ts {
                let (s, w) = (
                    k.basis_in::<{ super::SMALL }>(t, 2),
                    k.basis_in::<{ super::W }>(t, 2),
                );
                assert_eq!(s.span(), w.span());
                for order in 0..2 {
                    let o = k.basis_to(t, order);
                    for d in 0..=order {
                        let bits = |v: &[f64]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
                        assert_eq!(
                            bits(o.values(d)),
                            bits(w.values(d)),
                            "degree {p} at {t} to {order}"
                        );
                    }
                }
                for d in 0..3 {
                    let bits = |b: &super::Basis| {
                        b.values(d).iter().map(|v| v.to_bits()).collect::<Vec<_>>()
                    };
                    assert_eq!(bits(&s), bits(&w), "degree {p} at {t}, order {d}");
                }
            }
        }
    }

    #[test]
    fn the_book_example() {
        let b = book().basis(2.5);
        assert_eq!(b.span(), 4);
        assert_eq!(b.values(0), [0.125, 0.75, 0.125]);
        assert_eq!(b.values(1), [-0.5, 0.0, 0.5]);
        assert_eq!(b.values(2), [1.0, -2.0, 1.0]);
    }

    #[test]
    fn derivatives_above_the_degree_vanish() {
        let k = KnotVector::clamped(1, &[0.0, 1.0, 3.0], &[2, 1, 2]).unwrap();
        let b = k.basis(2.0);
        assert_eq!(b.span(), 2);
        assert_eq!(b.values(0), [0.5, 0.5]);
        assert_eq!(b.values(1), [-0.5, 0.5]);
        assert_eq!(b.values(2), [0.0, 0.0]);
    }

    #[test]
    fn partition_of_unity_up_to_the_highest_degree() {
        for p in [3, 7, crate::geom::MAX_DEGREE] {
            let knots: Vec<f64> = (0..=4).map(f64::from).collect();
            let mut mults = vec![1; 5];
            mults[0] = p + 1;
            mults[4] = p + 1;
            let k = KnotVector::clamped(p, &knots, &mults).unwrap();
            for i in 0..=40 {
                let b = k.basis(0.1 * f64::from(i));
                let sums: Vec<f64> = (0..3).map(|d| b.values(d).iter().sum()).collect();
                assert!((sums[0] - 1.0).abs() <= 1e-13, "p={p} {sums:?}");
                assert!(
                    sums[1].abs() <= 1e-10 && sums[2].abs() <= 1e-8,
                    "p={p} {sums:?}"
                );
            }
        }
    }

    #[test]
    fn periodic_parameters_wrap() {
        let k = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0], &[1, 1, 1, 1]).unwrap();
        let (a, b) = (k.basis(0.25), k.basis(3.25));
        assert_eq!(a.span(), b.span());
        assert_eq!(a.values(0), b.values(0));
        // A uniform quadratic: (1-s)²/2, (1 + 2s - 2s²)/2, s²/2.
        assert_eq!(a.values(0), [0.28125, 0.6875, 0.03125]);
    }
}
