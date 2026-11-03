//! What each vertex of a sheet's boundary becomes on one side of an
//! offset: the vertices where laminar edges meet.
//!
//! Round such a vertex the faces form an open fan, from the face of the
//! laminar edge arriving there to the face of the one leaving, joined by
//! inner edges. Two cases are covered:
//!
//! - **Moved**: every inner edge of the fan keeps its place or moves
//!   smoothly. The vertex moves along the faces' normal by their distance
//!   and stays one vertex, which both laminar edges' rails reach.
//! - **Sharp**: two faces across one inner edge the side opens or
//!   closes, whose laminar edges' walls lie on one plane (necessarily the
//!   plane square to the edge there). Opening, each face's rail ends at
//!   the vertex moved along that face's normal, and the edge's fill ends
//!   on the arc between them (its section, which lies on the wall);
//!   closing, the edge's curve is cut by the wall, and both rails end at
//!   that point.
//!
//! Anything else is `Unsupported` ([`OffsetCase::LaminarVertex`]).

use super::classify::{Class, Plan};
use super::edges::{EdgeGeo, NewCurve, moved_point, project_on};
use super::vertices::{
    Corner, FillEnd, MEET, Part, Step, VUse, VertexGeo, arc, end_param, hit, normal_out, one,
};
use super::walls::{Wall, same_plane};
use crate::geom::Surface;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{EdgeId, Sense, VertexId};
use crate::{Error, OffsetCase, OffsetError};
use std::collections::BTreeMap;

/// Every boundary vertex's new geometry, in parallel.
pub(crate) fn vertices(
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    rails: &BTreeMap<EdgeId, NewCurve>,
    walls: &BTreeMap<EdgeId, Wall>,
    meter: &crate::work::Meter,
) -> Result<BTreeMap<VertexId, VertexGeo>, Error> {
    let body = plan.body;
    let mut vs: Vec<VertexId> = body
        .vertices()
        .filter(|&v| body.vertex_edges(v).into_iter().any(|e| body.is_laminar(e)))
        .collect();
    vs.sort_by_key(|&v| body.vertices[v].name);
    meter
        .par_map(&vs, |meter, &v| {
            vertex(plan, geo, rails, walls, v, meter).map(|g| (v, g))
        })
        .into_iter()
        .collect()
}

/// The open fan round `v`: its corners, the steps between them (step `k`
/// from corner `k` to `k + 1`), and the laminar edges arriving at its
/// first corner and leaving its last, each with whether `v` is its end.
#[allow(clippy::type_complexity)]
fn fan(plan: &Plan, v: VertexId) -> Option<(Vec<Corner>, Vec<Step>, [(EdgeId, bool); 2])> {
    let body = plan.body;
    let lam = body
        .vertex_edges(v)
        .into_iter()
        .find(|&e| body.is_laminar(e))?;
    let f = body.edges[lam].fin;
    // The laminar fin ending at v: this one, or (a closed edge aside) the
    // laminar fin before it round its boundary.
    let start = if body.end_of(f) == v {
        f
    } else {
        let mut g = f;
        for _ in 0..=body.fins.slot_count() {
            let n = body.boundary_next(g);
            if n == f {
                break;
            }
            g = n;
        }
        if body.end_of(g) != v {
            return None;
        }
        g
    };
    let (fins, closed) = body.fan_from(start);
    if closed || body.twin(fins[0]).is_some() {
        return None;
    }
    let corners: Vec<Corner> = fins
        .iter()
        .map(|&fin| Corner {
            face: plan.face_of(fin),
            fin_in: fin,
            fin_out: body.fins[fin].next,
        })
        .collect();
    let steps: Vec<Step> = corners[..corners.len() - 1]
        .iter()
        .map(|c| {
            let e = body.fins[c.fin_out].edge;
            Step {
                edge: e,
                at_end: body.fins[c.fin_out].sense == Sense::Reversed,
                side: usize::from(plan.fins(e)[0] != c.fin_out),
            }
        })
        .collect();
    let fin_in = corners[0].fin_in;
    let fin_out = corners[corners.len() - 1].fin_out;
    if body.twin(fin_out).is_some() {
        return None;
    }
    let ends = [
        (
            body.fins[fin_in].edge,
            body.fins[fin_in].sense == Sense::Forward,
        ),
        (
            body.fins[fin_out].edge,
            body.fins[fin_out].sense == Sense::Reversed,
        ),
    ];
    Some((corners, steps, ends))
}

