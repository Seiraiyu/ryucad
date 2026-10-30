//! A NURBS surface's partial derivatives up to third order, which an
//! offset of it needs: its normal's second derivatives read the base's
//! third.

use super::NurbsSurface;
use super::bezier::BezierPatch;
use crate::geom::Vector3;

/// The orders `(i, j)` (`i` along `u`, `j` along `v`) of the derivatives
/// up to third order, in the order they are given: `S`, `Su`, `Sv`,
/// `Suu`, `Suv`, `Svv`, `Suuu`, `Suuv`, `Suvv`, `Svvv`.
pub(crate) const ORDERS: [(usize, usize); 10] = [
    (0, 0),
    (1, 0),
    (0, 1),
    (2, 0),
    (1, 1),
    (0, 2),
    (3, 0),
    (2, 1),
    (1, 2),
    (0, 3),
];

/// The index of order `(i, j)` in [`ORDERS`].
pub(crate) fn index(i: usize, j: usize) -> usize {
    ORDERS
        .iter()
        .position(|&o| o == (i, j))
        .expect("an order up to three")
}

pub(crate) fn binomial(n: usize, k: usize) -> f64 {
    match (n, k) {
        (_, 0) => 1.0,
        (n, k) if k == n => 1.0,
        (2, 1) => 2.0,
        (3, 1 | 2) => 3.0,
        _ => unreachable!("orders up to three"),
    }
}

/// The rational derivatives from the homogeneous numerator's `a` and the
/// denominator's `w` (each by [`ORDERS`]): the general Leibniz rule,
/// `S⁽ᵃᵇ⁾ = (A⁽ᵃᵇ⁾ - Σ C(a, i) C(b, j) W⁽ⁱʲ⁾ S⁽ᵃ⁻ⁱ ᵇ⁻ʲ⁾) / W` over
/// `(i, j) ≠ (0, 0)`.
pub(crate) fn quotient<V, W>(
    a: [V; 10],
    w: [W; 10],
    times: impl Fn(V, W, f64) -> V,
    div: impl Fn(V, W) -> V,
) -> [V; 10]
where
    V: Copy + core::ops::Sub<Output = V>,
    W: Copy,
{
    let mut a = a;
    a[0] = div(a[0], w[0]);
    quotient_from(a, w, times, div, |x, y| x - y)
}

/// [`quotient`] with the point `a[0]` already divided by the weight (an
/// enclosure may give it more tightly than the quotient would).
pub(crate) fn quotient_from<V, W>(
    a: [V; 10],
    w: [W; 10],
    times: impl Fn(V, W, f64) -> V,
    div: impl Fn(V, W) -> V,
    sub: impl Fn(V, V) -> V,
) -> [V; 10]
where
    V: Copy,
    W: Copy,
{
    let mut s: [Option<V>; 10] = [None; 10];
    s[0] = Some(a[0]);
    for (k, &(oa, ob)) in ORDERS.iter().enumerate().skip(1) {
        let mut num = a[k];
        for i in 0..=oa {
            for j in 0..=ob {
                if (i, j) == (0, 0) {
                    continue;
                }
                let lower = s[index(oa - i, ob - j)].expect("lower orders first");
                let c = binomial(oa, i) * binomial(ob, j);
                num = sub(num, times(lower, w[index(i, j)], c));
            }
        }
        s[k] = Some(div(num, w[0]));
    }
    s.map(|x| x.expect("every order"))
}

/// A polynomial in Bernstein form over `[0, 1]` at `x`.
fn casteljau(c: &[[f64; 4]], x: f64) -> [f64; 4] {
    let mut q = c.to_vec();
    for k in 1..q.len() {
        for i in 0..q.len() - k {
            q[i] = core::array::from_fn(|n| q[i][n] * (1.0 - x) + q[i + 1][n] * x);
        }
    }
    q[0]
}

impl NurbsSurface {
    /// The point and its partial derivatives up to third order at
    /// `(u, v)`, by [`ORDERS`], from the Bézier patch holding it: each
    /// derivative of the homogeneous numerator and denominator by de
    /// Casteljau's algorithm on its difference net, then the quotient
    /// rule. Periodic directions wrap; clamped ones extrapolate their end
    /// patches.
    pub(crate) fn jet3(&self, u: f64, v: f64) -> [Vector3; 10] {
        let (u, v) = (self.knots()[0].wrap(u), self.knots()[1].wrap(v));
        let patches = self.patches();
        let across = patches
            .iter()
            .take_while(|p| p.range[0] == patches[0].range[0])
            .count();
        let along = patches.len() / across;
        let a = (0..along)
            .position(|k| patches[k * across].range[0][1] > u)
            .unwrap_or(along - 1);
        let b = (0..across)
            .position(|k| patches[k].range[1][1] > v)
            .unwrap_or(across - 1);
        let patch = &patches[a * across + b];
        let (a, w) = patch_derivatives(patch, u, v);
        let a = a.map(|x| Vector3::new(x[0], x[1], x[2]));
        quotient(a, w, |s, w, c| s * (w * c), |s, w| s / w)
    }
}

