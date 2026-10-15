//! The validity checker.
//!
//! [`check_topology`] checks structure only: references, rings, loops,
//! fins, shells, the Euler–Poincaré formula and names. It holds for every
//! state an Euler operator leaves behind, including the half-built bodies
//! inside a primitive, so the operators debug-assert it.
//! [`check`] adds geometry and orientation (Tasks to come): it holds for
//! finished bodies.
//!
//! Neither panics or loops on a corrupted body: every lookup is checked and
//! every walk is bounded. Faults come out in a fixed order: by check, then
//! by entity in array order.

mod topology;

use crate::name::Name;
use crate::topo::{Body, EdgeId, EntityId, FaceId, FinId, LoopId, RegionId, ShellId, VertexId};

/// One thing wrong with a body.
#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    /// Region 0 is missing or not void, another region is void, or a solid
    /// region has no shells.
    BadRegion { region: RegionId },
    /// A live record refers to an entity that is not live.
    DeadReference { from: EntityId, to: EntityId },
    /// A face's surface or an edge's curve is not live.
    DeadGeometry { from: EntityId },
    /// The ring of members this entity owns is broken: a member is dead or
    /// claims another owner, a `prev` does not point back, the ring does
    /// not close, or a member is missing from it.
    BrokenRing { owner: EntityId },
    /// A loop must hold fins or a lone vertex, not both or neither, and a
    /// lone vertex must point back at it.
    BadLoop { loop_: LoopId },
    /// This fin ends where the next fin in its loop does not start.
    Disjoint { fin: FinId },
    /// An edge with other than two fins (phase 1 bodies are manifold).
    NotManifold { edge: EdgeId, fins: usize },
    /// Both fins of an edge run the same way along it.
    SameSense { edge: EdgeId },
    /// A vertex's edge does not touch it, or it is neither on an edge nor
    /// the lone vertex of its loop.
    BadVertex { vertex: VertexId },
    /// A face with no loops on a surface that does not close up.
    Loopless { face: FaceId },
    /// An edge's fins are on faces in different shells.
    EdgeAcrossShells { edge: EdgeId },
    /// A shell's faces are not all connected through edges.
    Disconnected { shell: ShellId },
    /// A shell's Euler characteristic is odd or above 2.
    EulerPoincare { shell: ShellId, chi: i64 },
    /// Two live entities share a name.
    DuplicateName { name: Name },
    /// The name index disagrees with the entities' names.
    NameIndex { name: Name },
}

