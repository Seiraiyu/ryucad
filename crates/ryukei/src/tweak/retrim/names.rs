//! The re-trim's names: booleans' names for pieces, cuts and merges;
//! old names back where faces meet as they did; `EditRole` names for
//! what is new. The journal speaks of the input's names only.

use super::body::WorkBody;
use crate::name::{EditRole, Event, Journal, Name, OpKey, Role};
use crate::topo::{Body, EdgeId, EntityId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// The `EditRole::Edge` name of the `k`th of `of` new edges between faces
/// `a` and `b`: their names sorted, and an ordinal where `of` is more
/// than one.
pub(crate) fn edge_name(key: OpKey, a: Name, b: Name, k: usize, of: usize) -> Name {
    let (a, b) = (a.min(b), a.max(b));
    if of > 1 {
        Name::derive(
            key,
            Role::Edit(EditRole::Edge),
            &[a, b, Name::from_u128(k as u128)],
        )
    } else {
        Name::derive(key, Role::Edit(EditRole::Edge), &[a, b])
    }
}

/// An edge's faces' names, sorted, where it has two.
fn faces_of_edge(b: &Body, e: EdgeId) -> Option<(Name, Name)> {
    let mut n: Vec<Name> = b
        .edge_fins(e)
        .map(|fin| b.faces[b.loops[b.fins[fin].loop_].face].name)
        .collect();
    n.sort();
    match n.as_slice() {
        [x, y] => Some((*x, *y)),
        _ => None,
    }
}

/// A vertex's faces' names, sorted and once each.
fn faces_of_vertex(b: &Body, v: VertexId) -> Vec<Name> {
    let set: BTreeSet<Name> = b
        .vertex_edges(v)
        .into_iter()
        .flat_map(|e| {
            b.edge_fins(e)
                .map(|fin| b.faces[b.loops[b.fins[fin].loop_].face].name)
                .collect::<Vec<_>>()
        })
        .collect();
    set.into_iter().collect()
}

/// Names `out`'s new edges and vertices and gives the journal of what
/// happened to `input`'s names: the boolean's `journal` less the
/// scaffold, its `Created` names renamed.
pub(crate) fn finish(
    key: OpKey,
    input: &Body,
    w: &WorkBody,
    out: &mut Body,
    journal: Journal,
    moved: &BTreeSet<Name>,
) -> Journal {
    let created: BTreeSet<Name> = journal
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::Created(n) => Some(*n),
            _ => None,
        })
        .collect();
    let mut rename: BTreeMap<Name, (Name, bool)> = BTreeMap::new(); // tmp -> (final, restored)
    // Edges.
    let mut old_edges: BTreeMap<(Name, Name), Vec<Name>> = BTreeMap::new();
    for e in input.edges() {
        if let Some(k) = faces_of_edge(input, e) {
            old_edges.entry(k).or_default().push(input.edges[e].name);
        }
    }
    let mut new_edges: BTreeMap<(Name, Name), Vec<EdgeId>> = BTreeMap::new();
    for e in out.edges() {
        if created.contains(&out.edges[e].name)
            && let Some(k) = faces_of_edge(out, e)
        {
            new_edges.entry(k).or_default().push(e);
        }
    }
    for ((a, b), list) in &new_edges {
        let old = old_edges.get(&(*a, *b));
        match (old.map(Vec::as_slice), list.as_slice()) {
            (Some([n]), [e]) if out.by_name(*n).is_none() => {
                rename.insert(out.edges[*e].name, (*n, true));
                out.rename(EntityId::Edge(*e), *n);
            }
            _ => {
                for (k, &e) in list.iter().enumerate() {
                    let n = edge_name(key, *a, *b, k, list.len());
                    rename.insert(out.edges[e].name, (n, false));
                    out.rename(EntityId::Edge(e), n);
                }
            }
        }
    }
    // Vertices, after the edges.
    let mut old_vertices: BTreeMap<Vec<Name>, Vec<Name>> = BTreeMap::new();
    for v in input.vertices() {
        old_vertices
            .entry(faces_of_vertex(input, v))
            .or_default()
            .push(input.vertices[v].name);
    }
    let mut new_vertices: BTreeMap<Vec<Name>, Vec<VertexId>> = BTreeMap::new();
    for v in out.vertices() {
        if created.contains(&out.vertices[v].name) {
            new_vertices
                .entry(faces_of_vertex(out, v))
                .or_default()
                .push(v);
        }
    }
    for (faces, list) in &new_vertices {
        match (old_vertices.get(faces).map(Vec::as_slice), list.as_slice()) {
            (Some([n]), [v]) if out.by_name(*n).is_none() => {
                rename.insert(out.vertices[*v].name, (*n, true));
                out.rename(EntityId::Vertex(*v), *n);
            }
            _ => {
                for (k, &v) in list.iter().enumerate() {
                    let mut edges: Vec<Name> = out
                        .vertex_edges(v)
                        .into_iter()
                        .map(|e| out.edges[e].name)
                        .collect();
                    edges.sort();
                    if list.len() > 1 {
                        edges.push(Name::from_u128(k as u128));
                    }
                    let n = Name::derive(key, Role::Edit(EditRole::Vertex), &edges);
                    rename.insert(out.vertices[v].name, (n, false));
                    out.rename(EntityId::Vertex(v), n);
                }
            }
        }
    }
    // The journal: scaffold dropped, created names renamed or restored.
    let mut j = Journal::new();
    let mentioned = |n: &Name| w.scaffold.contains(n);
    for e in journal.events() {
        match e {
            Event::Created(n) => match rename.get(n) {
                Some(&(m, true)) => j.push(Event::Modified(m)),
                Some(&(m, false)) => j.push(Event::Created(m)),
                None => j.push(e.clone()),
            },
            Event::Deleted(n) | Event::Modified(n) if mentioned(n) => {}
            Event::Modified(n) if unchanged(input, out, moved, *n) => {}
            Event::Split { parent, .. } if mentioned(parent) => {}
            other => j.push(other.clone()),
        }
    }
    // The input's edges and vertices of the region that didn't come back.
    for e in input.edges() {
        let n = input.edges[e].name;
        if out.by_name(n).is_none() && !j.events().iter().any(|x| names(x).contains(&n)) {
            j.push(Event::Deleted(n));
        }
    }
    for v in input.vertices() {
        let n = input.vertices[v].name;
        if out.by_name(n).is_none() && !j.events().iter().any(|x| names(x).contains(&n)) {
            j.push(Event::Deleted(n));
        }
    }
    j
}

