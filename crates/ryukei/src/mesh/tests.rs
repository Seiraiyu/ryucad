use super::*;
use crate::geom::{Cylinder, Frame, Rigid, Surface, Vector3};
use crate::name::OpKey;
use crate::primitive;
use crate::topo::Sense;
use core::f64::consts::PI;

/// The volume a closed mesh encloses (by the divergence theorem) and its
/// area.
fn volume_and_area(m: &BodyMesh) -> (f64, f64) {
    let (mut vol, mut area) = (0.0, 0.0);
    for [a, b, c] in m.welded_triangles() {
        let [a, b, c] = [a, b, c].map(|i| m.positions[i as usize].to_vector());
        vol += a.dot(b.cross(c)) / 6.0;
        area += (b - a).cross(c - a).length() / 2.0;
    }
    (vol, area)
}

fn tilted() -> Rigid {
    let axis = Vector3::new(1.0, 2.0, 3.0).normalize().unwrap();
    Rigid::translation(Vector3::new(10.0, -20.0, 30.0)).then_after(&Rigid::rotation(axis, 0.7))
}

/// Meshes a body, checks the mesh, and compares its volume with the
/// exact one: within the chord times the area.
fn sound(b: &Body, chord: f64, volume: f64, area: f64) -> BodyMesh {
    let m = tessellate(b, &MeshParams::new(chord, 0.3)).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(b, &m), Ok(()));
    let (v, a) = volume_and_area(&m);
    assert!((v - volume).abs() <= chord * area, "volume {v} vs {volume}");
    assert!(
        a <= area * (1.0 + 1e-9) && a >= area * 0.99,
        "area {a} vs {area}"
    );
    m
}

#[test]
fn every_primitive_meshes_soundly() {
    for at in [Rigid::IDENTITY, tilted()] {
        let b = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &at).unwrap().0;
        let m = sound(&b, 1e-3, 6.0, 22.0);
        // Twelve triangles.
        assert_eq!(m.triangles.len(), 12);
        let b = primitive::cylinder(OpKey(1), 1.0, 2.0, &at).unwrap().0;
        sound(&b, 1e-3, 2.0 * PI, 6.0 * PI);
        let b = primitive::cone(OpKey(1), 1.0, 2.0, &at).unwrap().0;
        let slant = 5.0f64.sqrt();
        sound(&b, 1e-3, 2.0 * PI / 3.0, PI * (1.0 + slant));
        let b = primitive::frustum(OpKey(1), 1.0, 0.5, 1.0, &at).unwrap().0;
        let side = PI * 1.5 * (1.0 + 0.25f64).sqrt();
        sound(&b, 1e-3, PI * (1.0 + 0.5 + 0.25) / 3.0, PI * 1.25 + side);
        let b = primitive::sphere(OpKey(1), 1.0, &at).unwrap().0;
        sound(&b, 1e-3, 4.0 * PI / 3.0, 4.0 * PI);
        let b = primitive::torus(OpKey(1), 2.0, 0.5, &at).unwrap().0;
        sound(
            &b,
            1e-3,
            2.0 * PI * PI * 2.0 * 0.25,
            4.0 * PI * PI * 2.0 * 0.5,
        );
    }
}

#[test]
fn every_point_is_on_its_face() {
    let b = primitive::torus(OpKey(1), 2.0, 0.5, &tilted()).unwrap().0;
    let m = tessellate(&b, &MeshParams::new(1e-3, 0.3)).unwrap();
    let s = b
        .surface(b.face(b.faces().next().unwrap()).unwrap().surface)
        .unwrap();
    for p in &m.positions {
        assert!(s.distance(*p) <= 1e-12);
    }
    // And the corner normals point out of the tube.
    for c in &m.corners {
        let p = m.positions[c.position as usize];
        let n = s.normal(c.uv.x, c.uv.y).unwrap();
        assert!(c.normal.get().dot(n.get()) > 0.999_999);
        assert!((s.point(c.uv.x, c.uv.y) - p).length() <= 1e-12);
    }
}

#[test]
fn finer_chords_give_more_triangles() {
    let b = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let coarse = tessellate(&b, &MeshParams::new(1e-2, 0.5)).unwrap();
    let fine = tessellate(&b, &MeshParams::new(1e-4, 0.5)).unwrap();
    assert!(fine.triangles.len() > 20 * coarse.triangles.len());
}

