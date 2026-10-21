//! Mitres: three chamfers meeting at a vertex of three edges.
//!
//! Each pair of chamfers shares a face, where their rails cross; from
//! there the two chamfers meet along the curve where their surfaces
//! cross, the mitre, up to the point all three surfaces share. Each
//! chamfer ends on its two mitres. That point must lie on all three
//! chamfers between their rails, else one chamfer would run across the
//! corner past the others: that is refused.

use super::chain::{Chain, Class};
use super::chamfer;
use super::end::EndCurve;
use super::plan::Partial;
use super::spec::Spec;
use super::spine::{nearest, solve3};
use super::trim::rails_cross;
use crate::geom::{Box3, Point3, Vector3};
use crate::intersect::{self, Failure, SurfaceHit};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId, VertexId};
use crate::{BlendError, Error};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// A chain's end at a mitre.
#[derive(Clone, Debug)]
pub(crate) struct MitreEnd {
    pub(crate) chain: usize,
    pub(crate) first: bool,
    pub(crate) spec: usize,
    /// The carrier's parameter where the rail on each face ends.
    pub(crate) at: [(FaceId, f64); 2],
}

/// Three chamfers mitred at a vertex.
#[derive(Clone, Debug)]
pub(crate) struct Mitre {
    pub(crate) vertex: VertexId,
    /// The faces there, in name order.
    pub(crate) faces: [FaceId; 3],
    /// Where the rails on each face cross.
    pub(crate) q: [Point3; 3],
    /// The point on all three chamfers.
    pub(crate) point: Point3,
    pub(crate) ends: Vec<MitreEnd>,
    /// From each `q` to `point`, the mitre between the two chamfers
    /// lying on that face (indices into `ends`), with its pcurves on
    /// their surfaces in that order.
    pub(crate) curves: Vec<(EndCurve, [usize; 2])>,
}

impl MitreEnd {
    /// The carrier's parameter where its rail on `face` ends.
    pub(crate) fn param(&self, face: FaceId) -> f64 {
        self.at.iter().find(|(f, _)| *f == face).expect("a face").1
    }
}

/// Every mitre of the chains, in order of their vertices' names.
pub(crate) fn mitres(
    body: &Body,
    specs: &[Spec],
    chains: &[Chain],
    partials: &[Vec<Partial>],
) -> Result<Vec<Mitre>, Error> {
    let mut at: BTreeMap<VertexId, Vec<(usize, bool)>> = BTreeMap::new();
    for (ci, c) in chains.iter().enumerate() {
        for (vi, (&v, class)) in c.vertices.iter().zip(&c.classes).enumerate() {
            if matches!(class, Class::Mitre) {
                at.entry(v).or_default().push((ci, vi == 0));
            }
        }
    }
    let mut list: Vec<(VertexId, Vec<(usize, bool)>)> = at.into_iter().collect();
    list.sort_by_key(|(v, _)| body.vertices[*v].name);
    list.par_iter()
        .map(|(v, ends)| mitre(body, specs, chains, partials, *v, ends))
        .collect()
}

