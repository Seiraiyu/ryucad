//! The rebuild: each chain's plan applied to the body through Euler
//! operators, then its geometry, pcurves and tolerances.
//!
//! A blended edge is not removed: it becomes the blend's rail on its
//! other face, and only the rail on the chain's rail face is new. So an
//! open chain takes a split of the rail face's cut at each end (SEMV), and
//! a path across the rail face along the rails (MEV between segments, MEF
//! to close): the face it cuts off, which holds the blended edges, is the
//! blend face. The blended edges' vertices move along the other faces'
//! cuts and smooth edges, the stretches of the rail face's cuts beside
//! them become the end curves, and at each smooth join an edge along the
//! ball's section (MEF, or MEKR on a closed chain's strip with two loops)
//! splits the blend face between segments.
//!
//! A closed rail path goes round the blended edges' loop on the rail
//! face. Where that face has no other loop, a spur from the loop (MEV) to
//! the path's first vertex lets MEF close the path into a new face: the
//! rail face beyond the path. Otherwise a spoke to another loop of the
//! face (MEKR), split where the path starts (SEMV), lets MEF close it so
//! that the strip keeps the blended edges and the rest keeps the other
//! loops; a single MEF can't move a loop without splitting the shell. The
//! spur or spoke is removed again (KEMR). Which way the path runs decides
//! which side MEF's new face is on: with the rail face's fins on the
//! blended edges for a spur, against them for a spoke.
//!
//! Edges at plain joins, on one rail curve, are joined again (JEKV) where
//! they run the same way.

use super::chain::Chain;
use super::end::EndCurve;
use super::plan::{Joint, Plan, Segment};
use super::spec::Spec;
use crate::euler::{self, Corner};
use crate::geom::{Curve, Curve2, Line, Surface};
use crate::name::{BlendRole, Name, OpKey, Role};
use crate::tol::{LINEAR_RESOLUTION, Tolerance};
use crate::topo::{
    Body, CurveId, EdgeId, EntityId, FaceId, FinId, LoopId, Pcurve, Sense, VertexId,
};
use crate::{Error, TopologyError};
use std::collections::{BTreeMap, BTreeSet};

/// What the rebuild made, for naming.
#[derive(Clone, Debug, Default)]
pub(crate) struct Made {
    pub(crate) segments: Vec<SegmentMade>,
    /// End edges: the edge, its segment, the end face and the name of the
    /// vertex the blend took away there.
    pub(crate) ends: Vec<(EdgeId, usize, FaceId, Name)>,
    /// Sections: the edge and the segments either side.
    pub(crate) sections: Vec<(EdgeId, [usize; 2])>,
    /// Vertices the blend took away, by their names, and the segments at
    /// them.
    pub(crate) replaced: Vec<(Name, Vec<usize>)>,
    /// Edges an operator renamed that keep their identity, with their
    /// names before the blend.
    pub(crate) keep: Vec<(EntityId, Name)>,
    /// Faces and edges whose geometry or boundary changed.
    pub(crate) modified: BTreeSet<EntityId>,
}

/// One blend face.
#[derive(Clone, Debug)]
pub(crate) struct SegmentMade {
    pub(crate) face: FaceId,
    /// The blended edges it replaces.
    pub(crate) edges: Vec<Name>,
    /// Its rails on the rail face (one) and on its other face (one per
    /// blended edge, fewer once joined).
    pub(crate) rails: [Vec<EdgeId>; 2],
    /// The names of the rail face and the other face before the blend.
    pub(crate) faces: [Name; 2],
}

/// Keys for the operators, one each, from the blend's key.
struct Keys {
    base: OpKey,
    n: u128,
}

impl Keys {
    fn next(&mut self) -> OpKey {
        self.n += 1;
        OpKey(
            Name::derive(
                self.base,
                Role::Blend(BlendRole::Vertex),
                &[Name::from_u128(u128::MAX - self.n)],
            )
            .to_u128(),
        )
    }
}

/// The plan and the topology disagree: a kernel bug, reported rather than
/// handed on.
fn inconsistent() -> Error {
    Error::Topology(TopologyError::Inconsistent)
}

fn face_of(body: &Body, fin: FinId) -> FaceId {
    body.loops[body.fins[fin].loop_].face
}

