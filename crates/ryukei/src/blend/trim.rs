//! Trims: two blends meeting at an angle where a third edge stays sharp.
//!
//! The blended edges share a face, and the sharp edge lies between their
//! other faces. Both blends run on until they meet, and each is trimmed
//! by the other along the curve where their surfaces cross: it starts
//! where their rails cross on the shared face and runs towards the
//! vertex until it leaves one of them across its rail on its other face.
//! That one, the narrow blend, ends there all along the trim curve. The
//! other, the wide blend, also ends on the narrow one's other face, in
//! an end curve from there to where its own rail crosses the sharp edge.
//! When both rails reach the sharp edge at one point (equal fillets on a
//! box) there is no end curve: the trim curve ends on the sharp edge.

use super::chain::{Chain, Class};
use super::chamfer;
use super::end::{EndCurve, EndFail, crossing, end_curve, on_edge};
use super::plan::{Partial, fillet_face, on_face};
use super::spec::Spec;
use super::spine::nearest;
use crate::geom::intersect2::{Meet2, crossings};
use crate::geom::{Box3, Point3, Surface};
use crate::intersect::{self, CurveHit, Failure, SurfaceHit, shifted, shifts};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, VertexId};
use crate::{BlendError, Error};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// A chain's end at a trim.
#[derive(Clone, Debug)]
pub(crate) struct TrimEnd {
    pub(crate) chain: usize,
    /// Whether the trim is at the chain's first vertex, else its last.
    pub(crate) first: bool,
    pub(crate) spec: usize,
    /// The carrier's parameters where the rails end: on the shared face,
    /// and on the other face.
    pub(crate) ends: [f64; 2],
    /// The other face.
    pub(crate) face: FaceId,
}

/// Two blends trimmed against each other.
#[derive(Clone, Debug)]
pub(crate) struct Trim {
    pub(crate) vertex: VertexId,
    pub(crate) shared: FaceId,
    pub(crate) sharp: EdgeId,
    /// The narrow blend and the wide one.
    pub(crate) ends: [TrimEnd; 2],
    /// Where the rails cross on the shared face.
    pub(crate) q: Point3,
    /// Where the trim curve leaves the narrow blend; on the sharp edge
    /// when there is no end curve.
    pub(crate) p: Point3,
    /// The trim curve from `q` to `p`, its pcurves on the narrow blend's
    /// surface and the wide one's.
    pub(crate) curve: EndCurve,
    /// The wide blend's end curve on the narrow one's other face, from
    /// `p` to `g`, where its rail crosses the sharp edge, its pcurves on
    /// the wide blend's surface and that face's.
    pub(crate) end: Option<(EndCurve, Point3)>,
    /// The sharp edge's parameter where it now ends: at `g`, or at `p`.
    pub(crate) at: f64,
}

impl Trim {
    /// Where the sharp edge now ends.
    pub(crate) fn g(&self) -> Point3 {
        self.end.as_ref().map_or(self.p, |(_, g)| *g)
    }
}

/// Every trim of the chains, in order of their vertices' names.
pub(crate) fn trims(
    body: &Body,
    specs: &[Spec],
    chains: &[Chain],
    partials: &[Vec<Partial>],
) -> Result<Vec<Trim>, Error> {
    let mut at: BTreeMap<VertexId, Vec<(usize, bool)>> = BTreeMap::new();
    for (ci, c) in chains.iter().enumerate() {
        for (vi, (&v, class)) in c.vertices.iter().zip(&c.classes).enumerate() {
            if matches!(class, Class::Trim) {
                at.entry(v).or_default().push((ci, vi == 0));
            }
        }
    }
    let mut list: Vec<(VertexId, Vec<(usize, bool)>)> = at.into_iter().collect();
    list.sort_by_key(|(v, _)| body.vertices[*v].name);
    list.par_iter()
        .map(|(v, ends)| trim(body, specs, chains, partials, *v, ends))
        .collect()
}

