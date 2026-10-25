//! Gaps: how far each edge and vertex lies from the geometry it bounds,
//! measured, and tolerances to cover it.
//!
//! An edge's gap is the farthest its curve strays from the surface of
//! any face it bounds: certified from above on an analytic surface (its
//! Bézier pieces substituted into the surface's implicit form,
//! [`crate::intersect::curve_distance`]), and elsewhere the farthest of
//! 33 samples, which include the checker's own. A vertex's gap is the
//! farthest it lies from its edges' curves at their ends and from its
//! faces' surfaces, measured as the checker measures them. Each entity's
//! tolerance grows to cover its gap; a gap past the cap is
//! [`HealError::GapTooLarge`], naming the least-named entity past it,
//! with its gap.

use super::Loop;
use super::Shape;
use super::names::Names;
use crate::geom::{Curve, Surface};
use crate::intersect::curve_distance;
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::Sense;
use crate::{Error, HealError};
use rayon::prelude::*;
use std::collections::BTreeSet;

/// Samples along an edge for surfaces with no implicit form: every
/// checker sample among them.
const SAMPLES: usize = 32;

/// The farthest `curve` over `interval` strays from `surface`: certified
/// where the surface has an implicit form, sampled elsewhere.
pub(super) fn edge_gap(curve: &Curve, [t0, t1]: [f64; 2], surface: &Surface) -> f64 {
    let sampled = (0..=SAMPLES)
        .map(|i| {
            let t = t0 + (t1 - t0) * i as f64 / SAMPLES as f64;
            surface.distance(curve.point(t))
        })
        .fold(0.0, f64::max);
    // Halve until within a hundredth of what the samples saw, or well
    // within resolution: an exact edge needs no tolerance it didn't have.
    let target = (sampled * 1.01).max(LINEAR_RESOLUTION / 2.0);
    match curve_distance(curve, [t0, t1], surface, target) {
        Some(bound) => bound.max(sampled),
        None => sampled,
    }
}

/// Grows every edge's and vertex's tolerance in `s` to cover its gap.
pub(super) fn tolerate(s: &mut Shape, names: &Names, cap: f64) -> Result<(), Error> {
    // The faces each edge and vertex bounds.
    let mut edge_faces: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); s.edges.len()];
    let mut vertex_faces: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); s.vertices.len()];
    let mut vertex_ends: Vec<BTreeSet<(usize, bool)>> = vec![BTreeSet::new(); s.vertices.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            match l {
                Loop::Vertex(v) => {
                    vertex_faces[*v].insert(k);
                }
                Loop::Edges(uses) => {
                    for u in uses {
                        let e = &s.edges[u.edge];
                        edge_faces[u.edge].insert(k);
                        let (a, b) = match u.sense {
                            Sense::Forward => (e.start, e.end),
                            Sense::Reversed => (e.end, e.start),
                        };
                        vertex_faces[a].insert(k);
                        vertex_faces[b].insert(k);
                        vertex_ends[e.start].insert((u.edge, true));
                        vertex_ends[e.end].insert((u.edge, false));
                    }
                }
            }
        }
    }
    let edge_gaps: Vec<f64> = (0..s.edges.len())
        .into_par_iter()
        .map(|g| {
            let e = &s.edges[g];
            let curve = &s.curves[e.curve];
            edge_faces[g]
                .iter()
                .map(|&k| edge_gap(curve, e.interval, &s.surfaces[s.faces[k].surface]))
                .fold(0.0, f64::max)
        })
        .collect();
    let vertex_gaps: Vec<f64> = (0..s.vertices.len())
        .map(|v| {
            let p = s.vertices[v].point;
            let on_faces = vertex_faces[v]
                .iter()
                .map(|&k| s.surfaces[s.faces[k].surface].distance(p))
                .fold(0.0, f64::max);
            vertex_ends[v]
                .iter()
                .map(|&(g, start)| {
                    let e = &s.edges[g];
                    let t = if start { e.interval[0] } else { e.interval[1] };
                    s.curves[e.curve].point(t).distance(p)
                })
                .fold(on_faces, f64::max)
        })
        .collect();
    // As the builder covers a curve's end, with room for rounding.
    let cover = |gap: f64| gap * 1.0001 + 1e-12;
    let mut over: Vec<(Name, f64)> = Vec::new();
    for (g, &gap) in edge_gaps.iter().enumerate() {
        if gap > cap {
            over.push((names.edges[g], gap));
        }
        let e = &mut s.edges[g];
        e.tolerance = e.tolerance.max(cover(gap));
    }
    for (v, &gap) in vertex_gaps.iter().enumerate() {
        if gap > cap {
            over.push((names.vertices[v], gap));
        }
        let x = &mut s.vertices[v];
        x.tolerance = x.tolerance.max(cover(gap));
    }
    match over.into_iter().min_by_key(|x| x.0) {
        Some((entity, gap)) => Err(HealError::GapTooLarge { entity, gap }.into()),
        None => Ok(()),
    }
}
