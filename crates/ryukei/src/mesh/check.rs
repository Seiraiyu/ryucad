//! The validity checker for meshes.

use super::BodyMesh;
use crate::predicates::{Sign, orient2d};
use crate::topo::{Body, EdgeId, FaceId, Sense};
use std::collections::{BTreeMap, BTreeSet};

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
    /// A welded edge used by only one triangle, off a sheet's boundary: a hole.
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
/// triangles in opposite directions (once, on a laminar edge of a sheet
/// or an edge of a face of no width, meshed as no triangles),
/// and edge polylines from vertex to
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
    // A sheet's laminar edges bound one face: their segments have one use.
    // So do the edges of a face of no width meshed as no triangles (see
    // the mesher's `no_width`): the faces either side meet along both, a
    // slit of no width. Only such a face: one whose triangles were all
    // lost another way is a hole, and reported as one.
    let empty: BTreeSet<FaceId> = mesh
        .faces
        .iter()
        .filter(|fm| fm.triangles.is_empty() && slit_face(body, mesh, fm.face))
        .map(|fm| fm.face)
        .collect();
    let mut boundary: BTreeSet<(u32, u32)> = BTreeSet::new();
    for p in &mesh.edges {
        let slit = !empty.is_empty()
            && body
                .edge_fins(p.edge)
                .any(|f| empty.contains(&body.loops[body.fins[f].loop_].face));
        if body.is_laminar(p.edge) || slit {
            for w in p.positions.windows(2) {
                boundary.insert((w[0].min(w[1]), w[0].max(w[1])));
            }
        }
    }
    for (&(a, b), &(fwd, back)) in &uses {
        match (fwd, back) {
            (1, 1) => {}
            (f, k) if f + k == 1 && boundary.contains(&(a, b)) => {}
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

/// Whether face `f`, meshed as no triangles, is a face of no width (see
/// the mesher's `no_width`), re-derived from the body and the mesh: one
/// loop of two edges between the same two vertices, each edge's samples
/// within both edges' tolerances of the other edge's curve. The slack is
/// twice the larger sagitta either polyline has from its own curve (its
/// segments' midpoints), so a lens of a line and a shallow arc, whose
/// middle sits a whole sagitta off the line, isn't one. A face whose
/// triangles were lost another way is a hole (#796).
fn slit_face(body: &Body, mesh: &BodyMesh, f: FaceId) -> bool {
    use crate::geom::Point3;
    let mut loops = body.face_loops(f);
    let (Some(l), None) = (loops.next(), loops.next()) else {
        return false;
    };
    let fins: Vec<_> = body.loop_fins(l).collect();
    let [x, y] = fins[..] else {
        return false;
    };
    let (ex, ey) = (body.fins[x].edge, body.fins[y].edge);
    let (Some(a), Some(c)) = (body.edge(ex), body.edge(ey)) else {
        return false;
    };
    let ends = |s, e| if s < e { [s, e] } else { [e, s] };
    if ex == ey || a.start == a.end || ends(a.start, a.end) != ends(c.start, c.end) {
        return false;
    }
    let line = |e: EdgeId| -> Option<Vec<Point3>> {
        let p = mesh.edges.iter().find(|p| p.edge == e)?;
        p.positions
            .iter()
            .map(|&k| mesh.positions.get(k as usize).copied())
            .collect()
    };
    let (Some(pa), Some(pc)) = (line(ex), line(ey)) else {
        return false;
    };
    let (ca, cc) = (&body.curves[a.curve], &body.curves[c.curve]);
    let sag = |q: &[Point3], curve: &crate::geom::Curve| {
        q.windows(2)
            .map(|s| curve.distance(s[0].lerp(s[1], 0.5)))
            .fold(0.0, f64::max)
    };
    let reach = a.tolerance.get()
        + c.tolerance.get()
        + 2.0 * sag(&pa, ca).max(sag(&pc, cc))
        + crate::tol::LINEAR_RESOLUTION;
    pa.iter().all(|&p| cc.distance(p) <= reach) && pc.iter().all(|&p| ca.distance(p) <= reach)
}