#[test]
fn faces_off_their_edges_are_reported() {
    let (mut b, _) = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
    let side = b.faces().next().unwrap();
    let s = b.add_surface(Surface::Cylinder(
        Cylinder::new(Frame::WORLD, 1.01).unwrap(),
    ));
    crate::euler::set_surface(&mut b, side, s, Sense::Forward).unwrap();
    let m = tessellate(&b, &MeshParams::new(1e-3, 0.3)).unwrap();
    let edge = b.edges().next().unwrap();
    assert_eq!(
        m.faults,
        [FaceFault {
            face: side,
            kind: MeshFaultKind::EdgeOffSurface { edge }
        }]
    );
    // The two disks are still there; the side is a hole.
    assert_eq!(m.faces.len(), 2);
    let faults = check_mesh(&b, &m).unwrap_err();
    assert!(
        faults
            .iter()
            .all(|f| matches!(f, MeshCheckFault::OpenEdge { .. }))
    );
}

#[test]
fn budgets_and_params() {
    let b = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let small = MeshParams {
        max_triangles: 100,
        ..MeshParams::new(1e-4, 0.5)
    };
    assert!(matches!(
        tessellate(&b, &small),
        Err(Error::MeshTooLarge { .. })
    ));
    assert_eq!(
        tessellate(&b, &MeshParams::new(0.0, 0.5)),
        Err(Error::InvalidMeshParams)
    );
}

#[test]
fn the_same_body_gives_the_same_mesh() {
    let b = primitive::cone(OpKey(1), 1.0, 2.0, &tilted()).unwrap().0;
    let p = MeshParams::new(1e-3, 0.3);
    assert_eq!(tessellate(&b, &p), tessellate(&b, &p));
}

#[test]
fn one_thread_or_many_gives_the_same_mesh() {
    let bodies = [
        primitive::torus(OpKey(1), 2.0, 0.5, &tilted()).unwrap().0,
        primitive::cylinder(OpKey(1), 1.0, 2.0, &tilted())
            .unwrap()
            .0,
    ];
    let p = MeshParams::new(1e-3, 0.3);
    for b in &bodies {
        let on = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| tessellate(b, &p).unwrap())
        };
        assert_eq!(on(1), on(8));
    }
}

#[test]
fn runaway_requests_are_refused_before_meshing() {
    // A 1 m sphere at 1e-7 m would need about 10^8 triangles.
    let b = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let start = std::time::Instant::now();
    let r = tessellate(&b, &MeshParams::new(1e-7, 1.5));
    assert!(matches!(r, Err(Error::MeshTooLarge { triangles }) if triangles > 50_000_000));
    assert!(start.elapsed().as_secs() < 10);
}

#[test]
fn a_cavity_meshes_pointing_into_itself() {
    let b = crate::topo::fixture::hollow_cube();
    // 27 less the hollow's 1; both shells' areas.
    sound(&b, 1e-3, 26.0, 6.0 * 9.0 + 6.0);
}

#[test]
fn an_empty_body_is_sound_and_meshes_to_nothing() {
    let b = Body::new();
    assert_eq!(crate::check::check(&b), []);
    let m = tessellate(&b, &MeshParams::new(1e-3, 0.3)).unwrap();
    assert!(m.positions.is_empty() && m.faults.is_empty());
    assert_eq!(m.welded_triangles().count(), 0);
    assert_eq!(check_mesh(&b, &m), Ok(()));
}

/// A body with the faces `off` killed, after making it a closed sheet.
fn opened(mut b: Body, key: OpKey, off: &[crate::name::FaceRole]) -> Body {
    use crate::name::{Name, Role};
    use crate::topo::EntityId;
    let s = b.shells().next().unwrap();
    crate::euler::mvsr(&mut b, s).unwrap();
    for &r in off {
        let Some(EntityId::Face(f)) = b.by_name(Name::derive(key, Role::Face(r), &[])) else {
            panic!("no {r:?}");
        };
        crate::euler::kfl(&mut b, f).unwrap();
    }
    b
}

