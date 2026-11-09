//! Unit tests of deleting faces.

// Until every fixture has its test (plan 13.B, Task 7).
#![allow(dead_code, unused_imports)]

use super::*;
use crate::boolean::{BooleanOp, Tool, boolean};
use crate::geom::{Rigid, Surface, Vector3};
use crate::name::{OpKey, ToolKey};
use crate::primitive;
use crate::topo::Body;
use crate::topo::EntityId;
use crate::work::Meter;

const K: OpKey = OpKey(0x13b0);
const D: OpKey = OpKey(0x13b1);

/// A plate 1 × 1 × 0.2 with a hole of radius 0.1 through its middle.
pub(crate) fn drilled() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0x13b2), 0.1, 0.6, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0x13b3),
        BooleanOp::Subtract,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &pin,
            place: Rigid::translation(Vector3::new(0.5, 0.5, -0.2)),
        }],
    )
    .unwrap()
    .0
}

/// The plate with a boss of radius 0.1, 0.15 high, on its top.
pub(crate) fn bossed() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (boss, _) = primitive::cylinder(OpKey(0x13b2), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0x13b3),
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

pub(crate) fn cylinders(b: &Body) -> Vec<Name> {
    b.faces()
        .filter(|&f| matches!(b.surfaces[b.faces[f].surface], Surface::Cylinder(_)))
        .map(|f| b.faces[f].name)
        .collect()
}

fn volume(b: &Body) -> f64 {
    assert_eq!(crate::check::check(b), []);
    assert_eq!(crate::check::check_intersections(b), []);
    let m = crate::mesh::tessellate(b, &crate::mesh::MeshParams::new(1e-4, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn spec(faces: Vec<Name>, heal: Heal) -> DeleteSpec {
    DeleteSpec { faces, heal }
}

#[test]
fn a_holes_rims_are_holes_of_the_plate_and_its_foot_is_not_the_bosss_tops() {
    let b = drilled();
    let wall: BTreeSet<FaceId> = b
        .faces()
        .filter(|&f| cylinders(&b).contains(&b.faces[f].name))
        .collect();
    let s = sets::sets(&b, &wall);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].hosts.len(), 2);
    assert!(s[0].closable(&b));
    let b = bossed();
    let side: BTreeSet<FaceId> = b
        .faces()
        .filter(|&f| cylinders(&b).contains(&b.faces[f].name))
        .collect();
    let s = sets::sets(&b, &side);
    assert_eq!(s[0].hosts.len(), 2);
    assert!(!s[0].closable(&b));
}

#[test]
fn a_cap_lays_the_plate_out_without_its_hole() {
    let b = drilled();
    let wall: BTreeSet<FaceId> = b
        .faces()
        .filter(|&f| cylinders(&b).contains(&b.faces[f].name))
        .collect();
    let s = sets::sets(&b, &wall);
    // unmetered: a unit test.
    let m = Meter::free();
    let (o, j) = cap::cap(D, &b, &[&s[0]], &m).unwrap();
    assert_eq!(
        (o.faces().count(), o.edges().count(), o.vertices().count()),
        (6, 12, 8)
    );
    assert!((volume(&o) - 0.2).abs() < 1e-9);
    let modified = j
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Modified(_)))
        .count();
    // The top and the bottom; the wall, both rims and their vertices go.
    assert_eq!(modified, 2);
    assert_eq!(j.events().len(), 2 + 1 + 2 + 2);
}
