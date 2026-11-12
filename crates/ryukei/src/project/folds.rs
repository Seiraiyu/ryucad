//! Images of NURBS curves, cut at their folds.

use super::{Image, ImageCurve, ImageMap, View};
use crate::geom::bernstein::{Bernstein, Root};
use crate::geom::chain3::Chain3;
use crate::geom::{Curve, Curve2, NurbsCurve, NurbsCurve2, Point2, Vector2, Vector3};
use crate::intersect::Failure;
use crate::interval::Interval;
use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use std::sync::Arc;

/// Roots of a piece's hodograph are bracketed this finely (local
/// parameter).
const FOLD_WIDTH: f64 = 1e-12;

/// A bracket wider than this (local parameter) that rounding can't sign
/// is a stretch running along the view.
const STRETCH: f64 = 1e-6;

pub(super) fn nurbs(
    n: &Arc<NurbsCurve>,
    [t0, t1]: [f64; 2],
    view: &View,
) -> Result<Vec<ImageCurve>, Failure> {
    let pts: Vec<Point2> = n.control_points().map(|p| view.image(p)).collect();
    let (a, b, spread) = principal(&pts);
    if spread <= LINEAR_RESOLUTION {
        return Ok(vec![ImageCurve {
            image: Image::Point(view.image(n.point((t0 + t1) / 2.0))),
            range: [t0, t1],
        }]);
    }
    let image = NurbsCurve2::new(n.knots().clone(), pts, Some(n.weights().to_vec()))
        .map_err(|_| Failure::Uncertain)?;
    let curve = Curve2::Nurbs2(Arc::new(image));
    let lift = |d: Vector2| view.x().get() * d.x + view.y().get() * d.y;
    let folds = folds(&Curve::Nurbs(n.clone()), [t0, t1], lift(a), lift(b))?;
    let mut ends = vec![t0];
    ends.extend(folds);
    ends.push(t1);
    Ok(ends
        .windows(2)
        .map(|w| ImageCurve {
            image: Image::Curve {
                curve: curve.clone(),
                map: ImageMap::Affine([0.0, 1.0]),
            },
            range: [w[0], w[1]],
        })
        .collect())
}

/// The image's principal axis `a` (of most spread), the one square to
/// it, and the largest distance of a control point from their mean.
fn principal(pts: &[Point2]) -> (Vector2, Vector2, f64) {
    let k = pts.len() as f64;
    let mean = pts
        .iter()
        .fold(Vector2::ZERO, |s, p| s + Vector2::new(p.x, p.y))
        / k;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    let mut far: f64 = 0.0;
    for p in pts {
        let d = Vector2::new(p.x, p.y) - mean;
        sxx += d.x * d.x;
        syy += d.y * d.y;
        sxy += d.x * d.y;
        far = far.max(d.length());
    }
    let th = 0.5 * math::atan2(2.0 * sxy, sxx - syy);
    let (s, c) = math::sin_cos(th);
    (Vector2::new(c, s), Vector2::new(-s, c), far)
}

/// `p · d` with every product and sum rounded outward.
fn idot(p: Vector3, d: Vector3) -> Interval {
    let i = Interval::point;
    i(p.x) * i(d.x) + i(p.y) * i(d.y) + i(p.z) * i(d.z)
}

/// True when the image whose hodograph is `(qa, qb) / w²` over a stretch
/// is one to one there: some direction `d` makes every Bernstein
/// coefficient of `d · (qa, qb)` positive (rounded outward), so the image
/// moves strictly along `d`. `d` is the sum of the coefficients' unit
/// directions (their midpoints'): any `d` that passes proves it.
#[cfg_attr(not(test), allow(dead_code))]
fn one_to_one(qa: &Bernstein, qb: &Bernstein) -> bool {
    let (ca, cb) = (qa.coefficients(), qb.coefficients());
    let mut d = Vector2::ZERO;
    for (x, y) in ca.iter().zip(cb) {
        let v = Vector2::new((x.lo() + x.hi()) / 2.0, (y.lo() + y.hi()) / 2.0);
        let l = v.length();
        if l > 0.0 {
            d = d + v / l;
        }
    }
    if d.length() == 0.0 {
        return false;
    }
    let (dx, dy) = (Interval::point(d.x), Interval::point(d.y));
    ca.iter()
        .zip(cb)
        .all(|(&x, &y)| (dx * x + dy * y).lo() > 0.0)
}

