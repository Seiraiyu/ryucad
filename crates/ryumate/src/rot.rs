//! Vectors and rotations over dual numbers, for residuals written once
//! with exact gradients.
//!
//! A body's rotation is its stored one turned by `exp([w])`, the rotation
//! about `w` by `|w|`, written as Rodrigues' formula in `s = w · w`:
//! `I + A(s) [w] + B(s) [w]²`, with `A = sin θ / θ` and
//! `B = (1 - cos θ) / θ²`. Both are smooth in `s`, so the map and its
//! derivatives are exact at `w = 0`, where a solve starts: there it is the
//! identity to the bit, and its derivative is `[δw]`.

use ryukei::math;
use ryusketch::core::dual::D;

pub(crate) type V<const N: usize> = [D<N>; 3];

/// Below this `s = θ²`, `A` and `B` and their derivatives come from their
/// series, where the closed forms cancel.
const SERIES: f64 = 1e-2;

pub(crate) fn konst<const N: usize>(v: [f64; 3]) -> V<N> {
    v.map(D::constant)
}

pub(crate) fn add<const N: usize>(a: V<N>, b: V<N>) -> V<N> {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub<const N: usize>(a: V<N>, b: V<N>) -> V<N> {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn times<const N: usize>(a: V<N>, k: D<N>) -> V<N> {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn scaled<const N: usize>(a: V<N>, k: f64) -> V<N> {
    [a[0].scale(k), a[1].scale(k), a[2].scale(k)]
}

pub(crate) fn dot<const N: usize>(a: V<N>, b: V<N>) -> D<N> {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross<const N: usize>(a: V<N>, b: V<N>) -> V<N> {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `sin θ / θ` and its derivative in `s = θ²`.
fn a_of(s: f64) -> (f64, f64) {
    if s < SERIES {
        let a = 1.0 - s / 6.0 * (1.0 - s / 20.0 * (1.0 - s / 42.0 * (1.0 - s / 72.0)));
        let da = -1.0 / 6.0 + s / 60.0 - s * s / 1680.0 + s * s * s / 90720.0;
        (a, da)
    } else {
        let t = math::sqrt(s);
        let (sin, cos) = math::sin_cos(t);
        let a = sin / t;
        (a, (cos - a) / (2.0 * s))
    }
}

/// `(1 - cos θ) / θ²` and its derivative in `s = θ²`.
fn b_of(s: f64) -> (f64, f64) {
    if s < SERIES {
        let b = 0.5 * (1.0 - s / 12.0 * (1.0 - s / 30.0 * (1.0 - s / 56.0 * (1.0 - s / 90.0))));
        let db = -1.0 / 24.0 + s / 360.0 - s * s / 13440.0 + s * s * s / 907200.0;
        (b, db)
    } else {
        let t = math::sqrt(s);
        let (sin, cos) = math::sin_cos(t);
        let a = sin / t;
        let b = (1.0 - cos) / s;
        (b, (a - 2.0 * b) / (2.0 * s))
    }
}

/// A rotation over duals: its columns.
#[derive(Clone, Copy)]
pub(crate) struct Rot<const N: usize> {
    pub cols: [V<N>; 3],
}

impl<const N: usize> Rot<N> {
    pub fn identity() -> Rot<N> {
        Rot {
            cols: [
                konst([1.0, 0.0, 0.0]),
                konst([0.0, 1.0, 0.0]),
                konst([0.0, 0.0, 1.0]),
            ],
        }
    }

    /// `exp([w])`.
    pub fn exp(w: V<N>) -> Rot<N> {
        let s = dot(w, w);
        let (a, da) = a_of(s.v);
        let (b, db) = b_of(s.v);
        let (a, b) = (s.map(a, da), s.map(b, db));
        // [w] e_j = w × e_j, and [w]² e_j = w × (w × e_j).
        let mut cols = Rot::identity().cols;
        for col in cols.iter_mut() {
            let e = *col;
            let we = cross(w, e);
            let wwe = cross(w, we);
            *col = add(add(e, times(we, a)), times(wwe, b));
        }
        Rot { cols }
    }

    /// The rotation by `angle` about the unit axis `n`.
    pub fn about(n: V<N>, angle: f64) -> Rot<N> {
        let (s, c) = math::sin_cos(angle);
        let mut cols = Rot::identity().cols;
        for col in cols.iter_mut() {
            let e = *col;
            let ne = cross(n, e);
            let nne = cross(n, ne);
            *col = add(add(e, scaled(ne, s)), scaled(nne, 1.0 - c));
        }
        Rot { cols }
    }

    /// `self · v`.
    pub fn apply(&self, v: V<N>) -> V<N> {
        add(
            add(times(self.cols[0], v[0]), times(self.cols[1], v[1])),
            times(self.cols[2], v[2]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(w: [f64; 3]) -> V<3> {
        [D::input(w[0], 0), D::input(w[1], 1), D::input(w[2], 2)]
    }

    #[test]
    fn the_map_is_the_identity_at_zero_with_the_cross_product_as_derivative() {
        let r = Rot::exp(input([0.0; 3]));
        for j in 0..3 {
            for i in 0..3 {
                assert_eq!(r.cols[j][i].v, if i == j { 1.0 } else { 0.0 });
                // d(R e_j)/dw_k = e_k × e_j.
                for k in 0..3 {
                    let mut e = [0.0; 3];
                    e[k] = 1.0;
                    let mut f = [0.0; 3];
                    f[j] = 1.0;
                    let want = crate::pose::cross(e, f)[i];
                    assert_eq!(r.cols[j][i].g[k], want);
                }
            }
        }
    }

    #[test]
    fn the_map_turns_by_its_length_and_its_derivatives_match_differences() {
        // A quarter turn about z.
        let q = core::f64::consts::FRAC_PI_2;
        let r = Rot::exp(input([0.0, 0.0, q]));
        assert!((r.cols[0][1].v - 1.0).abs() < 1e-15 && r.cols[0][0].v.abs() < 1e-15);
        for w in [
            [0.01, -0.02, 0.03],
            [0.04, 0.05, -0.06],
            [0.3, -0.2, 0.9],
            [1.0, 2.0, -0.5],
        ] {
            let r = Rot::exp(input(w));
            for k in 0..3 {
                let h = 1e-6;
                let (mut lo, mut hi) = (w, w);
                lo[k] -= h;
                hi[k] += h;
                let (a, b) = (Rot::exp(input(lo)), Rot::exp(input(hi)));
                for j in 0..3 {
                    for i in 0..3 {
                        let fd = (b.cols[j][i].v - a.cols[j][i].v) / (2.0 * h);
                        let err = (fd - r.cols[j][i].g[k]).abs();
                        assert!(err < 1e-8, "w {w:?} col {j} row {i} slot {k}: {err}");
                    }
                }
                // Orthonormal.
                let c = r.cols;
                assert!((dot(c[0], c[0]).v - 1.0).abs() < 1e-14);
                assert!(dot(c[0], c[1]).v.abs() < 1e-14);
            }
        }
        // The series and the closed forms meet where they hand over, to
        // the closed forms' rounding (B's cancels a little).
        let (below, above) = (a_of(SERIES * (1.0 - 1e-12)), a_of(SERIES));
        assert!((below.0 - above.0).abs() < 1e-14 && (below.1 - above.1).abs() < 1e-12);
        let (below, above) = (b_of(SERIES * (1.0 - 1e-12)), b_of(SERIES));
        assert!((below.0 - above.0).abs() < 1e-14 && (below.1 - above.1).abs() < 1e-12);
    }

    #[test]
    fn turns_about_an_axis() {
        let r = Rot::<3>::about(konst([0.0, 0.0, 1.0]), core::f64::consts::FRAC_PI_2);
        let v = r.apply(konst([1.0, 0.0, 0.0]));
        assert!(v[0].v.abs() < 1e-15 && (v[1].v - 1.0).abs() < 1e-15);
    }
}
