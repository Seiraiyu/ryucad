//! Coons patches as exact NURBS.
//!
//! Four sides in tensor order: `v0` along `u` at `v = 0`, `u1` along `v`
//! at `u = 1`, `v1` along `u` at `v = 1`, `u0` along `v` at `u = 0`, each
//! pair made compatible ([`compatible_mapped`]) over `[0, 1]`, so the
//! corners have weight 1. A B-spline basis reproduces every polynomial of
//! its degree or less: `v = Σ ηⱼ Nⱼ(v)` with `ηⱼ` the Greville abscissae,
//! and a polynomial's coefficients are its blossom at the knots. So the
//! bilinearly blended Coons patch, `(1 - v) v0(u) + v v1(u) + (1 - u)
//! u0(v) + u u1(v)` less the corners' bilinear patch, taken in
//! homogeneous coordinates, is a tensor-product NURBS on the sides' own
//! knots whose control net is that formula on the sides' control points:
//! its sides are the given curves exactly, rational ones included.
//!
//! A correction `E` with no weight, vanishing on all four sides, changes
//! only the cross-boundary derivatives: `E = Σₐ gᵥₐ(u) hₐ(v) + Σ_b
//! gᵤ_b(v) h_b(u) - Σ c_ab h_b(u) hₐ(v)`, with Hermite's cubics `h₀(t) =
//! t (1 - t)²` (derivative 1 at 0, 0 at 1) and `h₁(t) = -t² (1 - t)`, and
//! `c_ab` each corner's twist. `S_v` on `v = 0` gains `gᵥ₀ / w` exactly
//! where the twists agree.

use crate::error::{Error, FillError};
use crate::geom::nurbs::Hom;
use crate::geom::nurbs::skin::{CompatMap, compatible_mapped};
use crate::geom::{KnotVector, NurbsCurve, NurbsSurface, Point3, Vector3};

/// A side's place: `[v0, u1, v1, u0]`.
pub(crate) const V0: usize = 0;
pub(crate) const U1: usize = 1;
pub(crate) const V1: usize = 2;
pub(crate) const U0: usize = 3;

/// Four sides in tensor order, each pair compatible over `[0, 1]`.
#[derive(Clone, Debug)]
pub(crate) struct Quad {
    /// The sides as curves over `[0, 1]`: `[v0, u1, v1, u0]`.
    pub(crate) sides: [NurbsCurve; 4],
    /// How each side's parameter maps onto its curve's as given.
    pub(crate) maps: [CompatMap; 4],
}

impl Quad {
    /// The sides in tensor order, `v0` and `v1` running with `u`, `u0`
    /// and `u1` with `v`, their corners already equal, made compatible
    /// at degree `floor` at least.
    pub(crate) fn new(sides: [NurbsCurve; 4], floor: usize) -> Result<Self, Error> {
        let [v0, u1, v1, u0] = sides;
        let (ku, nu, mu) = compatible_mapped(&[v0, v1], floor)?;
        let (kv, nv, mv) = compatible_mapped(&[u0, u1], floor)?;
        let curve = |k: &KnotVector, net: &[Hom<Vector3>]| {
            NurbsCurve::new(
                k.clone(),
                net.iter().map(|h| Point3::ORIGIN + h.p).collect(),
                Some(net.iter().map(|h| h.w).collect()),
            )
        };
        let [mv0, mv1] = <[CompatMap; 2]>::try_from(mu).expect("two");
        let [mu0, mu1] = <[CompatMap; 2]>::try_from(mv).expect("two");
        Ok(Self {
            sides: [
                curve(&ku, &nu[0])?,
                curve(&kv, &nv[1])?,
                curve(&ku, &nu[1])?,
                curve(&kv, &nv[0])?,
            ],
            maps: [mv0, mu1, mv1, mu0],
        })
    }

    /// The knots along `u` (`d = 0`) or `v`.
    pub(crate) fn knots(&self, d: usize) -> &KnotVector {
        if d == 0 {
            self.sides[V0].knots()
        } else {
            self.sides[U0].knots()
        }
    }

