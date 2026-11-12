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
    let Folds { cuts, points } = folds(&Curve::Nurbs(n.clone()), [t0, t1], lift(a), lift(b))?;
    let mut ends = vec![t0];
    ends.extend(cuts);
    ends.push(t1);
    Ok(ends
        .windows(2)
        .map(|w| {
            let point = points.iter().any(|p| p[0] <= w[0] && w[1] <= p[1]);
            ImageCurve {
                image: if point {
                    Image::Point(view.image(n.point((w[0] + w[1]) / 2.0)))
                } else {
                    Image::Curve {
                        curve: curve.clone(),
                        map: ImageMap::Affine([0.0, 1.0]),
                    }
                },
                range: [w[0], w[1]],
            }
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

/// Where a NURBS's image is cut: its folds, and the ends of stretches
/// [`settle`] cut; `points` are the stretches whose image is a point.
#[derive(Default)]
struct Folds {
    cuts: Vec<f64>,
    points: Vec<[f64; 2]>,
}

/// One Bézier piece's image, as [`settle`] reads it: the weight, the
/// numerators along the image's axes, and their hodographs' numerators.
struct Hodograph {
    w: Bernstein,
    na: Bernstein,
    nb: Bernstein,
    qa: Bernstein,
    qb: Bernstein,
}

/// The folds of `curve` over `(t0, t1)`: parameters where the image's
/// derivative along `a` vanishes and along `b` isn't certified non-zero.
/// A root along `a` that can't be resolved over more than [`STRETCH`]
/// (the tangent nearly along the view, near a cusp of the image) is
/// [`settle`]d: cut into stretches each one to one or a point. Elsewhere
/// the folds are today's, so views that drew before are cut the same.
fn folds(curve: &Curve, [t0, t1]: [f64; 2], a: Vector3, b: Vector3) -> Result<Folds, Failure> {
    let chain = Chain3::new(curve, t0, t1);
    let mut out = Folds::default();
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
        let hodograph = |n: &Bernstein| n.deriv().mul(&w).sub(&n.mul(&w.deriv()));
        let (na, nb) = (num(a), num(b));
        let (qa, qb) = (hodograph(&na), hodograph(&nb));
        let h = Hodograph { w, na, nb, qa, qb };
        for r in h.qa.roots(FOLD_WIDTH) {
            let [lo, hi] = r.range();
            let other = if hi > lo {
                h.qb.part(lo, hi).range()
            } else {
                h.qb.eval(lo)
            };
            if other.lo() > 0.0 || other.hi() < 0.0 {
                continue;
            }
            if matches!(r, Root::Unresolved { .. }) && hi - lo > STRETCH {
                let mut parts = Vec::new();
                settle(&h, lo, hi, &mut parts)?;
                for (s, point) in parts {
                    let at = [chain.param(i, s[0]), chain.param(i, s[1])];
                    out.cuts.extend(at);
                    if point {
                        out.points.push(at);
                    }
                }
                continue;
            }
            out.cuts.push(chain.param(i, (lo + hi) / 2.0));
        }
    }
    out.cuts.retain(|&t| t > t0 && t < t1);
    out.cuts.sort_by(f64::total_cmp);
    out.cuts.dedup();
    Ok(out)
}

/// The stretch `[lo, hi]` of a piece (local parameter) cut into parts
/// each one to one ([`one_to_one`]) or a point ([`point_like`]), each
/// with whether it is a point. A part neither, under [`STRETCH`] long,
/// runs along the view and refuses, as before. Halving only nears the
/// one or two cusp points in a stretch, so the parts are few: about two
/// a level, at most 20 levels.
fn settle(h: &Hodograph, lo: f64, hi: f64, out: &mut Vec<([f64; 2], bool)>) -> Result<(), Failure> {
    if one_to_one(&h.qa.part(lo, hi), &h.qb.part(lo, hi)) {
        out.push(([lo, hi], false));
        return Ok(());
    }
    if point_like(&h.w.part(lo, hi), &h.na.part(lo, hi), &h.nb.part(lo, hi)) {
        out.push(([lo, hi], true));
        return Ok(());
    }
    if hi - lo < STRETCH {
        return Err(Failure::Uncertain);
    }
    let mid = (lo + hi) / 2.0;
    settle(h, lo, mid, out)?;
    settle(h, mid, hi, out)
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