fn vertex(
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    rails: &BTreeMap<EdgeId, NewCurve>,
    walls: &BTreeMap<EdgeId, Wall>,
    v: VertexId,
    meter: &crate::work::Meter,
) -> Result<VertexGeo, Error> {
    let body = plan.body;
    let at = body.vertices[v].name;
    let refuse = || -> Error {
        OffsetError::Unsupported {
            at,
            case: OffsetCase::LaminarVertex,
        }
        .into()
    };
    let (corners, steps, ends) = fan(plan, v).ok_or_else(refuse)?;
    let p = body.vertices[v].point;
    let class = |s: &Step| plan.class[&s.edge].0;
    let lam = |out: &mut VertexGeo, k: usize, point: usize, t: Option<f64>| {
        let (e, at_end) = ends[k];
        let t = t.unwrap_or_else(|| end_param(plan, e, at_end));
        out.laminar.insert((e, at_end), (point, t));
    };
    // Moved: every inner edge kept or smooth.
    if steps
        .iter()
        .all(|s| matches!(class(s), Class::Kept | Class::Smooth))
    {
        let c = corners[0];
        let d = plan.d(c.face);
        let point = if d == 0.0 {
            p
        } else {
            p + normal_out(plan, c.fin_out) * d
        };
        let mut out = VertexGeo {
            kept: true,
            points: vec![point],
            ..VertexGeo::default()
        };
        for s in &steps {
            out.one
                .insert((s.edge, s.at_end), (0, end_param(plan, s.edge, s.at_end)));
        }
        lam(&mut out, 0, 0, None);
        lam(&mut out, 1, 0, None);
        return Ok(out);
    }
    // Sharp: two faces, one edge, the walls on one plane.
    if corners.len() != 2 {
        return Err(refuse());
    }
    let (wa, wb) = (&walls[&ends[0].0], &walls[&ends[1].0]);
    if !same_plane(wa, wb) {
        return Err(refuse());
    }
    let s = steps[0];
    let t = end_param(plan, s.edge, s.at_end);
    let n = [
        normal_out(plan, corners[0].fin_out),
        normal_out(plan, corners[1].fin_out),
    ];
    let d = [plan.d(corners[0].face), plan.d(corners[1].face)];
    match class(&s) {
        Class::Opening => {
            let points = vec![p + n[0] * d[0], p + n[1] * d[1]];
            let mut out = VertexGeo {
                points,
                ..VertexGeo::default()
            };
            let mut ends_ = [(0, t); 2];
            ends_[s.side] = (0, t);
            ends_[1 - s.side] = (1, t);
            out.open.insert((s.edge, s.at_end), ends_);
            lam(&mut out, 0, 0, None);
            lam(&mut out, 1, 1, None);
            let edge = arc(
                plan,
                s,
                ends_,
                (&out.points, p, d[0]),
                [Part::Fill(s.edge), Part::Wall(ends[0].0)],
            )
            .ok_or(OffsetError::Uncertain { at })?;
            out.edges.push(edge);
            out.fill_ends.insert(
                (s.edge, s.at_end),
                FillEnd::Edges(vec![VUse {
                    edge: 0,
                    from: ends_[0].0,
                }]),
            );
            Ok(out)
        }
        Class::Closing => {
            let c = one(geo, s.edge);
            let plane = Surface::Plane(*wa.plane().expect("a plane wall"));
            let estimate = moved_point(p, n, d);
            let (tc, x) =
                hit(c, &plane, estimate, at, meter)?.ok_or(OffsetError::Uncertain { at })?;
            let reach = d[0].abs().max(d[1].abs()).max(10.0 * LINEAR_RESOLUTION);
            if x.distance(estimate) > 2.0 * reach {
                return Err(OffsetError::EdgeVanishes {
                    edge: body.edges[s.edge].name,
                }
                .into());
            }
            let mut out = VertexGeo {
                points: vec![x],
                ..VertexGeo::default()
            };
            out.one.insert((s.edge, s.at_end), (0, tc));
            for k in 0..2 {
                let r = &rails[&ends[k].0];
                let (tr, q) = project_on(&r.curve, r.range, x);
                if q.distance(x) > MEET + r.tolerance + c.tolerance {
                    return Err(OffsetError::Uncertain { at }.into());
                }
                lam(&mut out, k, 0, Some(tr));
            }
            Ok(out)
        }
        Class::Kept | Class::Smooth => Err(refuse()),
    }
}