    /// Both sides along `u` (`d = 0`) or `v` refined by the middle of each
    /// of their knot spans.
    pub(crate) fn refine(&mut self, d: usize) -> Result<(), Error> {
        let ks = self.knots(d).knots().to_vec();
        let mids: Vec<f64> = ks.windows(2).map(|w| 0.5 * (w[0] + w[1])).collect();
        let pair = if d == 0 { [V0, V1] } else { [U0, U1] };
        for k in pair {
            self.sides[k] = self.sides[k].refine(&mids)?;
        }
        Ok(())
    }

    /// The bilinearly blended Coons patch's homogeneous net, row-major
    /// (`(i, j)` at `i nv + j`): `(w P, w)` per control point.
    pub(crate) fn bilinear(&self) -> Net {
        let [v0, u1, v1, u0] = &self.sides;
        let hom = |c: &NurbsCurve| -> Vec<(Vector3, f64)> {
            c.control_points()
                .zip(c.weights())
                .map(|(p, &w)| (p.to_vector() * w, w))
                .collect()
        };
        let (a, b, c, d) = (hom(v0), hom(v1), hom(u0), hom(u1));
        let (xi, eta) = (greville(self.knots(0)), greville(self.knots(1)));
        let (nu, nv) = (xi.len(), eta.len());
        let (p00, p10, p01, p11) = (a[0], a[nu - 1], b[0], b[nu - 1]);
        let mut x = Vec::with_capacity(nu * nv);
        let mut w = Vec::with_capacity(nu * nv);
        for i in 0..nu {
            for j in 0..nv {
                let (s, t) = (xi[i], eta[j]);
                let mix = |f: &dyn Fn(&(Vector3, f64)) -> Vector3| {
                    f(&a[i]) * (1.0 - t) + f(&b[i]) * t + f(&c[j]) * (1.0 - s) + f(&d[j]) * s
                        - (f(&p00) * ((1.0 - s) * (1.0 - t))
                            + f(&p10) * (s * (1.0 - t))
                            + f(&p01) * ((1.0 - s) * t)
                            + f(&p11) * (s * t))
                };
                // The sides' own points on the net's border, exactly.
                let (px, pw) = if j == 0 {
                    a[i]
                } else if j == nv - 1 {
                    b[i]
                } else if i == 0 {
                    c[j]
                } else if i == nu - 1 {
                    d[j]
                } else {
                    let px = mix(&|h| h.0);
                    let pw = mix(&|h| Vector3::new(h.1, 0.0, 0.0)).x;
                    (px, pw)
                };
                x.push(px);
                w.push(pw);
            }
        }
        Net { x, w }
    }

    /// The patch: the bilinear net plus `correction` (no weight), as a
    /// NURBS. `Degenerate` where a weight isn't positive.
    pub(crate) fn surface(&self, net: &Net, correction: Option<&[Vector3]>) -> Result<NurbsSurface, Error> {
        let mut points = Vec::with_capacity(net.x.len());
        for (k, (&x, &w)) in net.x.iter().zip(&net.w).enumerate() {
            if !(w > 0.0) {
                return Err(FillError::Degenerate { side: None }.into());
            }
            let e = correction.map_or(Vector3::ZERO, |c| c[k]);
            points.push(Point3::ORIGIN + (x + e) / w);
        }
        NurbsSurface::new(
            [self.knots(0).clone(), self.knots(1).clone()],
            points,
            Some(net.w.clone()),
        )
    }
}

/// A homogeneous control net: `x` is `w P`.
#[derive(Clone, Debug)]
pub(crate) struct Net {
    pub(crate) x: Vec<Vector3>,
    pub(crate) w: Vec<f64>,
}

/// The Greville abscissae: each control point's mean of the `p` knots
/// after it.
pub(crate) fn greville(k: &KnotVector) -> Vec<f64> {
    let (flat, p) = (k.flat(), k.degree());
    (0..k.control_count())
        .map(|i| flat[i + 1..=i + p].iter().sum::<f64>() / p as f64)
        .collect()
}

