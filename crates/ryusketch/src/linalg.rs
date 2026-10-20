//! Dense linear algebra for the solver: Cholesky for its steps, and
//! Householder QR with column pivoting for ranks and null spaces. Plain
//! loops in a fixed order, so the same matrix gives the same bits
//! everywhere.

// Index loops read like the textbook algorithms they are.
#![allow(clippy::needless_range_loop)]

use ryukei::math;

/// A dense matrix, row by row.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Mat {
        Mat {
            rows,
            cols,
            data: vec![0.0; rows * cols],
        }
    }

    pub fn at(&self, i: usize, j: usize) -> f64 {
        self.data[i * self.cols + j]
    }

    pub fn at_mut(&mut self, i: usize, j: usize) -> &mut f64 {
        &mut self.data[i * self.cols + j]
    }

    pub fn row(&self, i: usize) -> &[f64] {
        &self.data[i * self.cols..(i + 1) * self.cols]
    }

    pub fn transpose(&self) -> Mat {
        let mut t = Mat::zeros(self.cols, self.rows);
        for i in 0..self.rows {
            for j in 0..self.cols {
                *t.at_mut(j, i) = self.at(i, j);
            }
        }
        t
    }

    /// `self * x`.
    pub fn mul_vec(&self, x: &[f64]) -> Vec<f64> {
        (0..self.rows)
            .map(|i| self.row(i).iter().zip(x).map(|(a, b)| a * b).sum())
            .collect()
    }

    /// `self * selfᵀ`.
    pub fn gram(&self) -> Mat {
        let mut g = Mat::zeros(self.rows, self.rows);
        for i in 0..self.rows {
            for j in 0..=i {
                let d: f64 = self
                    .row(i)
                    .iter()
                    .zip(self.row(j))
                    .map(|(a, b)| a * b)
                    .sum();
                *g.at_mut(i, j) = d;
                *g.at_mut(j, i) = d;
            }
        }
        g
    }

    /// The rows in `keep`, in that order.
    pub fn rows_of(&self, keep: &[usize]) -> Mat {
        let mut m = Mat::zeros(keep.len(), self.cols);
        for (k, &i) in keep.iter().enumerate() {
            m.data[k * self.cols..(k + 1) * self.cols].copy_from_slice(self.row(i));
        }
        m
    }
}

/// Solves `a x = b` for symmetric positive definite `a`; `None` if a pivot
/// isn't positive.
pub(crate) fn cholesky_solve(a: &Mat, b: &[f64]) -> Option<Vec<f64>> {
    let n = a.rows;
    let mut l = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..=i {
            let mut s = a.at(i, j);
            for k in 0..j {
                s -= l.at(i, k) * l.at(j, k);
            }
            if i == j {
                if s.is_nan() || s <= 0.0 {
                    return None;
                }
                *l.at_mut(i, i) = math::sqrt(s);
            } else {
                *l.at_mut(i, j) = s / l.at(j, j);
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l.at(i, k) * y[k];
        }
        y[i] = s / l.at(i, i);
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= l.at(k, i) * x[k];
        }
        x[i] = s / l.at(i, i);
    }
    Some(x)
}

/// `a P = Q R`, with `P` choosing at each step the remaining column of
/// largest norm (the lowest index among equals).
#[derive(Clone, Debug)]
pub(crate) struct Qr {
    /// `R` in its upper triangle; below it, what is left of `a`.
    r: Mat,
    /// Column `k` of `a P` is column `perm[k]` of `a`.
    perm: Vec<usize>,
    /// How many of `R`'s diagonal entries pass the tolerance.
    pub rank: usize,
}

/// Factors `a`. Its rank counts diagonal entries of `R` larger than `tol`
/// times the first, stopping at the first that isn't.
pub(crate) fn qr(a: &Mat, tol: f64) -> Qr {
    let (m, n) = (a.rows, a.cols);
    let mut w = a.clone();
    let mut perm: Vec<usize> = (0..n).collect();
    let col_norm2 = |w: &Mat, j: usize, from: usize| -> f64 {
        (from..m).map(|i| w.at(i, j) * w.at(i, j)).sum()
    };
    for k in 0..m.min(n) {
        let (mut best, mut best_norm) = (k, col_norm2(&w, k, k));
        for j in k + 1..n {
            let c = col_norm2(&w, j, k);
            if c > best_norm {
                best = j;
                best_norm = c;
            }
        }
        if best != k {
            for i in 0..m {
                w.data.swap(i * n + k, i * n + best);
            }
            perm.swap(k, best);
        }
        if best_norm == 0.0 {
            break;
        }
        let alpha = math::sqrt(best_norm);
        let s = if w.at(k, k) >= 0.0 { -alpha } else { alpha };
        let mut v: Vec<f64> = (k..m).map(|i| w.at(i, k)).collect();
        v[0] -= s;
        let vv: f64 = v.iter().map(|x| x * x).sum();
        if vv == 0.0 {
            continue;
        }
        for j in k..n {
            let d: f64 = (k..m).map(|i| v[i - k] * w.at(i, j)).sum();
            let f = 2.0 * d / vv;
            for i in k..m {
                *w.at_mut(i, j) -= f * v[i - k];
            }
        }
        // Exactly what the reflection gives, without rounding noise.
        *w.at_mut(k, k) = s;
        for i in k + 1..m {
            *w.at_mut(i, k) = 0.0;
        }
    }
    let first = if m.min(n) > 0 { w.at(0, 0).abs() } else { 0.0 };
    let rank = (0..m.min(n))
        .take_while(|&k| first > 0.0 && w.at(k, k).abs() > tol * first)
        .count();
    Qr { r: w, perm, rank }
}

