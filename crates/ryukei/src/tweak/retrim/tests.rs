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
        blends: Default::default(),
    };
    let (_, fixed) = move_faces(OpKey(0x13a9), &b, &spec).unwrap();
    let set = |j: &crate::name::Journal| {
        let mut e: Vec<String> = j.events().iter().map(|e| format!("{e:?}")).collect();
        e.sort();
        e
    };
    assert_eq!(set(&j), set(&fixed));
}

#[test]
fn only_an_uncertain_vertex_re_trims() {
    use crate::{Error, TweakError};
    let b = block();
    let un = |at| Error::from(TweakError::Uncertain { at });
    let v = b.vertices().next().unwrap();
    let e = b.edges().next().unwrap();
    let f = b.faces().next().unwrap();
    assert!(super::takes(&b, &un(b.vertices[v].name)));
    assert!(!super::takes(&b, &un(b.edges[e].name)));
    assert!(!super::takes(&b, &un(b.faces[f].name)));
    // A name the body hasn't (a trace's, a working entity's).
    assert!(!super::takes(&b, &un(Name::from_u128(0x13ac))));
    assert!(!super::takes(
        &b,
        &Error::from(TweakError::TangencyBroken {
            edge: b.edges[e].name,
        })
    ));
}

#[test]
fn an_offsets_reach_is_its_distance() {
    // `motion::apply` needs a normal to offset a point; the reach reads
    // the distance itself.
    let b = block();
    let top = face_of(&b, FaceRole::PlusZ);
    let c = Changes {
        motion: BTreeMap::from([(top, Motion::Offset(-0.3))]),
        ..Changes::default()
    };
    assert_eq!(super::reach(&b, &c), 0.3);
}

// Faces deleted through the re-trim (13.B): `Changes::deleted`.

/// `b` with `faces` taken away through the re-trim alone.
fn deleted(b: &Body, faces: &[FaceId]) -> Result<(Body, crate::name::Journal), crate::Error> {
    let ch = Changes {
        deleted: faces.iter().copied().collect(),
        ..Changes::default()
    };
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    retrim(OpKey(0x13b9), b, &ch, &m)
}

/// The faces of `b` that aren't planes.
fn curved(b: &Body) -> Vec<FaceId> {
    b.faces()
        .filter(|&f| {
            !matches!(
                b.surfaces[b.faces[f].surface],
                crate::geom::Surface::Plane(_)
            )
        })
        .collect()
}

#[test]
fn a_deleted_faces_neighbours_grow_to_meet() {
    let b = block();
    let e = b
        .edges()
        .find(|&e| {
            let fs: Vec<FaceId> = b
                .edge_fins(e)
                .map(|fin| b.loops[b.fins[fin].loop_].face)
                .collect();
            fs.contains(&face_of(&b, FaceRole::PlusZ))
                && fs.contains(&face_of(&b, FaceRole::MinusY))
        })
        .unwrap();
    let (r, _) = crate::blend::blend(
        OpKey(0x13b8),
        &b,
        &[crate::blend::EdgeBlend {
            edge: b.edges[e].name,
            kind: crate::blend::BlendKind::Fillet { radius: 0.1 },
        }],
    )
    .unwrap();
    let (o, _) = deleted(&r, &curved(&r)).unwrap();
    assert_eq!(o.faces().count(), 6);
    assert!((volume(&o) - 0.24).abs() < 1e-6);
}

#[test]
fn a_region_of_every_face_is_seeded_by_its_one_unclipped_piece() {
    // Every edge of the block rounded: the 20 blend faces touch all six
    // planes, so no face is outside the region and each plane's one
    // closed piece seeds the walk (its inner points lie in the strip it
    // grew by, outside the old face).
    let b = block();
    let edges: Vec<crate::blend::EdgeBlend> = b
        .edges()
        .map(|e| crate::blend::EdgeBlend {
            edge: b.edges[e].name,
            kind: crate::blend::BlendKind::Fillet { radius: 0.05 },
        })
        .collect();
    let (r, _) = crate::blend::blend(OpKey(0x13b7), &b, &edges).unwrap();
    let blends = curved(&r);
    assert_eq!(blends.len(), 20);
    let (o, _) = deleted(&r, &blends).unwrap();
    assert_eq!(o.faces().count(), 6);
    assert!((volume(&o) - 0.24).abs() < 1e-6);
}

#[test]
fn a_slabs_side_reaches_the_region_s_side() {
    // Nothing meets the four faces running past the deleted side:
    // `delete::grow` reads this as GapNotHealed.
    let b = block();
    assert!(matches!(
        deleted(&b, &[face_of(&b, FaceRole::PlusX)]),
        Err(crate::Error::InvalidTweak(
            crate::TweakError::RegionTooSmall { .. }
        ))
    ));
}

/// NIST FTC 07's seam pair (#652, design 551; public domain): a pipe-like
/// round's straight run (a cylinder, a working face) and its corner torus
/// (outside the region, across a frontier edge), tangent along the circle
/// they share, cut from the input and from the re-trim's working body
/// (plan 18.C, Task 1). Carried past the circle, the cylinder crosses the
/// torus along curves leaving it, so neither face keeps off the other
/// beside the seam: the seam built from the input's edge as the re-trim
/// builds it (`seam_for`) is withheld (Decision 4), and the pair is traced
/// whole as before. Task 1 found no FTC 07 pair the rule settles.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ftc_07s_seam_pair_crossing_past_its_seam_is_withheld() {
    use crate::codec::decode_body;
    use crate::intersect::known_seam;
    let input = decode_body(include_bytes!(
        "../../../tests/data/nist_ftc_07_seam_input.body"
    ))
    .unwrap();
    let work = decode_body(include_bytes!(
        "../../../tests/data/nist_ftc_07_seam_work.body"
    ))
    .unwrap();
    // The edge both input faces use.
    let e = input
        .edges()
        .find(|&e| input.edge_fins(e).count() == 2)
        .expect("the seam edge");
    let fins: Vec<_> = input.edge_fins(e).collect();
    let face = |fin| input.loops[input.fins[fin].loop_].face;
    // Working faces carry their input faces' names.
    let working = |f: FaceId| match work.by_name(input.faces[f].name) {
        Some(EntityId::Face(w)) => w,
        _ => panic!("no working face for {f:?}"),
    };
    let (wa, wb) = (working(face(fins[0])), working(face(fins[1])));
    let seam = super::body::seam_for(&input, e, (&work, wa), (&work, wb))
        .expect("tangent all along, with pcurves");
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    assert!(known_seam((&work, wa), (&work, wb), &[seam], &m).is_err());
}
