//! Tightening: an edge between two faces that meet in a known curve put
//! on that curve, so its tolerance need cover only how far its vertices
//! lie off it.
//!
//! An edge whose curve strays from its faces by more than resolution, on
//! whatever surfaces, is met with the surfaces' intersection
//! ([`crate::intersect::surface_surface`]) inside its box, grown by its
//! reach. Its reach is twice its measured gap (at
//! least ten resolutions, at most the cap): a branch that crosses
//! nowhere near tangentially, every one of 17 samples along the edge, and
//! both its vertices within the reach of it, closer to both surfaces than
//! the edge was, replaces the edge's curve. The edge runs from its start
//! vertex's foot on the branch to its end vertex's, the way the edge's own
//! curve runs at its middle (reversed, uses and all, where the branch runs
//! the other way); on a closed branch the stretch must hold the middle's
//! foot, and a closed edge takes a whole period, or a closed clamped
//! branch (a spline's) started at its vertex's foot
//! ([`crate::intersect::reseamed`]). Vertices stay where they are: what
//! the new curve's ends leave the tolerances cover (see `gaps`). The
//! file's pcurves for a tightened edge give way to the branch's, where
//! their maps are the identity (the general intersector's, certified with
//! the branch), so the pcurves step takes them as the file's; else it
//! finds exact ones or fits them. An edge with no such branch keeps
//! its curve and is tolerated instead, and so is an edge whose faces are
//! tangent or nearly so at its middle (their normals within a hundredth
//! of a radian): there the branch lies far further from the edge than
//! its gap, or runs tangentially, and tracing it only costs time.

use super::Loop;
use super::Shape;
use super::gaps::edge_gap;
use super::names::Names;
use crate::geom::{Curve, Curve2, Surface};
use crate::intersect::{SurfaceHit, surface_surface};
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use rayon::prelude::*;

/// Samples along an edge, ends included.
const SAMPLES: usize = 16;

/// The least cosine between the edge's direction and the branch's at the
/// edge's middle.
const ALIGNED: f64 = 0.5;

/// The least sine of the angle between the faces' normals at the edge's
/// middle for the edge to be tightened.
const CREASE: f64 = 0.01;

/// A tightened edge: its new curve, interval, whether it now runs the
/// other way, and how far the branch may be from its surfaces.
struct Tight {
    curve: Curve,
    interval: [f64; 2],
    flip: bool,
    tolerance: f64,
    /// The branch on each of the edge's two faces' surfaces, in order,
    /// where its map is the identity.
    pcurves: [Option<Curve2>; 2],
}

