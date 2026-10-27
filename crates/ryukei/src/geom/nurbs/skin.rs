//! Skinning: curves made compatible exactly, and the surface through
//! them, what a loft is.

use super::refine::{Hom, insert};
use super::ruled::elevated;
use super::{KnotVector, NurbsCurve, NurbsSurface};
use crate::geom::{Point3, Vector3};
use crate::{Error, math};

/// One curve's homogeneous control points.
pub(crate) type Net = Vec<Hom<Vector3>>;

/// Clamped curves in one form, exactly: each raised to the highest degree
/// in Bézier form (every interior break of multiplicity the degree), its
/// parameter mapped affinely onto `[0, 1]`, each Bézier piece's weights
/// scaled by a Möbius reparameterization (`wᵢ ρⁱ`) so that the piece's
/// end weights are 1, and given every other curve's breaks. Each is the
/// same curve as before, so a surface through the control points of all
/// of them, column by column, has each as an iso curve; and every curve's
/// end points have weight 1, so the columns there interpolate to the same
/// curve whichever surface they bound.
///
/// Returns the shared knot vector and each curve's control points. Fails
/// with `Degenerate` for no curves.
pub(crate) fn compatible(curves: &[NurbsCurve]) -> Result<(KnotVector, Vec<Net>), Error> {
    let q = curves
        .iter()
        .map(NurbsCurve::degree)
        .max()
        .ok_or(Error::Degenerate)?;
    let mut forms: Vec<(Vec<f64>, Vec<Hom<Vector3>>)> = Vec::with_capacity(curves.len());
    for c in curves {
        let (flat, mut hom) = elevated(c, q);
        let (a, b) = (flat[0], flat[flat.len() - 1]);
        let flat: Vec<f64> = flat
            .iter()
            .map(|&t| {
                if t == a {
                    0.0
                } else if t == b {
                    1.0
                } else {
                    (t - a) / (b - a)
                }
            })
            .collect();
        // Bézier form: piece j's control points are j q ..= (j + 1) q,
        // neighbours sharing their joint; each is scaled from the weights
        // as they were.
        let before: Vec<f64> = hom.iter().map(|h| h.w).collect();
        for piece in 0..(hom.len() - 1) / q {
            let w = &before[piece * q..=(piece + 1) * q];
            let rho = math::pow(w[0] / w[q], 1.0 / q as f64);
            let mut scale = 1.0 / w[0];
            for (i, h) in hom[piece * q..=(piece + 1) * q].iter_mut().enumerate() {
                h.w = w[i] * scale;
                scale *= rho;
            }
            // The ends are exactly 1, whatever the rounding.
            hom[piece * q].w = 1.0;
            hom[(piece + 1) * q].w = 1.0;
        }
        forms.push((flat, hom));
    }
    let breaks = |flat: &[f64]| -> Vec<f64> {
        let mut out: Vec<f64> = flat[q + 1..flat.len() - q - 1].to_vec();
        out.dedup();
        out
    };
    let mut all: Vec<f64> = forms.iter().flat_map(|(f, _)| breaks(f)).collect();
    all.sort_by(f64::total_cmp);
    all.dedup();
    for (flat, hom) in &mut forms {
        let own = breaks(flat);
        for &t in &all {
            if !own.contains(&t) {
                (*flat, *hom) = insert(flat, q, hom, t, q);
            }
        }
    }
    let flat = forms[0].0.clone();
    debug_assert!(forms.iter().all(|(f, _)| *f == flat));
    let knots = KnotVector::clamped_flat(q, &flat);
    Ok((knots, forms.into_iter().map(|(_, h)| h).collect()))
}

/// Solves `m x = rhs` (rows of four right-hand sides) by Gaussian
/// elimination with partial pivoting; `None` where `m` is singular.
fn solve(mut m: Vec<Vec<f64>>, mut rhs: Vec<[f64; 4]>) -> Option<Vec<[f64; 4]>> {
    let n = m.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[pivot][col] == 0.0 {
            return None;
        }
        m.swap(col, pivot);
        rhs.swap(col, pivot);
        for row in col + 1..n {
            let f = m[row][col] / m[col][col];
            if f == 0.0 {
                continue;
            }
            let (upper, lower) = m.split_at_mut(row);
            for (x, y) in lower[0][col..].iter_mut().zip(&upper[col][col..]) {
                *x -= f * y;
            }
            let pivot_rhs = rhs[col];
            for (x, y) in rhs[row].iter_mut().zip(pivot_rhs) {
                *x -= f * y;
            }
        }
    }
    let mut x = vec![[0.0; 4]; n];
    for row in (0..n).rev() {
        let mut v = rhs[row];
        for k in row + 1..n {
            for (c, value) in v.iter_mut().enumerate() {
                *value -= m[row][k] * x[k][c];
            }
        }
        for value in &mut v {
            *value /= m[row][row];
        }
        x[row] = v;
    }
    Some(x)
}

