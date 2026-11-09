//! Fixtures for the re-trim's tests.

use crate::boolean::{BooleanOp, Tool, boolean};
use crate::geom::{Rigid, Vector3};
use crate::name::{FaceRole, Name, OpKey, Role, ToolKey};
use crate::primitive;
use crate::topo::{Body, EntityId, FaceId};

pub(crate) const K: OpKey = OpKey(0x13a0);

pub(crate) fn block() -> Body {
    primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0
}

pub(crate) fn face_of(b: &Body, r: FaceRole) -> FaceId {
    match b.by_name(Name::derive(K, Role::Face(r), &[])) {
        Some(EntityId::Face(f)) => f,
        _ => panic!("no face {r:?}"),
    }
}

/// A plate 1 × 1 × 0.2 with a boss of radius 0.1, 0.15 high, at the
/// middle of its top.
pub(crate) fn bossed() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (boss, _) = primitive::cylinder(OpKey(0x13a1), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0x13a2),
        BooleanOp::Unite,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &boss,
            place: Rigid::translation(Vector3::new(0.5, 0.5, 0.2)),
        }],
    )
    .unwrap()
    .0
}

use super::retrim;
use crate::tweak::Motion;
use crate::tweak::engine::Changes;
use std::collections::BTreeMap;

/// The changes that raise a block's top by `dz`.
pub(crate) fn raised(b: &Body, dz: f64) -> Changes {
    let top = face_of(b, FaceRole::PlusZ);
    let up = Rigid::translation(Vector3::new(0.0, 0.0, dz));
    Changes {
        motion: BTreeMap::from([(top, Motion::Rigid(up))]),
        surface: BTreeMap::from([(top, b.surfaces[b.faces[top].surface].transformed(&up))]),
        ..Changes::default()
    }
}

fn volume(b: &Body) -> f64 {
    assert_eq!(crate::check::check(b), []);
    let m = crate::mesh::tessellate(b, &crate::mesh::MeshParams::new(1e-3, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

#[test]
fn a_raised_top_through_the_re_trim_is_the_taller_box() {
    let b = block();
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let (o, _) = retrim(OpKey(0x13a9), &b, &raised(&b, 0.1), &m).unwrap();
    assert_eq!(
        (o.faces().count(), o.edges().count(), o.vertices().count()),
        (6, 12, 8)
    );
    assert!((volume(&o) - 0.6 * 0.5).abs() < 1e-9);
    assert_eq!(crate::check::check_intersections(&o), []);
}

#[test]
fn a_sheet_is_refused_before_the_re_trim() {
    let b = block();
    let (s, _) = crate::sheet::extract(
        OpKey(0x13aa),
        &b,
        &[b.faces[face_of(&b, FaceRole::PlusZ)].name],
    )
    .unwrap();
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let c = Changes::default();
    assert!(matches!(
        retrim(OpKey(0x13ab), &s, &c, &m),
        Err(crate::Error::NotASolid { .. })
    ));
}

fn all_names(b: &Body) -> Vec<Name> {
    let mut n: Vec<Name> = b
        .faces()
        .map(|f| b.faces[f].name)
        .chain(b.edges().map(|e| b.edges[e].name))
        .chain(b.vertices().map(|v| b.vertices[v].name))
        .collect();
    n.sort();
    n
}

#[test]
fn a_raised_top_through_the_re_trim_keeps_every_name() {
    let b = block();
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let (o, j) = retrim(OpKey(0x13a9), &b, &raised(&b, 0.1), &m).unwrap();
    assert_eq!(all_names(&o), all_names(&b));
    use crate::name::Event;
    // Nothing created or deleted, and no scaffolding in the journal.
    assert!(
        j.events().iter().all(|e| matches!(e, Event::Modified(_))),
        "{:?}",
        j.events()
    );
}

#[test]
fn a_raised_tops_journal_is_the_fixed_engines() {
    use crate::tweak::{MoveSpec, Topology, move_faces};
    let b = block();
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let (_, j) = retrim(OpKey(0x13a9), &b, &raised(&b, 0.1), &m).unwrap();
    let spec = MoveSpec {
        faces: vec![b.faces[face_of(&b, FaceRole::PlusZ)].name],
        motion: Rigid::translation(Vector3::new(0.0, 0.0, 0.1)),
        tangent: false,
        topology: Topology::Keep,
    };
    let (_, fixed) = move_faces(OpKey(0x13a9), &b, &spec).unwrap();
    let set = |j: &crate::name::Journal| {
        let mut e: Vec<String> = j.events().iter().map(|e| format!("{e:?}")).collect();
        e.sort();
        e
    };
    assert_eq!(set(&j), set(&fixed));
}