/// The mitre at `v`, between the chain ends there.
fn mitre(
    body: &Body,
    specs: &[Spec],
    chains: &[Chain],
    partials: &[Vec<Partial>],
    v: VertexId,
    ends: &[(usize, bool)],
) -> Result<Mitre, Error> {
    let at = body.vertices[v].name;
    let uncertain = || -> Error { BlendError::Uncertain { at }.into() };
    let unsupported = || -> Error { BlendError::Unsupported { at }.into() };
    if ends.len() != 3 {
        return Err(unsupported());
    }
    let parts: Vec<&Partial> = ends
        .iter()
        .map(|&(ci, first)| {
            let p = &partials[ci];
            if first { &p[0] } else { &p[p.len() - 1] }
        })
        .collect();
    let ks: Vec<usize> = ends
        .iter()
        .map(|&(ci, first)| {
            let c = &chains[ci];
            c.edges[if first { 0 } else { c.edges.len() - 1 }].0
        })
        .collect();
    let mut faces: Vec<FaceId> = ks.iter().flat_map(|&k| specs[k].faces).collect();
    faces.sort_by_key(|&f| body.faces[f].name);
    faces.dedup();
    let [fa, fb, fc] = faces[..] else {
        return Err(unsupported());
    };
    let faces = [fa, fb, fc];
    let mut surfaces = Vec::with_capacity(3);
    for (i, &(ci, _)) in ends.iter().enumerate() {
        let name = specs[ks[i]].name;
        let (s, _) = chamfer::face(body, specs, &chains[ci], parts[i], name)?;
        surfaces.push(s);
    }
    let vp = body.vertices[v].point;
    // The two ends on each face, and where their rails cross.
    let mut q = [Point3::ORIGIN; 3];
    let mut pairs = [[0usize; 2]; 3];
    let mut params: Vec<BTreeMap<FaceId, f64>> = vec![BTreeMap::new(); 3];
    for (x, &face) in faces.iter().enumerate() {
        let on: Vec<usize> = (0..3)
            .filter(|&i| specs[ks[i]].faces.contains(&face))
            .collect();
        let [i, j] = on[..] else {
            return Err(unsupported());
        };
        let side = |p: &Partial| usize::from(p.faces[1] == face);
        let (si, sj) = (side(parts[i]), side(parts[j]));
        let tau =
            rails_cross(body, face, [parts[i], parts[j]], [si, sj], vp).ok_or_else(uncertain)?;
        let to_t = |p: &Partial, s: usize, t: f64| (t - p.rails[s].map[0]) / p.rails[s].map[1];
        params[i].insert(face, to_t(parts[i], si, tau[0]));
        params[j].insert(face, to_t(parts[j], sj, tau[1]));
        q[x] = parts[i].rails[si].curve.point(tau[0]);
        pairs[x] = [i, j];
    }
    // The point on all three chamfers, by Newton's method on their
    // tangent planes from the middle of the crossings.
    let mut c = Point3::new(
        (q[0].x + q[1].x + q[2].x) / 3.0,
        (q[0].y + q[1].y + q[2].y) / 3.0,
        (q[0].z + q[1].z + q[2].z) / 3.0,
    );
    let scale = vp.to_vector().length().max(1.0);
    for _ in 0..50 {
        let mut rows = [Vector3::ZERO; 3];
        let mut rhs = [0.0; 3];
        for (k, s) in surfaces.iter().enumerate() {
            let pr = s.project(c);
            let (u, w) = pr.param;
            let m = s.normal(u, w).ok_or_else(uncertain)?.get();
            rows[k] = m;
            rhs[k] = m.dot(pr.point.to_vector());
        }
        let next = solve3(rows, rhs).ok_or_else(unsupported)?;
        let step = next.distance(c);
        c = next;
        if step <= 1e-15 * scale {
            break;
        }
    }
    if surfaces.iter().any(|s| s.distance(c) > LINEAR_RESOLUTION) {
        return Err(uncertain());
    }
    // The point must lie on each chamfer between its rails.
    for p in &parts {
        let (t, _) = nearest(&p.carrier, p.span, c);
        let a = p.rails[0].curve.point(p.rails[0].param(t));
        let b = p.rails[1].curve.point(p.rails[1].param(t));
        let ab = b - a;
        let s = (c - a).dot(ab) / ab.length_squared();
        if !(0.0..=1.0).contains(&s) {
            return Err(unsupported());
        }
    }
    // The mitres, from each crossing to the point.
    let reach = parts.iter().map(|p| p.reach).fold(0.0, f64::max);
    let mut curves = Vec::with_capacity(3);
    for x in 0..3 {
        let [i, j] = pairs[x];
        let region = Box3::new(q[x], c).grow(reach);
        let hit = intersect::surface_surface(&surfaces[i], &surfaces[j], &region);
        let branches = match hit {
            Ok(SurfaceHit::Meet { branches, .. }) => branches,
            Ok(SurfaceHit::Coincident { .. }) | Err(Failure::Unsupported) => {
                return Err(unsupported());
            }
            Err(Failure::Uncertain) => return Err(uncertain()),
        };
        let mut found = None;
        for b in branches {
            let tol = 10.0 * b.tolerance.max(LINEAR_RESOLUTION);
            let (t0, d0) = nearest(&b.curve, b.range, q[x]);
            let (t1, d1) = nearest(&b.curve, b.range, c);
            if d0 <= tol && d1 <= tol {
                let mut e = EndCurve {
                    curve: b.curve,
                    interval: [t0, t1],
                    pcurves: b.pcurves,
                    tolerance: b.tolerance.max(LINEAR_RESOLUTION),
                };
                if t1 < t0 {
                    e = e.reversed();
                }
                found = Some(
                    e.covered([&surfaces[i], &surfaces[j]])
                        .map_err(|_| uncertain())?,
                );
                break;
            }
        }
        curves.push((found.ok_or_else(uncertain)?, [i, j]));
    }
    let out_ends = ends
        .iter()
        .enumerate()
        .map(|(i, &(ci, first))| {
            let m: Vec<(FaceId, f64)> = params[i].iter().map(|(&f, &t)| (f, t)).collect();
            MitreEnd {
                chain: ci,
                first,
                spec: ks[i],
                at: [m[0], m[1]],
            }
        })
        .collect();
    Ok(Mitre {
        vertex: v,
        faces,
        q,
        point: c,
        ends: out_ends,
        curves,
    })
}
