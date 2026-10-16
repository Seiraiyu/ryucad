//! Bézier decomposition (The NURBS Book, A5.6, by repeated knot
//! insertion) and control-hull bounds.

use super::KnotVector;
use super::eval::{Coords, Spline};
use super::refine::{Hom, clamp, insert};
use crate::geom::{Box3, Point3, Vector3};

/// One polynomial piece of a curve: `degree + 1` control points and the
/// parameter range of the curve it covers. Local parameters run over
/// `[0, 1]`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BezierCurve<V> {
    pub(crate) range: [f64; 2],
    pub(crate) pts: Vec<Hom<V>>,
}

/// The componentwise least and greatest control points: with positive
/// weights the piece lies in their convex hull, so also in this box.
fn hull<V: Coords>(pts: &[Hom<V>]) -> (V, V) {
    pts.iter().fold((pts[0].p, pts[0].p), |(lo, hi), h| {
        (lo.min(h.p), hi.max(h.p))
    })
}

fn hull_box(pts: &[Hom<Vector3>]) -> Box3 {
    let (lo, hi) = hull(pts);
    Box3::new(Point3::ORIGIN + lo, Point3::ORIGIN + hi)
}

impl BezierCurve<Vector3> {
    pub(crate) fn bounds(&self) -> Box3 {
        hull_box(&self.pts)
    }
}

/// One direction brought to Bézier form: clamped, then every interior knot
/// raised to multiplicity `p`. Returns the distinct knots and the control
/// points, piece `i` being `pts[i p ..= i p + p]`.
pub(crate) fn decompose_line<V: Coords>(
    knots: &KnotVector,
    pw: &[Hom<V>],
) -> (Vec<f64>, Vec<Hom<V>>) {
    let p = knots.degree();
    let (mut flat, mut hom) = clamp(knots, pw);
    let mut distinct = flat.clone();
    distinct.dedup();
    for &t in &distinct[1..distinct.len() - 1] {
        let s = flat.iter().filter(|&&u| u == t).count();
        if s < p {
            (flat, hom) = insert(&flat, p, &hom, t, p - s);
        }
    }
    (distinct, hom)
}

impl<V: Coords> Spline<V> {
    /// The curve's Bézier pieces, in parameter order.
    pub(crate) fn pieces(&self) -> Vec<BezierCurve<V>> {
        let p = self.knots.degree();
        let (d, hom) = decompose_line(&self.knots, &self.hom());
        (0..d.len() - 1)
            .map(|i| BezierCurve {
                range: [d[i], d[i + 1]],
                pts: hom[i * p..=i * p + p].to_vec(),
            })
            .collect()
    }
}

/// One polynomial patch of a surface: a row-major `(p + 1) × (q + 1)` net
/// and the parameter ranges it covers. Local parameters run over
/// `[0, 1]²`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BezierPatch {
    pub(crate) range: [[f64; 2]; 2],
    pub(crate) degrees: [usize; 2],
    pub(crate) pts: Vec<Hom<Vector3>>,
}

impl BezierPatch {
    pub(crate) fn bounds(&self) -> Box3 {
        hull_box(&self.pts)
    }
}