/// The structural checks. Shell-level checks (connectivity, the
/// Euler–Poincaré formula) run only when everything below them is sound.
pub fn check_topology(body: &Body) -> Vec<Fault> {
    let mut out = Vec::new();
    topology::structure(body, &mut out);
    if out.is_empty() {
        topology::shells(body, &mut out);
    }
    topology::names(body, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame, Point3, Surface, Torus};
    use crate::topo::fixture::{polyhedron, tetrahedron};
    use crate::topo::{CurveId, Id, RegionKind, Sense};

    fn faults(b: &Body) -> Vec<Fault> {
        check_topology(b)
    }

    fn v(i: usize) -> VertexId {
        VertexId::from_index(i)
    }

    #[test]
    fn a_tetrahedron_is_sound() {
        assert_eq!(faults(&tetrahedron()), []);
    }

    #[test]
    fn regions() {
        let mut b = tetrahedron();
        b.regions[RegionId::from_index(0)].kind = RegionKind::Solid;
        assert_eq!(
            faults(&b),
            [Fault::BadRegion {
                region: RegionId::from_index(0)
            }]
        );
        let mut b = tetrahedron();
        b.regions.push(crate::topo::Region {
            kind: RegionKind::Solid,
            first_shell: None,
        });
        assert_eq!(
            faults(&b),
            [Fault::BadRegion {
                region: RegionId::from_index(2)
            }]
        );
    }

    #[test]
    fn broken_rings_name_their_owner() {
        let mut b = tetrahedron();
        let f0 = FaceId::from_index(0);
        b.faces[f0].next = f0;
        assert_eq!(
            faults(&b),
            [Fault::BrokenRing {
                owner: EntityId::Shell(ShellId::from_index(0))
            }]
        );
        // A face that claims another shell is a foreign member.
        let mut b = tetrahedron();
        b.faces[FaceId::from_index(2)].shell = ShellId::from_index(5);
        assert_eq!(
            faults(&b),
            [
                Fault::BrokenRing {
                    owner: EntityId::Shell(ShellId::from_index(0))
                },
                Fault::DeadReference {
                    from: EntityId::Face(FaceId::from_index(2)),
                    to: EntityId::Shell(ShellId::from_index(5))
                }
            ]
        );
    }

    #[test]
    fn dead_references() {
        let mut b = tetrahedron();
        b.vertices.kill(v(3));
        let f = faults(&b);
        assert!(f.contains(&Fault::DeadReference {
            from: EntityId::Edge(EdgeId::from_index(3)),
            to: EntityId::Vertex(v(3))
        }));
        let mut b = tetrahedron();
        b.curves.kill(CurveId::from_index(0));
        assert_eq!(
            faults(&b),
            [Fault::DeadGeometry {
                from: EntityId::Edge(EdgeId::from_index(0))
            }]
        );
    }

    #[test]
    fn loops_and_fins() {
        let mut b = tetrahedron();
        b.loops[LoopId::from_index(1)].vertex = Some(v(0));
        assert_eq!(
            faults(&b),
            [Fault::BadLoop {
                loop_: LoopId::from_index(1)
            }]
        );
        // Swapping an edge's ends breaks continuity in both of its loops.
        let mut b = tetrahedron();
        let e = EdgeId::from_index(0);
        let (s, t) = (b.edges[e].start, b.edges[e].end);
        (b.edges[e].start, b.edges[e].end) = (t, s);
        let f = faults(&b);
        assert!(
            f.iter().all(|x| matches!(x, Fault::Disjoint { .. })),
            "{f:?}"
        );
        assert_eq!(f.len(), 4);
    }

    #[test]
    fn edge_fin_rings() {
        let mut b = tetrahedron();
        let fin = FinId::from_index(0);
        b.fins[fin].sense = Sense::Reversed;
        assert!(faults(&b).contains(&Fault::SameSense {
            edge: EdgeId::from_index(0)
        }));
        // One fin left alone on the edge: the other is orphaned.
        let mut b = tetrahedron();
        b.fins[fin].around = fin;
        let f = faults(&b);
        assert!(f.contains(&Fault::NotManifold {
            edge: EdgeId::from_index(0),
            fins: 1
        }));
        assert!(f.contains(&Fault::BrokenRing {
            owner: EntityId::Edge(EdgeId::from_index(0))
        }));
    }

    #[test]
    fn vertices_and_faces() {
        let mut b = tetrahedron();
        // Edge 1 runs between vertices 2 and 1.
        b.vertices[v(0)].edge = Some(EdgeId::from_index(1));
        assert_eq!(faults(&b), [Fault::BadVertex { vertex: v(0) }]);
        let mut b = tetrahedron();
        let f0 = FaceId::from_index(0);
        b.faces[f0].first_loop = None;
        let f = faults(&b);
        assert_eq!(f[0], Fault::Loopless { face: f0 });
        assert!(f.contains(&Fault::BrokenRing {
            owner: EntityId::Face(f0)
        }));
    }

    #[test]
    fn euler_characteristic_counts_closed_surface_genus() {
        let mut b = tetrahedron();
        let s = ShellId::from_index(0);
        assert_eq!(topology::euler_characteristic(&b, s), 2);
        let torus = b.add_surface(Surface::Torus(Torus::new(Frame::WORLD, 2.0, 1.0).unwrap()));
        b.faces[FaceId::from_index(0)].surface = torus;
        assert_eq!(topology::euler_characteristic(&b, s), 0);
    }

    #[test]
    fn disconnected_shells() {
        let p = |x, y, z| Point3::new(x, y, z);
        let b = polyhedron(
            &[
                p(0.0, 0.0, 0.0),
                p(1.0, 0.0, 0.0),
                p(0.0, 1.0, 0.0),
                p(0.0, 0.0, 1.0),
                p(5.0, 0.0, 0.0),
                p(6.0, 0.0, 0.0),
                p(5.0, 1.0, 0.0),
                p(5.0, 0.0, 1.0),
            ],
            &[
                &[0, 2, 1],
                &[0, 1, 3],
                &[1, 2, 3],
                &[0, 3, 2],
                &[4, 6, 5],
                &[4, 5, 7],
                &[5, 6, 7],
                &[4, 7, 6],
            ],
        );
        let s = ShellId::from_index(0);
        assert_eq!(
            faults(&b),
            [
                Fault::Disconnected { shell: s },
                Fault::EulerPoincare { shell: s, chi: 4 }
            ]
        );
    }

    #[test]
    fn names() {
        let mut b = tetrahedron();
        let n0 = b.faces[FaceId::from_index(0)].name;
        let n1 = b.faces[FaceId::from_index(1)].name;
        b.faces[FaceId::from_index(1)].name = n0;
        // Face 1 now duplicates face 0's name, and its own name is left in
        // the index with nothing holding it. Faults come in name order.
        let mut expected = [
            (n0, Fault::DuplicateName { name: n0 }),
            (n1, Fault::NameIndex { name: n1 }),
        ];
        expected.sort_by_key(|(n, _)| *n);
        assert_eq!(faults(&b), expected.map(|(_, f)| f));
        let mut b = tetrahedron();
        b.names.remove(&n0);
        assert_eq!(faults(&b), [Fault::NameIndex { name: n0 }]);
    }
}
