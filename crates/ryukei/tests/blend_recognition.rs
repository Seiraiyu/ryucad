//! Blend recognition (plan 13.C): fillets, chamfers and ball corners read
//! back from bodies `blend::blend` built, their NURBS forms, and faces
//! that are nearly blends left out.

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::check;
use ryukei::geom::{Frame, Point3, Rigid, Surface, Vector3};
use ryukei::name::{FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId};
use ryukei::tweak::{RecogniseOptions, Recognition, recognise_blends};
use ryukei::{Error, TweakError};

const K: OpKey = OpKey(0x13c0);

fn face(r: FaceRole) -> Name {
    Name::derive(K, Role::Face(r), &[])
}

fn faces_of(b: &Body, e: EdgeId) -> Vec<Name> {
    b.edge_fins(e)
        .map(|fin| {
            b.face(b.loop_(b.fin(fin).unwrap().loop_).unwrap().face)
                .unwrap()
                .name
        })
        .collect()
}

/// The edge of `b` between faces named `a` and `c`.
fn edge_between(b: &Body, a: Name, c: Name) -> Name {
    let e = b
        .edges()
        .find(|&e| {
            let n = faces_of(b, e);
            n.contains(&a) && n.contains(&c)
        })
        .unwrap();
    b.edge(e).unwrap().name
}

/// The names of `b`'s faces on surfaces `pick` takes.
fn faces_on(b: &Body, pick: impl Fn(&Surface) -> bool) -> Vec<Name> {
    b.faces()
        .filter(|&f| pick(b.surface(b.face(f).unwrap().surface).unwrap()))
        .map(|f| b.face(f).unwrap().name)
        .collect()
}

fn block() -> Body {
    primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0
}

/// `b` with every edge blended by `kind`.
fn every_edge(b: &Body, kind: BlendKind) -> Body {
    let all: Vec<EdgeBlend> = b
        .edges()
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind,
        })
        .collect();
    blend(OpKey(7), b, &all).unwrap().0
}

fn recognise(b: &Body) -> Recognition {
    recognise_blends(b, &RecogniseOptions::default()).unwrap()
}

/// A plate 1 × 1 × 0.2 with a cylinder of radius 0.1 united on its top
/// (`boss`) or drilled through it.
fn plate_with(boss: bool) -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(
        OpKey(9),
        0.1,
        if boss { 0.2 } else { 0.6 },
        &Rigid::IDENTITY,
    )
    .unwrap();
    let (op, z) = if boss {
        (BooleanOp::Unite, 0.2)
    } else {
        (BooleanOp::Subtract, -0.2)
    };
    let tool = Tool {
        key: ToolKey(1),
        body: &pin,
        place: Rigid::translation(Vector3::new(0.5, 0.5, z)),
    };
    boolean(OpKey(10), op, &plate, &[tool]).unwrap().0
}

#[test]
fn a_rounded_boxs_fillets_and_corners_are_recognised() {
    let r = every_edge(&block(), BlendKind::Fillet { radius: 0.05 });
    let rec = recognise(&r);
    assert_eq!(rec.blends.len(), 12);
    assert_eq!(rec.corners.len(), 8);
    for b in &rec.blends {
        let BlendKind::Fillet { radius } = b.kind else {
            panic!("{:?}", b.kind)
        };
        assert!((radius - 0.05).abs() < 1e-12, "{radius}");
    }
    // The edges meet only through corners, so each is a chain of its own.
    let mut chains: Vec<usize> = rec.blends.iter().map(|b| b.chain).collect();
    chains.dedup();
    assert_eq!(chains.len(), 12);
    for c in &rec.corners {
        assert_eq!(c.blends.len(), 3);
    }
}

#[test]
fn a_chamfered_boxs_mitred_chamfers_are_recognised() {
    let c = every_edge(
        &block(),
        BlendKind::Chamfer(Chamfer::Equal { distance: 0.05 }),
    );
    let rec = recognise(&c);
    assert_eq!(rec.blends.len(), 12);
    for b in &rec.blends {
        let BlendKind::Chamfer(Chamfer::Equal { distance }) = b.kind else {
            panic!("{:?}", b.kind)
        };
        assert!((distance - 0.05).abs() < 1e-12, "{distance}");
    }
}

#[test]
fn a_bosss_foot_fillet_is_a_concave_torus() {
    let boss = plate_with(true);
    let side = faces_on(&boss, |s| matches!(s, Surface::Cylinder(_)))[0];
    let e = edge_between(&boss, face(FaceRole::PlusZ), side);
    let edge = EdgeBlend {
        edge: e,
        kind: BlendKind::Fillet { radius: 0.02 },
    };
    let (b, _) = blend(OpKey(11), &boss, &[edge]).unwrap();
    let rec = recognise(&b);
    assert_eq!(rec.blends.len(), 1);
    let mut supports = [face(FaceRole::PlusZ), side];
    supports.sort();
    assert_eq!(rec.blends[0].supports, supports);
    assert!(
        matches!(rec.blends[0].kind, BlendKind::Fillet { radius } if (radius - 0.02).abs() < 1e-12)
    );
}