/// The surface of a chain's end segment, near its end.
fn surface_of(
    body: &Body,
    specs: &[Spec],
    chain: &Chain,
    part: &Partial,
) -> Result<Surface, Error> {
    let name = specs[chain.edges[part.edges[0]].0].name;
    let (s, _) = match &part.spine {
        Some(spine) => fillet_face(body, part, spine, part.span, name)?,
        None => chamfer::face(body, specs, chain, part, name)?,
    };
    Ok(s)
}

/// The trim at `v`, between the chain ends there.
fn trim(
    body: &Body,
    specs: &[Spec],
    chains: &[Chain],
    partials: &[Vec<Partial>],
    v: VertexId,
    ends: &[(usize, bool)],
) -> Result<Trim, Error> {
    let at = body.vertices[v].name;
    let uncertain = || -> Error { BlendError::Uncertain { at }.into() };
    let unsupported = || -> Error { BlendError::Unsupported { at }.into() };
    let [(ca, fa), (cb, fb)] = ends[..] else {
        return Err(unsupported());
    };
    let part = |ci: usize, first: bool| -> &Partial {
        let p = &partials[ci];
        if first { &p[0] } else { &p[p.len() - 1] }
    };
    let spec_of = |ci: usize, first: bool| -> usize {
        let c = &chains[ci];
        c.edges[if first { 0 } else { c.edges.len() - 1 }].0
    };
    let parts = [part(ca, fa), part(cb, fb)];
    let ks = [spec_of(ca, fa), spec_of(cb, fb)];
    let faces = ks.map(|k| specs[k].faces);
    let shared = faces[0]
        .iter()
        .copied()
        .find(|f| faces[1].contains(f))
        .ok_or_else(unsupported)?;
    let other = faces.map(|f| f[usize::from(f[0] == shared)]);
    let sharp = body
        .vertex_edges(v)
        .into_iter()
        .find(|&e| specs.iter().all(|s| s.edge != e))
        .ok_or_else(unsupported)?;
    let surfaces = [
        surface_of(body, specs, &chains[ca], parts[0])?,
        surface_of(body, specs, &chains[cb], parts[1])?,
    ];
    let vp = body.vertices[v].point;
    // The side of each part's rails on the shared face and on the other.
    let side = |p: &Partial, f: FaceId| usize::from(p.faces[1] == f);
    let ss = [side(parts[0], shared), side(parts[1], shared)];
    let xs = [side(parts[0], other[0]), side(parts[1], other[1])];
    let to_t = |p: &Partial, s: usize, tau: f64| (tau - p.rails[s].map[0]) / p.rails[s].map[1];
    let range = |p: &Partial, s: usize| p.span.map(|t| p.rails[s].param(t));
    // Where the rails cross on the shared face, nearest the vertex.
    let q_tau = rails_cross(body, shared, parts, ss, vp).ok_or_else(uncertain)?;
    let tq = [
        to_t(parts[0], ss[0], q_tau[0]),
        to_t(parts[1], ss[1], q_tau[1]),
    ];
    let q = parts[0].rails[ss[0]].curve.point(q_tau[0]);
    // The trim curve: the branch of the surfaces' intersection through q.
    let reach = parts[0].reach.max(parts[1].reach);
    let region = Box3::new(q, vp).grow(2.0 * reach);
    let hit = match (&surfaces[0], &surfaces[1]) {
        (Surface::Pipe(_), Surface::Pipe(_)) => return Err(unsupported()),
        (Surface::Pipe(_), _) | (_, Surface::Pipe(_)) => {
            intersect::pipe_surface(&surfaces[0], &surfaces[1], &region)
        }
        _ => intersect::surface_surface(&surfaces[0], &surfaces[1], &region),
    };
    let branches = match hit {
        Ok(SurfaceHit::Meet { branches, .. }) => branches,
        Ok(SurfaceHit::Coincident { .. }) | Err(Failure::Unsupported) => {
            return Err(unsupported());
        }
        Err(Failure::Uncertain) => return Err(uncertain()),
    };
    // Both surfaces touch the shared face where their rails cross, so two
    // branches cross there (or one passes twice): the trim curve leaves
    // q along the one heading most nearly towards the vertex.
    let toward = (vp - q).normalize().ok_or_else(uncertain)?.get();
    let mut best: Option<(f64, usize, f64, f64)> = None;
    for (i, b) in branches.iter().enumerate() {
        let near = 10.0 * b.tolerance.max(LINEAR_RESOLUTION);
        // The ways out of q along the branch: from a parameter at q, the
        // way the parameter runs.
        let mut ways: Vec<(f64, f64)> = Vec::new();
        if b.curve.period().is_some() {
            let (t, d) = nearest(&b.curve, b.range, q);
            if d <= near {
                ways.extend([(t, 1.0), (t, -1.0)]);
            }
        } else {
            let [t0, t1] = b.range;
            let (t, d) = nearest(&b.curve, b.range, q);
            if b.curve.point(t0).distance(q) <= near {
                ways.push((t0, 1.0));
            }
            if b.curve.point(t1).distance(q) <= near {
                ways.push((t1, -1.0));
            }
            if ways.is_empty() && d <= near {
                ways.extend([(t, 1.0), (t, -1.0)]);
            }
        }
        for (t, dir) in ways {
            let Some(along) = b.curve.eval(t).d1.normalize() else {
                continue;
            };
            let c = along.get().dot(toward) * dir;
            if best.is_none_or(|x| c > x.0) {
                best = Some((c, i, t, dir));
            }
        }
    }
    let (_, bi, t_q, dir) = best.ok_or_else(uncertain)?;
    let branch = &branches[bi];
    let tol = branch.tolerance.max(LINEAR_RESOLUTION);
    let travel = |p: Point3| -> Option<f64> {
        let mut t = branch.curve.project(p).param;
        if let Some(period) = branch.curve.period() {
            // The first time the branch reaches it going `dir` from q.
            let k = ((t_q - t) * dir / period).ceil();
            t += k * period * dir;
        }
        let d = (t - t_q) * dir;
        (branch.curve.point(t).distance(p) <= 10.0 * tol && d > 0.0).then_some(d)
    };
    // Where each blend's rail on its other face meets the other blend:
    // the first such point along the branch.
    let mut first: [Option<(f64, f64, Point3)>; 2] = [None, None];
    for i in 0..2 {
        let (p, s) = (parts[i], xs[i]);
        let rail = &p.rails[s];
        let r = range(p, s);
        let r = [r[0].min(r[1]), r[0].max(r[1])];
        let o = &surfaces[1 - i];
        let t = rail.tolerance.max(tol);
        let hits = match o {
            Surface::Pipe(_) => intersect::curve_pipe(&rail.curve, r, o, t),
            _ => intersect::curve_surface(&rail.curve, r, o, t),
        }
        .map_err(|f| match f {
            Failure::Unsupported => unsupported(),
            Failure::Uncertain => uncertain(),
        })?;
        for h in hits {
            let CurveHit::Point { t: tau, point, .. } = h else {
                continue;
            };
            if let Some(d) = travel(point)
                && first[i].is_none_or(|x| d < x.0)
            {
                first[i] = Some((d, tau, point));
            }
        }
    }
    let close = |a: Point3, b: Point3| a.distance(b) <= 10.0 * tol;
    let (n, equal) = match first {
        [Some(a), Some(b)] if close(a.2, b.2) => (0, true),
        [Some(a), Some(b)] => (usize::from(b.0 < a.0), false),
        [Some(_), None] => (0, false),
        [None, Some(_)] => (1, false),
        [None, None] => return Err(uncertain()),
    };
    let w = 1 - n;
    let (_, tau_p, p) = first[n].expect("the narrow blend's exit");
    let sharp_e = &body.edges[sharp];
    let sharp_c = &body.curves[sharp_e.curve];
    // The wide blend's rail on its other face crosses the sharp edge.
    let xw = {
        let pw = parts[w];
        let r = range(pw, xs[w]);
        let surface = &body.surfaces[body.faces[other[n]].surface];
        crossing(body, &pw.rails[xs[w]], r, surface, sharp, vp).map_err(|e| match e {
            EndFail::Overflow => BlendError::Overflow {
                edge: specs[ks[w]].name,
                face: body.faces[other[w]].name,
            }
            .into(),
            EndFail::Unsupported => unsupported(),
            EndFail::Uncertain => uncertain(),
        })?
    };
    let (end, sharp_at) = if equal {
        let reach = sharp_e.tolerance.get() + 10.0 * tol;
        let a = on_edge(sharp_c, sharp_e.interval, p, reach).ok_or_else(uncertain)?;
        (None, a)
    } else {
        let face = &body.surfaces[body.faces[other[n]].surface];
        let e = end_curve(&surfaces[w], face, p, xw.point, vp, reach, &|m| {
            on_face(parts[w], m)
        })
        .map_err(|f| match f {
            EndFail::Unsupported => unsupported(),
            _ => uncertain(),
        })?;
        (Some((e, xw.point)), xw.at)
    };
    // The trim curve from q to p.
    let mut tp = branch.curve.project(p).param;
    if let Some(period) = branch.curve.period() {
        let k = ((t_q - tp) * dir / period).ceil();
        tp += k * period * dir;
    }
    let pcurves = if n == 0 {
        branch.pcurves.clone()
    } else {
        let [a, b] = branch.pcurves.clone();
        [b, a]
    };
    let mut curve = EndCurve {
        curve: branch.curve.clone(),
        interval: [t_q, tp],
        pcurves,
        tolerance: tol,
    };
    if tp < t_q {
        curve = curve.reversed();
    }
    let curve = curve
        .covered([&surfaces[n], &surfaces[w]])
        .map_err(|_| uncertain())?;
    let x_end = |i: usize| -> f64 {
        if i == n {
            to_t(parts[n], xs[n], tau_p)
        } else {
            to_t(parts[w], xs[w], xw.tau)
        }
    };
    let make = |i: usize| TrimEnd {
        chain: [ca, cb][i],
        first: [fa, fb][i],
        spec: ks[i],
        ends: [tq[i], x_end(i)],
        face: other[i],
    };
    Ok(Trim {
        vertex: v,
        shared,
        sharp,
        ends: [make(n), make(w)],
        q,
        p,
        curve,
        end,
        at: sharp_at,
    })
}