/// The names of the edges bounding face `f` of `b`, sorted.
fn boundary(b: &Body, f: crate::topo::FaceId) -> Vec<Name> {
    let mut n: Vec<Name> = b
        .face_loops(f)
        .flat_map(|l| b.loop_fins(l).map(|fin| b.edges[b.fins[fin].edge].name))
        .collect();
    n.sort();
    n
}

/// Whether the entity named `n` came through as it was, the fixed
/// engine's rule (`kept_changes`): a face no motion moved whose boundary
/// is the same edges, an edge on the same curve between the same points,
/// a vertex at the same point.
fn unchanged(input: &Body, out: &Body, moved: &BTreeSet<Name>, n: Name) -> bool {
    match (input.by_name(n), out.by_name(n)) {
        (Some(EntityId::Face(a)), Some(EntityId::Face(b))) => {
            !moved.contains(&n) && boundary(input, a) == boundary(out, b)
        }
        (Some(EntityId::Edge(a)), Some(EntityId::Edge(b))) => {
            let (x, y) = (&input.edges[a], &out.edges[b]);
            input.curves[x.curve] == out.curves[y.curve]
                && x.interval == y.interval
                && input.vertices[x.start].point == out.vertices[y.start].point
                && input.vertices[x.end].point == out.vertices[y.end].point
        }
        (Some(EntityId::Vertex(a)), Some(EntityId::Vertex(b))) => {
            input.vertices[a].point == out.vertices[b].point
        }
        _ => false,
    }
}

/// The names an event speaks of.
fn names(e: &Event) -> Vec<Name> {
    match e {
        Event::Created(n) | Event::Deleted(n) | Event::Modified(n) => vec![*n],
        Event::Split { parent, children } => {
            let mut v = vec![*parent];
            v.extend(children);
            v
        }
        Event::Merged { parents, child } => {
            let mut v = parents.clone();
            v.push(*child);
            v
        }
        Event::Placed { from, to, .. } => vec![*from, *to],
        Event::Replaced { old, new } => {
            let mut v = vec![*old];
            v.extend(new);
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_between_the_same_faces_take_ordinals_in_order() {
        let (a, b) = (Name::from_u128(1), Name::from_u128(2));
        let k = OpKey(3);
        let one = edge_name(k, a, b, 0, 1);
        assert_eq!(one, Name::derive(k, Role::Edit(EditRole::Edge), &[a, b]));
        let first = edge_name(k, b, a, 0, 2);
        let second = edge_name(k, b, a, 1, 2);
        assert_ne!(first, second);
        assert_eq!(
            first,
            Name::derive(k, Role::Edit(EditRole::Edge), &[a, b, Name::from_u128(0)])
        );
    }
}