#[test]
fn sheets_mesh_with_their_boundaries_open() {
    use crate::name::FaceRole;
    let key = OpKey(0x51);
    let (b, _) = primitive::block(key, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let open = opened(b, key, &[FaceRole::PlusZ]);
    let m = tessellate(&open, &MeshParams::new(1e-3, 0.3)).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(&open, &m), Ok(()));
    let (_, area) = volume_and_area(&m);
    assert!((area - 1.88).abs() < 1e-9, "{area}");
    let (c, _) = primitive::cylinder(key, 0.3, 0.5, &Rigid::IDENTITY).unwrap();
    let band = opened(c, key, &[FaceRole::Top, FaceRole::Bottom]);
    let m = tessellate(&band, &MeshParams::new(1e-3, 0.3)).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(&band, &m), Ok(()));
    let (_, area) = volume_and_area(&m);
    assert!((area - 2.0 * PI * 0.3 * 0.5).abs() < 2e-3 * area, "{area}");
}

/// Issue #449: fillets' quarter rounds in a file whose cylinder's seam
/// lies a hair inside the face (an edge 15 nm past it) or a hair short of
/// it (a loop starting 1e-13 rad off it): cut at the seam, the face left
/// slivers its triangulation couldn't mark, and healing's shell facing
/// was refused uncertain. Here a block turned a radian about an axis, its
/// outer cylinder face given a copy of its surface whose seam sits close
/// to the face's start, or on it with the face's first edge moved 15 nm
/// past it, tolerant (the file's case, which the exact ones never hit).
#[test]
fn a_face_grazing_its_cylinders_seam_meshes() {
    use crate::geom::{Axis, Curve2, Line2, Point2, Point3};
    use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};

    let pts = [(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)];
    let segments = (0..4)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)).unwrap();
    let (base, _) = revolve(OpKey(1), &profile, axis, 0.0, 1.0).unwrap();
    let (face, c) = base
        .faces()
        .find_map(|f| match &base.surfaces[base.faces[f].surface] {
            Surface::Cylinder(c) if (c.radius() - 2.0).abs() < 1e-12 => Some((f, *c)),
            _ => None,
        })
        .unwrap();
    // Where the face starts on its cylinder.
    let fr = *Surface::Cylinder(c).frame().unwrap();
    let z = fr.z().get();
    let start = base
        .face_loops(face)
        .flat_map(|l| base.loop_fins(l).collect::<Vec<_>>())
        .map(|fin| {
            Surface::Cylinder(c)
                .project(base.vertices[base.start_of(fin)].point)
                .param
                .0
        })
        .map(|u| {
            if u > core::f64::consts::PI {
                u - core::f64::consts::TAU
            } else {
                u
            }
        })
        .fold(f64::INFINITY, f64::min);
    // `shift`: the face's first edge (and its vertices) moved that far
    // into the face across the seam, tolerant as a file's, the arcs ending
    // on the seam itself: the file's case.
    for (delta, shift) in [
        (7.5e-8, 0.0),
        (1.5e-4, 0.0),
        (-1e-13, 0.0),
        (-1e-9, 0.0),
        (-1.5e-4, 0.0),
        (0.0, 1.5e-8),
        (0.0, 1e-13),
    ] {
        let phi = start + delta;
        let (s, co) = (crate::math::sin(phi), crate::math::cos(phi));
        let x = fr.x().get() * co + z.cross(fr.x().get()) * s;
        let turned = Cylinder::new(Frame::new(fr.origin(), z, x).unwrap(), 2.0).unwrap();
        let mut b = base.clone();
        let id = b.add_surface(Surface::Cylinder(turned));
        let sense = b.faces[face].sense;
        crate::euler::set_surface(&mut b, face, id, sense).unwrap();
        if shift > 0.0 {
            let along = z.cross(x);
            let first = base
                .face_loops(face)
                .flat_map(|l| base.loop_fins(l).collect::<Vec<_>>())
                .map(|fin| b.fins[fin].edge)
                .find(|&e| {
                    matches!(b.curves[b.edges[e].curve], crate::geom::Curve::Line(_))
                        && (0..2).all(|i| {
                            let v = [b.edges[e].start, b.edges[e].end][i];
                            let u = Surface::Cylinder(c).project(b.vertices[v].point).param.0;
                            let u = if u > PI {
                                u - core::f64::consts::TAU
                            } else {
                                u
                            };
                            (u - start).abs() < 1e-9
                        })
                })
                .unwrap();
            let moved = Rigid::translation(along * shift);
            let curve = b.add_curve(b.curves[b.edges[first].curve].transformed(&moved));
            let interval = b.edges[first].interval;
            crate::euler::set_curve(&mut b, first, curve, interval).unwrap();
            let tol = crate::tol::Tolerance::new(3e-8).unwrap();
            for v in [b.edges[first].start, b.edges[first].end] {
                let p = b.vertices[v].point;
                crate::euler::set_point(&mut b, v, p + along * shift).unwrap();
                crate::euler::set_vertex_tolerance(&mut b, v, tol).unwrap();
            }
            let edges: Vec<_> = b.edges().collect();
            for e in edges {
                crate::euler::set_edge_tolerance(&mut b, e, tol).unwrap();
            }
        }
        crate::pcurve::fill(&mut b).unwrap();
        assert_eq!(sense, b.faces[face].sense);
        for chord in [1e-2, 1e-4] {
            let m = tessellate(&b, &MeshParams::new(chord, 0.5)).unwrap();
            assert!(
                m.faults.is_empty(),
                "{delta} {shift} at {chord}: {:?}",
                m.faults
            );
            assert_eq!(check_mesh(&b, &m), Ok(()));
        }
    }
}

