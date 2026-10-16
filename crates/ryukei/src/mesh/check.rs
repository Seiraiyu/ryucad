//! The validity checker for meshes.

use super::BodyMesh;
use crate::predicates::{Sign, orient2d};
use crate::topo::{Body, EdgeId, FaceId, Sense};
use std::collections::BTreeMap;

/// Something wrong with a mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshCheckFault {
    /// An index past the end of its array, or face ranges that don't tile
    /// the corners and triangles in order.
    IndexOutOfRange,
    /// A face's triangle uses a corner of another face.
    CornerOutsideFace { face: FaceId },
    /// A triangle with two corners at one position.
    DegenerateTriangle { face: FaceId, triangle: u32 },
    /// A triangle that winds against its face in the face's parameters.
    Inverted { face: FaceId, triangle: u32 },
    /// A welded edge used by only one triangle: a hole.
    OpenEdge { a: u32, b: u32 },
    /// A welded edge used by more than two triangles.
    NonManifoldEdge { a: u32, b: u32 },
    /// A welded edge used twice in the same direction.
    InconsistentEdge { a: u32, b: u32 },
    /// An edge polyline that doesn't run from its start vertex's position
    /// to its end vertex's, or doesn't match its parameters.
    EdgeEnds { edge: EdgeId },
}

/// Checks a mesh against the body it was made from: indices in range,
/// every face's triangles on its own corners, wound with its parameters,
/// without degenerate triangles, every welded edge used by exactly two
/// triangles in opposite directions, and edge polylines from vertex to
/// vertex. A mesh with face faults fails here with the holes they leave.
pub fn check_mesh(body: &Body, mesh: &BodyMesh) -> Result<(), Vec<MeshCheckFault>> {
    let mut out = Vec::new();
    let np = mesh.positions.len() as u32;
    if mesh.corners.iter().any(|c| c.position >= np) {
        out.push(MeshCheckFault::IndexOutOfRange);
        return Err(out);
    }
    let (mut next_corner, mut next_triangle) = (0, 0);
    for fm in &mesh.faces {
        if fm.corners.start != next_corner
            || fm.triangles.start != next_triangle
            || fm.corners.end as usize > mesh.corners.len()
            || fm.triangles.end as usize > mesh.triangles.len()
        {
            out.push(MeshCheckFault::IndexOutOfRange);
            return Err(out);
        }
        (next_corner, next_triangle) = (fm.corners.end, fm.triangles.end);
        let reversed = body.face(fm.face).map(|f| f.sense) == Some(Sense::Reversed);
        for t in fm.triangles.clone() {
            let tri = mesh.triangles[t as usize];
            if tri.iter().any(|c| !fm.corners.contains(c)) {
                out.push(MeshCheckFault::CornerOutsideFace { face: fm.face });
                continue;
            }
            let [a, b, c] = tri.map(|i| mesh.corners[i as usize]);
            if a.position == b.position || b.position == c.position || c.position == a.position {
                out.push(MeshCheckFault::DegenerateTriangle {
                    face: fm.face,
                    triangle: t,
                });
            }
            let want = if reversed {
                Sign::Negative
            } else {
                Sign::Positive
            };
            if orient2d(a.uv, b.uv, c.uv) != want {
                out.push(MeshCheckFault::Inverted {
                    face: fm.face,
                    triangle: t,
                });
            }
        }
    }
    if next_corner as usize != mesh.corners.len() || next_triangle as usize != mesh.triangles.len()
    {
        out.push(MeshCheckFault::IndexOutOfRange);
        return Err(out);
    }
    // Each welded edge: how often it is used each way.
    let mut uses: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for [a, b, c] in mesh.welded_triangles() {
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let e = uses.entry((x.min(y), x.max(y))).or_default();
            if x < y {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
    }
    for (&(a, b), &(fwd, back)) in &uses {
        match (fwd, back) {
            (1, 1) => {}
            (f, k) if f + k == 1 => out.push(MeshCheckFault::OpenEdge { a, b }),
            (f, k) if f + k > 2 => out.push(MeshCheckFault::NonManifoldEdge { a, b }),
            _ => out.push(MeshCheckFault::InconsistentEdge { a, b }),
        }
    }
    let vertex_at: BTreeMap<_, u32> = body
        .vertices()
        .enumerate()
        .map(|(i, v)| (v, i as u32))
        .collect();
    for p in &mesh.edges {
        let Some(e) = body.edge(p.edge) else {
            out.push(MeshCheckFault::EdgeEnds { edge: p.edge });
            continue;
        };
        let ends = (p.positions.first(), p.positions.last());
        let want = (vertex_at.get(&e.start), vertex_at.get(&e.end));
        if ends != want
            || p.params.len() != p.positions.len()
            || p.positions.iter().any(|&i| i >= np)
        {
            out.push(MeshCheckFault::EdgeEnds { edge: p.edge });
        }
    }
    if out.is_empty() { Ok(()) } else { Err(out) }
}
