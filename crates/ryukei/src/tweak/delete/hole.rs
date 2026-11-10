//! Whether a loop is a hole of its face: it winds round no period of the
//! face's surface and runs clockwise about the face's normal, read from
//! its fins' pcurves.

use crate::geom::{Point2, Surface};
use crate::topo::{Body, FaceId, LoopId, Sense};

/// Samples per fin.
const SAMPLES: usize = 16;

/// The loop's points in its face's parameter plane, each periodic
/// direction unwrapped onto the one before, and whether it comes back to
/// its start (winds round no period). `None` for a lone vertex or a fin
/// without a pcurve.
pub(crate) fn polygon(body: &Body, f: FaceId, l: LoopId) -> Option<(Vec<Point2>, bool)> {
    if body.loops[l].vertex.is_some() {
        return None;
    }
    let (pu, pv) = body.surfaces[body.faces[f].surface].periods();
    let near = |x: f64, to: f64, p: Option<f64>| match p {
        Some(p) => x + p * ((to - x) / p).round(),
        None => x,
    };
    let mut pts: Vec<Point2> = Vec::new();
    for fin in body.loop_fins(l) {
        let fi = &body.fins[fin];
        let pc = fi.pcurve?;
        let [a, b] = body.edges[fi.edge].interval;
        for k in 0..SAMPLES {
            let s = k as f64 / SAMPLES as f64;
            let t = if fi.sense == Sense::Forward {
                a + (b - a) * s
            } else {
                b - (b - a) * s
            };
            let q = body.curves2[pc.curve].point(pc.param(t));
            let q = match pts.last() {
                Some(prev) => Point2::new(near(q.x, prev.x, pu), near(q.y, prev.y, pv)),
                None => q,
            };
            pts.push(q);
        }
    }
    let (first, last) = (*pts.first()?, *pts.last()?);
    let back = Point2::new(near(first.x, last.x, pu), near(first.y, last.y, pv));
    let closes = (back.x - first.x).abs() < 1e-9 && (back.y - first.y).abs() < 1e-9;
    Some((pts, closes))
}

/// Twice the signed area of `pts`, and the area of their box.
fn areas(pts: &[Point2]) -> (f64, f64) {
    let mut a = 0.0;
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for (i, p) in pts.iter().enumerate() {
        let q = pts[(i + 1) % pts.len()];
        a += p.x * q.y - q.x * p.y;
        lo = Point2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (a, (hi.x - lo.x) * (hi.y - lo.y))
}

/// Whether loop `l` of face `f` is a hole a cap may take out: it winds
/// round no period, its signed area (by the explicit rule: more than a
/// part in 10¹² of its box's) says it runs clockwise about the face's
/// normal, and on a cone it doesn't reach the apex's row.
pub(crate) fn is_hole(body: &Body, f: FaceId, l: LoopId) -> bool {
    let Some((pts, closes)) = polygon(body, f, l) else {
        return false;
    };
    if !closes {
        return false;
    }
    if let Surface::Cone(c) = &body.surfaces[body.faces[f].surface] {
        let apex = c.apex_v();
        let (lo, hi) = pts
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p.y), hi.max(p.y))
            });
        if lo <= apex && apex <= hi {
            return false;
        }
    }
    let (a, bx) = areas(&pts);
    // Undecided (or not a number): no hole.
    if a.is_nan() || a.abs() <= 1e-12 * bx {
        return false;
    }
    match body.faces[f].sense {
        Sense::Forward => a < 0.0,
        Sense::Reversed => a > 0.0,
    }
}