/// The homogeneous numerator's and the denominator's derivatives by
/// [`ORDERS`] at `(u, v)` of one patch.
fn patch_derivatives(patch: &BezierPatch, u: f64, v: f64) -> ([[f64; 3]; 10], [f64; 10]) {
    let [p, q] = patch.degrees;
    let [[u0, u1], [v0, v1]] = patch.range;
    let (hu, hv) = (u1 - u0, v1 - v0);
    let (x, y) = ((u - u0) / hu, (v - v0) / hv);
    let net: Vec<Vec<[f64; 4]>> = (0..=p)
        .map(|i| {
            (0..=q)
                .map(|j| {
                    let h = patch.pts[i * (q + 1) + j];
                    let a = h.p * h.w;
                    [a.x, a.y, a.z, h.w]
                })
                .collect()
        })
        .collect();
    let diff_u = |n: &Vec<Vec<[f64; 4]>>| -> Vec<Vec<[f64; 4]>> {
        let k = n.len().saturating_sub(1) as f64;
        n.windows(2)
            .map(|r| {
                r[0].iter()
                    .zip(&r[1])
                    .map(|(a, b)| core::array::from_fn(|c| (b[c] - a[c]) * k / hu))
                    .collect()
            })
            .collect()
    };
    let diff_v = |n: &Vec<Vec<[f64; 4]>>| -> Vec<Vec<[f64; 4]>> {
        n.iter()
            .map(|r| {
                let k = r.len().saturating_sub(1) as f64;
                r.windows(2)
                    .map(|w| core::array::from_fn(|c| (w[1][c] - w[0][c]) * k / hv))
                    .collect()
            })
            .collect()
    };
    let at = |n: &Vec<Vec<[f64; 4]>>| -> [f64; 4] {
        if n.is_empty() || n[0].is_empty() {
            return [0.0; 4];
        }
        let rows: Vec<[f64; 4]> = n.iter().map(|r| casteljau(r, y)).collect();
        casteljau(&rows, x)
    };
    let mut a = [[0.0; 3]; 10];
    let mut w = [0.0; 10];
    for (k, &(i, j)) in ORDERS.iter().enumerate() {
        let mut n = net.clone();
        for _ in 0..i {
            n = diff_u(&n);
        }
        for _ in 0..j {
            n = diff_v(&n);
        }
        let h = at(&n);
        a[k] = [h[0], h[1], h[2]];
        w[k] = h[3];
    }
    (a, w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{KnotVector, Point3};
    use crate::math;

    /// A rational bicubic by quadratic patch of two by two spans.
    pub(crate) fn wavy() -> NurbsSurface {
        let ku = KnotVector::clamped(3, &[0.0, 0.4, 1.0], &[4, 1, 4]).unwrap();
        let kv = KnotVector::clamped(2, &[0.0, 0.6, 1.0], &[3, 1, 3]).unwrap();
        let mut pts = Vec::new();
        let mut w = Vec::new();
        for i in 0..5 {
            for j in 0..4 {
                let (x, y) = (i as f64 * 0.5, j as f64 * 0.4);
                pts.push(Point3::new(
                    x,
                    y,
                    0.3 * math::sin(x * 1.3) * math::cos(y + 0.2),
                ));
                w.push(1.0 + 0.3 * ((i + 2 * j) % 3) as f64);
            }
        }
        NurbsSurface::new([ku, kv], pts, Some(w)).unwrap()
    }

    /// Lower orders agree with `eval`, and each third derivative is the
    /// central difference of the second derivatives.
    #[test]
    fn third_derivatives_are_the_differences_of_the_second() {
        let s = wavy();
        let h = 1e-5;
        for (u, v) in [(0.1, 0.2), (0.55, 0.75), (0.9, 0.3), (0.39, 0.61)] {
            let j = s.jet3(u, v);
            let e = s.eval(u, v);
            let want = [e.point.to_vector(), e.du, e.dv, e.duu, e.duv, e.dvv];
            for k in 0..6 {
                assert!(
                    (j[k] - want[k]).length() <= 1e-10 * (1.0 + want[k].length()),
                    "{k}"
                );
            }
            let du = |a: f64| s.eval(u + a, v);
            let dv = |a: f64| s.eval(u, v + a);
            let close = |a: Vector3, b: Vector3| (a - b).length() <= 1e-4 * (1.0 + b.length());
            assert!(close(j[6], (du(h).duu - du(-h).duu) / (2.0 * h)));
            assert!(close(j[7], (dv(h).duu - dv(-h).duu) / (2.0 * h)));
            assert!(close(j[8], (du(h).dvv - du(-h).dvv) / (2.0 * h)));
            assert!(close(j[9], (dv(h).dvv - dv(-h).dvv) / (2.0 * h)));
        }
    }

    /// The interval enclosures hold every value at a grid over boxes in
    /// one patch, across knots and over the whole domain.
    #[test]
    fn enclosures_hold() {
        let s = wavy();
        for cell in [
            [[0.1, 0.15], [0.2, 0.3]],
            [[0.3, 0.6], [0.5, 0.7]],
            [[0.0, 1.0], [0.0, 1.0]],
        ] {
            let b = s.jet3_box(cell);
            for i in 0..=6 {
                for k in 0..=6 {
                    let u = cell[0][0] + (cell[0][1] - cell[0][0]) * f64::from(i) / 6.0;
                    let v = cell[1][0] + (cell[1][1] - cell[1][0]) * f64::from(k) / 6.0;
                    let j = s.jet3(u, v);
                    for (n, (jn, bn)) in j.iter().zip(&b).enumerate() {
                        for (c, x) in jn.to_array().into_iter().enumerate() {
                            let slack = 1e-9 * (1.0 + x.abs());
                            let iv = bn[c];
                            assert!(
                                iv.lo() - slack <= x && x <= iv.hi() + slack,
                                "{n} {cell:?} at {u}, {v}: {x} not in {iv:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
