//! Ruled surfaces between two rails, as exact NURBS: what a chamfer
//! between curved rails is.

use super::project::binom;
use super::refine::{Hom, insert};
use super::{KnotVector, NurbsCurve, NurbsSurface};
use crate::Error;
use crate::geom::{Point3, Vector3};

/// A curve's clamped flat knots and control points at degree `q` (at
/// least its own), from its Bézier pieces each raised to degree `q` in
/// homogeneous coordinates: `Q_i = Σ_j binom(p, j) binom(q - p, i - j) /
/// binom(q, i) P_j`. Every interior break gets multiplicity `q`, so the
/// spline is the same curve, piece by piece.
pub(super) fn elevated(c: &NurbsCurve, q: usize) -> (Vec<f64>, Vec<Hom<Vector3>>) {
    let p = c.degree();
    let k = q - p;
    let pieces = c.pieces();
    let mut flat = Vec::with_capacity(q * pieces.len() + 2);
    let mut hom: Vec<Hom<Vector3>> = Vec::with_capacity(q * pieces.len() + 1);
    for (n, piece) in pieces.iter().enumerate() {
        let h: Vec<(Vector3, f64)> = piece.pts.iter().map(|x| (x.p * x.w, x.w)).collect();
        let raised = (0..=q).map(|i| {
            let (mut a, mut w) = (Vector3::ZERO, 0.0);
            for (j, &(hp, hw)) in h
                .iter()
                .enumerate()
                .take(i.min(p) + 1)
                .skip(i.saturating_sub(k))
            {
                let f = binom(p, j) * binom(k, i - j) / binom(q, i);
                a += hp * f;
                w += hw * f;
            }
            Hom { p: a / w, w }
        });
        if n == 0 {
            flat.extend(core::iter::repeat_n(piece.range[0], q + 1));
            hom.extend(raised);
        } else {
            hom.extend(raised.skip(1));
        }
        flat.extend(core::iter::repeat_n(piece.range[1], q));
    }
    flat.push(pieces[pieces.len() - 1].range[1]);
    (flat, hom)
}

/// The distinct interior knots of flat clamped knots of degree `q`.
fn breaks(flat: &[f64], q: usize) -> Vec<f64> {
    let mut out = flat[q + 1..flat.len() - q - 1].to_vec();
    out.dedup();
    out
}

