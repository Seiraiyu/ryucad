//! Chains of blended edges and the vertices between them.
//!
//! Every vertex of a blended edge is classed by how many edges meet there,
//! how many are blended, which meet tangentially (see [`super::convex`])
//! and how they are blended: a closed edge alone at its vertex; an end,
//! one blended edge cut off by the other faces there (one, or several in
//! a fan); two kinds of join, where two blended edges meet tangentially
//! and the blend runs on from one into the other; and the corners, where
//! blended edges meet at angles: a trim of two, a ball, a roll or a
//! mitre of three. Every other vertex is refused, naming it.
//!
//! Blended edges joined at joins make chains. The new rails of a chain
//! all go on one face, its rail face: at a smooth join it is the face the
//! two edges share, and the edge on the other side of every blended edge
//! becomes the rail there (see [`super::build`]).

use super::convex::{Bend, BendFail, bend, leaving, tangent_directions};
use super::spec::Spec;
use super::{BlendKind, Chamfer};
use crate::name::Name;
use crate::topo::{Body, EdgeId, FaceId, VertexId};
use crate::{BlendError, Error};
use std::collections::BTreeMap;

/// What happens at a vertex of a blended edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// A closed edge alone at its vertex.
    Closed,
    /// One blended edge and two others, all meeting at an angle: the
    /// blend runs on until `face`, the third face there, cuts it off.
    /// `cuts[i]` is the edge between the blended edge's `faces[i]` and
    /// `face`.
    End { face: FaceId, cuts: [EdgeId; 2] },
    /// Two blended edges meeting tangentially, and no other edge.
    Join,
    /// Two blended edges meeting tangentially, sharing the face `common`,
    /// and `smooth`, an edge between their other faces, which meet
    /// tangentially along it.
    SmoothJoin { smooth: EdgeId, common: FaceId },
    /// Three blended edges, fillets of one radius bending the same way,
    /// and no other edge: a ball corner (see [`super::corner`]).
    Corner,
    /// Two blended edges meeting at an angle, sharing a face, bending the
    /// same way, and a sharp edge between their other faces: each blend is
    /// trimmed by the other (see [`super::trim`]).
    Trim,
    /// Three blended edges, fillets, and no other edge, where one ball
    /// can't close them: `first`, the larger fillet or the only one
    /// bending its way, is built through the corner first, and the others
    /// roll over it (see [`super::stage`]).
    Rolled { first: EdgeId },
    /// Three blended edges, chamfers, and no other edge: mitred (see
    /// [`super::mitre`]).
    Mitre,
    /// One blended edge and three or more others: the blend runs on until
    /// every other face there cuts it off, in a chain of end curves (see
    /// [`fan`]).
    Fan,
}

/// The faces round `v` from one face of `edge` to the other, not
/// including them, and the edges between them, starting from the edge
/// between `from` and the first: the end faces of a blend of `edge`
/// ending at `v`, and the edges they meet along. `None` where the faces
/// don't go round the vertex once.
pub(crate) fn fan_from(
    body: &Body,
    edge: EdgeId,
    v: VertexId,
    from: FaceId,
) -> Option<(Vec<FaceId>, Vec<EdgeId>)> {
    let faces_of = |e: EdgeId| -> Vec<FaceId> {
        body.edge_fins(e)
            .map(|f| body.loops[body.fins[f].loop_].face)
            .collect()
    };
    let ends = faces_of(edge);
    let to = *ends.iter().find(|&&f| f != from)?;
    let around = body.vertex_edges(v);
    let (mut face, mut prev) = (from, edge);
    let (mut faces, mut edges) = (Vec::new(), Vec::new());
    for _ in 0..around.len() {
        let next = around
            .iter()
            .copied()
            .filter(|&g| g != prev && faces_of(g).contains(&face))
            .collect::<Vec<_>>();
        let [g] = next[..] else {
            return None;
        };
        edges.push(g);
        let f = faces_of(g);
        let [a, b] = f[..] else {
            return None;
        };
        face = if a == face { b } else { a };
        if face == to {
            return (edges.len() == around.len() - 1).then_some((faces, edges));
        }
        faces.push(face);
        prev = g;
    }
    None
}

