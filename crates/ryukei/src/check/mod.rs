//! The validity checker.
//!
//! [`check_topology`] checks structure only: references, rings, loops,
//! fins, shells, the Euler–Poincaré formula and names. It holds for every
//! state an Euler operator leaves behind, including the half-built bodies
//! inside a primitive, so the operators debug-assert it.
//! [`check`] adds geometry and orientation: it holds for finished bodies.
//! [`check_intersections`] adds that faces meet only at the edges and
//! vertices they share; it is too costly to run after every operation, so
//! it runs in tests, fuzzing, and on booleans' inputs and results.
//!
//! Neither panics or loops on a corrupted body: every lookup is checked and
//! every walk is bounded. Faults come out in a fixed order: by check, then
//! by entity in array order.

mod geometry;
mod intersect;
mod topology;

pub(crate) use geometry::outside_size_box;
pub use intersect::{check_intersections, check_overlaps};

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
    /// A face's surface, an edge's curve or a fin's pcurve is not live.
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
    /// Geometry outside the size box: a vertex, a face's surface frame, or
    /// an edge's curve frame or samples.
    OutsideSizeBox { entity: EntityId },
    /// An edge's interval does not fit its curve: a closed edge that is not
    /// exactly one period (or, on a clamped curve whose ends meet, its whole
    /// domain), an open edge of a period or more, or an interval reaching
    /// outside a clamped curve's domain.
    BadInterval { edge: EdgeId },
    /// An end of an edge's interval is farther from its vertex than the
    /// larger of their tolerances.
    VertexOffCurve {
        vertex: VertexId,
        edge: EdgeId,
        distance: f64,
    },
    /// A vertex is farther than its tolerance from a surface it bounds.
    VertexOffSurface {
        vertex: VertexId,
        face: FaceId,
        distance: f64,
    },
    /// An edge strays farther than its tolerance from a surface on either
    /// side (worst of a fixed set of samples).
    EdgeOffSurface {
        edge: EdgeId,
        face: FaceId,
        distance: f64,
    },
    /// A fin with no pcurve.
    MissingPcurve { fin: FinId },
    /// A fin's pcurve, taken onto its face's surface, strays farther than
    /// the edge's tolerance from the edge (worst of the edge's samples).
    PcurveOffEdge { fin: FinId, distance: f64 },
    /// A planar face with straight edges whose loops wind clockwise about
    /// its normal.
    FaceInsideOut { face: FaceId },
    /// A solid whose faces are all planar with straight edges does not
    /// have exactly one shell enclosing positive volume (its outside) with
    /// every other shell enclosing negative volume (a cavity): a shell is
    /// inside out or flat.
    NotOutward { region: RegionId },
    /// Two faces meet away from the edges and vertices they share: they
    /// cross, or touch along a curve or at a point. From
    /// [`check_intersections`].
    FacesCross { a: FaceId, b: FaceId },
    /// Two faces on one surface overlap in an area. From
    /// [`check_intersections`].
    FacesOverlap { a: FaceId, b: FaceId },
    /// Two faces that may meet could not be checked: a surface with no
    /// implicit form not certified apart, or an intersection that stayed
    /// uncertain. From [`check_intersections`].
    CannotCheck { a: FaceId, b: FaceId },
}

