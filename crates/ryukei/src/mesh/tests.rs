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
