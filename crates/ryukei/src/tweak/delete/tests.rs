//! Unit tests of deleting faces.

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
    // The foot is a hole of the plate's top; the rim is the boss top's
    // only loop.
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

#[test]
fn a_through_hole_is_capped_bit_for_bit() {
    let b = drilled();
    let (o, j) = delete_faces(D, &b, &spec(cylinders(&b), Heal::Auto)).unwrap();
    assert_eq!(o.faces().count(), 6);
    assert!((volume(&o) - 0.2).abs() < 1e-9);
    // The plate's own faces keep their surfaces.
    for f in o.faces() {
        let n = o.faces[f].name;
        let EntityId::Face(g) = b.by_name(n).unwrap() else {
            panic!()
        };
        assert_eq!(
            o.surfaces[o.faces[f].surface],
            b.surfaces[b.faces[g].surface]
        );
    }
    assert!(j.events().contains(&Event::Deleted(cylinders(&b)[0])));
    assert_eq!(
        j.events()
            .iter()
            .filter(|e| matches!(e, Event::Modified(_)))
            .count(),
        2
    );
    assert_eq!(
        delete_faces(D, &b, &spec(cylinders(&b), Heal::Grow)).unwrap_err(),
        TweakError::GrowWouldCap {
            face: cylinders(&b)[0]
        }
        .into()
    );
}

#[test]
fn a_fillet_grows_back_to_its_sharp_edge() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let top = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::PlusZ),
        &[],
    );
    let front = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::MinusY),
        &[],
    );
    let e = b
        .edges()
        .find(|&e| {
            let n: Vec<Name> = sets::faces_of(&b, e)
                .iter()
                .map(|&f| b.faces[f].name)
                .collect();
            n.contains(&top) && n.contains(&front)
        })
        .unwrap();
    let (r, _) = crate::blend::blend(
        OpKey(0x13b4),
        &b,
        &[crate::blend::EdgeBlend {
            edge: b.edges[e].name,
            kind: crate::blend::BlendKind::Fillet { radius: 0.1 },
        }],
    )
    .unwrap();
    let fillet = cylinders(&r)[0];
    let EntityId::Face(f) = r.by_name(fillet).unwrap() else {
        panic!()
    };
    // unmetered: a unit test.
    let m = Meter::free();
    let (o, j) = grow::grow(D, &r, &BTreeSet::from([f]), &m).unwrap();
    assert!((volume(&o) - 0.24).abs() < 1e-9);
    let sharp: Vec<Name> = o
        .edges()
        .filter(|&e| {
            let n: Vec<Name> = sets::faces_of(&o, e)
                .iter()
                .map(|&f| o.faces[f].name)
                .collect();
            n.contains(&top) && n.contains(&front)
        })
        .map(|e| o.edges[e].name)
        .collect();
    assert_eq!(sharp.len(), 1);
    assert!(
        j.events().contains(&Event::Replaced {
            old: fillet,
            new: sharp.clone()
        }),
        "{:?}",
        j.events()
    );
    // Both rails go to the sharp edge too.
    let rails = j
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Replaced { old, new } if *old != fillet && *new == sharp))
        .count();
    assert_eq!(rails, 2);
}

#[test]
fn deleting_a_whole_body_or_a_slabs_side_heals_nothing() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let all: Vec<Name> = b.faces().map(|f| b.faces[f].name).collect();
    assert!(matches!(
        delete_faces(D, &b, &spec(all, Heal::Auto)),
        Err(Error::InvalidTweak(TweakError::GapNotHealed { .. }))
    ));
    let side = Name::derive(
        K,
        crate::name::Role::Face(crate::name::FaceRole::PlusX),
        &[],
    );
    assert_eq!(
        delete_faces(D, &b, &spec(vec![side], Heal::Auto)).unwrap_err(),
        TweakError::GapNotHealed { face: side }.into()
    );
}