/// Every check: structure, then (for a structurally sound body) geometry
/// and orientation.
pub fn check(body: &Body) -> Vec<Fault> {
    let mut out = check_topology(body);
    if out.is_empty() {
        geometry::geometry(body, &mut out);
        geometry::orientation(body, &mut out);
    }
    out
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
    fn pcurves_must_be_there_live_and_on_their_edges() {
        use crate::geom::{Curve2, Line2, Vector2};
        use crate::topo::{Curve2Id, Pcurve};
        let mut b = tetrahedron();
        let f = FinId::from_index(0);
        let p = b.fins[f].pcurve.unwrap();
        b.fins[f].pcurve = None;
        assert_eq!(check(&b), [Fault::MissingPcurve { fin: f }]);
        // Its own pcurve moved a little (within the edge's tolerance), then
        // far.
        let shifted = |b: &mut Body, d: f64| {
            let Curve2::Line2(l) = b.curves2[p.curve].clone() else {
                panic!("a line")
            };
            let moved = Line2::new(l.origin() + Vector2::new(0.0, d), l.dir()).unwrap();
            let curve = b.add_curve2(Curve2::Line2(moved));
            Pcurve { curve, ..p }
        };
        let near = shifted(&mut b, 0.5e-8);
        b.fins[f].pcurve = Some(near);
        assert_eq!(check(&b), []);
        let far = shifted(&mut b, 0.25);
        b.fins[f].pcurve = Some(far);
        assert!(check(&b).iter().any(
            |x| matches!(x, Fault::PcurveOffEdge { fin, distance } if *fin == f && *distance > 0.2)
        ));
        b.fins[f].pcurve = Some(Pcurve {
            curve: Curve2Id::from_index(999),
            ..p
        });
        assert_eq!(
            check_topology(&b),
            [Fault::DeadGeometry {
                from: EntityId::Fin(f)
            }]
        );
    }

    #[test]
    fn a_hollow_cube_is_sound_and_its_shells_point_the_right_way() {
        use crate::topo::fixture::{cube, hollow_cube, polyhedron, split_shell};
        let b = hollow_cube();
        assert_eq!(check(&b), []);
        let solid = RegionId::from_index(1);
        assert_eq!(b.region_shells(solid).count(), 2);
        // The cavity's faces pointing out of it, into the solid: two shells
        // enclose positive volume.
        let (mut points, mut faces) = cube(0.0, 3.0, 0, false);
        let (inner, holes) = cube(1.0, 2.0, 8, false);
        points.extend(inner);
        faces.extend(holes);
        let cycles: Vec<&[usize]> = faces.iter().map(Vec::as_slice).collect();
        let mut b = polyhedron(&points, &cycles);
        split_shell(&mut b, &(6..12).map(FaceId::from_index).collect::<Vec<_>>());
        assert_eq!(check(&b), [Fault::NotOutward { region: solid }]);
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

    fn contains(faults: &[Fault], pred: impl Fn(&Fault) -> bool) -> bool {
        faults.iter().any(pred)
    }

    #[test]
    fn a_tetrahedron_is_valid() {
        assert_eq!(check(&tetrahedron()), []);
    }

    #[test]
    fn vertices_off_their_geometry() {
        let mut b = tetrahedron();
        b.vertices[v(1)].point = Point3::new(1.0, 1e-6, 0.0);
        let f = check(&b);
        assert!(contains(
            &f,
            |x| matches!(x, Fault::VertexOffSurface { vertex, .. } if *vertex == v(1))
        ));
        assert!(contains(
            &f,
            |x| matches!(x, Fault::VertexOffCurve { vertex, .. } if *vertex == v(1))
        ));
        assert!(!contains(&f, |x| matches!(x, Fault::EdgeOffSurface { .. })));
    }

    #[test]
    fn edges_off_their_faces() {
        let mut b = tetrahedron();
        let e = EdgeId::from_index(0);
        let start = b.vertices[b.edges[e].start].point;
        let end = b.vertices[b.edges[e].end].point;
        let bent =
            crate::geom::Line::through(start, end + crate::geom::Vector3::new(0.0, 0.0, 1e-3))
                .unwrap();
        let c = b.add_curve(crate::geom::Curve::Line(bent));
        b.edges[e].curve = c;
        let f = check(&b);
        assert!(contains(
            &f,
            |x| matches!(x, Fault::EdgeOffSurface { edge, .. } if *edge == e)
        ));
        assert!(contains(
            &f,
            |x| matches!(x, Fault::VertexOffCurve { edge, .. } if *edge == e)
        ));
    }

    #[test]
    fn intervals_must_lie_in_a_nurbs_domain() {
        use crate::geom::Curve;
        use std::sync::Arc;
        let mut b = tetrahedron();
        let e = EdgeId::from_index(0);
        let (start, end) = (
            b.vertices[b.edges[e].start].point,
            b.vertices[b.edges[e].end].point,
        );
        let len = start.distance(end);
        let line = crate::geom::Line::through(start, end).unwrap();
        let (n, _) = line.to_nurbs([0.0, len]).unwrap();
        let c = b.add_curve(Curve::Nurbs(Arc::new(n)));
        b.edges[e].curve = c;
        assert_eq!(check(&b), []);
        // Reaching past the start of the knot domain.
        b.edges[e].interval = [-0.5, len];
        assert!(check(&b).contains(&Fault::BadInterval { edge: e }));
    }

    #[test]
    fn closed_edges_on_closed_clamped_nurbs() {
        use crate::geom::Curve;
        use std::sync::Arc;
        let (mut b, s) = crate::euler::tests::plane_body();
        let (m, _) = crate::euler::mvfs(&mut b, crate::name::OpKey(1), Point3::ORIGIN, s).unwrap();
        let circle = crate::geom::Circle::new(Frame::WORLD.flipped(), 1.0).unwrap();
        // A whole circle with clamped knots: closed, but not periodic.
        let (n, _) = circle.to_nurbs().unwrap();
        let c = b.add_curve(Curve::Nurbs(Arc::new(n.to_clamped())));
        let lone = crate::euler::Corner::Lone(m.loop_);
        crate::euler::set_point(&mut b, m.vertex, n.point(0.0)).unwrap();
        let tau = core::f64::consts::TAU;
        let (mef, _) =
            crate::euler::mef(&mut b, crate::name::OpKey(2), lone, lone, c, [0.0, tau]).unwrap();
        crate::pcurve::fill(&mut b).unwrap();
        assert_eq!(check(&b), []);
        b.edges[mef.edge].interval = [0.0, 3.0];
        assert!(check(&b).contains(&Fault::BadInterval { edge: mef.edge }));
    }

    #[test]
    fn loopless_nurbs_faces_are_faults() {
        use crate::geom::{Rigid, Surface};
        use std::sync::Arc;
        let (mut b, _) =
            crate::primitive::sphere(crate::name::OpKey(1), 1.0, &Rigid::IDENTITY).unwrap();
        let f = b.faces().next().unwrap();
        let (n, _) = b.surfaces[b.faces[f].surface]
            .to_nurbs([[0.0; 2]; 2])
            .unwrap();
        let s = b.add_surface(Surface::Nurbs(Arc::new(n)));
        b.faces[f].surface = s;
        assert_eq!(check_topology(&b), [Fault::Loopless { face: f }]);
    }

    #[test]
    fn intervals_must_fit_the_curve() {
        let mut b = tetrahedron();
        let e = EdgeId::from_index(0);
        b.edges[e].interval = [0.0, 0.5];
        let f = check(&b);
        assert!(contains(
            &f,
            |x| matches!(x, Fault::VertexOffCurve { edge, .. } if *edge == e)
        ));
        // A closed edge on a line can have no interval that fits.
        let (mut b, s) = crate::euler::tests::plane_body();
        let (m, _) = crate::euler::mvfs(&mut b, crate::name::OpKey(1), Point3::ORIGIN, s).unwrap();
        let circle = crate::geom::Circle::new(Frame::WORLD.flipped(), 1.0).unwrap();
        let c = b.add_curve(crate::geom::Curve::Circle(circle));
        let lone = crate::euler::Corner::Lone(m.loop_);
        crate::euler::set_point(&mut b, m.vertex, Point3::new(1.0, 0.0, 0.0)).unwrap();
        let tau = core::f64::consts::TAU;
        let (mef, _) =
            crate::euler::mef(&mut b, crate::name::OpKey(2), lone, lone, c, [0.0, tau]).unwrap();
        crate::pcurve::fill(&mut b).unwrap();
        assert_eq!(check(&b), []);
        b.edges[mef.edge].interval = [0.0, 3.0];
        assert!(check(&b).contains(&Fault::BadInterval { edge: mef.edge }));
    }

    #[test]
    fn size_box() {
        let mut b = tetrahedron();
        b.vertices[v(2)].point = Point3::new(0.0, 501.0, 0.0);
        assert!(check(&b).contains(&Fault::OutsideSizeBox {
            entity: EntityId::Vertex(v(2))
        }));
    }

    #[test]
    fn orientation() {
        let mut b = tetrahedron();
        let f0 = FaceId::from_index(0);
        b.faces[f0].sense = Sense::Reversed;
        assert_eq!(check(&b), [Fault::FaceInsideOut { face: f0 }]);
        // Every face cycle reversed: each face winds the way its own plane
        // faces, but the solid is inside out.
        let p = |x, y, z| Point3::new(x, y, z);
        let b = polyhedron(
            &[
                p(0.0, 0.0, 0.0),
                p(1.0, 0.0, 0.0),
                p(0.0, 1.0, 0.0),
                p(0.0, 0.0, 1.0),
            ],
            &[&[0, 1, 2], &[0, 3, 1], &[1, 3, 2], &[0, 2, 3]],
        );
        assert_eq!(
            check(&b),
            [Fault::NotOutward {
                region: RegionId::from_index(1)
            }]
        );
    }
}
