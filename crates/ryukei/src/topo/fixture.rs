//! Test bodies built directly into the arrays, for testing the storage
//! and the checker without the Euler operators.

use super::*;
use crate::geom::{Frame, Line, Plane, Point3};
use crate::name::{OpKey, Role};
use crate::tol::Tolerance;

/// A closed polyhedron from vertex positions and faces given as vertex
/// cycles, counterclockwise seen from outside, with exact pcurves. Each
/// entity's name uses a key made from its index; every recipe reads
/// `fixture#0/<role>`.
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
    crate::pcurve::fill(&mut b).expect("planar faces with straight edges");
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

/// Moves `faces` of the first solid region's only shell to a new shell of
/// the same region, relinking both rings of faces in ID order.
pub(crate) fn split_shell(b: &mut Body, faces: &[FaceId]) {
    let region = RegionId::from_index(1);
    let first = ShellId::from_index(0);
    let new = b.shells.push(Shell {
        region,
        next: first,
        prev: first,
        first_face: None,
    });
    b.shells[first].next = new;
    b.shells[first].prev = new;
    let all: Vec<FaceId> = b.faces().collect();
    for (shell, members) in [
        (
            first,
            all.iter()
                .copied()
                .filter(|f| !faces.contains(f))
                .collect::<Vec<_>>(),
        ),
        (new, faces.to_vec()),
    ] {
        let n = members.len();
        for (i, &f) in members.iter().enumerate() {
            let face = &mut b.faces[f];
            face.shell = shell;
            face.next = members[(i + 1) % n];
            face.prev = members[(i + n - 1) % n];
        }
        b.shells[shell].first_face = Some(members[0]);
    }
}

/// The corners of the box `[lo, hi]³` and its faces as vertex cycles,
/// counterclockwise seen from outside (or from inside, `inward`), with
/// indices starting at `base`.
pub(crate) fn cube(lo: f64, hi: f64, base: usize, inward: bool) -> (Vec<Point3>, Vec<Vec<usize>>) {
    let points = (0..8)
        .map(|i| {
            let pick = |bit: usize| if i & bit == 0 { lo } else { hi };
            Point3::new(pick(1), pick(2), pick(4))
        })
        .collect();
    let faces: Vec<Vec<usize>> = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ]
    .iter()
    .map(|c| {
        let mut c: Vec<usize> = c.iter().map(|&i| base + i).collect();
        if inward {
            c.reverse();
        }
        c
    })
    .collect();
    (points, faces)
}

/// A cube of side 3 with a cube of side 1 hollowed out of its middle: one
/// solid region with an outer shell and a void shell.
pub(crate) fn hollow_cube() -> Body {
    let (mut points, mut faces) = cube(0.0, 3.0, 0, false);
    let (inner, holes) = cube(1.0, 2.0, 8, true);
    points.extend(inner);
    faces.extend(holes);
    let cycles: Vec<&[usize]> = faces.iter().map(Vec::as_slice).collect();
    let mut b = polyhedron(&points, &cycles);
    let void: Vec<FaceId> = (6..12).map(FaceId::from_index).collect();
    split_shell(&mut b, &void);
    b
}