/// The fin of loop `l` that starts at `v`.
fn starting(body: &Body, l: LoopId, v: VertexId) -> Result<FinId, Error> {
    body.loop_fins(l)
        .find(|&f| body.start_of(f) == v)
        .ok_or_else(inconsistent)
}

/// The fin of one of `face`'s loops that starts at `v`.
fn starting_in(body: &Body, face: FaceId, v: VertexId) -> Result<FinId, Error> {
    body.face_loops(face)
        .find_map(|l| starting(body, l, v).ok())
        .ok_or_else(inconsistent)
}

/// The edge beside fin `f` at its end at `v`: the one before it in its
/// loop if it starts at `v`, after it if it ends there.
fn beside(body: &Body, f: FinId, v: VertexId) -> EdgeId {
    let fin = &body.fins[f];
    if body.start_of(f) == v {
        body.fins[fin.prev].edge
    } else {
        body.fins[fin.next].edge
    }
}

/// The fin of `edge` with `sense`.
fn fin_with(body: &Body, edge: EdgeId, sense: Sense) -> Result<FinId, Error> {
    body.edge_fins(edge)
        .find(|&f| body.fins[f].sense == sense)
        .ok_or_else(inconsistent)
}

/// Moves the end of `edge` at `v` to the parameter `at` of its curve.
fn move_end(body: &mut Body, edge: EdgeId, v: VertexId, at: f64) -> Result<(), Error> {
    let e = &body.edges[edge];
    let interval = if e.start == v {
        [at, e.interval[1]]
    } else {
        [e.interval[0], at]
    };
    euler::set_curve(body, edge, e.curve, interval)?;
    Ok(())
}

/// A curve and interval run one way or the other, and the affine map
/// `(c, s)` that takes the new parameter back to the old: `t = c + s t'`.
fn oriented(curve: &Curve, [a, b]: [f64; 2], along: bool) -> (Curve, [f64; 2], [f64; 2]) {
    if along {
        (curve.clone(), [a, b], [0.0, 1.0])
    } else {
        let (c, k) = curve.reversed();
        (c, [k - b, k - a], [k, -1.0])
    }
}

/// A pcurve map composed with `t = c + s t'`.
fn compose(m: [f64; 2], [c, s]: [f64; 2]) -> [f64; 2] {
    [m[0] + m[1] * c, m[1] * s]
}

/// The map from an edge's parameter to the spine's, for an edge on a
/// rail whose parameter is `rail[0] + rail[1] t` at the spine's `t`, run
/// so that the rail's parameter is `c + s t'` at the edge's `t'`.
fn spine_map(rail: [f64; 2], [c, s]: [f64; 2]) -> [f64; 2] {
    [(c - rail[0]) / rail[1], s / rail[1]]
}

/// The parameters on a pipe of a point in its section at `s`.
fn pipe_uv(pipe: &crate::geom::Pipe, s: f64, p: crate::geom::Point3) -> crate::geom::Point2 {
    let c = pipe.spine().point(s);
    let r = pipe.radius();
    let (e1, e2) = (
        (pipe.point(s, 0.0) - c) / r,
        (pipe.point(s, core::f64::consts::FRAC_PI_2) - c) / r,
    );
    let d = p - c;
    crate::geom::Point2::new(s, crate::math::atan2(d.dot(e2), d.dot(e1)))
}

/// The plain-join pieces of a segment's other rail: one interval per
/// edge, in chain order.
fn pieces(seg: &Segment) -> Vec<[f64; 2]> {
    let [a, b] = seg.intervals[1];
    let mut cuts = vec![a];
    cuts.extend(&seg.joins);
    cuts.push(b);
    cuts.windows(2).map(|w| [w[0], w[1]]).collect()
}