/// The coefficients of the cubic `c[0] + c[1] t + c[2] t² + c[3] t³` in
/// the basis of `k` (degree at least 3): its blossom at each control
/// point's `p` knots, `Σ c_m e_m(t₁..t_p) / binom(p, m)`.
pub(crate) fn cubic_coefficients(k: &KnotVector, c: [f64; 4]) -> Vec<f64> {
    let (flat, p) = (k.flat(), k.degree());
    debug_assert!(p >= 3);
    (0..k.control_count())
        .map(|i| {
            // Elementary symmetric polynomials of the knots, up to e₃.
            let mut e = [1.0, 0.0, 0.0, 0.0];
            for &t in &flat[i + 1..=i + p] {
                for m in (1..4).rev() {
                    e[m] += e[m - 1] * t;
                }
            }
            let mut binom = 1.0;
            let mut out = 0.0;
            for m in 0..4 {
                if m > 0 {
                    binom *= (p + 1 - m) as f64 / m as f64;
                }
                out += c[m] * e[m] / binom;
            }
            out
        })
        .collect()
}

/// `h₀(t) = t - 2t² + t³` and `h₁(t) = -t² + t³`.
pub(crate) const HERMITE: [[f64; 4]; 2] = [[0.0, 1.0, -2.0, 1.0], [0.0, 0.0, -1.0, 1.0]];

/// Hermite's blends local to the basis's end spans: `ĥ₀` is zero but for
/// its second coefficient, set so its derivative at the start is 1;
/// `ĥ₁` likewise at the end. Each vanishes at both ends, its derivative
/// at the other end is 0 (the basis has at least four functions), and it
/// is zero past its end's first `p` spans, so it narrows as the knots
/// are refined.
pub(crate) fn local_hermite(k: &KnotVector) -> [Vec<f64>; 2] {
    let (flat, p, n) = (k.flat(), k.degree(), k.control_count());
    debug_assert!(n >= 4);
    let mut h0 = vec![0.0; n];
    h0[1] = (flat[p + 1] - flat[1]) / p as f64;
    let mut h1 = vec![0.0; n];
    h1[n - 2] = -(flat[n + p - 1] - flat[n - 1]) / p as f64;
    [h0, h1]
}

/// The coefficients in the basis of `k` that interpolate `values` at its
/// Greville abscissae. The collocation matrix is banded and totally
/// positive, so elimination without pivoting is stable (de Boor).
pub(crate) fn interpolate(k: &KnotVector, values: &[Vector3]) -> Vec<Vector3> {
    let xi = greville(k);
    let n = xi.len();
    let p = k.degree();
    // Row r: the basis functions at ξ_r, at columns span - p ..= span.
    let mut rows: Vec<(usize, Vec<f64>)> = xi
        .iter()
        .map(|&t| {
            let b = k.basis(t);
            (b.span() - p, b.values(0).to_vec())
        })
        .collect();
    // Dense band: column offsets relative to each row's first.
    let mut rhs: Vec<Vector3> = values.to_vec();
    let get = |rows: &[(usize, Vec<f64>)], r: usize, c: usize| -> f64 {
        let (s, v) = &rows[r];
        if c >= *s && c < s + v.len() { v[c - s] } else { 0.0 }
    };
    for col in 0..n {
        let piv = get(&rows, col, col);
        for r in col + 1..(col + p + 1).min(n) {
            let f = get(&rows, r, col) / piv;
            if f == 0.0 {
                continue;
            }
            // Row r -= f row col, over the columns row col holds.
            let (s, v) = rows[col].clone();
            let (rs, rv) = &mut rows[r];
            let lo = (*rs).min(s);
            let hi = (*rs + rv.len()).max(s + v.len());
            let mut out = vec![0.0; hi - lo];
            for (c, x) in out.iter_mut().enumerate() {
                let cc = lo + c;
                let mine = if cc >= *rs && cc < *rs + rv.len() { rv[cc - *rs] } else { 0.0 };
                let theirs = if cc >= s && cc < s + v.len() { v[cc - s] } else { 0.0 };
                *x = mine - f * theirs;
            }
            *rs = lo;
            *rv = out;
            rhs[r] = rhs[r] - rhs[col] * f;
        }
    }
    let mut x = vec![Vector3::ZERO; n];
    for r in (0..n).rev() {
        let mut v = rhs[r];
        for c in r + 1..(r + p + 1).min(n) {
            v = v - x[c] * get(&rows, r, c);
        }
        x[r] = v / get(&rows, r, r);
    }
    x
}