/// A surface's Bézier patches, row-major over its spans: decomposed along
/// `u` column by column, then along `v` row by row.
pub(crate) fn patches(
    knots: &[KnotVector; 2],
    counts: [usize; 2],
    hom: &[Hom<Vector3>],
) -> Vec<BezierPatch> {
    let [nu, nv] = counts;
    let [p, q] = [knots[0].degree(), knots[1].degree()];
    let columns: Vec<_> = (0..nv)
        .map(|j| {
            let line: Vec<_> = (0..nu).map(|i| hom[i * nv + j]).collect();
            decompose_line(&knots[0], &line)
        })
        .collect();
    let du = columns[0].0.clone();
    let rows: Vec<_> = (0..columns[0].1.len())
        .map(|i| {
            let line: Vec<_> = columns.iter().map(|c| c.1[i]).collect();
            decompose_line(&knots[1], &line)
        })
        .collect();
    let dv = rows[0].0.clone();
    let mut out = Vec::with_capacity((du.len() - 1) * (dv.len() - 1));
    for a in 0..du.len() - 1 {
        for b in 0..dv.len() - 1 {
            let pts = (a * p..=a * p + p)
                .flat_map(|i| rows[i].1[b * q..=b * q + q].iter().copied())
                .collect();
            out.push(BezierPatch {
                range: [[du[a], du[a + 1]], [dv[b], dv[b + 1]]],
                degrees: [p, q],
                pts,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::geom::{KnotVector, NurbsCurve, NurbsSurface, Point3};
    use core::f64::consts::FRAC_1_SQRT_2;

    fn wavy() -> NurbsCurve {
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0, 2.5], &[4, 1, 2, 4]).unwrap();
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 3.0, -1.0),
            Point3::new(2.0, -1.0, 0.5),
            Point3::new(4.0, 2.0, 2.0),
            Point3::new(5.0, 0.0, 1.0),
            Point3::new(6.0, 1.0, -2.0),
            Point3::new(8.0, 0.0, 0.0),
        ];
        let w = vec![1.0, 0.5, 2.0, 0.8, 1.5, 0.3, 1.0];
        NurbsCurve::new(k, pts, Some(w)).unwrap()
    }

    fn wavy_surface() -> NurbsSurface {
        let ku = KnotVector::clamped(3, &[0.0, 0.5, 2.0], &[4, 1, 4]).unwrap();
        let kv = KnotVector::periodic(2, &[0.0, 1.0, 1.5, 3.0], &[1, 2, 1, 1]).unwrap();
        let (nu, nv) = (ku.control_count(), kv.control_count());
        let mut points = Vec::new();
        let mut weights = Vec::new();
        for i in 0..nu {
            for j in 0..nv {
                let (x, y) = (i as f64, j as f64);
                points.push(Point3::new(x, y, ((x * 1.7 + y * 0.9) % 2.0) - 1.0));
                weights.push(0.5 + ((i * 7 + j * 3) % 5) as f64 * 0.3);
            }
        }
        NurbsSurface::new([ku, kv], points, Some(weights)).unwrap()
    }

    /// A Bézier knot vector of degree `p` over `[a, b]`.
    fn bezier([a, b]: [f64; 2], p: usize) -> KnotVector {
        KnotVector::clamped(p, &[a, b], &[p + 1, p + 1]).unwrap()
    }

    #[test]
    fn curve_pieces_follow_the_curve() {
        let c = wavy();
        let pieces = c.pieces();
        let ranges: Vec<_> = pieces.iter().map(|p| p.range).collect();
        assert_eq!(ranges, [[0.0, 0.4], [0.4, 1.0], [1.0, 2.5]]);
        for piece in pieces {
            assert_eq!(piece.pts.len(), 4);
            let as_curve = NurbsCurve::new(
                bezier(piece.range, 3),
                piece.pts.iter().map(|h| Point3::ORIGIN + h.p).collect(),
                Some(piece.pts.iter().map(|h| h.w).collect()),
            )
            .unwrap();
            let [a, b] = piece.range;
            for i in 0..=10 {
                let t = a + (b - a) * f64::from(i) / 10.0;
                assert!(as_curve.point(t).distance(c.point(t)) <= 1e-14);
            }
        }
    }

    #[test]
    fn periodic_curves_decompose_over_one_period() {
        let k = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
        let pts = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(0.0, -1.0, 0.0),
        ];
        let c = NurbsCurve::new(k, pts, None).unwrap();
        assert_eq!(c.pieces().len(), 4);
        assert_eq!(c.pieces()[3].range, [3.0, 4.0]);
    }

    #[test]
    fn hull_bounds() {
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let p = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ];
        let quarter = NurbsCurve::new(k, p, Some(vec![1.0, FRAC_1_SQRT_2, 1.0])).unwrap();
        let b = quarter.bounds([0.0, 1.0]);
        assert_eq!(
            (b.min(), b.max()),
            (Point3::ORIGIN, Point3::new(1.0, 1.0, 0.0))
        );
        let c = wavy();
        let all = c.bounds(c.domain());
        let part = c.bounds([0.5, 0.9]);
        for i in 0..=100 {
            let t = 2.5 * f64::from(i) / 100.0;
            assert!(all.contains_with_tol(c.point(t), crate::tol::Tolerance::RESOLUTION));
            if (0.5..=0.9).contains(&t) {
                assert!(part.contains_with_tol(c.point(t), crate::tol::Tolerance::RESOLUTION));
            }
        }
        // Only the middle piece overlaps [0.5, 0.9].
        assert_eq!(part, c.pieces()[1].bounds());
        // The cache doesn't take part in equality, and survives a clone.
        assert_eq!(c.clone(), wavy());
    }

    #[test]
    fn surface_patches_follow_the_surface() {
        let s = wavy_surface();
        let patches = s.patches();
        assert_eq!(patches.len(), 6);
        assert_eq!(patches[0].range, [[0.0, 0.5], [0.0, 1.0]]);
        assert_eq!(patches[5].range, [[0.5, 2.0], [1.5, 3.0]]);
        let b = s.bounds();
        for patch in patches {
            assert_eq!(patch.pts.len(), 4 * 3);
            let [ru, rv] = patch.range;
            let as_surface = NurbsSurface::new(
                [bezier(ru, 3), bezier(rv, 2)],
                patch.pts.iter().map(|h| Point3::ORIGIN + h.p).collect(),
                Some(patch.pts.iter().map(|h| h.w).collect()),
            )
            .unwrap();
            for i in 0..=6 {
                for j in 0..=6 {
                    let u = ru[0] + (ru[1] - ru[0]) * f64::from(i) / 6.0;
                    let v = rv[0] + (rv[1] - rv[0]) * f64::from(j) / 6.0;
                    let r = s.point(u, v);
                    assert!(as_surface.point(u, v).distance(r) <= 1e-14);
                    assert!(b.contains_with_tol(r, crate::tol::Tolerance::RESOLUTION));
                }
            }
        }
    }
}