/// Pcurves and tolerances to settle once every chain is built.
#[derive(Default)]
struct Work {
    /// Fins with a known pcurve.
    pcurves: Vec<(FinId, Curve2, [f64; 2])>,
    /// Edges with a tolerance known from their geometry.
    tolerances: BTreeMap<EdgeId, f64>,
    /// Rails on rail faces, whose fin off the blend face gets the plan's
    /// pcurve: the edge, the pcurve and its map.
    rail_face: Vec<(EdgeId, Curve2, [f64; 2])>,
    /// Rails on rail faces whose pcurve on the blend surface is known.
    blend_side: Vec<(EdgeId, Curve2, [f64; 2])>,
    /// Vertices whose tolerance must be settled.
    vertices: BTreeSet<VertexId>,
    /// Faces the blend made, which get every missing pcurve.
    blend_faces: BTreeSet<FaceId>,
    /// Rails along a spine, with the map from the edge's parameter to the
    /// spine's (`t = a + b t'`), which is a pipe's own parameter.
    spines: BTreeMap<EdgeId, [f64; 2]>,
}

/// Applies every chain's plan to `body`.
pub(crate) fn build(
    body: &mut Body,
    key: OpKey,
    specs: &[Spec],
    chains: &[Chain],
    plans: &[Plan],
) -> Result<Made, Error> {
    let mut keys = Keys { base: key, n: 0 };
    let mut made = Made::default();
    let mut work = Work::default();
    // Faces keep their names: chains split and merge them on the way.
    let names: BTreeMap<FaceId, Name> = body.faces().map(|f| (f, body.faces[f].name)).collect();
    for (chain, plan) in chains.iter().zip(plans) {
        chain_build(
            body, &mut keys, specs, chain, plan, &names, &mut made, &mut work,
        )?;
    }
    finish(body, &work)?;
    for (&f, &n) in &names {
        if body.face(f).is_some() && body.faces[f].name != n && !work.blend_faces.contains(&f) {
            made.keep.push((EntityId::Face(f), n));
        }
    }
    for s in &mut made.segments {
        s.rails[1].retain(|&e| body.edge(e).is_some());
    }
    Ok(made)
}