/// The end faces round `v` of a blend of `edge`, from its first face.
fn fan(body: &Body, edge: EdgeId, v: VertexId) -> Option<(Vec<FaceId>, Vec<EdgeId>)> {
    let fin = body.edge_fins(edge).next()?;
    fan_from(body, edge, v, body.loops[body.fins[fin].loop_].face)
}

/// A maximal run of blended edges joined at joins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chain {
    /// Indices into the specs, in order along the chain, each with
    /// whether the edge runs along the chain.
    pub(crate) edges: Vec<(usize, bool)>,
    /// The vertex where each edge starts along the chain, and for an open
    /// chain the vertex after the last: one more than the edges, or as
    /// many for a closed chain.
    pub(crate) vertices: Vec<VertexId>,
    /// Each vertex's class.
    pub(crate) classes: Vec<Class>,
    pub(crate) closed: bool,
    /// The face the chain's new rails lie on: a face of every edge.
    pub(crate) rail_face: FaceId,
}

impl Chain {
    /// Which of spec `k`'s faces is the rail face: 0 or 1.
    pub(crate) fn rail_side(&self, specs: &[Spec], k: usize) -> usize {
        usize::from(specs[k].faces[1] == self.rail_face)
    }
}

/// The other edge at `v` that meets `edge` tangentially, when exactly one
/// does.
fn next_tangent(body: &Body, edge: EdgeId, v: VertexId) -> Option<EdgeId> {
    let (here, slack) = leaving(body, edge, v)?;
    let mut found = None;
    for g in body.vertex_edges(v) {
        if g == edge {
            continue;
        }
        if leaving(body, g, v).is_some_and(|(d, s)| tangent_directions(here, d, slack + s)) {
            if found.is_some() {
                return None;
            }
            found = Some(g);
        }
    }
    found
}

/// The far end of `edge` from `v`.
fn other_end(body: &Body, edge: EdgeId, v: VertexId) -> VertexId {
    let e = &body.edges[edge];
    if e.start == v { e.end } else { e.start }
}

/// The edges reachable from `edge` through vertices where an edge meets
/// exactly one other tangentially, in order along the chain: from one
/// end to the other for an open chain, from `edge` along its direction
/// for a closed one. Fails with `DeadEntity` for a dead edge.
pub(crate) fn tangent_chain(body: &Body, edge: EdgeId) -> Result<Vec<EdgeId>, Error> {
    let e = crate::euler::live(&body.edges, edge)?;
    let (start, end) = (e.start, e.end);
    let limit = body.edges().count();
    let walk = |from: VertexId| -> (Vec<EdgeId>, bool) {
        let (mut out, mut cur, mut v) = (Vec::new(), edge, from);
        while out.len() < limit {
            match next_tangent(body, cur, v) {
                Some(g) if g == edge => return (out, true),
                Some(g) => {
                    out.push(g);
                    v = other_end(body, g, v);
                    cur = g;
                }
                None => break,
            }
        }
        (out, false)
    };
    let (ahead, closed) = walk(end);
    if closed {
        return Ok(core::iter::once(edge).chain(ahead).collect());
    }
    let (behind, _) = if start == end {
        (Vec::new(), false)
    } else {
        walk(start)
    };
    Ok(behind
        .into_iter()
        .rev()
        .chain([edge])
        .chain(ahead)
        .collect())
}

/// The chains of the resolved edges, each vertex classed, in order of
/// their first edge's name. Fails at the first vertex, by name, that
/// can't be built: an edge whose bend can't be given, a blended edge
/// meeting an unblended one tangentially, tangent edges blended
/// differently, or a vertex of any other kind.
pub(crate) fn chains(body: &Body, specs: &[Spec]) -> Result<Vec<Chain>, Error> {
    let index: BTreeMap<EdgeId, usize> =
        specs.iter().enumerate().map(|(k, s)| (s.edge, k)).collect();
    let mut vertices: Vec<VertexId> = specs
        .iter()
        .flat_map(|s| {
            let e = &body.edges[s.edge];
            [e.start, e.end]
        })
        .collect();
    vertices.sort_by_key(|&v| body.vertices[v].name);
    vertices.dedup();
    let mut classes: BTreeMap<VertexId, Class> = BTreeMap::new();
    for &v in &vertices {
        classes.insert(v, classify(body, specs, &index, v)?);
    }
    let mut seen = vec![false; specs.len()];
    let mut out = Vec::new();
    for k in 0..specs.len() {
        if !seen[k] {
            let chain = walk(body, specs, &index, &classes, k)?;
            for &(j, _) in &chain.edges {
                seen[j] = true;
            }
            out.push(chain);
        }
    }
    Ok(out)
}