impl Qr {
    /// A basis of `{x : a x = 0}` (as far as the rank says), each vector of
    /// unit length, in the order of the columns `P` put last.
    pub fn null_space(&self) -> Vec<Vec<f64>> {
        let n = self.r.cols;
        let r = self.rank;
        let mut out = Vec::new();
        for j in r..n {
            // Solve R11 z = -R12 e_j, then z's tail is e_j.
            let mut z = vec![0.0; n];
            z[j] = 1.0;
            for i in (0..r).rev() {
                let mut s = -self.r.at(i, j);
                for k in i + 1..r {
                    s -= self.r.at(i, k) * z[k];
                }
                z[i] = s / self.r.at(i, i);
            }
            let mut x = vec![0.0; n];
            for (k, &p) in self.perm.iter().enumerate() {
                x[p] = z[k];
            }
            let norm = math::sqrt(x.iter().map(|v| v * v).sum());
            out.push(x.into_iter().map(|v| v / norm).collect());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei_testkit::Lcg;

    fn random(g: &mut Lcg, rows: usize, cols: usize) -> Mat {
        Mat {
            rows,
            cols,
            data: (0..rows * cols).map(|_| g.next_f64(-1.0, 1.0)).collect(),
        }
    }

    fn product(a: &Mat, b: &Mat) -> Mat {
        let mut c = Mat::zeros(a.rows, b.cols);
        for i in 0..a.rows {
            for j in 0..b.cols {
                *c.at_mut(i, j) = (0..a.cols).map(|k| a.at(i, k) * b.at(k, j)).sum();
            }
        }
        c
    }

    #[test]
    fn cholesky_solves() {
        let mut g = Lcg::new(1);
        for n in 1..8 {
            let a = random(&mut g, n, n + 2).gram();
            let x: Vec<f64> = (0..n).map(|_| g.next_f64(-1.0, 1.0)).collect();
            let b = a.mul_vec(&x);
            let got = cholesky_solve(&a, &b).unwrap();
            for (u, v) in got.iter().zip(&x) {
                assert!((u - v).abs() < 1e-9, "{u} vs {v}");
            }
        }
        // Singular: refused.
        let a = random(&mut g, 3, 2).gram();
        assert!(cholesky_solve(&a, &[1.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn rank_and_null_space() {
        let mut g = Lcg::new(2);
        for (m, n, r) in [
            (5, 5, 5),
            (6, 4, 4),
            (4, 7, 4),
            (6, 6, 3),
            (3, 8, 1),
            (7, 5, 2),
        ] {
            let a = product(&random(&mut g, m, r), &random(&mut g, r, n));
            let f = qr(&a, 1e-10);
            assert_eq!(f.rank, r, "{m}x{n} of rank {r}");
            let ns = f.null_space();
            assert_eq!(ns.len(), n - r);
            for v in &ns {
                let len: f64 = v.iter().map(|x| x * x).sum();
                assert!((len - 1.0).abs() < 1e-12);
                for y in a.mul_vec(v) {
                    assert!(y.abs() < 1e-9, "{y}");
                }
            }
        }
    }

    #[test]
    fn zero_and_empty_matrices() {
        assert_eq!(qr(&Mat::zeros(3, 2), 1e-10).rank, 0);
        assert_eq!(qr(&Mat::zeros(3, 2), 1e-10).null_space().len(), 2);
        assert_eq!(qr(&Mat::zeros(0, 2), 1e-10).null_space().len(), 2);
        assert_eq!(qr(&Mat::zeros(2, 0), 1e-10).rank, 0);
    }

    #[test]
    fn the_same_matrix_gives_the_same_bits() {
        let mut g = Lcg::new(3);
        let a = random(&mut g, 6, 9);
        let (x, y) = (qr(&a, 1e-10), qr(&a.clone(), 1e-10));
        assert_eq!(x.r, y.r);
        assert_eq!(x.perm, y.perm);
    }

    #[test]
    fn helpers() {
        let a = Mat {
            rows: 2,
            cols: 3,
            data: vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        };
        assert_eq!(a.transpose().row(2), [3.0, 6.0]);
        assert_eq!(a.mul_vec(&[1.0, 0.0, -1.0]), [-2.0, -2.0]);
        assert_eq!(a.gram().data, [14.0, 32.0, 32.0, 77.0]);
        assert_eq!(a.rows_of(&[1]).data, [4.0, 5.0, 6.0]);
    }
}
