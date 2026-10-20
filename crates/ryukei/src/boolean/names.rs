//! Names and the journal: what the result's faces, edges and vertices
//! are called, and what happened to every operand's.
//!
//! Each operand entity has an operand name: a target entity its own; a
//! tool entity `(key, Placed, [tool, own])`, `tool` being
//! `(key, Tool, [the tool's key])`, so copies of one body placed as
//! several tools never collide and moving a tool renames nothing. Then:
//!
//! - a face kept whole (perhaps trimmed, with new holes) keeps its operand
//!   name; a face split into several kept pieces names each
//!   `(key, Piece, [face, cutters...])`, the cutters being the sorted
//!   names of the other operands' faces whose curves bound the piece;
//! - an edge kept whole keeps its operand name; one cut into several kept
//!   pieces names each `(key, Cut, [edge, cutters...])`, the cutters being
//!   the faces of other operands at its two ends, sorted;
//! - an edge where two faces meet is `(key, IntersectionEdge, [a, b])`,
//!   the target's or the lower tool's face first;
//! - a vertex an operand had keeps its operand name; any other is
//!   `(key, IntersectionVertex, [faces...])`, the faces meeting there,
//!   sorted;
//! - faces and edges merged afterwards are `(key, Merged, [names...])`,
//!   the merged names sorted.
//!
//! Names that would still collide get an ordinal, counted in the order
//! the pieces or edges were made, as a last parent. Names depend on the
//! caller's keys and on which faces cut which, never on coordinates.
//!
//! The journal records `Placed` for every tool entity, `Split` for each
//! entity split into several, `Merged` for every merge, `Created` for
//! intersection edges and vertices, `Modified` for entities kept whole
//! whose boundary or geometry changed, and `Deleted` for entities that
//! didn't survive.

