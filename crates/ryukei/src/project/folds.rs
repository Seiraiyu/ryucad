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