impl NurbsSurface {
    /// The ruled surface from rail `a` (at `v = 0`) to rail `b` (at
    /// `v = 1`), exactly: degree 1 in `v`, and along `u` both rails made
    /// compatible (raised to the higher degree, and given each other's
    /// knots) with their own parameter. For each `u` the line `v ↦ S(u, v)`
    /// is the segment from `a(u)` to `b(u)`; where the rails' weights
    /// differ, `v` runs along it rationally rather than in proportion.
    ///
    /// The rails must share their domain (else `Degenerate`); periodic
    /// rails are taken over one period and give a clamped surface. Fails
    /// like [`NurbsSurface::new`] otherwise.
    pub fn ruled(a: &NurbsCurve, b: &NurbsCurve) -> Result<Self, Error> {
        if a.domain() != b.domain() {
            return Err(Error::Degenerate);
        }
        let q = a.degree().max(b.degree());
        let (mut fa, mut ha) = elevated(a, q);
        let (mut fb, mut hb) = elevated(b, q);
        let (ba, bb) = (breaks(&fa, q), breaks(&fb, q));
        for &t in &bb {
            if !ba.contains(&t) {
                (fa, ha) = insert(&fa, q, &ha, t, q);
            }
        }
        for &t in &ba {
            if !bb.contains(&t) {
                (fb, hb) = insert(&fb, q, &hb, t, q);
            }
        }
        debug_assert_eq!(fa, fb);
        let u = KnotVector::clamped_flat(q, &fa);
        let v = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2])?;
        let mut points = Vec::with_capacity(2 * ha.len());
        let mut weights = Vec::with_capacity(2 * ha.len());
        for (x, y) in ha.iter().zip(&hb) {
            points.push(Point3::ORIGIN + x.p);
            points.push(Point3::ORIGIN + y.p);
            weights.push(x.w);
            weights.push(y.w);
        }
        NurbsSurface::new([u, v], points, Some(weights))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Frame};
    use core::f64::consts::FRAC_1_SQRT_2;

    fn cubic() -> NurbsCurve {
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0], &[4, 1, 4]).unwrap();
        NurbsCurve::new(
            k,
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 1.0, 0.5),
                Point3::new(2.0, 0.0, 1.0),
                Point3::new(3.0, -1.0, 0.0),
                Point3::new(4.0, 0.0, 0.0),
            ],
            None,
        )
        .unwrap()
    }

    /// A rational quarter circle of radius 2 at height 1, over `[0, 1]`.
    fn arc() -> NurbsCurve {
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        NurbsCurve::new(
            k,
            vec![
                Point3::new(2.0, 0.0, 1.0),
                Point3::new(2.0, 2.0, 1.0),
                Point3::new(0.0, 2.0, 1.0),
            ],
            Some(vec![1.0, FRAC_1_SQRT_2, 1.0]),
        )
        .unwrap()
    }

    /// The rails at `v = 0` and `v = 1`, and every point between them on
    /// the segment joining them.
    fn rules(s: &NurbsSurface, a: &NurbsCurve, b: &NurbsCurve) {
        let [d0, d1] = a.domain();
        for i in 0..=50 {
            let u = d0 + (d1 - d0) * f64::from(i) / 50.0;
            let (pa, pb) = (a.point(u), b.point(u));
            assert!(s.point(u, 0.0).distance(pa) <= 1e-13, "{u}");
            assert!(s.point(u, 1.0).distance(pb) <= 1e-13, "{u}");
            let d = pb - pa;
            for j in 1..10 {
                let x = s.point(u, f64::from(j) / 10.0) - pa;
                assert!(
                    x.cross(d).length() <= 1e-12 * (1.0 + d.length_squared()),
                    "{u}"
                );
                let along = x.dot(d) / d.length_squared();
                assert!((0.0..=1.0).contains(&along));
            }
        }
    }

    #[test]
    fn between_polynomial_rails() {
        let a = cubic();
        // A line rail over the same domain, raised to degree 3 and given
        // the cubic's knot.
        let k = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let b = NurbsCurve::new(
            k,
            vec![Point3::new(0.0, 3.0, 0.0), Point3::new(4.0, 3.0, 2.0)],
            None,
        )
        .unwrap();
        let s = NurbsSurface::ruled(&a, &b).unwrap();
        assert_eq!(s.degrees(), [3, 1]);
        rules(&s, &a, &b);
        // Weights equal: v runs in proportion.
        let m = s.point(0.3, 0.25);
        let want = a.point(0.3) + (b.point(0.3) - a.point(0.3)) * 0.25;
        assert!(m.distance(want) <= 1e-13);
    }

    #[test]
    fn between_rational_rails_with_different_knots() {
        let a = arc();
        let b = cubic();
        let s = NurbsSurface::ruled(&a, &b).unwrap();
        rules(&s, &a, &b);
        // A circle's rail stays on its circle.
        for i in 0..=10 {
            let p = s.point(f64::from(i) / 10.0, 0.0);
            assert!((crate::math::hypot(p.x, p.y) - 2.0).abs() <= 1e-14);
        }
    }

    #[test]
    fn periodic_rails_and_refusals() {
        let (c0, _) = Circle::new(Frame::WORLD, 1.0).unwrap().to_nurbs().unwrap();
        let up = Frame::new(
            Point3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let (c1, _) = Circle::new(up, 2.0).unwrap().to_nurbs().unwrap();
        let s = NurbsSurface::ruled(&c0, &c1).unwrap();
        assert_eq!(s.periods(), (None, None));
        rules(&s, &c0, &c1);
        // A cone frustum: halfway up, the radius is 1.5.
        let p = s.point(1.0, 0.5);
        assert!((crate::math::hypot(p.x, p.y) - 1.5).abs() <= 1e-14 && (p.z - 0.5).abs() <= 1e-14);
        assert_eq!(NurbsSurface::ruled(&cubic(), &c0), Err(Error::Degenerate));
    }
}