/// The rails' parameters where they cross on `face`, nearest `near`.
pub(crate) fn rails_cross(
    body: &Body,
    face: FaceId,
    parts: [&Partial; 2],
    sides: [usize; 2],
    near: Point3,
) -> Option<[f64; 2]> {
    let surface = &body.surfaces[body.faces[face].surface];
    let (pu, pv) = surface.periods();
    let c = [0, 1].map(|i| {
        let rl = &parts[i].rails[sides[i]];
        let (pc, m) = &rl.pcurve;
        let r = parts[i].span.map(|t| m[0] + m[1] * rl.param(t));
        (pc, r, *m, rl.tolerance)
    });
    let tol = c[0].3.max(c[1].3);
    let mut best: Option<(f64, [f64; 2])> = None;
    for shift in shifts(c[0].0, c[0].1, c[1].0, c[1].1, [pu, pv]) {
        let moved = shifted(c[1].0, shift)?;
        for m in crossings((c[0].0, c[0].1), (&moved, c[1].1), tol)? {
            let Meet2::Point { ta, tb, .. } = m else {
                return None;
            };
            let tau = [(ta - c[0].2[0]) / c[0].2[1], (tb - c[1].2[0]) / c[1].2[1]];
            let p = parts[0].rails[sides[0]].curve.point(tau[0]);
            let d = p.distance(near);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, tau));
            }
        }
    }
    best.map(|b| b.1)
}