/// Builds one chain; `names` are the faces' names before the blend.
#[allow(clippy::too_many_arguments)]
fn chain_build(
    body: &mut Body,
    keys: &mut Keys,
    specs: &[Spec],
    chain: &Chain,
    plan: &Plan,
    names: &BTreeMap<FaceId, Name>,
    made: &mut Made,
    work: &mut Work,
) -> Result<(), Error> {
    let n = chain.edges.len();
    let first_seg = made.segments.len();
    // Each chain edge's fins on the rail face and on its other face.
    let fins: Vec<[FinId; 2]> = (0..n)
        .map(|p| {
            let (k, _) = chain.edges[p];
            let side = chain.rail_side(specs, k);
            [specs[k].fins[side], specs[k].fins[1 - side]]
        })
        .collect();
    // The faces' names before the blend, by the faces the edges were
    // resolved on.
    let rail_name = names[&chain.rail_face];
    let other_names: Vec<Name> = plan
        .segments
        .iter()
        .map(|s| {
            let k = chain.edges[s.edges[0]].0;
            names[&specs[k].faces[1 - chain.rail_side(specs, k)]]
        })
        .collect();
    // Ends: split the rail face's cut where its rail crosses it.
    let mut end_edges: Vec<Option<(EdgeId, VertexId)>> = vec![None; chain.vertices.len()];
    for (vi, joint) in plan.joints.iter().enumerate() {
        let Joint::End { crossings, .. } = joint else {
            continue;
        };
        let v = chain.vertices[vi];
        let p = if vi == 0 { 0 } else { n - 1 };
        let cut = beside(body, fins[p][0], v);
        // An edge cut at both ends keeps the name it had before the blend.
        let old = made
            .keep
            .iter()
            .find(|(x, _)| *x == EntityId::Edge(cut))
            .map_or(body.edges[cut].name, |&(_, n)| n);
        let starts_here = body.edges[cut].start == v;
        let x = crossings[0];
        let (s, _) = euler::semv(body, keys.next(), cut, x.at, x.point)?;
        let (near, far) = if starts_here {
            (cut, s.edge)
        } else {
            (s.edge, cut)
        };
        made.keep.push((EntityId::Edge(far), old));
        made.modified.insert(EntityId::Edge(far));
        end_edges[vi] = Some((near, s.vertex));
        work.vertices.insert(s.vertex);
    }
    // The rail path on the rail face.
    let path = if chain.closed {
        closed_path(body, keys, chain, plan, &fins)?
    } else {
        open_path(body, keys, plan, &fins, &end_edges)?
    };
    for (k, &(edge, map)) in path.rails.iter().enumerate() {
        let seg = &plan.segments[k];
        let rail = &seg.rails[0];
        let (pc, m) = &rail.pcurve;
        work.rail_face.push((edge, pc.clone(), compose(*m, map)));
        if let Some((pc, m)) = &rail.on_blend {
            work.blend_side.push((edge, pc.clone(), compose(*m, map)));
        }
        work.tolerances.insert(edge, rail.tolerance);
        if seg.spine {
            work.spines.insert(edge, spine_map(rail.map, map));
        }
    }
    // The other faces: blended edges become their rails.
    let mut others: Vec<Vec<EdgeId>> = Vec::new();
    for seg in &plan.segments {
        let rail = &seg.rails[1];
        let parts = pieces(seg);
        let mut ids: [Option<(CurveId, [f64; 2])>; 2] = [None, None];
        let mut list = Vec::new();
        for (i, &p) in seg.edges.iter().enumerate() {
            let e = specs[chain.edges[p].0].edge;
            let along = chain.edges[p].1;
            let (curve, interval, map) = oriented(&rail.curve, parts[i], along);
            let slot = usize::from(!along);
            let (id, _) = *ids[slot].get_or_insert_with(|| (body.add_curve(curve), map));
            euler::set_curve(body, e, id, interval)?;
            let (pc, m) = &rail.pcurve;
            work.pcurves
                .push((fins[p][1], pc.clone(), compose(*m, map)));
            if let Some((pc, m)) = &rail.on_blend {
                work.pcurves
                    .push((fins[p][0], pc.clone(), compose(*m, map)));
            }
            work.tolerances.insert(e, rail.tolerance);
            if seg.spine {
                work.spines.insert(e, spine_map(rail.map, map));
            }
            made.modified
                .insert(EntityId::Face(face_of(body, fins[p][1])));
            list.push(e);
        }
        others.push(list);
    }
    // Vertices move onto the other faces' rails; cuts and smooth edges
    // follow them; the rail face's cuts beside them become end curves.
    for (vi, joint) in plan.joints.iter().enumerate() {
        let v = chain.vertices[vi];
        let old = body.vertices[v].name;
        let segs = segments_at(plan, chain, vi);
        match joint {
            Joint::End {
                crossings,
                curve,
                face,
                ..
            } => {
                let p = if vi == 0 { 0 } else { n - 1 };
                let cut = beside(body, fins[p][1], v);
                euler::set_point(body, v, crossings[1].point)?;
                move_end(body, cut, v, crossings[1].at)?;
                made.modified.insert(EntityId::Edge(cut));
                let (near, pa) = end_edges[vi].ok_or_else(inconsistent)?;
                let strip = face_of(body, fins[p][0]);
                end_curve(body, near, pa, strip, curve, work)?;
                made.ends.push((near, first_seg + segs[0], *face, old));
                made.modified.insert(EntityId::Face(*face));
            }
            Joint::SmoothJoin {
                smooth, point, at, ..
            } => {
                euler::set_point(body, v, *point)?;
                move_end(body, *smooth, v, *at)?;
                made.modified.insert(EntityId::Edge(*smooth));
            }
            Joint::Join | Joint::Closed => {
                let seg = &plan.segments[segs[0]];
                let t = join_param(seg, chain, vi);
                euler::set_point(body, v, seg.rails[1].curve.point(t))?;
            }
        }
        made.replaced
            .push((old, segs.iter().map(|s| first_seg + s).collect()));
        work.vertices.insert(v);
    }
    // Sections split the strip between segments.
    for (vi, joint) in plan.joints.iter().enumerate() {
        let Joint::SmoothJoin { section, .. } = joint else {
            continue;
        };
        let v = chain.vertices[vi];
        let corner = path.corners[vi].ok_or_else(inconsistent)?;
        let before = (vi + n - 1) % n;
        let face = face_of(body, fins[before][0]);
        let at_corner = starting_in(body, face, corner)?;
        let at_vertex = starting_in(body, face, v)?;
        let c = body.add_curve(section.curve.clone());
        let (from, to) = (Corner::Before(at_corner), Corner::Before(at_vertex));
        let edge = if body.fins[at_corner].loop_ == body.fins[at_vertex].loop_ {
            euler::mef(body, keys.next(), from, to, c, section.interval)?
                .0
                .edge
        } else {
            euler::mekr(body, keys.next(), from, to, c, section.interval)?
                .0
                .edge
        };
        let segs = segments_at(plan, chain, vi);
        made.sections
            .push((edge, [first_seg + segs[0], first_seg + segs[1]]));
        work.vertices.insert(corner);
    }
    // Plain joins on one curve become one edge again.
    for (vi, joint) in plan.joints.iter().enumerate() {
        if matches!(joint, Joint::Join) {
            match euler::jekv(body, keys.next(), chain.vertices[vi]) {
                Ok(_) | Err(Error::Topology(TopologyError::NotRemovable)) => {}
                Err(e) => return Err(e),
            }
        }
    }
    // Each segment's blend face holds its first edge's fin.
    for (k, seg) in plan.segments.iter().enumerate() {
        let face = face_of(body, fins[seg.edges[0]][0]);
        let s = body.add_surface(seg.surface.clone());
        euler::set_surface(body, face, s, seg.sense)?;
        work.blend_faces.insert(face);
        made.segments.push(SegmentMade {
            face,
            edges: seg
                .edges
                .iter()
                .map(|&p| specs[chain.edges[p].0].name)
                .collect(),
            rails: [vec![path.rails[k].0], others[k].clone()],
            faces: [rail_name, other_names[k]],
        });
    }
    Ok(())
}