/// A spline's derivative at the start (`end` false) or end of its clamped
/// domain, from its coefficients.
pub(crate) fn end_derivative(k: &KnotVector, c: &[Vector3], end: bool) -> Vector3 {
    let (flat, p, n) = (k.flat(), k.degree(), c.len());
    if end {
        (c[n - 1] - c[n - 2]) * (p as f64 / (flat[n + p - 1] - flat[n - 1]))
    } else {
        (c[1] - c[0]) * (p as f64 / (flat[p + 1] - flat[1]))
    }
}

/// The correction net (row-major, `nu × nv`) from each side's fitted
/// coefficients `g` (`None` for a side with no target; `v0` and `v1`'s in
/// the `u` basis, `u0` and `u1`'s in the `v` basis).
///
/// At a corner where only one side is targeted, the twist is zero (the
/// other side has no term), and the targeted side's cross derivative is
/// exact. Where both are, their twists `x` (from `gᵥₐ`) and `y` (from
/// `gᵤ_b`) generally differ; the twist is their mean `c`, which leaves
/// each side a residual: `(y - c) h_b(u)` on `v = a`, `(x - c) hₐ(v)` on
/// `u = b`, over the whole side. Two more terms, `-(y - c)(h_b(u) -
/// ĥ_b(u)) hₐ(v)` and `-(x - c)(hₐ(v) - ĥₐ(v)) h_b(u)`, each with no
/// derivative across the other side, trade those for the local blends
/// [`local_hermite`]: the residual is confined to the corner's end spans
/// and shrinks as they are refined.
pub(crate) fn correction(quad: &Quad, g: &[Option<Vec<Vector3>>; 4]) -> Vec<Vector3> {
    let (ku, kv) = (quad.knots(0), quad.knots(1));
    let hu = [cubic_coefficients(ku, HERMITE[0]), cubic_coefficients(ku, HERMITE[1])];
    let hv = [cubic_coefficients(kv, HERMITE[0]), cubic_coefficients(kv, HERMITE[1])];
    let (lu, lv) = (local_hermite(ku), local_hermite(kv));
    let (nu, nv) = (ku.control_count(), kv.control_count());
    // gv[a] for the side at v = a (a = 0: v0, 1: v1); gu[b] at u = b.
    let gv = [&g[V0], &g[V1]];
    let gu = [&g[U0], &g[U1]];
    // Per corner (a, b): the twist, and the residuals traded for local
    // blends on the v side (y - c) and the u side (x - c).
    let mut twist = [[Vector3::ZERO; 2]; 2];
    let mut on_v = [[Vector3::ZERO; 2]; 2];
    let mut on_u = [[Vector3::ZERO; 2]; 2];
    for a in 0..2 {
        for b in 0..2 {
            let x = gv[a].as_ref().map(|x| end_derivative(ku, x, b == 1));
            let y = gu[b].as_ref().map(|y| end_derivative(kv, y, a == 1));
            match (x, y) {
                (Some(x), Some(y)) => {
                    let c = (x + y) * 0.5;
                    twist[a][b] = c;
                    on_v[a][b] = y - c;
                    on_u[a][b] = x - c;
                }
                // The untargeted side has no term, so a zero twist keeps
                // the targeted side's cross derivative exact.
                _ => {}
            }
        }
    }
    let mut e = vec![Vector3::ZERO; nu * nv];
    for i in 0..nu {
        for j in 0..nv {
            let mut s = Vector3::ZERO;
            for a in 0..2 {
                if let Some(x) = gv[a] {
                    s += x[i] * hv[a][j];
                }
            }
            for b in 0..2 {
                if let Some(y) = gu[b] {
                    s += y[j] * hu[b][i];
                }
            }
            for a in 0..2 {
                for b in 0..2 {
                    s -= twist[a][b] * (hu[b][i] * hv[a][j]);
                    s -= on_v[a][b] * ((hu[b][i] - lu[b][i]) * hv[a][j]);
                    s -= on_u[a][b] * ((hv[a][j] - lv[a][j]) * hu[b][i]);
                }
            }
            e[i * nv + j] = s;
        }
    }
    // The border is the sides' own, exactly.
    for i in 0..nu {
        e[i * nv] = Vector3::ZERO;
        e[i * nv + nv - 1] = Vector3::ZERO;
    }
    for j in 0..nv {
        e[j] = Vector3::ZERO;
        e[(nu - 1) * nv + j] = Vector3::ZERO;
    }
    e
}
