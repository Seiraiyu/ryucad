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

/// FTC 07's torus/cylinder pair (#652, pairs 3 and 4), from the same
/// cuts as the guard above, with the run stopped at its seam (design
/// 642): the torus stays outside the region, so the seam is a frontier
/// one, and the cylinder's pcurve along it is iso at its own box's end.
/// Stopped half a strip past the circle, the run lies on its side of the
/// circle's plane outside the strip, the torus on the other: met along
/// the circle by the section plane.
///
/// The run is a whole turn and the seam a quarter of its end circle, so
/// the rest of the stopped rim lies past the circle's plane, but apart
/// from the torus face: the section plane leaves those parts out on a
/// stopped pair (plan 19.B, Decision 1).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ftc_07s_seam_pair_is_met_once_its_run_stops() {
    use crate::codec::decode_body;
    use crate::geom::Surface;
    use crate::intersect::{SeamKind, known_seam};
    let input = decode_body(include_bytes!(
        "../../../tests/data/nist_ftc_07_seam_input.body"
    ))
    .unwrap();
    let work = decode_body(include_bytes!(
        "../../../tests/data/nist_ftc_07_seam_work.body"
    ))
    .unwrap();
    let e = input
        .edges()
        .find(|&e| input.edge_fins(e).count() == 2)
        .expect("the seam edge");
    let fins: Vec<_> = input.edge_fins(e).collect();
    let face = |fin| input.loops[input.fins[fin].loop_].face;
    let (ia, ib) = (face(fins[0]), face(fins[1]));
    let working = |f: FaceId| match work.by_name(input.faces[f].name) {
        Some(EntityId::Face(w)) => w,
        _ => panic!("no working face for {f:?}"),
    };
    let is_run = |f: FaceId| matches!(input.surfaces[input.faces[f].surface], Surface::Cylinder(_));
    assert!(is_run(ia) != is_run(ib), "a cylinder and a torus");
    let run = if is_run(ia) { ia } else { ib };
    let h = super::held::Held {
        edge: e,
        faces: [ia, ib],
        kind: SeamKind::Frontier,
    };
    let wr = working(run);
    let surface = work.surfaces[work.faces[wr].surface].clone();
    let rect = crate::intersect::parameter_box(&work, wr);
    let stopped = super::held::iso_stops(&input, run, &[h], &surface, rect);
    assert_ne!(stopped, rect, "the run is stopped at its seam");
    let (sheet, _) = crate::sheet::from_surface(K, &surface, stopped).unwrap();
    let sf = sheet.faces().next().unwrap();
    let torus = (&work, working(if run == ia { ib } else { ia }));
    // The pair in the input's fin order, as `seam_for` takes it.
    let (x, y) = if run == ia {
        ((&sheet, sf), torus)
    } else {
        (torus, (&sheet, sf))
    };
    let mut seam = super::body::seam_for(&input, e, x, y, SeamKind::Frontier).expect("pcurves");
    // As the stopped build marks it (plan 19.B, Decision 1).
    seam.stopped = true;
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    assert!(known_seam(x, y, &[seam], &m).is_ok());
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
    use super::{Narrow, retrim_with};
    let (rod, rim) = crate::testing::bent_rod();
    let changes = Changes {
        deleted: std::collections::BTreeSet::from([rim]),
        ..Changes::default()
    };
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let never = retrim_with(OpKey(0x19a4), &rod, &changes, Narrow::Never, &m);
    assert!(
        matches!(
            never,
            Err(crate::Error::InvalidTweak(
                crate::TweakError::Uncertain { .. }
            ))
        ),
        "{never:?}"
    );
    let (o, _) = retrim_with(OpKey(0x19a4), &rod, &changes, Narrow::AfterUncertain, &m).unwrap();
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

fn uncertain_rung() -> super::Rung {
    super::Rung::Uncertain(
        crate::TweakError::Uncertain {
            at: Name::from_u128(1),
        }
        .into(),
    )
}

fn walked_rung() -> super::Rung {
    super::Rung::Walked(
        crate::TweakError::RegionTooSmall {
            face: Name::from_u128(2),
        }
        .into(),
    )
}