#[test]
fn a_holes_rim_chamfer_keeps_its_two_distances() {
    let hole = plate_with(false);
    let wall = faces_on(&hole, |s| matches!(s, Surface::Cylinder(_)))[0];
    let e = edge_between(&hole, face(FaceRole::PlusZ), wall);
    let kind = BlendKind::Chamfer(Chamfer::Distances {
        first: 0.02,
        second: 0.03,
        first_face: wall,
    });
    let (b, _) = blend(OpKey(13), &hole, &[EdgeBlend { edge: e, kind }]).unwrap();
    let rec = recognise(&b);
    assert_eq!(rec.blends.len(), 1);
    let BlendKind::Chamfer(Chamfer::Distances {
        first,
        second,
        first_face,
    }) = rec.blends[0].kind
    else {
        panic!("{:?}", rec.blends[0].kind)
    };
    let d = if first_face == wall {
        [first, second]
    } else {
        [second, first]
    };
    assert!(
        (d[0] - 0.02).abs() < 1e-9 && (d[1] - 0.03).abs() < 1e-9,
        "{d:?}"
    );
}

#[test]
fn spline_forms_are_recognised() {
    let b = block();
    let e = edge_between(&b, face(FaceRole::PlusZ), face(FaceRole::PlusX));
    let edge = EdgeBlend {
        edge: e,
        kind: BlendKind::Fillet { radius: 0.1 },
    };
    let mut f = blend(OpKey(1), &b, &[edge]).unwrap().0;
    ryukei_testkit::nurbs::convert_body(&mut f).unwrap();
    let rec = recognise(&f);
    assert_eq!(rec.blends.len(), 1);
    assert!(
        matches!(rec.blends[0].kind, BlendKind::Fillet { radius } if (radius - 0.1).abs() < 1e-9)
    );
    let mut c = every_edge(&b, BlendKind::Chamfer(Chamfer::Equal { distance: 0.05 }));
    ryukei_testkit::nurbs::convert_body(&mut c).unwrap();
    assert_eq!(recognise(&c).blends.len(), 12);
}

#[test]
fn plain_faces_and_near_blends_are_left_out() {
    assert!(recognise(&block()).blends.is_empty());
    assert!(recognise(&plate_with(false)).blends.is_empty());
    // A cove cut along an edge, its cylinder not tangent to the faces.
    let (pin, _) = primitive::cylinder(OpKey(9), 0.1, 2.0, &Rigid::IDENTITY).unwrap();
    let place = Rigid::from_frame(
        &ryukei::geom::Frame::new(
            ryukei::geom::Point3::new(-0.5, 0.6, 0.4),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    );
    let tool = Tool {
        key: ToolKey(1),
        body: &pin,
        place,
    };
    let (cove, _) = boolean(OpKey(12), BooleanOp::Subtract, &block(), &[tool]).unwrap();
    assert!(recognise(&cove).blends.is_empty());
}

#[test]
fn a_bad_tolerance_is_refused() {
    for tolerance in [0.0, 1e-9, 1.0, f64::NAN] {
        let got = recognise_blends(&block(), &RecogniseOptions { tolerance });
        assert!(
            matches!(got, Err(Error::InvalidTweak(TweakError::BadTolerance))),
            "{got:?}"
        );
    }
}

#[test]
fn one_thread_and_eight_recognise_alike() {
    let r = every_edge(&block(), BlendKind::Fillet { radius: 0.05 });
    let pool = |n| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
    };
    let one = pool(1).install(|| recognise(&r));
    let eight = pool(8).install(|| recognise(&r));
    assert_eq!(one, eight);
}

/// A rib 2.8 mm wide whose top is rounded whole at 1.4 mm, one wall 4 µm
/// inside the round's tangent, so the round's arc runs on past a half
/// turn on that side: NIST CTC 03's, 04's, 05's, FTC 06's, 08's and 09's
/// rib (#650, #651, #653, #654). Its walls face apart and never meet: no
/// fillet.
#[test]
fn a_ribs_full_round_is_no_fillet() {
    let r = 1.4e-3;
    let (block, _) = primitive::block(K, 2.0 * r - 4e-6, 0.01, 0.005, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(9), r, 0.01, &Rigid::IDENTITY).unwrap();
    let place = Rigid::from_frame(
        &Frame::new(
            Point3::new(r, 0.0, 0.005),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
    );
    let tool = Tool {
        key: ToolKey(1),
        body: &pin,
        place,
    };
    let (rib, _) = boolean(OpKey(12), BooleanOp::Unite, &block, &[tool]).unwrap();
    assert_eq!(check(&rib), []);
    assert!(recognise(&rib).blends.is_empty());
}
