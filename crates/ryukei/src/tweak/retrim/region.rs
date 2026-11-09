//! The region a re-trim works in: the changed faces, every face sharing
//! an edge or a vertex with one, and the frontier between it and the
//! faces outside.

use crate::topo::{Body, EdgeId, FaceId, VertexId};
use std::collections::{BTreeMap, BTreeSet};

/// A re-trim's region.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Region {
    /// The faces whose surfaces change.
    pub(crate) changed: BTreeSet<FaceId>,
    /// The changed faces and their ring.
    pub(crate) faces: BTreeSet<FaceId>,
    /// Edges between a face of the region and one outside it.
    pub(crate) frontier: BTreeSet<EdgeId>,
}

/// The faces using edge `e`.
fn edge_faces(body: &Body, e: EdgeId) -> impl Iterator<Item = FaceId> + '_ {
    body.edge_fins(e)
        .map(move |fin| body.loops[body.fins[fin].loop_].face)
}

/// The faces round vertex `v`.
fn vertex_faces(body: &Body, v: VertexId) -> BTreeSet<FaceId> {
    body.vertex_edges(v)
        .into_iter()
        .flat_map(|e| edge_faces(body, e).collect::<Vec<_>>())
        .collect()
}

/// The vertices of face `f` (its edges' ends, and a lone vertex).
fn face_vertices(body: &Body, f: FaceId) -> BTreeSet<VertexId> {
    let mut out = BTreeSet::new();
    for l in body.face_loops(f) {
        if let Some(v) = body.loops[l].vertex {
            out.insert(v);
        }
        for fin in body.loop_fins(l) {
            let e = &body.edges[body.fins[fin].edge];
            out.insert(e.start);
            out.insert(e.end);
        }
    }
    out
}

/// The region of `changed`: with every face sharing a vertex with a
/// changed face (an edge's faces share its vertices), grown at each pinch
/// until the frontier is rings: each vertex on it ends exactly two of its
/// edges (the working body is a sheet, and a sheet's boundary can't touch
/// itself at a vertex).
pub(crate) fn region(body: &Body, changed: &BTreeSet<FaceId>) -> Region {
    let mut faces = changed.clone();
    for &f in changed {
        for v in face_vertices(body, f) {
            faces.extend(vertex_faces(body, v));
        }
    }
    closed(body, changed.clone(), faces)
}

/// `faces` grown at pinches, and its frontier.
fn closed(body: &Body, changed: BTreeSet<FaceId>, mut faces: BTreeSet<FaceId>) -> Region {
    loop {
        let frontier = frontier(body, &faces);
        let mut ends: BTreeMap<VertexId, usize> = BTreeMap::new();
        for &e in &frontier {
            *ends.entry(body.edges[e].start).or_default() += 1;
            *ends.entry(body.edges[e].end).or_default() += 1;
        }
        let pinched: Vec<VertexId> = ends
            .into_iter()
            .filter(|&(_, n)| n > 2)
            .map(|(v, _)| v)
            .collect();
        if pinched.is_empty() {
            return Region {
                changed,
                faces,
                frontier,
            };
        }
        for v in pinched {
            faces.extend(vertex_faces(body, v));
        }
    }
}

/// The edges with a face in `faces` and a face outside it.
fn frontier(body: &Body, faces: &BTreeSet<FaceId>) -> BTreeSet<EdgeId> {
    body.edges()
        .filter(|&e| {
            let (inside, outside): (Vec<FaceId>, Vec<FaceId>) =
                edge_faces(body, e).partition(|f| faces.contains(f));
            !inside.is_empty() && !outside.is_empty()
        })
        .collect()
}

impl Region {
    /// The region grown by one more ring: every face sharing a vertex
    /// with one of its faces.
    pub(crate) fn grown(&self, body: &Body) -> Region {
        let mut faces = self.faces.clone();
        for &f in &self.faces {
            for v in face_vertices(body, f) {
                faces.extend(vertex_faces(body, v));
            }
        }
        closed(body, self.changed.clone(), faces)
    }

    /// Whether the region holds every face of `body`.
    pub(crate) fn whole(&self, body: &Body) -> bool {
        body.faces().all(|f| self.faces.contains(&f))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{block, bossed, face_of};
    use super::*;
    use crate::name::FaceRole;

    #[test]
    fn a_boxs_top_takes_its_sides_and_leaves_the_bottom_outside() {
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let r = region(&b, &BTreeSet::from([top]));
        assert_eq!(r.changed, BTreeSet::from([top]));
        assert_eq!(r.faces.len(), 5);
        assert!(!r.faces.contains(&face_of(&b, FaceRole::MinusZ)));
        // The bottom's four edges.
        assert_eq!(r.frontier.len(), 4);
    }

    #[test]
    fn a_bosss_faces_take_the_plates_top() {
        let b = bossed();
        let boss: BTreeSet<FaceId> = b
            .faces()
            .filter(|&f| {
                matches!(
                    b.surfaces[b.faces[f].surface],
                    crate::geom::Surface::Cylinder(_)
                )
            })
            .collect();
        let r = region(&b, &boss);
        // The boss's side, and through its foot the plate's top; the
        // boss's top shares an edge with the side.
        assert_eq!(r.faces.len(), 3);
        assert_eq!(r.frontier.len(), 4);
    }

    #[test]
    fn grown_by_a_ring_takes_every_neighbour() {
        let b = block();
        let top = face_of(&b, FaceRole::PlusZ);
        let r = region(&b, &BTreeSet::from([top])).grown(&b);
        assert_eq!(r.faces.len(), 6);
        assert!(r.frontier.is_empty());
        assert!(r.whole(&b));
    }
}