fn answers_uncertain(r: &super::Rung) -> bool {
    matches!(r, super::Rung::Done(r) if matches!(**r, Err(crate::Error::InvalidTweak(crate::TweakError::Uncertain { .. }))))
}

/// Design 669's retry: a first build's `Uncertain` at any table pair is
/// built again narrowed; one that narrows nothing leaves it; any other
/// narrowed outcome is the rung's, a failed walk or `Next` climbing.
#[test]
fn a_narrowed_rung_answers_unless_it_narrows_nothing() {
    use super::{Narrow, Rung, settle};
    let (r, narrowed) = settle(uncertain_rung(), Narrow::AfterUncertain, || Rung::Unchanged);
    assert!(answers_uncertain(&r) && !narrowed);
    let (r, narrowed) = settle(uncertain_rung(), Narrow::AfterUncertain, walked_rung);
    assert!(matches!(r, Rung::Walked(_)) && narrowed);
    let (r, narrowed) = settle(uncertain_rung(), Narrow::AfterUncertain, || {
        Rung::Next(
            crate::TweakError::HealAmbiguous {
                face: Name::from_u128(3),
            }
            .into(),
        )
    });
    assert!(matches!(r, Rung::Next(_)) && narrowed);
    let (r, narrowed) = settle(uncertain_rung(), Narrow::Never, || panic!("built narrowed"));
    assert!(answers_uncertain(&r) && !narrowed);
    let (r, narrowed) = settle(walked_rung(), Narrow::AfterUncertain, || {
        panic!("built narrowed")
    });
    assert!(matches!(r, Rung::Walked(_)) && !narrowed);
}

/// Design 669's ladder: once a rung was built narrowed, every later rung
/// is built narrowed only; first builds whose walks fail climb as before;
/// the ladder ends where the region can't grow.
#[test]
fn a_narrowed_ladder_climbs_and_stays_narrowed() {
    use super::{Build, Narrow, Rung, climb};
    use crate::TweakError;
    // Rung 0's first build refuses at a pair with no seam; its narrowed
    // walk fails; rungs 1 to 3 are built narrowed only, and walk short.
    let mut calls = Vec::new();
    let r = climb(Narrow::AfterUncertain, |rung, b| {
        calls.push((rung, b));
        Some(match (rung, b) {
            (0, Build::First) => uncertain_rung(),
            _ => walked_rung(),
        })
    });
    assert_eq!(
        calls,
        [
            (0, Build::First),
            (0, Build::Retry),
            (1, Build::Only),
            (2, Build::Only),
            (3, Build::Only)
        ]
    );
    assert!(matches!(
        r,
        Err(crate::Error::InvalidTweak(
            TweakError::RegionTooSmall { .. }
        ))
    ));
    // A retry that narrows nothing: the first `Uncertain` answers.
    let mut calls = Vec::new();
    let r = climb(Narrow::AfterUncertain, |rung, b| {
        calls.push((rung, b));
        Some(match b {
            Build::First => uncertain_rung(),
            _ => Rung::Unchanged,
        })
    });
    assert_eq!(calls, [(0, Build::First), (0, Build::Retry)]);
    assert!(matches!(
        r,
        Err(crate::Error::InvalidTweak(TweakError::Uncertain { .. }))
    ));
    // Without narrowing, rung 0's `Uncertain` answers.
    let mut calls = Vec::new();
    let r = climb(Narrow::Never, |rung, b| {
        calls.push((rung, b));
        Some(uncertain_rung())
    });
    assert_eq!(calls, [(0, Build::First)]);
    assert!(r.is_err());
    // First builds' failed walks climb, first builds all the way, until
    // the region can't grow (`None` at rung 2).
    let mut calls = Vec::new();
    let r = climb(Narrow::AfterUncertain, |rung, b| {
        calls.push((rung, b));
        (rung < 2).then(walked_rung)
    });
    assert_eq!(
        calls,
        [(0, Build::First), (1, Build::First), (2, Build::First)]
    );
    assert!(matches!(
        r,
        Err(crate::Error::InvalidTweak(
            TweakError::RegionTooSmall { .. }
        ))
    ));
}