/// What [`skin`] couldn't make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkinFault {
    /// A weight of the interpolated net at or below zero, at this column
    /// of the sections' control points.
    Weights { column: usize },
    /// The interpolation's system is singular.
    Singular,
}

/// The surface through compatible sections (see [`compatible`]): `u`
/// along them on `knots`, `v` across them, each section the iso curve at
/// its parameter `v[k]` (increasing, from 0 to below 1).
///
/// Each column of homogeneous control points `(w P, w)` is interpolated
/// in `v` by a spline of degree `min(3, conditions - 1)` through the
/// sections: clamped over `[0, 1]` with `v` ending at 1, its interior
/// knots averaged from the parameters (each end's repeated once more for
/// a derivative given there: The NURBS Book's (9.8) and (9.21)), or
/// periodic with period 1 and its knots at the parameters when `closed`.
/// `start` and `end` give a clamped skin's derivative by `v` at its first
/// and last section, the same for every column, in model units: the
/// homogeneous derivative `(w D, 0)`. The weights are interpolated with
/// the points, so they may fall to zero or below between sections.
pub(crate) fn skin(
    knots: &KnotVector,
    nets: &[Net],
    v: &[f64],
    closed: bool,
    (start, end): (Option<Vector3>, Option<Vector3>),
) -> Result<Result<NurbsSurface, SkinFault>, Error> {
    let m = nets.len();
    let ders = usize::from(start.is_some()) + usize::from(end.is_some());
    let count = m + ders;
    let q = (count - 1).min(3);
    let kv = if closed {
        let mut at: Vec<f64> = v.to_vec();
        at.push(1.0);
        KnotVector::periodic(q, &at, &vec![1; at.len()])?
    } else {
        // The parameters, each end's repeated once more per derivative.
        let mut ext = Vec::with_capacity(count);
        ext.push(v[0]);
        if start.is_some() {
            ext.push(v[0]);
        }
        ext.extend_from_slice(&v[1..m - 1]);
        if end.is_some() {
            ext.push(v[m - 1]);
        }
        ext.push(v[m - 1]);
        let mut knots = vec![0.0];
        let mut mults = vec![q + 1];
        for j in 1..count - q {
            let avg = ext[j..j + q].iter().sum::<f64>() / q as f64;
            knots.push(avg);
            mults.push(1);
        }
        knots.push(1.0);
        mults.push(q + 1);
        KnotVector::clamped(q, &knots, &mults)?
    };
    let n = kv.control_count();
    // The rows: a value at each section, then the derivatives.
    let row = |t: f64, k: usize| -> Vec<f64> {
        let b = kv.basis(t);
        let mut out = vec![0.0; n];
        for (r, &x) in b.values(k).iter().enumerate() {
            let i = (b.span() + r) as isize - q as isize;
            let i = if closed {
                i.rem_euclid(n as isize) as usize
            } else {
                i as usize
            };
            out[i] += x;
        }
        out
    };
    let mut rows: Vec<Vec<f64>> = v.iter().map(|&t| row(t, 0)).collect();
    if start.is_some() {
        rows.push(row(v[0], 1));
    }
    if end.is_some() {
        rows.push(row(v[m - 1], 1));
    }
    let columns = nets[0].len();
    let mut net: Vec<Vec<[f64; 4]>> = Vec::with_capacity(columns);
    for j in 0..columns {
        let hom = |h: &Hom<Vector3>| {
            let p = h.p * h.w;
            [p.x, p.y, p.z, h.w]
        };
        let mut rhs: Vec<[f64; 4]> = nets.iter().map(|net| hom(&net[j])).collect();
        let der = |d: Vector3, w: f64| [d.x * w, d.y * w, d.z * w, 0.0];
        if let Some(d) = start {
            rhs.push(der(d, nets[0][j].w));
        }
        if let Some(d) = end {
            rhs.push(der(d, nets[m - 1][j].w));
        }
        let Some(x) = solve(rows.clone(), rhs) else {
            return Ok(Err(SkinFault::Singular));
        };
        if x.iter().any(|c| c[3].is_nan() || c[3] <= 0.0) {
            return Ok(Err(SkinFault::Weights { column: j }));
        }
        net.push(x);
    }
    let mut points = Vec::with_capacity(columns * n);
    let mut weights = Vec::with_capacity(columns * n);
    for col in &net {
        for c in col {
            points.push(Point3::new(c[0] / c[3], c[1] / c[3], c[2] / c[3]));
            weights.push(c[3]);
        }
    }
    Ok(Ok(NurbsSurface::new(
        [knots.clone(), kv],
        points,
        Some(weights),
    )?))
}

#[cfg(test)]
mod tests;