/// The segments meeting at chain vertex `vi`: the one before it and the
/// one after (one when they are the same, or at an end).
fn segments_at(plan: &Plan, chain: &Chain, vi: usize) -> Vec<usize> {
    let n = chain.edges.len();
    let holds = |p: usize| {
        plan.segments
            .iter()
            .position(|s| s.edges.contains(&p))
            .expect("every edge in a segment")
    };
    let after = (vi < n || chain.closed).then(|| holds(vi % n));
    let before = (vi > 0 || chain.closed).then(|| holds((vi + n - 1) % n));
    let mut out: Vec<usize> = before.into_iter().chain(after).collect();
    out.dedup();
    if out.len() == 2 && out[0] == out[1] {
        out.pop();
    }
    out
}

/// The other rail's parameter where chain vertex `vi` goes: a plain
/// join's, or the start of a closed segment.
fn join_param(seg: &Segment, chain: &Chain, vi: usize) -> f64 {
    let n = chain.edges.len();
    match seg.edges.iter().position(|&p| p == vi % n) {
        Some(i) if i > 0 => seg.joins[i - 1],
        _ => seg.intervals[1][0],
    }
}

/// The rail face's path: its edges (one per segment, with the map from
/// the edge's parameter back to the rail's) and the vertices at smooth
/// joins.
struct Path {
    rails: Vec<(EdgeId, [f64; 2])>,
    corners: Vec<Option<VertexId>>,
}

/// An open chain's path, from the crossing at its first end to the one at
/// its last.
fn open_path(
    body: &mut Body,
    keys: &mut Keys,
    plan: &Plan,
    fins: &[[FinId; 2]],
    end_edges: &[Option<(EdgeId, VertexId)>],
) -> Result<Path, Error> {
    let nv = end_edges.len();
    let (_, start) = end_edges[0].ok_or_else(inconsistent)?;
    let (_, stop) = end_edges[nv - 1].ok_or_else(inconsistent)?;
    let l = body.fins[fins[0][0]].loop_;
    let mut at = Corner::Before(starting(body, l, start)?);
    let mut corners = vec![None; nv];
    let mut rails = Vec::new();
    let last = plan.segments.len() - 1;
    for (k, seg) in plan.segments.iter().enumerate() {
        let rail = &seg.rails[0];
        let c = body.add_curve(rail.curve.clone());
        let interval = seg.intervals[0];
        if k == last {
            let l = match at {
                Corner::Before(f) => body.fins[f].loop_,
                Corner::Lone(l) => l,
            };
            let into = starting(body, l, stop)?;
            // The face's other loops stay with the side holding the loop's
            // first fin: the far side, where the path's end fin leads.
            euler::start_loop_at(body, into)?;
            let to = Corner::Before(into);
            let (m, _) = euler::mef(body, keys.next(), at, to, c, interval)?;
            rails.push((m.edge, [0.0, 1.0]));
        } else {
            let p = rail.curve.point(interval[1]);
            let (m, _) = euler::mev(body, keys.next(), at, p, c, interval)?;
            rails.push((m.edge, [0.0, 1.0]));
            corners[plan.segments[k + 1].edges[0]] = Some(m.vertex);
            at = Corner::Before(m.fins[1]);
        }
    }
    Ok(Path { rails, corners })
}

