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
    let seam = super::body::seam_for(
        &input,
        e,
        (&work, wa),
        (&work, wb),
        crate::intersect::SeamKind::Running,
    )
    .expect("tangent all along, with pcurves");
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    assert!(known_seam((&work, wa), (&work, wb), &[seam], &m).is_err());
}

/// Meets a committed cut's pair along the edges its two input faces
/// share: each held (`partly`: tangent only in places), built as the
/// re-trim builds it (`seam_for`, with `kind`), and met by `known_seam`
/// on the working pair (design 642).
fn met_along_the_cut(input: &[u8], work: &[u8], kind: crate::intersect::SeamKind, partly: bool) {
    use crate::codec::decode_body;
    use crate::intersect::known_seam;
    let input = decode_body(input).unwrap();
    let work = decode_body(work).unwrap();
    let edges: Vec<_> = input
        .edges()
        .filter(|&e| input.edge_fins(e).count() == 2)
        .collect();
    assert!(!edges.is_empty(), "the seam edges");
    let face = |fin| input.loops[input.fins[fin].loop_].face;
    let working = |f: FaceId| match work.by_name(input.faces[f].name) {
        Some(EntityId::Face(w)) => w,
        _ => panic!("no working face for {f:?}"),
    };
    let fins: Vec<_> = input.edge_fins(edges[0]).collect();
    let (wa, wb) = (working(face(fins[0])), working(face(fins[1])));
    let mut seams = Vec::new();
    for &e in &edges {
        assert!(super::held::held_tangent(&input, e));
        assert_eq!(
            matches!(crate::tweak::classify::tangent(&input, e), Ok(true)),
            !partly
        );
        let fs: Vec<_> = input.edge_fins(e).collect();
        // Each seam's pcurves in the pair's order.
        let (x, y) = if working(face(fs[0])) == wa {
            ((&work, wa), (&work, wb))
        } else {
            ((&work, wb), (&work, wa))
        };
        let mut s = super::body::seam_for(&input, e, x, y, kind).expect("pcurves");
        if x.1 != wa {
            s.pcurves.swap(0, 1);
        }
        seams.push(s);
    }
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    assert!(known_seam((&work, wa), (&work, wb), &seams, &m).is_ok());
}

/// CTC 02 with PMI's one failing pair (#658, census II): its NURBS face,
/// outside the region, against a working plane, tangent all along a
/// closed ellipse. A frontier seam, met along the edge: the NURBS face
/// keeps off the plane beside it.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ctc_02s_closed_frontier_ellipse_is_met_along_it() {
    met_along_the_cut(
        include_bytes!("../../../tests/data/nist_ctc_02_ellipse_input.body"),
        include_bytes!("../../../tests/data/nist_ctc_02_ellipse_work.body"),
        crate::intersect::SeamKind::Frontier,
        false,
    );
}

/// FTC 07's NURBS/plane pair (#652, census II pair 5): both working,
/// tangent along an ellipse only in places (`PartlySmooth`), pcurves
/// Nurbs2 and Ellipse2, a running seam (the deleted blend is round one of
/// its ends). Held, and met along the edge: the NURBS face keeps off the
/// plane (the plane's signs one way only over it in the census).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ftc_07s_partly_smooth_ellipse_is_met_along_it() {
    met_along_the_cut(
        include_bytes!("../../../tests/data/nist_ftc_07_partly_input.body"),
        include_bytes!("../../../tests/data/nist_ftc_07_partly_work.body"),
        crate::intersect::SeamKind::Running,
        true,
    );
}

/// The bent rod's rim deleted needs its stop (design 642): without stops
/// the run, carried past the bend, meets the bend's torus uncertain;
/// with them the rung is built again with the run stopped at their seam,
/// met along it, and the rod ends square. The public delete goes the same
/// way.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_bent_rods_rim_needs_its_stop() {
    use super::{Stops, retrim_with};
    let (rod, rim) = crate::testing::bent_rod();
    let changes = Changes {
        deleted: std::collections::BTreeSet::from([rim]),
        ..Changes::default()
    };
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let never = retrim_with(OpKey(0x19a4), &rod, &changes, Stops::Never, &m);
    assert!(
        matches!(
            never,
            Err(crate::Error::InvalidTweak(
                crate::TweakError::Uncertain { .. }
            ))
        ),
        "{never:?}"
    );
    let (o, _) = retrim_with(OpKey(0x19a4), &rod, &changes, Stops::AfterHeld, &m).unwrap();
    assert_eq!(crate::check::check(&o), []);
    assert_eq!(o.faces().count(), rod.faces().count() - 1);
    let spec = crate::tweak::DeleteBlends {
        max_size: Some(0.1),
        faces: Vec::new(),
        options: Default::default(),
    };
    let (d, _) = crate::tweak::delete_blends(OpKey(0x19a4), &rod, &spec).unwrap();
    assert_eq!(d.dump_unnamed(), o.dump_unnamed());
}

/// Design 642's fallback: a stopped build whose walk fails, or that stops
/// nothing, leaves the first build's answer; a stopped build's own
/// refusal is the rung's; without stops, or where the first build didn't
/// refuse at a held pair, nothing is built again.
#[test]
fn a_stopped_rung_answers_only_where_its_walk_closes() {
    use super::{Rung, Stops, settle};
    use crate::TweakError;
    let held = || {
        Rung::Held(
            TweakError::Uncertain {
                at: Name::from_u128(1),
            }
            .into(),
        )
    };
    let walked = || {
        Rung::Walked(
            TweakError::GapNotHealed {
                face: Name::from_u128(2),
            }
            .into(),
        )
    };
    let uncertain = |r: Rung| matches!(r, Rung::Done(r) if matches!(*r, Err(crate::Error::InvalidTweak(TweakError::Uncertain { .. }))));
    assert!(uncertain(settle(held(), Stops::AfterHeld, walked)));
    assert!(uncertain(settle(held(), Stops::AfterHeld, || {
        Rung::Unstopped
    })));
    assert!(matches!(
        settle(held(), Stops::AfterHeld, || Rung::Next(
            TweakError::HealAmbiguous {
                face: Name::from_u128(3)
            }
            .into()
        )),
        Rung::Next(crate::Error::InvalidTweak(TweakError::HealAmbiguous { .. }))
    ));
    assert!(uncertain(settle(held(), Stops::Never, || panic!(
        "built with stops"
    ))));
    assert!(matches!(
        settle(walked(), Stops::AfterHeld, || panic!("built with stops")),
        Rung::Walked(_)
    ));
}