/// Tightens every edge of `s` that can be, returning their names, sorted.
pub(super) fn tighten(s: &mut Shape, names: &Names, cap: f64) -> Vec<Name> {
    let mut faces: Vec<Vec<usize>> = vec![Vec::new(); s.edges.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    faces[u.edge].push(k);
                }
            }
        }
    }
    let found: Vec<Option<(Tight, [usize; 2])>> = (0..s.edges.len())
        .into_par_iter()
        .map(|g| match faces[g][..] {
            [a, b] if a != b => tight(s, g, [a, b], cap).map(|t| (t, [a, b])),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for (g, t) in found.into_iter().enumerate() {
        let Some((t, pair)) = t else { continue };
        s.curves.push(t.curve);
        let e = &mut s.edges[g];
        e.curve = s.curves.len() - 1;
        e.interval = t.interval;
        e.tolerance = e.tolerance.max(t.tolerance);
        if t.flip {
            std::mem::swap(&mut e.start, &mut e.end);
        }
        let mut given = [None; 2];
        for (i, c2) in t.pcurves.into_iter().enumerate() {
            if let Some(c2) = c2 {
                s.curves2.push(c2);
                given[i] = Some(s.curves2.len() - 1);
            }
        }
        for (k, f) in s.faces.iter_mut().enumerate() {
            let side = pair.iter().position(|&x| x == k);
            for l in &mut f.loops {
                if let Loop::Edges(uses) = l {
                    for u in uses.iter_mut().filter(|u| u.edge == g) {
                        u.pcurve = side.and_then(|i| given[i]);
                        if t.flip {
                            u.sense = u.sense.flipped();
                        }
                    }
                }
            }
        }
        out.push(names.edges[g]);
    }
    out.sort();
    out
}

/// A branch's pcurves whose maps are the identity.
fn identity(pcurves: &[(Curve2, [f64; 2]); 2]) -> [Option<Curve2>; 2] {
    pcurves.clone().map(|(c, m)| (m == [0.0, 1.0]).then_some(c))
}

/// Edge `g`'s curve on the intersection of `faces`' surfaces, if one runs
/// along it.
fn tight(s: &Shape, g: usize, faces: [usize; 2], cap: f64) -> Option<Tight> {
    let e = &s.edges[g];
    let [sa, sb] = faces.map(|k| &s.surfaces[s.faces[k].surface]);
    let curve = &s.curves[e.curve];
    let gap = edge_gap(curve, e.interval, sa).max(edge_gap(curve, e.interval, sb));
    if gap <= LINEAR_RESOLUTION {
        return None;
    }
    let reach = (2.0 * gap).max(10.0 * LINEAR_RESOLUTION).min(cap);
    let [t0, t1] = e.interval;
    let at = |i: usize| t0 + (t1 - t0) * i as f64 / SAMPLES as f64;
    let normal = |x: &Surface| {
        let (u, v) = x.project(curve.point(at(SAMPLES / 2))).param;
        x.normal(u, v)
    };
    if normal(sa)?.get().cross(normal(sb)?.get()).length() < CREASE {
        return None;
    }
    let samples: Vec<_> = (0..=SAMPLES).map(|i| curve.point(at(i))).collect();
    let start = s.vertices[e.start].point;
    let end = s.vertices[e.end].point;
    let region = crate::geom::Box3::from_points(samples.iter().copied().chain([start, end]))?
        .grow(reach + gap);
    let SurfaceHit::Meet { branches, .. } = surface_surface(sa, sb, &region).ok()? else {
        return None;
    };
    let mid = at(SAMPLES / 2);
    let along = curve.tangent(mid)?;
    branches
        .iter()
        .filter(|b| !b.tangent && b.tolerance < gap)
        .filter(|b| {
            samples
                .iter()
                .chain([&start, &end])
                .all(|&p| b.curve.distance(p) <= reach)
        })
        .find_map(|b| {
            let foot = |p| b.curve.project(p).param;
            let (p0, p1, pm) = (foot(start), foot(end), foot(curve.point(mid)));
            let cos = along.get().dot(b.curve.tangent(pm)?.get());
            if cos.abs() < ALIGNED {
                return None;
            }
            let forward = cos > 0.0;
            // From the foot the edge starts at to the one it ends at,
            // along the branch.
            let (from, to) = if forward { (p0, p1) } else { (p1, p0) };
            let interval = match b.curve.period() {
                Some(per) => {
                    let mut hi = from + (to - from).rem_euclid(per);
                    if e.start == e.end || hi - from <= LINEAR_RESOLUTION {
                        hi = from + per;
                    }
                    let m = from + (pm - from).rem_euclid(per);
                    if !(from < m && m < hi) {
                        return None;
                    }
                    [from, hi]
                }
                None if e.start == e.end => {
                    // A closed edge on a closed clamped branch (a spline's):
                    // the branch started at the foot of the edge's vertex.
                    let [lo, hi] = b.range;
                    let tol = b.tolerance.max(LINEAR_RESOLUTION);
                    if b.curve.point(lo).distance(b.curve.point(hi)) > tol {
                        return None;
                    }
                    let edge = (hi - lo) * 1e-6;
                    let b = if p0 > lo + edge && p0 < hi - edge {
                        let moved = crate::intersect::reseamed(b.clone(), p0);
                        if moved.range == b.range {
                            return None;
                        }
                        moved
                    } else {
                        b.clone()
                    };
                    return Some(Tight {
                        pcurves: identity(&b.pcurves),
                        curve: b.curve,
                        interval: b.range,
                        flip: !forward,
                        tolerance: b.tolerance,
                    });
                }
                None => {
                    if e.start == e.end || !(from < pm && pm < to) {
                        return None;
                    }
                    let [lo, hi] = b.range;
                    if from < lo || to > hi {
                        return None;
                    }
                    [from, to]
                }
            };
            Some(Tight {
                pcurves: identity(&b.pcurves),
                curve: b.curve.clone(),
                interval,
                flip: !forward,
                tolerance: b.tolerance,
            })
        })
}
