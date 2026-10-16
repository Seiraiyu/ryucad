//! Assembly: the per-edge samples and per-face triangulations as one
//! [`BodyMesh`], in ID order, whatever order they were made in.

use super::domain::Anchor;
use super::edge::EdgeSamples;
use super::refine::position;
use super::uv::FaceGeom;
use super::{BodyMesh, Corner, EdgePolyline, FaceFault, FaceMesh, FaceWork};
use crate::geom::UnitVector3;
use crate::topo::Body;
use std::collections::BTreeMap;

pub(super) fn assemble(
    b: &Body,
    geoms: &[Option<FaceGeom>],
    samples: &[Option<EdgeSamples>],
    work: &[Option<FaceWork>],
) -> BodyMesh {
    let mut mesh = BodyMesh {
        positions: Vec::new(),
        corners: Vec::new(),
        triangles: Vec::new(),
        faces: Vec::new(),
        edges: Vec::new(),
        faults: Vec::new(),
    };
    // Vertices, then edge samples.
    let mut vertex_at = vec![u32::MAX; b.vertices.slot_count()];
    for v in b.vertices() {
        vertex_at[v.index()] = mesh.positions.len() as u32;
        mesh.positions.push(b.vertices[v].point);
    }
    let mut sample_at: Vec<Vec<u32>> = vec![Vec::new(); b.edges.slot_count()];
    for e in b.edges() {
        let s = samples[e.index()].as_ref().expect("sampled");
        let edge = &b.edges[e];
        let n = s.points.len() - 1;
        let mut at = vec![vertex_at[edge.start.index()]];
        for p in &s.points[1..n] {
            at.push(mesh.positions.len() as u32);
            mesh.positions.push(*p);
        }
        at.push(vertex_at[edge.end.index()]);
        mesh.edges.push(EdgePolyline {
            edge: e,
            positions: at.clone(),
            params: s.params.clone(),
        });
        sample_at[e.index()] = at;
    }
    let fixed = |a: Anchor| match a {
        Anchor::Vertex(v) => b.vertices[v].point,
        Anchor::Sample(e, k) => samples[e.index()].as_ref().expect("sampled").points[k as usize],
        Anchor::Free => unreachable!("free points have no fixed position"),
    };
    for f in b.faces() {
        let w = work[f.index()].as_ref().expect("meshed");
        if let Some(kind) = w.fault {
            mesh.faults.push(FaceFault { face: f, kind });
            continue;
        }
        let d = w.domain.as_ref().expect("a domain");
        let g = geoms[f.index()].as_ref().expect("live face");
        let corner_start = mesh.corners.len() as u32;
        let triangle_start = mesh.triangles.len() as u32;
        // Free classes get a position the first time they are used.
        let mut free_at: BTreeMap<u32, u32> = BTreeMap::new();
        let mut corner_of: BTreeMap<u32, u32> = BTreeMap::new();
        // The face's triangles by position.
        let mut kept: Vec<([u32; 3], [u32; 3])> = Vec::new();
        for t in 0..d.cdt.triangle_count() as u32 {
            if !d.inside[t as usize] {
                continue;
            }
            let c = d.cdt.corners(t);
            let at = c.map(|v| match d.classes.anchor(v) {
                Anchor::Vertex(x) => vertex_at[x.index()],
                Anchor::Sample(e, k) => sample_at[e.index()][k as usize],
                Anchor::Free => *free_at.entry(d.classes.root(v)).or_insert_with(|| {
                    mesh.positions.push(position(d, g, v, &fixed));
                    (mesh.positions.len() - 1) as u32
                }),
            });
            // A triangle with two corners at one position (at a pole, or
            // across a seam) has no area: its two other edges pair up.
            if at[0] == at[1] || at[1] == at[2] || at[2] == at[0] {
                continue;
            }
            kept.push((c, at));
        }
        // Near a pole a point can meet the pole's row at two places, with
        // points between: once the row is welded into one position, the
        // triangles between come in pairs on the same three positions,
        // wound opposite ways. Each pair covers nothing; together they
        // would make the edges to the pole non-manifold. Drop them.
        let key = |at: [u32; 3]| {
            let mut k = at;
            k.sort_unstable();
            // Even when `at` is an even permutation of the sorted order.
            let even = (at[0] == k[0] && at[1] == k[1])
                || (at[0] == k[1] && at[1] == k[2])
                || (at[0] == k[2] && at[1] == k[0]);
            (k, even)
        };
        let mut wound: BTreeMap<[u32; 3], Vec<(usize, bool)>> = BTreeMap::new();
        for (i, &(_, at)) in kept.iter().enumerate() {
            let (k, even) = key(at);
            wound.entry(k).or_default().push((i, even));
        }
        let mut drop = vec![false; kept.len()];
        for list in wound.values() {
            let evens: Vec<usize> = list.iter().filter(|x| x.1).map(|x| x.0).collect();
            let odds: Vec<usize> = list.iter().filter(|x| !x.1).map(|x| x.0).collect();
            for (&a, &b) in evens.iter().zip(&odds) {
                drop[a] = true;
                drop[b] = true;
            }
        }
        for (i, &(c, at)) in kept.iter().enumerate() {
            if drop[i] {
                continue;
            }
            let ids = [0, 1, 2].map(|k| {
                *corner_of.entry(c[k]).or_insert_with(|| {
                    let uv = d.uv[c[k] as usize];
                    // No normal at this point (a NURBS pole): the flat
                    // triangle's, which winds like the parameters.
                    let natural = g.surface.normal(uv.x, uv.y).unwrap_or_else(|| {
                        let p = at.map(|i| mesh.positions[i as usize]);
                        (p[1] - p[0])
                            .cross(p[2] - p[0])
                            .normalize()
                            .unwrap_or(UnitVector3::Z)
                    });
                    let normal = if g.forward { natural } else { -natural };
                    mesh.corners.push(Corner {
                        position: at[k],
                        uv,
                        normal,
                    });
                    (mesh.corners.len() - 1) as u32
                })
            });
            mesh.triangles.push(if g.forward {
                ids
            } else {
                [ids[0], ids[2], ids[1]]
            });
        }
        mesh.faces.push(FaceMesh {
            face: f,
            corners: corner_start..mesh.corners.len() as u32,
            triangles: triangle_start..mesh.triangles.len() as u32,
        });
    }
    mesh
}
