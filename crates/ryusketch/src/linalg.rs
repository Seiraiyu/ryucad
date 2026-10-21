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
    #[cfg(test)]
    pub fn mul_vec(&self, x: &[f64]) -> Vec<f64> {
        (0..self.rows)
            .map(|i| self.row(i).iter().zip(x).map(|(a, b)| a * b).sum())
            .collect()
    }

    /// `self * selfᵀ`. A sketch's Jacobian has a dozen entries a row at
    /// most, so this goes column by column over the nonzero entries only.
    /// Each entry still sums its products in column order, as the dense
    /// product would, and skipped products are zeros, so the bits are the
    /// same.
    pub fn gram(&self) -> Mat {
        let mut cols: Vec<Vec<(usize, f64)>> = vec![Vec::new(); self.cols];
        for i in 0..self.rows {
            for (k, &v) in self.row(i).iter().enumerate() {
                if v != 0.0 {
                    cols[k].push((i, v));
                }
            }
        }
        let mut g = Mat::zeros(self.rows, self.rows);
        for col in &cols {
            for (x, &(i, a)) in col.iter().enumerate() {
                for &(j, b) in &col[..=x] {
                    *g.at_mut(i, j) += a * b;
                }
            }
        }
        for i in 0..self.rows {
            for j in 0..i {
                *g.at_mut(j, i) = g.at(i, j);
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
    /// `R`, column by column, in each column's first entries; below them,
    /// what is left of `a`.
    cols: Vec<Vec<f64>>,
    /// Column `k` of `a P` is column `perm[k]` of `a`.
    perm: Vec<usize>,
    /// How many of `R`'s diagonal entries pass the tolerance.
    pub rank: usize,
}

/// Factors `a`. Its rank counts diagonal entries of `R` larger than `tol`
/// times the first, stopping at the first that isn't. It works on columns
/// held one after another in memory, the order Householder steps read
/// them in.
pub(crate) fn qr(a: &Mat, tol: f64) -> Qr {
    qr_against(a, tol, None)
}

/// [`qr`], with the rank judged against `reference` instead of `R`'s
/// first entry (the largest column's norm): a piece of a larger matrix is
/// judged by the whole one's scale.
pub(crate) fn qr_against(a: &Mat, tol: f64, reference: Option<f64>) -> Qr {
    let (m, n) = (a.rows, a.cols);
    let mut w: Vec<Vec<f64>> = (0..n)
        .map(|j| (0..m).map(|i| a.at(i, j)).collect())
        .collect();
    let mut perm: Vec<usize> = (0..n).collect();
    let norm2 = |c: &[f64]| -> f64 { c.iter().map(|x| x * x).sum() };
    for k in 0..m.min(n) {
        let (mut best, mut best_norm) = (k, norm2(&w[k][k..]));
        for j in k + 1..n {
            let c = norm2(&w[j][k..]);
            if c > best_norm {
                best = j;
                best_norm = c;
            }
        }
        if best != k {
            w.swap(k, best);
            perm.swap(k, best);
        }
        if best_norm == 0.0 {
            break;
        }
        let alpha = math::sqrt(best_norm);
        let s = if w[k][k] >= 0.0 { -alpha } else { alpha };
        let mut v: Vec<f64> = w[k][k..].to_vec();
        v[0] -= s;
        let vv = norm2(&v);
        if vv == 0.0 {
            continue;
        }
        for col in &mut w[k..] {
            let tail = &mut col[k..];
            let d: f64 = v.iter().zip(tail.iter()).map(|(a, b)| a * b).sum();
            let f = 2.0 * d / vv;
            for (x, vi) in tail.iter_mut().zip(&v) {
                *x -= f * vi;
            }
        }
        // Exactly what the reflection gives, without rounding noise.
        w[k][k] = s;
        for x in &mut w[k][k + 1..] {
            *x = 0.0;
        }
    }
    let first = match reference {
        Some(r) => r,
        None if m.min(n) > 0 => w[0][0].abs(),
        None => 0.0,
    };
    let rank = (0..m.min(n))
        .take_while(|&k| first > 0.0 && w[k][k].abs() > tol * first)
        .count();
    Qr {
        cols: w,
        perm,
        rank,
    }
}

impl Qr {
    /// A basis of `{x : a x = 0}` (as far as the rank says), each vector of
    /// unit length, in the order of the columns `P` put last.
    pub fn null_space(&self) -> Vec<Vec<f64>> {
        let n = self.cols.len();
        let r = self.rank;
        let at = |i: usize, j: usize| self.cols[j][i];
        let mut out = Vec::new();
        for j in r..n {
            // Solve R11 z = -R12 e_j, then z's tail is e_j.
            let mut z = vec![0.0; n];
            z[j] = 1.0;
            for i in (0..r).rev() {
                let mut s = -at(i, j);
                for k in i + 1..r {
                    s -= at(i, k) * z[k];
                }
                z[i] = s / at(i, i);
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
    fn ranks_against_a_reference() {
        // A small column is full rank alone, not beside a large one.
        let a = Mat {
            rows: 1,
            cols: 1,
            data: vec![1e-9],
        };
        assert_eq!(qr(&a, 1e-8).rank, 1);
        assert_eq!(qr_against(&a, 1e-8, Some(1.0)).rank, 0);
        assert_eq!(qr_against(&a, 1e-8, Some(1e-3)).rank, 1);
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
        assert_eq!(x.cols, y.cols);
        assert_eq!(x.perm, y.perm);
    }

    #[test]
    fn the_sparse_gram_is_the_dense_one() {
        let mut g = Lcg::new(4);
        let mut a = random(&mut g, 9, 14);
        for x in a.data.iter_mut() {
            if !g.next_u64().is_multiple_of(3) {
                *x = 0.0;
            }
        }
        let gram = a.gram();
        for i in 0..9 {
            for j in 0..9 {
                let mut d = 0.0;
                for k in 0..14 {
                    d += a.at(i, k) * a.at(j, k);
                }
                assert_eq!(gram.at(i, j).to_bits(), d.to_bits());
            }
        }
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
