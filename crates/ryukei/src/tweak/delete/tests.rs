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
fn a_holes_wall_is_one_set_round_two_host_loops() {
    let b = drilled();
    let wall: BTreeSet<FaceId> = b
        .faces()
        .filter(|&f| cylinders(&b).contains(&b.faces[f].name))
        .collect();
    let s = sets::sets(&b, &wall);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].boundary.len(), 2);
    assert_eq!(s[0].hosts.len(), 2);
}
