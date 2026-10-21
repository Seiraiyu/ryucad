//! Overflow: every rail and end curve stays inside the face it lies on.
//!
//! Each new curve is met with the edges of its face in the face's
//! parameter plane ([`crate::geom::intersect2`]), shifted by whole
//! periods where the surface has them. A curve may touch only at its
//! ends, and only the edges it is meant to end on there: a rail its cut
//! at an end or the smooth edge at a smooth join, an end curve the two
//! cuts beside its vertex. The blended edges themselves are left out,
//! since they become rails. New curves on one face may not meet either,
//! except end to end. And no other loop of the face, and no other blend's
//! curve, may lie in a strip a blend takes: a point is in a strip when it
//! lies between its nearest points on the strip's rail and on the
//! strip's blended edges, within the strip's ends. A loop that crosses a
//! rail is caught by the crossings, so one point of each tells.

use super::Near;
use super::chain::Chain;
use super::plan::{Joint, Plan};
use super::spec::Spec;
use crate::geom::intersect2::{Meet2, crossings};
use crate::geom::{Box3, Curve, Curve2, Point3};
use crate::intersect::{shifted, shifts};
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, FinId};
use crate::{BlendError, Error};
use std::collections::{BTreeMap, BTreeSet};

/// A new curve on a face.
struct Item<'a> {
    face: FaceId,
    curve: &'a Curve,
    /// The curve's own parameters it covers.
    interval: [f64; 2],
    pcurve: (&'a Curve2, [f64; 2]),
    tolerance: f64,
    /// Edges it may touch at its ends.
    allowed: Vec<EdgeId>,
    /// The blended edge it belongs to, for the error.
    edge: Name,
    /// The chain it belongs to.
    chain: usize,
    /// For a rail, the blended edges the strip it bounds runs along.
    strip: Vec<EdgeId>,
}

impl Item<'_> {
    fn ends(&self) -> [Point3; 2] {
        self.interval.map(|t| self.curve.point(t))
    }

    fn range2(&self) -> [f64; 2] {
        let m = self.pcurve.1;
        self.interval.map(|t| m[0] + m[1] * t)
    }

    /// The pcurve's range less, at each end, the stretch over which a
    /// curve touching an edge there tangentially stays within the
    /// tolerances of it: ten times `sqrt(2 × size × tolerance)`, the
    /// size about its length. `None` for a curve with nothing it may end
    /// on.
    fn inner(&self) -> Option<[f64; 2]> {
        if self.allowed.is_empty() {
            return None;
        }
        let [a, b] = self.interval;
        let [p, q] = self.ends();
        let size = p.distance(self.curve.point((a + b) / 2.0)) + q.distance(p);
        let s = 10.0 * (2.0 * size * self.tolerance.max(LINEAR_RESOLUTION)).sqrt();
        let speed = |t: f64| self.curve.eval(t).d1.length();
        let (va, vb) = (speed(a), speed(b));
        if !(va > 0.0 && vb > 0.0) {
            return None;
        }
        let dir = (b - a).signum();
        let [a2, b2] = [a + dir * s / va, b - dir * s / vb];
        if (b2 - a2) * dir <= 0.0 {
            return None;
        }
        let m = self.pcurve.1;
        Some([m[0] + m[1] * a2, m[0] + m[1] * b2])
    }

    fn bounds(&self) -> Box3 {
        let [a, b] = self.interval;
        self.curve.bounds([a.min(b), a.max(b)]).grow(self.tolerance)
    }
}

