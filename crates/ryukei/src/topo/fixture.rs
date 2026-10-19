//! Test bodies built directly into the arrays, for testing the storage
//! and the checker without the Euler operators.

use super::*;
use crate::geom::{Frame, Line, Plane, Point3};
use crate::name::{OpKey, Role};
use crate::tol::Tolerance;

/// A closed polyhedron from vertex positions and faces given as vertex
/// cycles, counterclockwise seen from outside. Each entity's name uses a
/// key made from its index; every recipe reads `fixture#0/<role>`.
pub(crate) fn polyhedron(points: &[Point3], faces: &[&[usize]]) -> Body {
    let mut b = Body::new();
    let named = |b: &mut Body, i: usize, role: Role, entity: EntityId| {
        let key = OpKey(i as u128);
        let n = Name::derive(key, role, &[]);
        b.names.insert(n, entity);
        if let Some(r) = &mut b.recipes {
            r.insert(n, format!("fixture#0/{role}"));
        }
        n
    };
    let region = b.regions.push(Region {
        kind: RegionKind::Solid,
        first_shell: None,
    });
    let shell = ShellId::from_index(0);
    b.shells.push(Shell {
        region,
        next: shell,
        prev: shell,
        first_face: Some(FaceId::from_index(0)),
    });
    b.regions[region].first_shell = Some(shell);
    let verts: Vec<VertexId> = points
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let id = VertexId::from_index(i);
            let name = named(&mut b, i, Role::NewVertex, EntityId::Vertex(id));
            b.vertices.push(Vertex {
                point: p,
                tolerance: Tolerance::RESOLUTION,
                edge: None,
                lone: None,
                name,
            })
        })
        .collect();
    let mut edge_of: BTreeMap<(usize, usize), EdgeId> = BTreeMap::new();
    let nf = faces.len();
    for (fi, cycle) in faces.iter().enumerate() {
        let (p0, p1, p2) = (points[cycle[0]], points[cycle[1]], points[cycle[2]]);
        let normal = (p1 - p0).cross(p2 - p0);
        let surface = b.add_surface(Surface::Plane(Plane::new(
            Frame::new(p0, normal, p1 - p0).unwrap(),
        )));
        let face = FaceId::from_index(fi);
        let lp = LoopId::from_index(fi);
        let name = named(&mut b, 1000 + fi, Role::NewFace, EntityId::Face(face));
        let first_fin = FinId::from_index(b.fins.slot_count());
        b.faces.push(Face {
            shell,
            next: FaceId::from_index((fi + 1) % nf),
            prev: FaceId::from_index((fi + nf - 1) % nf),
            first_loop: Some(lp),
            surface,
            sense: Sense::Forward,
            name,
        });
        b.loops.push(Loop {
            face,
            next: lp,
            prev: lp,
            first_fin: Some(first_fin),
            vertex: None,
        });
        let n = cycle.len();
        for k in 0..n {
            let (a, c) = (cycle[k], cycle[(k + 1) % n]);
            let fin = FinId::from_index(first_fin.index() + k);
            let key = (a.min(c), a.max(c));
            let (edge, sense) = match edge_of.get(&key) {
                Some(&e) => {
                    // Second use: pair it with the first.
                    let other = b.edges[e].fin;
                    b.fins[other].around = fin;
                    (e, Sense::Reversed)
                }
                None => {
                    let curve =
                        b.add_curve(Curve::Line(Line::through(points[a], points[c]).unwrap()));
                    let len = points[a].distance(points[c]);
                    let e = EdgeId::from_index(b.edges.slot_count());
                    let name = named(&mut b, 2000 + e.index(), Role::NewEdge, EntityId::Edge(e));
                    b.edges.push(Edge {
                        fin,
                        start: verts[a],
                        end: verts[c],
                        curve,
                        interval: [0.0, len],
                        tolerance: Tolerance::RESOLUTION,
                        name,
                    });
                    b.vertices[verts[a]].edge.get_or_insert(e);
                    b.vertices[verts[c]].edge.get_or_insert(e);
                    edge_of.insert(key, e);
                    (e, Sense::Forward)
                }
            };
            let around = b.edges[edge].fin;
            b.fins.push(Fin {
                loop_: lp,
                edge,
                sense,
                next: FinId::from_index(first_fin.index() + (k + 1) % n),
                prev: FinId::from_index(first_fin.index() + (k + n - 1) % n),
                around,
                pcurve: None,
            });
        }
    }
    b
}

/// The unit right-angled tetrahedron.
pub(crate) fn tetrahedron() -> Body {
    polyhedron(
        &[
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ],
        &[&[0, 2, 1], &[0, 1, 3], &[1, 2, 3], &[0, 3, 2]],
    )
}