/// The class of vertex `v`.
fn classify(
    body: &Body,
    specs: &[Spec],
    index: &BTreeMap<EdgeId, usize>,
    v: VertexId,
) -> Result<Class, Error> {
    let at = body.vertices[v].name;
    let unsupported = || -> Error { BlendError::Unsupported { at }.into() };
    let edges = body.vertex_edges(v);
    let blended: Vec<EdgeId> = edges
        .iter()
        .copied()
        .filter(|e| index.contains_key(e))
        .collect();
    let closed = |e: EdgeId| body.edges[e].start == body.edges[e].end;
    if edges.len() == 1 {
        return if closed(edges[0]) {
            Ok(Class::Closed)
        } else {
            Err(unsupported())
        };
    }
    if edges.iter().any(|&e| closed(e)) {
        return Err(unsupported());
    }
    let dirs: Vec<_> = edges.iter().map(|&e| leaving(body, e, v)).collect();
    let tangent = |i: usize, j: usize| match (dirs[i], dirs[j]) {
        (Some(a), Some(b)) => tangent_directions(a.0, b.0, a.1 + b.1),
        _ => false,
    };
    let pos = |e: EdgeId| edges.iter().position(|&x| x == e).expect("at v");
    for &e in &blended {
        for (j, &g) in edges.iter().enumerate() {
            if !index.contains_key(&g) && tangent(pos(e), j) {
                return Err(BlendError::OpenTangentChain {
                    blended: body.edges[e].name,
                    open: body.edges[g].name,
                }
                .into());
            }
        }
    }
    let face_set = |e: EdgeId| specs[index[&e]].faces;
    let faces_of = |e: EdgeId| -> [FaceId; 2] {
        let fins: Vec<_> = body.edge_fins(e).collect();
        [0, 1].map(|k| body.loops[body.fins[fins[k]].loop_].face)
    };
    match (blended.len(), edges.len()) {
        (1, 3) => {
            let e = blended[0];
            let faces = face_set(e);
            let others: Vec<EdgeId> = edges.iter().copied().filter(|&g| g != e).collect();
            let [x, y] = [others[0], others[1]].map(faces_of);
            // The third face is the one the other two edges share.
            let third = x
                .iter()
                .copied()
                .find(|f| y.contains(f) && !faces.contains(f));
            let Some(third) = third else {
                return Err(unsupported());
            };
            let mut cuts = [others[0]; 2];
            for (i, &f) in faces.iter().enumerate() {
                let cut = others
                    .iter()
                    .copied()
                    .find(|&g| faces_of(g).contains(&f) && faces_of(g).contains(&third));
                cuts[i] = cut.ok_or_else(unsupported)?;
            }
            if cuts[0] == cuts[1] {
                return Err(unsupported());
            }
            Ok(Class::End { face: third, cuts })
        }
        (2, n @ (2 | 3)) if tangent(pos(blended[0]), pos(blended[1])) => {
            let (a, b) = (blended[0], blended[1]);
            same_size(body, specs, index[&a], index[&b])?;
            if n == 2 {
                return Ok(Class::Join);
            }
            let (fa, fb) = (face_set(a), face_set(b));
            let common: Vec<FaceId> = fa.iter().copied().filter(|f| fb.contains(f)).collect();
            let [common] = common[..] else {
                return Err(unsupported());
            };
            let smooth = edges
                .iter()
                .copied()
                .find(|&g| g != a && g != b)
                .expect("a third edge");
            let sides = faces_of(smooth);
            let others = [fa, fb].map(|f| f[usize::from(f[0] == common)]);
            let between = sides.contains(&others[0]) && sides.contains(&others[1]);
            match bend(body, smooth) {
                Ok(Bend::Smooth) if between => Ok(Class::SmoothJoin { smooth, common }),
                Err(BendFail::Uncertain) => Err(BlendError::Uncertain { at }.into()),
                _ => Err(unsupported()),
            }
        }
        (1, n) if n > 3 => {
            // The blend runs on past the vertex and is cut off by every
            // other face there, one after another.
            fan(body, blended[0], v).ok_or_else(unsupported)?;
            Ok(Class::Fan)
        }
        (2, 3) => {
            // Two blended edges meeting at an angle and a sharp one: the
            // blends trim each other, if they share a face and the sharp
            // edge lies between their other faces.
            let (fa, fb) = (face_set(blended[0]), face_set(blended[1]));
            let shared = fa.iter().filter(|f| fb.contains(f)).count();
            let sharp = edges
                .iter()
                .copied()
                .find(|g| !blended.contains(g))
                .expect("a third edge");
            let others = [fa, fb].map(|f| {
                f.iter()
                    .copied()
                    .find(|x| !fa.contains(x) || !fb.contains(x))
            });
            let between = others
                .iter()
                .all(|o| o.is_some_and(|o| faces_of(sharp).contains(&o)));
            if !(shared == 1 && between) {
                return Err(unsupported());
            }
            // Bending opposite ways, the blends lie either side of the
            // shared face and never meet.
            let bends = [blended[0], blended[1]].map(|e| bend(body, e));
            match bends {
                [Ok(a), Ok(b)] if a == b && a != Bend::Smooth => Ok(Class::Trim),
                [Err(BendFail::Uncertain), _] | [_, Err(BendFail::Uncertain)] => {
                    Err(BlendError::Uncertain { at }.into())
                }
                _ => Err(unsupported()),
            }
        }
        (3, 3) => corner_class(body, specs, &blended, at),
        _ => Err(unsupported()),
    }
}