/// Fails with `Overflow` where a blend's rail or end curve leaves its
/// face, crosses another blend's, or takes in another loop of its face,
/// and `Uncertain` where a meeting can't be certified.
pub(crate) fn check(
    body: &Body,
    specs: &[Spec],
    chains: &[Chain],
    plans: &[Plan],
    near: &Near,
) -> Result<(), Error> {
    // Where curves of different chains may meet, end to end.
    let feet: Vec<Point3> = near.corners.iter().flat_map(|c| c.feet).collect();
    let mut items: Vec<Item> = Vec::new();
    let mut blended: BTreeSet<EdgeId> = BTreeSet::new();
    for (ci, (chain, plan)) in chains.iter().zip(plans).enumerate() {
        for &(k, _) in &chain.edges {
            blended.insert(specs[k].edge);
        }
        let ends: Vec<(usize, [EdgeId; 2])> = plan
            .joints
            .iter()
            .enumerate()
            .filter_map(|(vi, j)| match j {
                Joint::End { crossings, .. } => Some((vi, [crossings[0].cut, crossings[1].cut])),
                _ => None,
            })
            .collect();
        let smooth: Vec<EdgeId> = plan
            .joints
            .iter()
            .filter_map(|j| match j {
                Joint::SmoothJoin { smooth, .. } => Some(*smooth),
                _ => None,
            })
            .collect();
        for seg in &plan.segments {
            let edge = specs[chain.edges[seg.edges[0]].0].name;
            for side in 0..2 {
                let mut allowed: Vec<EdgeId> = ends.iter().map(|(_, c)| c[side]).collect();
                if side == 1 {
                    allowed.extend(&smooth);
                }
                let rail = &seg.rails[side];
                items.push(Item {
                    face: seg.faces[side],
                    curve: &rail.curve,
                    interval: seg.intervals[side],
                    pcurve: (&rail.pcurve.0, rail.pcurve.1),
                    tolerance: rail.tolerance,
                    allowed,
                    edge,
                    chain: ci,
                    strip: seg
                        .edges
                        .iter()
                        .map(|&p| specs[chain.edges[p].0].edge)
                        .collect(),
                });
            }
        }
        for (vi, cuts) in &ends {
            let Joint::End { face, curve, .. } = &plan.joints[*vi] else {
                unreachable!("an end")
            };
            let seg = if *vi == 0 {
                &plan.segments[0]
            } else {
                &plan.segments[plan.segments.len() - 1]
            };
            items.push(Item {
                face: *face,
                curve: &curve.curve,
                interval: curve.interval,
                pcurve: (&curve.pcurves[1].0, curve.pcurves[1].1),
                tolerance: curve.tolerance,
                allowed: cuts.to_vec(),
                edge: specs[chain.edges[seg.edges[0]].0].name,
                chain: ci,
                strip: Vec::new(),
            });
        }
    }
    let mut by_face: BTreeMap<FaceId, Vec<usize>> = BTreeMap::new();
    for (i, it) in items.iter().enumerate() {
        by_face.entry(it.face).or_default().push(i);
    }
    for (&face, list) in &by_face {
        let overflow = |it: &Item| -> Error {
            BlendError::Overflow {
                edge: it.edge,
                face: body.faces[face].name,
            }
            .into()
        };
        let edges = face_edges(body, face, &blended);
        for &i in list {
            let it = &items[i];
            let bx = it.bounds();
            for &(g, fin) in &edges {
                let e = &body.edges[g];
                let gc = &body.curves[e.curve];
                if !gc
                    .bounds(e.interval)
                    .grow(e.tolerance.get())
                    .intersects(&bx)
                {
                    continue;
                }
                let Some(pc) = body.fins[fin].pcurve else {
                    return Err(BlendError::Uncertain { at: it.edge }.into());
                };
                let range = e.interval.map(|t| pc.param(t));
                let c2 = &body.curves2[pc.curve];
                let reach = 10.0 * (it.tolerance + e.tolerance.get());
                for p in meets(body, face, (it.pcurve.0, it.range2()), (c2, range))
                    .or_else(|| {
                        // Rounding can't settle a tangent touch where the
                        // curve ends on an edge it may end on: leave out
                        // the stretch where the two stay that close.
                        let inner = it.inner()?;
                        meets(body, face, (it.pcurve.0, inner), (c2, range))
                    })
                    .ok_or(BlendError::Uncertain { at: it.edge })?
                {
                    let at_end = it.ends().iter().any(|q| q.distance(p) <= reach);
                    if !(at_end && it.allowed.contains(&g)) {
                        return Err(overflow(it));
                    }
                }
            }
        }
        for (x, &i) in list.iter().enumerate() {
            for &j in &list[x + 1..] {
                let (a, b) = (&items[i], &items[j]);
                if !a.bounds().intersects(&b.bounds()) {
                    continue;
                }
                let reach = 10.0 * (a.tolerance + b.tolerance);
                for p in meets(
                    body,
                    face,
                    (a.pcurve.0, a.range2()),
                    (b.pcurve.0, b.range2()),
                )
                .ok_or(BlendError::Uncertain { at: a.edge })?
                {
                    let end_of = |it: &Item| it.ends().iter().any(|q| q.distance(p) <= reach);
                    let corner = feet.iter().any(|q| q.distance(p) <= reach);
                    if !((a.chain == b.chain || corner) && end_of(a) && end_of(b)) {
                        return Err(overflow(a));
                    }
                }
            }
        }
        // Another loop of the face, or another chain's curve, inside a
        // strip a blend takes.
        for &i in list {
            let it = &items[i];
            if it.strip.is_empty() {
                continue;
            }
            let mut points: Vec<Point3> = Vec::new();
            for l in body.face_loops(face) {
                let skip = body.loop_fins(l).any(|f| {
                    let g = body.fins[f].edge;
                    blended.contains(&g) || items.iter().any(|x| x.allowed.contains(&g))
                });
                if skip {
                    continue;
                }
                let v = match body.loops[l].vertex {
                    Some(v) => v,
                    None => body.start_of(body.loops[l].first_fin.expect("a loop of fins")),
                };
                points.push(body.vertices[v].point);
            }
            for &j in list {
                if items[j].chain != it.chain {
                    let [a, b] = items[j].interval;
                    points.push(items[j].curve.point((a + b) / 2.0));
                }
            }
            if points.iter().any(|&q| in_strip(body, it, q)) {
                return Err(overflow(it));
            }
        }
    }
    Ok(())
}