/// A closed chain's path, round the blended edges' loop on the rail face.
fn closed_path(
    body: &mut Body,
    keys: &mut Keys,
    chain: &Chain,
    plan: &Plan,
    fins: &[[FinId; 2]],
) -> Result<Path, Error> {
    let l = body.fins[fins[0][0]].loop_;
    let face = body.loops[l].face;
    let other = body.face_loops(face).find(|&x| x != l);
    // The rail face's fins run with the chain or against it.
    let with = (body.fins[fins[0][0]].sense == Sense::Forward) == chain.edges[0].1;
    let along = if other.is_none() { with } else { !with };
    let k = plan.segments.len();
    let order: Vec<usize> = if along {
        (0..k).collect()
    } else {
        (0..k).rev().collect()
    };
    // Each path edge: its segment, curve, interval and map back.
    let edges: Vec<(usize, Curve, [f64; 2], [f64; 2])> = order
        .iter()
        .map(|&s| {
            let seg = &plan.segments[s];
            let (c, i, m) = oriented(&seg.rails[0].curve, seg.intervals[0], along);
            (s, c, i, m)
        })
        .collect();
    let start = |j: usize| edges[j].1.point(edges[j].2[0]);
    let v0 = chain.vertices[0];
    let f0 = starting(body, l, v0)?;
    let p0 = body.vertices[v0].point;
    let spoke_line = |b: &mut Body, q: crate::geom::Point3| -> Result<(CurveId, f64), Error> {
        let line = Line::through(p0, q)?;
        Ok((b.add_curve(Curve::Line(line)), p0.distance(q)))
    };
    let mut made_rails: Vec<(usize, EdgeId, [f64; 2])> = Vec::new();
    let mut vertices: Vec<VertexId> = Vec::new();
    // The first path vertex, with the corner the path leaves it by, and
    // the corner the closing edge arrives at.
    let (mut at, close_to, temporary) = match other {
        None => {
            let (c, len) = spoke_line(body, start(0))?;
            let (spur, _) = euler::mev(
                body,
                keys.next(),
                Corner::Before(f0),
                start(0),
                c,
                [0.0, len],
            )?;
            vertices.push(spur.vertex);
            (Corner::Before(spur.fins[1]), None, vec![spur.edge])
        }
        Some(o) => {
            let (to, q) = match body.loops[o].vertex {
                Some(lone) => (Corner::Lone(o), body.vertices[lone].point),
                None => {
                    let f = body.loops[o].first_fin.ok_or_else(inconsistent)?;
                    (Corner::Before(f), body.vertices[body.start_of(f)].point)
                }
            };
            let line = Line::through(p0, q)?;
            let len = p0.distance(q);
            let c = body.add_curve(Curve::Line(line));
            let (mk, _) = euler::mekr(body, keys.next(), Corner::Before(f0), to, c, [0.0, len])?;
            let (sv, _) = euler::semv(body, keys.next(), mk.edge, len / 2.0, start(0))?;
            vertices.push(sv.vertex);
            let out = fin_with(body, sv.edge, Sense::Forward)?;
            let back = fin_with(body, mk.edge, Sense::Reversed)?;
            // The face's other loops stay with the side holding the loop's
            // first fin: the far side, past the spoke's outer part.
            euler::start_loop_at(body, out)?;
            (Corner::Before(out), Some(back), vec![mk.edge, sv.edge])
        }
    };
    for j in 0..edges.len() {
        let (s, ref curve, interval, map) = edges[j];
        let c = body.add_curve(curve.clone());
        if j + 1 < edges.len() {
            let (m, _) = euler::mev(body, keys.next(), at, start(j + 1), c, interval)?;
            vertices.push(m.vertex);
            made_rails.push((s, m.edge, map));
            at = Corner::Before(m.fins[1]);
        } else {
            let to = match (close_to, edges.len()) {
                (Some(back), _) => Corner::Before(back),
                (None, 1) => at,
                (None, _) => {
                    // Before the first path edge's fin out of the first
                    // vertex.
                    let first = made_rails[0].1;
                    let f = body
                        .edge_fins(first)
                        .find(|&f| body.start_of(f) == vertices[0])
                        .ok_or_else(inconsistent)?;
                    Corner::Before(f)
                }
            };
            let (m, _) = euler::mef(body, keys.next(), at, to, c, interval)?;
            made_rails.push((s, m.edge, map));
        }
    }
    for e in temporary {
        euler::kemr(body, e)?;
    }
    // The vertices at smooth joins, by the segments starting there.
    let mut corners = vec![None; chain.vertices.len()];
    for (j, &(s, _, _)) in made_rails.iter().enumerate() {
        // Path edge j starts at path vertex j: the start of its segment
        // along the chain, or its end against it.
        let seg = &plan.segments[s];
        let vi = if along {
            seg.edges[0]
        } else {
            let last = *seg.edges.last().expect("a segment has edges");
            (last + 1) % chain.edges.len()
        };
        if !matches!(plan.joints[vi], Joint::Closed) {
            corners[vi] = Some(vertices[j]);
        }
    }
    let mut rails: Vec<Option<(EdgeId, [f64; 2])>> = vec![None; plan.segments.len()];
    for (s, e, map) in made_rails {
        rails[s] = Some((e, map));
    }
    let rails = rails
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(inconsistent)?;
    Ok(Path { rails, corners })
}