use super::FaceRef;
use super::build::Kept;
use super::merge::Sources;
use super::table::{Boundary, GeId, GvId, Member, Table};
use crate::name::{BooleanRole, Event, Journal, Name, OpKey, Role, ToolKey};
use crate::topo::{Body, EdgeId, EntityId, FaceId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// Names every face, edge and vertex of `body` and returns the journal.
pub(crate) fn name(
    key: OpKey,
    ops: &[Body],
    tools: &[ToolKey],
    table: &Table,
    kept: &[Kept],
    body: &mut Body,
    sources: &Sources,
) -> Journal {
    let mut journal = Journal::new();
    body.set_recipes(true);
    // Operand names.
    let tool_names: Vec<Name> = tools
        .iter()
        .map(|t| {
            body.derive_name(
                "boolean",
                key,
                Role::Boolean(BooleanRole::Tool),
                &[Name::from_u128(t.0)],
            )
        })
        .collect();
    let mut placed: BTreeMap<(usize, EntityId), Name> = BTreeMap::new();
    for (op, b) in ops.iter().enumerate() {
        let entities = b
            .faces()
            .map(EntityId::Face)
            .chain(b.edges().map(EntityId::Edge))
            .chain(b.vertices().map(EntityId::Vertex));
        for x in entities {
            let own = b.name_of(x).expect("live");
            let n = if op == 0 {
                own
            } else {
                let to = body.derive_name(
                    "boolean",
                    key,
                    Role::Boolean(BooleanRole::Placed),
                    &[tool_names[op - 1], own],
                );
                journal.push(Event::Placed {
                    tool: tools[op - 1],
                    from: own,
                    to,
                });
                to
            };
            placed.insert((op, x), n);
        }
    }
    let face_name = |f: FaceRef| placed[&(f.op, EntityId::Face(f.face))];
    // Kept pieces per operand face.
    let mut pieces_of: BTreeMap<FaceRef, Vec<usize>> = BTreeMap::new();
    for (k, kf) in kept.iter().enumerate() {
        pieces_of.entry(kf.piece.face).or_default().push(k);
    }
    // Each kept piece's name.
    let mut piece_name: Vec<Name> = vec![Name::from_u128(0); kept.len()];
    let mut parents_seen: BTreeMap<Vec<Name>, usize> = BTreeMap::new();
    for (f, list) in &pieces_of {
        if list.len() == 1 {
            piece_name[list[0]] = face_name(*f);
            continue;
        }
        for &k in list {
            let mut cutters: BTreeSet<Name> = BTreeSet::new();
            for lp in &kept[k].piece.loops {
                if let Boundary::Arcs(a) = lp {
                    for &(g, _) in a {
                        for c in cutters_of(ops, table, g, f.op) {
                            cutters.insert(face_name(c));
                        }
                    }
                }
            }
            let mut parents = vec![face_name(*f)];
            parents.extend(cutters);
            let ordinal = parents_seen.entry(parents.clone()).or_insert(0);
            if *ordinal > 0 {
                parents.push(Name::from_u128(*ordinal as u128));
            }
            *ordinal += 1;
            piece_name[k] =
                body.derive_name("boolean", key, Role::Boolean(BooleanRole::Piece), &parents);
        }
    }
    // Faces.
    let mut result_faces: BTreeSet<Name> = BTreeSet::new();
    let faces: Vec<(FaceId, Vec<usize>)> =
        sources.faces.iter().map(|(f, v)| (*f, v.clone())).collect();
    for (f, ks) in &faces {
        let n = if ks.len() == 1 {
            piece_name[ks[0]]
        } else {
            let mut ps: Vec<Name> = ks.iter().map(|&k| piece_name[k]).collect();
            ps.sort();
            ps.dedup();
            let child = body.derive_name("boolean", key, Role::Boolean(BooleanRole::Merged), &ps);
            journal.push(Event::merged_all(ps, child));
            child
        };
        body.rename(EntityId::Face(*f), n);
        result_faces.insert(n);
    }
    // What happened to each operand face.
    for (op, b) in ops.iter().enumerate() {
        for face in b.faces() {
            let r = FaceRef { op, face };
            let n = face_name(r);
            match pieces_of.get(&r).map(Vec::len) {
                None | Some(0) => journal.push(Event::Deleted(n)),
                Some(1) => {
                    let k = pieces_of[&r][0];
                    let changed = kept[k].reversed || kept[k].piece.loops != table.faces[&r].loops;
                    if changed && result_faces.contains(&n) {
                        journal.push(Event::Modified(n));
                    }
                }
                Some(_) => {
                    let mut children: Vec<Name> =
                        pieces_of[&r].iter().map(|&k| piece_name[k]).collect();
                    children.sort();
                    journal.push(Event::Split {
                        parent: n,
                        children,
                    });
                }
            }
        }
    }
    // Edges: each kept global edge's name, then merges.
    let used: BTreeSet<GeId> = sources.edges.values().flatten().copied().collect();
    let mut kept_of_edge: BTreeMap<(usize, EdgeId), Vec<GeId>> = BTreeMap::new();
    for &g in &used {
        if let Member::Edge(e) = table.edges[g].members[0] {
            kept_of_edge.entry((e.op, e.edge)).or_default().push(g);
        }
    }
    let mut ge_name: BTreeMap<GeId, Name> = BTreeMap::new();
    let mut by_pair: BTreeMap<(FaceRef, FaceRef), usize> = BTreeMap::new();
    let mut by_cut: BTreeMap<Vec<Name>, usize> = BTreeMap::new();
    for &g in &used {
        let ge = &table.edges[g];
        let n = match ge.members[0] {
            Member::Edge(e) => {
                let own = placed[&(e.op, EntityId::Edge(e.edge))];
                if kept_of_edge[&(e.op, e.edge)].len() == 1 {
                    own
                } else {
                    let mut cutters: BTreeSet<Name> = BTreeSet::new();
                    for v in [ge.start, ge.end] {
                        for c in &table.vertices[v].faces {
                            if c.op != e.op {
                                cutters.insert(face_name(*c));
                            }
                        }
                    }
                    let mut parents = vec![own];
                    parents.extend(cutters);
                    let ordinal = by_cut.entry(parents.clone()).or_insert(0);
                    if *ordinal > 0 {
                        parents.push(Name::from_u128(*ordinal as u128));
                    }
                    *ordinal += 1;
                    body.derive_name("boolean", key, Role::Boolean(BooleanRole::Cut), &parents)
                }
            }
            Member::Stretch { pair, .. } => {
                let (a, b) = pair_faces(table, pair);
                let mut parents = vec![face_name(a), face_name(b)];
                let ordinal = by_pair.entry((a, b)).or_insert(0);
                if *ordinal > 0 {
                    parents.push(Name::from_u128(*ordinal as u128));
                }
                *ordinal += 1;
                let n = body.derive_name(
                    "boolean",
                    key,
                    Role::Boolean(BooleanRole::IntersectionEdge),
                    &parents,
                );
                journal.push(Event::Created(n));
                n
            }
        };
        ge_name.insert(g, n);
    }
    let edges: Vec<(EdgeId, Vec<GeId>)> =
        sources.edges.iter().map(|(e, v)| (*e, v.clone())).collect();
    let mut result_edges: BTreeSet<Name> = BTreeSet::new();
    for (e, gs) in &edges {
        let n = if gs.len() == 1 {
            ge_name[&gs[0]]
        } else {
            let mut ps: Vec<Name> = gs.iter().map(|g| ge_name[g]).collect();
            ps.sort();
            ps.dedup();
            let child = body.derive_name("boolean", key, Role::Boolean(BooleanRole::Merged), &ps);
            journal.push(Event::merged_all(ps, child));
            child
        };
        body.rename(EntityId::Edge(*e), n);
        result_edges.insert(n);
    }
    for (op, b) in ops.iter().enumerate() {
        for edge in b.edges() {
            let n = placed[&(op, EntityId::Edge(edge))];
            match kept_of_edge.get(&(op, edge)).map(Vec::len) {
                None | Some(0) => {
                    if !result_edges.contains(&n) {
                        journal.push(Event::Deleted(n));
                    }
                }
                Some(1) => {
                    if result_edges.contains(&n) {
                        let g = kept_of_edge[&(op, edge)][0];
                        let x = &b.edges[edge];
                        if table.edges[g].interval != x.interval {
                            journal.push(Event::Modified(n));
                        }
                    }
                }
                Some(_) => {
                    let mut children: Vec<Name> = kept_of_edge[&(op, edge)]
                        .iter()
                        .map(|g| ge_name[g])
                        .collect();
                    children.sort();
                    journal.push(Event::Split {
                        parent: n,
                        children,
                    });
                }
            }
        }
    }
    // Vertices.
    let mut by_faces: BTreeMap<Vec<Name>, usize> = BTreeMap::new();
    let vertices: Vec<(VertexId, GvId)> = sources.vertices.iter().map(|(v, g)| (*v, *g)).collect();
    let mut result_vertices: BTreeSet<Name> = BTreeSet::new();
    for (v, g) in vertices {
        let gv = &table.vertices[g];
        let n = match gv.originals.first() {
            Some(o) => placed[&(o.op, EntityId::Vertex(o.vertex))],
            None => {
                let mut parents: Vec<Name> = gv.faces.iter().map(|&f| face_name(f)).collect();
                parents.sort();
                parents.dedup();
                let ordinal = by_faces.entry(parents.clone()).or_insert(0);
                if *ordinal > 0 {
                    parents.push(Name::from_u128(*ordinal as u128));
                }
                *ordinal += 1;
                let n = body.derive_name(
                    "boolean",
                    key,
                    Role::Boolean(BooleanRole::IntersectionVertex),
                    &parents,
                );
                journal.push(Event::Created(n));
                n
            }
        };
        body.rename(EntityId::Vertex(v), n);
        result_vertices.insert(n);
    }
    for (op, b) in ops.iter().enumerate() {
        for vertex in b.vertices() {
            let n = placed[&(op, EntityId::Vertex(vertex))];
            if !result_vertices.contains(&n) {
                journal.push(Event::Deleted(n));
            }
        }
    }
    journal
}

/// The faces of other operands whose curves bound a piece of an operand
/// `op` face along global edge `g`.
fn cutters_of(ops: &[Body], table: &Table, g: GeId, op: usize) -> Vec<FaceRef> {
    let mut out = Vec::new();
    for m in &table.edges[g].members {
        match *m {
            Member::Stretch { pair, .. } => {
                let (a, b) = pair_faces(table, pair);
                for f in [a, b] {
                    if f.op != op {
                        out.push(f);
                    }
                }
            }
            Member::Edge(e) if e.op != op => {
                let b = &ops[e.op];
                for fin in b.edge_fins(e.edge) {
                    out.push(FaceRef {
                        op: e.op,
                        face: b.loops[b.fins[fin].loop_].face,
                    });
                }
            }
            Member::Edge(_) => {}
        }
    }
    out
}

/// The two faces of a pair, target or lower tool first.
fn pair_faces(table: &Table, pair: usize) -> (FaceRef, FaceRef) {
    let (a, b) = table.pairs[pair];
    if a.op <= b.op { (a, b) } else { (b, a) }
}