/// Whether `q` is in the strip a rail bounds: between its nearest points
/// on the rail and on the strip's blended edges (the section through it),
/// not beyond the strip's ends.
fn in_strip(body: &Body, rail: &Item, q: Point3) -> bool {
    let nearest = |c: &Curve, [a, b]: [f64; 2]| -> (Point3, bool) {
        let mut t = c.project(q).param;
        if let Some(period) = c.period() {
            t += ((a.min(b) - t) / period).ceil() * period;
        }
        let inside = (a.min(b)..=a.max(b)).contains(&t);
        (c.point(t.clamp(a.min(b), a.max(b))), inside)
    };
    let (r, on_rail) = nearest(rail.curve, rail.interval);
    let mut best: Option<(f64, Point3, bool)> = None;
    for &g in &rail.strip {
        let e = &body.edges[g];
        let (p, inside) = nearest(&body.curves[e.curve], e.interval);
        let d = p.distance(q);
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, p, inside));
        }
    }
    let Some((_, e, on_edge)) = best else {
        return false;
    };
    let v = e - r;
    let len2 = v.length_squared();
    if !(on_rail || on_edge) || len2 <= LINEAR_RESOLUTION * LINEAR_RESOLUTION {
        return false;
    }
    let w = q - r;
    let s = w.dot(v) / len2;
    let lateral = (w - v * s).length();
    (-1e-9..=1.0 + 1e-9).contains(&s) && lateral * lateral <= 0.25 * len2
}

/// Every edge of `face` but the blended ones, with its fin on the face.
fn face_edges(body: &Body, face: FaceId, blended: &BTreeSet<EdgeId>) -> Vec<(EdgeId, FinId)> {
    body.face_loops(face)
        .flat_map(|l| body.loop_fins(l).collect::<Vec<_>>())
        .filter(|&f| !blended.contains(&body.fins[f].edge))
        .map(|f| (body.fins[f].edge, f))
        .collect()
}

/// Where two curves in `face`'s parameter plane meet, as points on the
/// first's image, every shift by whole periods tried. `None` where a
/// meeting can't be certified.
fn meets(
    body: &Body,
    face: FaceId,
    a: (&Curve2, [f64; 2]),
    b: (&Curve2, [f64; 2]),
) -> Option<Vec<Point3>> {
    let surface = &body.surfaces[body.faces[face].surface];
    let (pu, pv) = surface.periods();
    let mut out = Vec::new();
    for shift in shifts(a.0, a.1, b.0, b.1, [pu, pv]) {
        let moved = shifted(b.0, shift)?;
        for m in crossings(a, (&moved, b.1), LINEAR_RESOLUTION)? {
            let s = match m {
                Meet2::Point { ta, .. } => vec![ta],
                Meet2::Overlap { ta, .. } => ta.to_vec(),
            };
            for t in s {
                let uv = a.0.point(t);
                out.push(surface.point(uv.x, uv.y));
            }
        }
    }
    Some(out)
}