/// A curved face whose tolerant arc's pcurve ends apart from the lines'
/// at their vertices (#465, the owner's files at mass's chord): the corner
/// triangle between them keeps its size however finely the arc is
/// sampled, and was asked for samples until the rounds ran out
/// (`BoundaryNotResolved`). Within the vertices' tube it is held to the
/// chord plus their tolerance, as the guarantee near tolerant edges says.
#[test]
fn a_tolerant_arcs_pcurve_ending_apart_meshes_at_a_fine_chord() {
    use crate::geom::{Axis, Curve, Curve2, Line2, Point2, Point3, Vector2};
    use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};
    use crate::tol::Tolerance;

    let mm = 1e-3;
    let pts = [(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)].map(|(x, y)| (x * mm, y * mm));
    let segments = (0..4)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 1.0, 0.0)).unwrap();
    let (mut b, _) = revolve(OpKey(1), &profile, axis, 0.0, 1.0).unwrap();
    // The inner wall, 1 mm round, and its arc at y = 0.
    let face = b
        .faces()
        .find(|&f| match &b.surfaces[b.faces[f].surface] {
            Surface::Cylinder(c) => (c.radius() - mm).abs() < 1e-12,
            _ => false,
        })
        .unwrap();
    let fin = b
        .face_loops(face)
        .flat_map(|l| b.loop_fins(l).collect::<Vec<_>>())
        .find(|&fin| {
            let e = &b.edges[b.fins[fin].edge];
            matches!(b.curves[e.curve], Curve::Circle(_))
                && b.vertices[e.start].point.y.abs() < 1e-12
        })
        .unwrap();
    let edge = b.fins[fin].edge;
    let [t0, t1] = b.edges[edge].interval;
    // The arc's pcurve starts 0.05 rad round the wall: it misses the
    // line's end there by 50 µm, inside a tolerance of 80 µm, and a
    // triangle across that gap strays more than the chord from the wall.
    let shift = Vector2::new(0.05, 0.0);
    let (mut p, mut q) = (b.fin_uv(fin, t0).unwrap(), b.fin_uv(fin, t1).unwrap());
    if p.x < q.x {
        p = p + shift;
    } else {
        q = q + shift;
    }
    let curve = b.add_curve2(Curve2::Line2(Line2::through(p, q).unwrap()));
    let slope = p.distance(q) / (t1 - t0);
    crate::euler::set_pcurve(
        &mut b,
        fin,
        Some(crate::topo::Pcurve {
            curve,
            map: [-t0 * slope, slope],
        }),
    )
    .unwrap();
    let tol = Tolerance::new(8e-5).unwrap();
    crate::euler::set_edge_tolerance(&mut b, edge, tol).unwrap();
    for v in [b.edges[edge].start, b.edges[edge].end] {
        crate::euler::set_vertex_tolerance(&mut b, v, tol).unwrap();
    }
    let m = tessellate(&b, &MeshParams::new(1e-7, 0.05)).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(&b, &m), Ok(()));
}