/// FNV-1a of `b`'s `dump_unnamed` (as `ryukei-testkit`'s `fnv`, which the
/// crate can't depend on).
fn free_hash(b: &Body) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b.dump_unnamed().as_bytes() {
        h ^= u64::from(x);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The bent rod's rim deleted with stops, as plan 19.A built it.
const BENT_ROD_DELETED_HASH: u64 = 0xcd93_79f0_14d0_8ffc;

/// A plane stop goes only where no iso stop did (plan 19.B, Decision 3):
/// the bent rod's run, stopped by its iso stop, takes no plane stop, and
/// its rim's delete builds as 19.A's did, bit for bit.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn an_iso_stopped_run_takes_no_plane_stop() {
    use super::{Narrow, retrim_with};
    let (rod, rim) = crate::testing::bent_rod();
    let changes = Changes {
        deleted: std::collections::BTreeSet::from([rim]),
        ..Changes::default()
    };
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let (o, _) = retrim_with(OpKey(0x19a4), &rod, &changes, Narrow::AfterUncertain, &m).unwrap();
    // `held_seams`' golden pins the same body; here, its hash.
    assert_eq!(
        free_hash(&o),
        BENT_ROD_DELETED_HASH,
        "the bent rod's delete moved: {:#x}",
        free_hash(&o)
    );
}

/// A body whose first build settles is the same with stops allowed and
/// without (question 916): 18.C's rounded block, its top raised so the
/// round runs on past its moved end. Its first build builds, so `settle`
/// never runs the stopped build here and no plane stop is taken; plane
/// stops at running seams are covered by `held`'s `RUNNING_PLANE_STOPS`
/// and stop-plane tests and by 19.A's
/// `a_stopped_rung_answers_only_where_its_walk_closes`. A body whose first
/// build refuses at a running seam is still wanted for a test of its own.
#[test]
fn a_body_that_builds_is_the_same_with_stops_allowed() {
    use super::{Narrow, retrim_with};
    use crate::blend::{BlendKind, EdgeBlend, blend};
    let b0 = block();
    let (px, py) = (face_of(&b0, FaceRole::PlusX), face_of(&b0, FaceRole::PlusY));
    let edge = b0
        .edges()
        .find(|&e| {
            let fs: Vec<FaceId> = b0
                .edge_fins(e)
                .map(|fin| b0.loops[b0.fins[fin].loop_].face)
                .collect();
            fs.contains(&px) && fs.contains(&py)
        })
        .unwrap();
    let (b, _) = blend(
        K,
        &b0,
        &[EdgeBlend {
            edge: b0.edges[edge].name,
            kind: BlendKind::Fillet { radius: 0.1 },
        }],
    )
    .unwrap();
    let changes = raised(&b, 0.2);
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let never =
        retrim_with(OpKey(0x19b8), &b, &changes, Narrow::Never, &m).map(|(o, _)| o.dump_unnamed());
    let after = retrim_with(OpKey(0x19b8), &b, &changes, Narrow::AfterUncertain, &m)
        .map(|(o, _)| o.dump_unnamed());
    assert_eq!(format!("{never:?}"), format!("{after:?}"));
}

/// Design 669: a retry that narrows nothing is never built, and a rung
/// built narrowed only, where nothing narrows, is its first build to the
/// bit. The block's top raised: no held seam, no straddling face.
#[test]
fn a_retry_that_narrows_nothing_is_never_built() {
    use super::{Build, Rung, attempt};
    let b = block();
    let changes = raised(&b, 0.2);
    let changed = changes
        .motion
        .keys()
        .chain(&changes.deleted)
        .copied()
        .collect();
    let region = super::region::region(&b, &changed);
    let bx = super::body::region_box(&b, &region, super::reach(&b, &changes));
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    assert!(matches!(
        attempt(K, &b, &changes, &region, &bx, 0, Build::Retry, &m),
        Rung::Unchanged
    ));
    let dump = |r: Rung| match r {
        Rung::Done(r) => r.ok().map(|(o, _)| o.dump_unnamed()),
        _ => None,
    };
    let first = dump(attempt(K, &b, &changes, &region, &bx, 0, Build::First, &m));
    assert!(first.is_some(), "the raised block builds");
    assert_eq!(
        dump(attempt(K, &b, &changes, &region, &bx, 0, Build::Only, &m)),
        first
    );
}