/// The class of a vertex where three blended edges meet at angles: a
/// ball corner when they are fillets of one radius that bend the same
/// way; rolled, built in two stages, when one of them is a larger fillet
/// than the other two, or the only one bending its way, and the other two
/// are fillets of one radius; mitred when all three are chamfers.
/// Anything else is refused.
fn corner_class(body: &Body, specs: &[Spec], blended: &[EdgeId], at: Name) -> Result<Class, Error> {
    let unsupported = || -> Error { BlendError::Unsupported { at }.into() };
    let mut radii = Vec::with_capacity(3);
    let mut convex = Vec::with_capacity(3);
    let chamfers = blended
        .iter()
        .filter(|&&e| {
            let s = specs.iter().find(|s| s.edge == e).expect("a blended edge");
            matches!(s.kind, BlendKind::Chamfer(_))
        })
        .count();
    match chamfers {
        0 => {}
        3 => return Ok(Class::Mitre),
        _ => return Err(unsupported()),
    }
    for &e in blended {
        let s = specs.iter().find(|s| s.edge == e).expect("a blended edge");
        let BlendKind::Fillet { radius } = s.kind else {
            return Err(unsupported());
        };
        radii.push(radius);
        convex.push(match bend(body, e) {
            Ok(Bend::Convex) => true,
            Ok(Bend::Concave) => false,
            Ok(Bend::Smooth) => return Err(BlendError::SmoothEdge { edge: s.name }.into()),
            Err(BendFail::Changes) => {
                return Err(BlendError::ConvexityChanges { edge: s.name }.into());
            }
            Err(BendFail::Uncertain) => return Err(BlendError::Uncertain { at }.into()),
        });
    }
    // The edge unlike the other two, which are alike, if there is one.
    let odd = |same: &dyn Fn(usize, usize) -> bool| -> Option<usize> {
        (0..3).find(|&i| {
            let (j, k) = ((i + 1) % 3, (i + 2) % 3);
            same(j, k) && !same(i, j)
        })
    };
    let rest_alike = |i: usize| radii[(i + 1) % 3] == radii[(i + 2) % 3];
    match odd(&|i, j| convex[i] == convex[j]) {
        Some(i) if rest_alike(i) => Ok(Class::Rolled { first: blended[i] }),
        Some(_) => Err(unsupported()),
        None => match odd(&|i, j| radii[i] == radii[j]) {
            None if radii[0] == radii[1] => Ok(Class::Corner),
            Some(i) if radii[i] > radii[(i + 1) % 3] => Ok(Class::Rolled { first: blended[i] }),
            _ => Err(unsupported()),
        },
    }
}

