//! Import names: what a file says about each entity's geometry.
//!
//! A file has no identity to offer (instance numbers change on every
//! export), so each vertex, edge and face is named from the caller's key,
//! its [`ImportRole`] and a hash of its geometry as read, in meters:
//!
//! - a vertex, its point;
//! - an edge, its curve's kind and data, its interval, and its two
//!   vertices' points, sorted, so the way it runs doesn't matter;
//! - a face, its surface's kind and data, its sense, and its edges'
//!   hashes, sorted, but for edges it uses twice: a seam is where a
//!   writer cut the face open, which says nothing about the face.
//!
//! The name has no parents beyond that, so a face's name doesn't change
//! because a neighbour did; a face whose boundary changed, or that moved,
//! is a new face. Entities with equal hashes (the equal faces of a
//! patterned body) take ordinals: ranked by their neighbours' hashes,
//! sorted (a vertex's edges, an edge's faces, a face's faces across its
//! edges), and by their order in the shape where that ties too.

use super::{Loop, Shape};
use crate::name::{ImportRole, Name, OpKey, Role};
use crate::topo::dump;
use std::collections::BTreeMap;

/// The import names of a shape's entities, by index: what [`super::heal`]
/// names them by in its journal and its refusals. Healing names what it
/// makes (a sewn edge, a joined vertex) afresh, so those are not here.
#[derive(Clone, Debug, PartialEq)]
pub struct Names {
    pub vertices: Vec<Name>,
    pub edges: Vec<Name>,
    pub faces: Vec<Name>,
}

/// SipHash of a text, as a 128-bit hash.
fn hash(text: &str) -> u128 {
    OpKey::from_bytes(text.as_bytes()).0
}

/// The import names [`super::heal`] gives the entities of `s` under
/// `key`, before anything changes.
pub fn names(key: OpKey, s: &Shape) -> Names {
    let vh: Vec<u128> = s
        .vertices
        .iter()
        .map(|v| hash(&format!("vertex {}", dump::point(v.point))))
        .collect();
    let eh: Vec<u128> = s
        .edges
        .iter()
        .map(|e| {
            let mut ends = [
                dump::point(s.vertices[e.start].point),
                dump::point(s.vertices[e.end].point),
            ];
            ends.sort();
            hash(&format!(
                "edge {} interval={},{} ends={};{}",
                dump::curve(&s.curves[e.curve]),
                dump::bits(e.interval[0]),
                dump::bits(e.interval[1]),
                ends[0],
                ends[1]
            ))
        })
        .collect();
    // Each face's edges and each edge's faces.
    let mut face_edges: Vec<Vec<usize>> = vec![Vec::new(); s.faces.len()];
    let mut edge_faces: Vec<Vec<usize>> = vec![Vec::new(); s.edges.len()];
    let mut vertex_edges: Vec<Vec<usize>> = vec![Vec::new(); s.vertices.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    face_edges[k].push(u.edge);
                    edge_faces[u.edge].push(k);
                }
            }
        }
    }
    // A face's own seams: edges it uses twice.
    let seams: Vec<Vec<usize>> = face_edges
        .iter()
        .map(|list| {
            let mut x = list.clone();
            x.sort_unstable();
            x.windows(2)
                .filter(|w| w[0] == w[1])
                .map(|w| w[0])
                .collect()
        })
        .collect();
    for (k, e) in s.edges.iter().enumerate() {
        vertex_edges[e.start].push(k);
        if e.end != e.start {
            vertex_edges[e.end].push(k);
        }
    }
    let fh: Vec<u128> = s
        .faces
        .iter()
        .enumerate()
        .map(|(k, f)| {
            let mut edges: Vec<String> = face_edges[k]
                .iter()
                .filter(|e| !seams[k].contains(e))
                .map(|&e| format!("{:032x}", eh[e]))
                .collect();
            edges.sort();
            hash(&format!(
                "face {} sense={:?} edges={}",
                dump::surface(&s.surfaces[f.surface]),
                f.sense,
                edges.join(",")
            ))
        })
        .collect();
    let sorted = |list: &[usize], h: &[u128]| {
        let mut x: Vec<u128> = list.iter().map(|&i| h[i]).collect();
        x.sort_unstable();
        x
    };
    let face_faces: Vec<Vec<u128>> = (0..s.faces.len())
        .map(|k| {
            let others: Vec<usize> = face_edges[k]
                .iter()
                .flat_map(|&e| edge_faces[e].iter().copied())
                .filter(|&g| g != k)
                .collect();
            sorted(&others, &fh)
        })
        .collect();
    let vertex_near: Vec<Vec<u128>> = vertex_edges.iter().map(|l| sorted(l, &eh)).collect();
    let edge_near: Vec<Vec<u128>> = edge_faces.iter().map(|l| sorted(l, &fh)).collect();
    Names {
        vertices: named(key, ImportRole::Vertex, &vh, &vertex_near),
        edges: named(key, ImportRole::Edge, &eh, &edge_near),
        faces: named(key, ImportRole::Face, &fh, &face_faces),
    }
}

/// Names from hashes, with ordinals among equal hashes ranked by
/// neighbours and then by index.
pub(super) fn named(
    key: OpKey,
    role: ImportRole,
    hashes: &[u128],
    near: &[Vec<u128>],
) -> Vec<Name> {
    let mut groups: BTreeMap<u128, Vec<usize>> = BTreeMap::new();
    for (i, &h) in hashes.iter().enumerate() {
        groups.entry(h).or_default().push(i);
    }
    let mut ordinal = vec![0u128; hashes.len()];
    for list in groups.values_mut() {
        list.sort_by(|&a, &b| near[a].cmp(&near[b]).then(a.cmp(&b)));
        for (o, &i) in list.iter().enumerate() {
            ordinal[i] = o as u128;
        }
    }
    hashes
        .iter()
        .zip(&ordinal)
        .map(|(&h, &o)| {
            Name::derive(
                key,
                Role::Import(role),
                &[Name::from_u128(h), Name::from_u128(o)],
            )
        })
        .collect()
}