/// FTC 07's quarter run (#652; census III): its pcurves straddle the
/// period's seam (one side stored at u = 5.5065, the other at 0.7767, its
/// rims over [-0.7767, 0.7767]), so read plainly it spans nearly the
/// whole turn, and round the circle (design 669) it is its rims' arc,
/// just under a quarter (89°), starting at the side moved a period down.
#[test]
fn ftc_07s_run_is_a_quarter_round_the_circle() {
    use crate::codec::decode_body;
    use crate::geom::Surface;
    use core::f64::consts::{PI, TAU};
    let input = decode_body(include_bytes!(
        "../../../tests/data/nist_ftc_07_seam_input.body"
    ))
    .unwrap();
    let run = input
        .faces()
        .find(|&f| matches!(input.surfaces[input.faces[f].surface], Surface::Cylinder(_)))
        .expect("the run");
    let [lo, hi] = super::working::own_span(&input, run, 0).unwrap();
    assert!(hi - lo > 1.5 * PI, "read plainly: {lo} {hi}");
    let [a, z] = super::working::own_hull(&input, run, 0, TAU).unwrap();
    assert!(
        z - a > 0.45 * PI && z - a < 0.6 * PI,
        "round the circle: {a} {z}"
    );
    // Every fin's span lies in the hull, read a whole period at most away.
    for fin in input.face_loops(run).flat_map(|l| input.loop_fins(l)) {
        let [c, d] = super::working::fin_span(&input, fin, 0).unwrap();
        let shift = ((a - c) / TAU).ceil() * TAU;
        assert!(
            c + shift >= a - 1e-12 && d + shift <= z + 1e-12,
            "{c} {d} in {a} {z}"
        );
    }
}

/// A committed cut's pair (plan 20.B): carried over the rung's box (the
/// work cut) the pair is `Uncertain` in a first build within a rung's
/// cap; carried over their near boxes, with every input face but the pair
/// counted deleted, their working faces' boxes are apart, so the table
/// never meets them.
fn apart_over_near_boxes(input: &[u8], work: &[u8]) {
    use crate::codec::decode_body;
    let input = decode_body(input).unwrap();
    let work = decode_body(work).unwrap();
    let pair: Vec<FaceId> = work.faces().collect();
    assert_eq!(pair.len(), 2);
    let inputs: Vec<FaceId> = pair
        .iter()
        .map(|&w| match input.by_name(work.faces[w].name) {
            Some(EntityId::Face(f)) => f,
            _ => panic!("no input face for {w:?}"),
        })
        .collect();
    // Over the rung's box: uncertain in a first build (no tube contact
    // there), within a rung's cap.
    // unmetered: a unit test.
    let m = crate::work::Meter::free();
    let met = m.first_build(|| {
        m.capped(crate::work::Scope::Rung, || {
            crate::intersect::face_face_in((&work, pair[0]), (&work, pair[1]), &m)
        })
    });
    assert!(met.is_err(), "met over the rung's box: {met:?}");
    // Every other input face is changed.
    let changes = Changes {
        deleted: input.faces().filter(|f| !inputs.contains(f)).collect(),
        ..Changes::default()
    };
    let region = super::region::Region {
        changed: changes.deleted.clone(),
        faces: input.faces().collect(),
        frontier: Default::default(),
    };
    let x = (0..2)
        .map(|k| crate::intersect::face_box(&work, pair[k]))
        .reduce(|a, b| a.union(&b))
        .unwrap();
    let boxes: Vec<_> = inputs
        .iter()
        .map(|&f| {
            let near = super::near::near_box(&input, &changes, &region, f, &x, 0);
            let w = super::working::working(&input, f, None, input.faces[f].sense, true, &near)
                .unwrap();
            let (sheet, _) = crate::sheet::from_surface(K, &w.surface, w.rect).unwrap();
            let sf = sheet.faces().next().unwrap();
            crate::intersect::face_box(&sheet, sf)
        })
        .collect();
    assert!(!boxes[0].intersects(&boxes[1]), "{boxes:?}");
}