/// How an edge is blended, seen from a face it shares with a tangent
/// neighbour: a chamfer's setbacks on that face and on the other.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sizes {
    Fillet(f64),
    Setbacks([f64; 2]),
    Angle {
        distance: f64,
        angle: f64,
        on_shared: bool,
    },
}

fn sizes(body: &Body, s: &Spec, shared: Option<FaceId>) -> Sizes {
    let on_shared = |face: Name| Some(face) == shared.map(|f| body.faces[f].name);
    match s.kind {
        BlendKind::Fillet { radius } => Sizes::Fillet(radius),
        BlendKind::Chamfer(Chamfer::Equal { distance }) => Sizes::Setbacks([distance; 2]),
        BlendKind::Chamfer(Chamfer::Distances {
            first,
            second,
            first_face,
        }) => Sizes::Setbacks(if on_shared(first_face) {
            [first, second]
        } else {
            [second, first]
        }),
        BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance,
            angle,
            face,
        }) => Sizes::Angle {
            distance,
            angle,
            on_shared: on_shared(face),
        },
    }
}

/// Fails with `SizeChangeAlongChain` unless specs `a` and `b`, which meet
/// tangentially, blend the same way: the same fillet radius, or the same
/// chamfer on the face they share and on their other faces.
fn same_size(body: &Body, specs: &[Spec], a: usize, b: usize) -> Result<(), Error> {
    let (x, y) = (&specs[a], &specs[b]);
    let shared = x.faces.iter().copied().find(|f| y.faces.contains(f));
    if sizes(body, x, shared) == sizes(body, y, shared) {
        return Ok(());
    }
    let mut names = [x.name, y.name];
    names.sort();
    Err(BlendError::SizeChangeAlongChain {
        a: names[0],
        b: names[1],
    }
    .into())
}

/// The chain holding spec `k`.
fn walk(
    body: &Body,
    specs: &[Spec],
    index: &BTreeMap<EdgeId, usize>,
    classes: &BTreeMap<VertexId, Class>,
    k: usize,
) -> Result<Chain, Error> {
    let e = &body.edges[specs[k].edge];
    if e.start == e.end {
        let rail_face = choose_rail_face(body, specs, &[k]);
        return Ok(Chain {
            edges: vec![(k, true)],
            vertices: vec![e.start],
            classes: vec![Class::Closed],
            closed: true,
            rail_face,
        });
    }
    // The next blended edge on past `v` from spec `j`, at a join.
    let next = |j: usize, v: VertexId| -> Option<usize> {
        match classes[&v] {
            Class::Join | Class::SmoothJoin { .. } => {
                let other = body
                    .vertex_edges(v)
                    .into_iter()
                    .find(|g| *g != specs[j].edge && index.contains_key(g))?;
                Some(index[&other])
            }
            _ => None,
        }
    };
    let mut edges = vec![(k, true)];
    let mut vertices = vec![e.start, e.end];
    let mut closed = false;
    // Ahead, from the edge's end.
    let (mut j, mut v) = (k, e.end);
    while let Some(n) = next(j, v) {
        if n == k {
            closed = true;
            vertices.pop();
            break;
        }
        let forward = body.edges[specs[n].edge].start == v;
        v = other_end(body, specs[n].edge, v);
        edges.push((n, forward));
        vertices.push(v);
        j = n;
    }
    if !closed {
        // Behind, from the edge's start.
        let (mut j, mut v) = (k, e.start);
        while let Some(n) = next(j, v) {
            let forward = body.edges[specs[n].edge].end == v;
            v = other_end(body, specs[n].edge, v);
            edges.insert(0, (n, forward));
            vertices.insert(0, v);
            j = n;
        }
    }
    let chain_classes: Vec<Class> = vertices.iter().map(|v| classes[v]).collect();
    let members: Vec<usize> = edges.iter().map(|&(j, _)| j).collect();
    let commons: Vec<(VertexId, FaceId)> = vertices
        .iter()
        .zip(&chain_classes)
        .filter_map(|(&v, c)| match c {
            Class::SmoothJoin { common, .. } => Some((v, *common)),
            _ => None,
        })
        .collect();
    let rail_face = match commons.first() {
        Some(&(_, f)) => {
            for &(v, g) in &commons {
                if g != f {
                    let at = body.vertices[v].name;
                    return Err(BlendError::Unsupported { at }.into());
                }
            }
            f
        }
        None => choose_rail_face(body, specs, &members),
    };
    Ok(Chain {
        edges,
        vertices,
        classes: chain_classes,
        closed,
        rail_face,
    })
}