/// Puts the rail face's cut beside a blended vertex on the end curve,
/// running from the rail face's crossing `pa`; `strip` is the face that
/// becomes the blend face.
fn end_curve(
    body: &mut Body,
    edge: EdgeId,
    pa: VertexId,
    strip: FaceId,
    curve: &EndCurve,
    work: &mut Work,
) -> Result<(), Error> {
    let along = body.edges[edge].start == pa;
    let (c, interval, map) = oriented(&curve.curve, curve.interval, along);
    let id = body.add_curve(c);
    euler::set_curve(body, edge, id, interval)?;
    work.tolerances.insert(edge, curve.tolerance);
    // The end curve's pcurves are on the blend surface, then the end
    // face's.
    let fins: Vec<FinId> = body.edge_fins(edge).collect();
    for f in fins {
        let k = usize::from(face_of(body, f) != strip);
        let (pc, m) = &curve.pcurves[k];
        work.pcurves.push((f, pc.clone(), compose(*m, map)));
    }
    Ok(())
}

/// Sets every pcurve the plan knows, finds the rest, and settles
/// tolerances.
fn finish(body: &mut Body, work: &Work) -> Result<(), Error> {
    let mut tolerances = work.tolerances.clone();
    let set = |body: &mut Body, fin: FinId, pc: &Curve2, map: [f64; 2]| -> Result<(), Error> {
        let curve = body.add_curve2(pc.clone());
        euler::set_pcurve(body, fin, Some(Pcurve { curve, map }))?;
        Ok(())
    };
    for (fin, pc, map) in &work.pcurves {
        if body.fin(*fin).is_some() {
            set(body, *fin, pc, *map)?;
        }
    }
    for (edge, pc, map) in &work.rail_face {
        let fins: Vec<FinId> = body.edge_fins(*edge).collect();
        for f in fins {
            if !work.blend_faces.contains(&face_of(body, f)) {
                set(body, f, pc, *map)?;
            }
        }
    }
    for (edge, pc, map) in &work.blend_side {
        let fins: Vec<FinId> = body.edge_fins(*edge).collect();
        for f in fins {
            if work.blend_faces.contains(&face_of(body, f)) {
                set(body, f, pc, *map)?;
            }
        }
    }
    // Every other fin of a blend face: an exact pcurve where there is
    // one, else a fitted one, whose bound the edge's tolerance covers.
    for &face in &work.blend_faces {
        let fins: Vec<FinId> = body
            .face_loops(face)
            .flat_map(|l| body.loop_fins(l).collect::<Vec<_>>())
            .collect();
        for f in fins {
            if body.fins[f].pcurve.is_some() {
                continue;
            }
            let edge = body.fins[f].edge;
            let e = &body.edges[edge];
            let tol = tolerances.get(&edge).copied().unwrap_or(LINEAR_RESOLUTION);
            let surface = &body.surfaces[body.faces[face].surface];
            let curve = &body.curves[e.curve];
            let exact = match (surface, work.spines.get(&edge)) {
                // A rail on a pipe: its spine parameter and angle are known.
                (Surface::Pipe(_), Some(_)) => None,
                _ => crate::pcurve::exact(surface, curve, e.interval, tol),
            };
            let (pc, map, bound) = match exact {
                Some((c, m)) => (c, m, tol),
                None => {
                    let fitted = match (surface, work.spines.get(&edge)) {
                        (Surface::Pipe(pipe), Some(&[a, b])) => {
                            let uv = |t: f64| pipe_uv(pipe, a + b * t, curve.point(t));
                            // Aim at how far the rail is from the pipe: as
                            // far as the spine is from the offsets.
                            let [t0, t1] = e.interval;
                            let off = (0..=64)
                                .map(|i| {
                                    let t = t0 + (t1 - t0) * f64::from(i) / 64.0;
                                    let q = uv(t);
                                    surface.point(q.x, q.y).distance(curve.point(t))
                                })
                                .fold(tol, f64::max);
                            crate::intersect::fit_pcurve_by(
                                surface,
                                curve,
                                e.interval,
                                &uv,
                                4.0 * off,
                            )
                        }
                        _ => crate::intersect::fit_pcurve(surface, curve, e.interval),
                    };
                    let (c, bound) = fitted.map_err(|_| Error::NoPcurve { edge, face })?;
                    (Curve2::Nurbs2(std::sync::Arc::new(c)), [0.0, 1.0], bound)
                }
            };
            set(body, f, &pc, map)?;
            let t = tolerances.entry(edge).or_insert(LINEAR_RESOLUTION);
            *t = t.max(bound);
        }
    }
    // Edges: their own tolerance, and the gaps to their vertices.
    let mut edges: BTreeSet<EdgeId> = tolerances.keys().copied().collect();
    for &v in &work.vertices {
        if body.vertex(v).is_some() {
            edges.extend(body.vertex_edges(v));
        }
    }
    let gap = |body: &Body, edge: EdgeId| -> [f64; 2] {
        let e = &body.edges[edge];
        let c = &body.curves[e.curve];
        [
            c.point(e.interval[0])
                .distance(body.vertices[e.start].point),
            c.point(e.interval[1]).distance(body.vertices[e.end].point),
        ]
    };
    let grow = |x: f64| x * (1.0 + 1e-4) + 1e-12;
    for &edge in &edges {
        if body.edge(edge).is_none() {
            continue;
        }
        let [a, b] = gap(body, edge);
        let own = tolerances
            .get(&edge)
            .copied()
            .unwrap_or(body.edges[edge].tolerance.get());
        let t = own.max(grow(a)).max(grow(b)).max(LINEAR_RESOLUTION);
        euler::set_edge_tolerance(body, edge, Tolerance::new(t)?)?;
    }
    // Vertices: the gaps to their edges and their distance from their
    // faces.
    let mut vertices: BTreeSet<VertexId> = work.vertices.clone();
    for &edge in &edges {
        if let Some(e) = body.edge(edge) {
            vertices.insert(e.start);
            vertices.insert(e.end);
        }
    }
    for v in vertices {
        if body.vertex(v).is_none() {
            continue;
        }
        let p = body.vertices[v].point;
        let mut t = body.vertices[v].tolerance.get();
        for edge in body.vertex_edges(v) {
            let e = &body.edges[edge];
            let [a, b] = gap(body, edge);
            if e.start == v {
                t = t.max(grow(a));
            }
            if e.end == v {
                t = t.max(grow(b));
            }
            for f in body.edge_fins(edge) {
                let s = &body.surfaces[body.faces[face_of(body, f)].surface];
                t = t.max(grow(s.distance(p)));
            }
        }
        euler::set_vertex_tolerance(body, v, Tolerance::new(t.max(LINEAR_RESOLUTION))?)?;
    }
    Ok(())
}