/// FTC 07 rd's rung-2 pair after 20.A (#652, census IV): cylinder
/// `18dae4b7` and NURBS `33b57350`, both working, no shared edge, the
/// cylinder carried 0.34 m from an input 0.4 mm tall.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ftc_07s_carried_pair_is_apart_over_near_boxes() {
    apart_over_near_boxes(
        include_bytes!("../../../tests/data/nist_ftc_07_near_input.body"),
        include_bytes!("../../../tests/data/nist_ftc_07_near_work.body"),
    );
}

/// FTC 10's rung-2 pair (#652, census V): torus `a289199e` and the
/// cylinder `24401201` continuing its tube, touching no deleted face,
/// the cylinder carried 0.5 m along its axis.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ftc_10s_tube_pair_is_apart_over_near_boxes() {
    apart_over_near_boxes(
        include_bytes!("../../../tests/data/nist_ftc_10_near_input.body"),
        include_bytes!("../../../tests/data/nist_ftc_10_near_work.body"),
    );
}

/// STC 10 #3044's rung-2 pair (#658, census V): torus `e550357e` and
/// cylinder `872b2691`, FTC 10's shape in another edition.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn stc_10s_tube_pair_is_apart_over_near_boxes() {
    apart_over_near_boxes(
        include_bytes!("../../../tests/data/nist_stc_10_near_input.body"),
        include_bytes!("../../../tests/data/nist_stc_10_near_work.body"),
    );
}

/// CTC 02 with PMI's rung-3 pair (#658, censuses IV and V): cylinders
/// `a9b6e499` (6 mm) and `8031cb1f` (1.42 mm), axes 2° apart, each
/// carried about 0.9 m along y.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ctc_02s_carried_cylinders_are_apart_over_near_boxes() {
    apart_over_near_boxes(
        include_bytes!("../../../tests/data/nist_ctc_02_near_input.body"),
        include_bytes!("../../../tests/data/nist_ctc_02_near_work.body"),
    );
}

/// A committed cut's untouched pair (plan 21.A, census VI): carried over
/// the rung's box (the work cut) and met, as main's narrowed build meets
/// them, the pair is `Uncertain` within a rung's cap. Task 7 adds that
/// once pinned they are never met.
fn untouched_pair(input: &[u8], work: &[u8]) -> (Body, Vec<FaceId>) {
    use crate::codec::decode_body;
    let input = decode_body(input).unwrap();
    let work = decode_body(work).unwrap();
    let pair: Vec<FaceId> = work.faces().collect();
    assert_eq!(pair.len(), 2);
    let inputs: Vec<FaceId> = pair
        .iter()
        .map(|&w| match input.by_name(work.faces[w].name) {
            Some(EntityId::Face(f)) => f,
            _ => panic!("no input face for {w:?}"),
        })
        .collect();
    // unmetered but capped: a rung's cap, as the re-trim gives it.
    let m = crate::work::Meter::free();
    let met = m.capped(crate::work::Scope::Rung, || {
        crate::intersect::face_face_in((&work, pair[0]), (&work, pair[1]), &m)
    });
    assert!(met.is_err(), "met over the rung's box: {met:?}");
    (input, inputs)
}

/// FTC 07 rd's rung-3 pair after 20.B (#652, census V and VI): torus
/// `14131c92` and plane `9d1d527c`, both touching no change, overlapping,
/// sharing no input edge (census VI).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ftc_07s_untouched_pair_is_pinned() {
    untouched_pair(
        include_bytes!("../../../tests/data/nist_ftc_07_pinned_input.body"),
        include_bytes!("../../../tests/data/nist_ftc_07_pinned_work.body"),
    );
}

/// CTC 02 with PMI's retry pair after 20.B (#658, #295): plane
/// `74fca8b1`, working, and NURBS `b150cb1a`, outside rung 3's region,
/// both touching no change, tangent along their shared frontier edge
/// (census VI).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn ctc_02s_untouched_pair_is_pinned() {
    untouched_pair(
        include_bytes!("../../../tests/data/nist_ctc_02_pinned_input.body"),
        include_bytes!("../../../tests/data/nist_ctc_02_pinned_work.body"),
    );
}