/// True when the image whose control points are `(na, nb) / w` lies
/// within [`LINEAR_RESOLUTION`] of a point: the box of the control points
/// (rounded outward; positive weights keep the curve in their hull) has
/// a diagonal no longer. The rule `nurbs` applies to a whole curve.
#[cfg_attr(not(test), allow(dead_code))]
fn point_like(w: &Bernstein, na: &Bernstein, nb: &Bernstein) -> bool {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for ((x, y), wk) in na
        .coefficients()
        .iter()
        .zip(nb.coefficients())
        .zip(w.coefficients())
    {
        let (Some(px), Some(py)) = (x.checked_div(*wk), y.checked_div(*wk)) else {
            return false;
        };
        lo = [lo[0].min(px.lo()), lo[1].min(py.lo())];
        hi = [hi[0].max(px.hi()), hi[1].max(py.hi())];
    }
    let (dx, dy) = (hi[0] - lo[0], hi[1] - lo[1]);
    dx * dx + dy * dy <= LINEAR_RESOLUTION * LINEAR_RESOLUTION
}

/// The folds of `curve` over `(t0, t1)`: parameters where the image's
/// derivative along `a` vanishes and along `b` isn't certified non-zero.
fn folds(curve: &Curve, [t0, t1]: [f64; 2], a: Vector3, b: Vector3) -> Result<Vec<f64>, Failure> {
    let chain = Chain3::new(curve, t0, t1);
    let mut out = Vec::new();
    for (i, piece) in chain.pieces.iter().enumerate() {
        let w = Bernstein::new(piece.pts.iter().map(|h| Interval::point(h.w)).collect());
        let num = |d: Vector3| {
            Bernstein::new(
                piece
                    .pts
                    .iter()
                    .map(|h| idot(h.p, d) * Interval::point(h.w))
                    .collect(),
            )
        };
        let hodograph = |n: Bernstein| n.deriv().mul(&w).sub(&n.mul(&w.deriv()));
        let (qa, qb) = (hodograph(num(a)), hodograph(num(b)));
        for r in qa.roots(FOLD_WIDTH) {
            let [lo, hi] = r.range();
            let other = if hi > lo {
                qb.part(lo, hi).range()
            } else {
                qb.eval(lo)
            };
            if other.lo() > 0.0 || other.hi() < 0.0 {
                continue;
            }
            if matches!(r, Root::Unresolved { .. }) && hi - lo > STRETCH {
                return Err(Failure::Uncertain);
            }
            out.push(chain.param(i, (lo + hi) / 2.0));
        }
    }
    out.retain(|&t| t > t0 && t < t1);
    out.sort_by(f64::total_cmp);
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(c: &[f64]) -> Bernstein {
        Bernstein::from_f64(c)
    }

    #[test]
    fn a_hodograph_in_a_half_plane_is_one_to_one() {
        // Turning through a right angle: every coefficient within 90° of
        // the diagonal.
        assert!(one_to_one(&poly(&[1.0, 1.0, 0.0]), &poly(&[0.0, 1.0, 1.0])));
        // Through a half turn and back: no direction sees every
        // coefficient ahead.
        assert!(!one_to_one(
            &poly(&[1.0, 0.0, -1.0]),
            &poly(&[0.0, 1.0, 0.0])
        ));
        // Through zero (a cusp): no direction either.
        assert!(!one_to_one(&poly(&[1.0, -1.0]), &poly(&[1.0, -1.0])));
    }

    #[test]
    fn a_hull_within_resolution_is_a_point() {
        let w = poly(&[1.0, 1.0, 1.0]);
        assert!(point_like(
            &w,
            &poly(&[0.0, 3e-9, 0.0]),
            &poly(&[1.0, 1.0, 1.0 + 2e-9])
        ));
        assert!(!point_like(
            &w,
            &poly(&[0.0, 3e-8, 0.0]),
            &poly(&[1.0, 1.0, 1.0])
        ));
    }
}