/// The rail face of a chain with no smooth join, whose edges all have the
/// same two faces: the one with the lower name.
fn choose_rail_face(body: &Body, specs: &[Spec], members: &[usize]) -> FaceId {
    let faces = specs[members[0]].faces;
    if body.faces[faces[0]].name < body.faces[faces[1]].name {
        faces[0]
    } else {
        faces[1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::EdgeBlend;
    use crate::blend::spec::resolve;
    use crate::blend::tests::{block, edge_of, slot};
    use crate::name::FaceRole;
    use crate::topo::EntityId;

    fn fillet(edge: Name, radius: f64) -> EdgeBlend {
        EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius },
        }
    }

    fn chains_of(b: &Body, blends: &[EdgeBlend]) -> Result<Vec<Chain>, Error> {
        let specs = resolve(b, blends)?;
        chains(b, &specs)
    }

    /// The cap edges of a slot at `z`, in name order.
    fn cap_edges(b: &Body, z: f64) -> Vec<Name> {
        let mut out: Vec<Name> = b
            .edges()
            .filter(|&e| {
                let ed = b.edge(e).unwrap();
                let p = b.vertex(ed.start).unwrap().point;
                let q = b.vertex(ed.end).unwrap().point;
                (p.z - z).abs() < 1e-9 && (q.z - z).abs() < 1e-9
            })
            .map(|e| b.edge(e).unwrap().name)
            .collect();
        out.sort();
        out
    }

    #[test]
    fn one_box_edge_ends_at_both_vertices() {
        let b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let c = chains_of(&b, &[fillet(e, 0.1)]).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].edges, [(0, true)]);
        assert!(!c[0].closed);
        assert_eq!(c[0].vertices.len(), 2);
        for class in &c[0].classes {
            let Class::End { face, cuts } = class else {
                panic!("{class:?}")
            };
            let name = b.face(*face).unwrap().name;
            assert!(
                name == crate::blend::tests::face_name(FaceRole::PlusY)
                    || name == crate::blend::tests::face_name(FaceRole::MinusY)
            );
            assert_ne!(cuts[0], cuts[1]);
        }
    }

    #[test]
    fn two_box_edges_at_a_corner_trim_each_other() {
        let b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let g = edge_of(&b, FaceRole::PlusY, FaceRole::PlusZ);
        let c = chains_of(&b, &[fillet(e, 0.1), fillet(g, 0.1)]).unwrap();
        assert_eq!(c.len(), 2);
        for chain in &c {
            assert_eq!(
                chain.classes.iter().filter(|x| **x == Class::Trim).count(),
                1
            );
        }
    }

    #[test]
    fn three_box_edges_meet_in_a_ball_or_roll() {
        let b = block();
        let [e, g, h] = [
            edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ),
            edge_of(&b, FaceRole::PlusY, FaceRole::PlusZ),
            edge_of(&b, FaceRole::PlusX, FaceRole::PlusY),
        ];
        let c = chains_of(&b, &[fillet(e, 0.1), fillet(g, 0.1), fillet(h, 0.1)]).unwrap();
        assert_eq!(c.len(), 3);
        assert!(c.iter().all(|x| x.classes.contains(&Class::Corner)));
        // One larger is rolled over by the others; three sizes are
        // refused.
        let c = chains_of(&b, &[fillet(e, 0.1), fillet(g, 0.1), fillet(h, 0.2)]).unwrap();
        let he = crate::blend::tests::edge_id(&b, h);
        assert!(
            c.iter()
                .all(|x| x.classes.contains(&Class::Rolled { first: he }))
        );
        let err = chains_of(&b, &[fillet(e, 0.1), fillet(g, 0.2), fillet(h, 0.3)]).unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidBlend(BlendError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_rim_is_closed() {
        let (b, _) = crate::primitive::cylinder(
            crate::name::OpKey(1),
            1.0,
            1.0,
            &crate::geom::Rigid::IDENTITY,
        )
        .unwrap();
        let e = b.edges().next().unwrap();
        let c = chains_of(&b, &[fillet(b.edge(e).unwrap().name, 0.1)]).unwrap();
        assert_eq!(c[0].classes, [Class::Closed]);
        assert!(c[0].closed);
    }

    #[test]
    fn a_slots_cap_is_a_closed_chain_of_smooth_joins() {
        let b = slot();
        let caps = cap_edges(&b, 1.0);
        assert_eq!(caps.len(), 4);
        let blends: Vec<EdgeBlend> = caps.iter().map(|&n| fillet(n, 0.1)).collect();
        let c = chains_of(&b, &blends).unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].closed);
        assert_eq!(c[0].edges.len(), 4);
        assert_eq!(c[0].vertices.len(), 4);
        let top = c[0].rail_face;
        for class in &c[0].classes {
            assert!(matches!(class, Class::SmoothJoin { common, .. } if *common == top));
        }
        // Consecutive edges share the vertex between them.
        let specs = resolve(&b, &blends).unwrap();
        for (i, &(k, fwd)) in c[0].edges.iter().enumerate() {
            let e = b.edge(specs[k].edge).unwrap();
            let (s, t) = if fwd {
                (e.start, e.end)
            } else {
                (e.end, e.start)
            };
            assert_eq!(s, c[0].vertices[i]);
            assert_eq!(t, c[0].vertices[(i + 1) % 4]);
        }
        // One of them alone is open at both ends.
        let err = chains_of(&b, &blends[..1]).unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidBlend(BlendError::OpenTangentChain { .. })
        ));
        // Different sizes along the chain.
        let mut mixed = blends.clone();
        mixed[0].kind = BlendKind::Fillet { radius: 0.2 };
        let err = chains_of(&b, &mixed).unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidBlend(BlendError::SizeChangeAlongChain { .. })
        ));
    }

    #[test]
    fn an_edge_split_in_two_joins() {
        let mut b = block();
        let n = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let Some(EntityId::Edge(e)) = b.by_name(n) else {
            panic!()
        };
        let (t0, t1) = (
            b.edge(e).unwrap().interval[0],
            b.edge(e).unwrap().interval[1],
        );
        let p = b
            .curve(b.edge(e).unwrap().curve)
            .unwrap()
            .point((t0 + t1) / 2.0);
        let (s, _) =
            crate::euler::semv(&mut b, crate::name::OpKey(5), e, (t0 + t1) / 2.0, p).unwrap();
        crate::pcurve::fill(&mut b).unwrap();
        let halves = [b.edge(e).unwrap().name, b.edge(s.edge).unwrap().name];
        let c = chains_of(&b, &[fillet(halves[0], 0.1), fillet(halves[1], 0.1)]).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].edges.len(), 2);
        assert!(matches!(
            c[0].classes[..],
            [Class::End { .. }, Class::Join, Class::End { .. }]
        ));
        let err = chains_of(&b, &[fillet(halves[0], 0.1)]).unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidBlend(BlendError::OpenTangentChain { .. })
        ));
    }

    #[test]
    fn tangent_chains_follow_the_slot() {
        let b = slot();
        let caps = cap_edges(&b, 0.0);
        let Some(EntityId::Edge(e)) = b.by_name(caps[0]) else {
            panic!()
        };
        let chain = tangent_chain(&b, e).unwrap();
        assert_eq!(chain.len(), 4);
        assert_eq!(chain[0], e);
        let mut names: Vec<Name> = chain.iter().map(|&x| b.edge(x).unwrap().name).collect();
        names.sort();
        assert_eq!(names, caps);
        // A box edge is a chain of one.
        let bx = block();
        let n = edge_of(&bx, FaceRole::PlusX, FaceRole::PlusZ);
        let Some(EntityId::Edge(g)) = bx.by_name(n) else {
            panic!()
        };
        assert_eq!(tangent_chain(&bx, g).unwrap(), [g]);
    }
}
