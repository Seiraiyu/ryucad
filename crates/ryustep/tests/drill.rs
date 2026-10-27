//! Bodies read from STEP with spline faces take part in booleans: a
//! lofted frustum and a sphere in NURBS form written and read back, then
//! drilled and halved, against exact volumes; and the committed corpus's
//! spline-filleted body drilled through a fillet (release only).

use core::f64::consts::PI;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Curve2, Frame, Line2, Point2, Point3, Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft};
use ryukei::topo::Body;
use ryukei_testkit::mesh::volume_and_area;
use ryustep::{LengthUnit, Product, ReadOptions, WriteModel, WriteOptions, read, write};

/// `b` written to STEP in meters and read back, healed.
fn through_step(b: &Body) -> Body {
    let bytes = write(
        &WriteModel {
            products: vec![Product {
                id: "part".into(),
                name: "part".into(),
                bodies: vec![b.clone()],
                children: vec![],
            }],
        },
        &WriteOptions {
            unit: LengthUnit::Meter,
            file_name: "drill".into(),
            time_stamp: "2026-10-27T00:00:00".into(),
            ..WriteOptions::default()
        },
    )
    .unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(5),
            heal: Default::default(),
        },
    )
    .unwrap();
    m.bodies[0].result.clone().unwrap().0
}

fn spline_faces(b: &Body) -> usize {
    b.faces()
        .filter(|&f| {
            matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Nurbs(_))
            )
        })
        .count()
}

/// The volume of a sound body, checked and meshed at a chord of a
/// millimetre.
fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-3, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    volume_and_area(&m).0
}

fn near(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 2e-3 * want.abs().max(1e-3),
        "{got} against {want}"
    );
}

fn cut(op: BooleanOp, target: &Body, tool: &Body, place: Rigid) -> Body {
    let tools = [Tool {
        key: ToolKey(1),
        body: tool,
        place,
    }];
    boolean(OpKey(9), op, target, &tools).unwrap().0
}

/// A square of side `s` in the plane `z = h`.
fn square(h: f64, first: u128, s: f64) -> ValidProfile {
    let d = s / 2.0;
    let pts = [(-d, -d), (d, -d), (d, d), (-d, d)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(first + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap()
}

#[test]
fn spline_bodies_read_from_step_are_drilled() {
    let (frustum, _) = loft(
        OpKey(3),
        &[square(0.0, 10, 2.0), square(1.0, 20, 1.0)],
        &LoftOptions::default(),
    )
    .unwrap();
    let frustum = through_step(&frustum);
    assert_eq!(spline_faces(&frustum), 4);
    let rod = primitive::cylinder(OpKey(2), 0.2, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let drilled = cut(
        BooleanOp::Subtract,
        &frustum,
        &rod,
        Rigid::translation(Vector3::new(0.0, 0.0, -1.0)),
    );
    near(volume(&drilled), 7.0 / 3.0 - PI * 0.04);
    let mut ball = primitive::sphere(OpKey(4), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    ryukei_testkit::nurbs::convert_body(&mut ball).unwrap();
    let ball = through_step(&ball);
    assert_eq!(spline_faces(&ball), 1);
    let block = primitive::block(OpKey(7), 2.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let half = cut(
        BooleanOp::Intersect,
        &ball,
        &block,
        Rigid::translation(Vector3::new(0.0, -1.5, -1.5)),
    );
    near(volume(&half), 2.0 / 3.0 * PI);
}

/// The committed corpus's body with spline fillets (NIST's CTC 05),
/// drilled through a fillet and through its plate: what the drill takes
/// and what it leaves make up the body. Its tolerant edges, read from the
/// file, take fitted pcurves on the faces they merge into. Its own
/// self-intersection check is plan 8.F's (25 pairs it can't settle yet).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn the_corpus_spline_body_is_drilled() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let bytes = std::fs::read(dir.join("nist_ctc_05_asme1_rd.stp")).unwrap();
    let m = read(
        &bytes,
        &ReadOptions {
            key: OpKey(7),
            heal: Default::default(),
        },
    )
    .unwrap();
    let body = m
        .bodies
        .iter()
        .find_map(|b| b.result.as_ref().ok())
        .unwrap()
        .0
        .clone();
    assert_eq!(spline_faces(&body), 9);
    let size = |b: &Body| {
        let bx = b
            .faces()
            .map(|f| ryukei::intersect::face_box(b, f))
            .reduce(|x, y| x.union(&y))
            .unwrap();
        (bx.max() - bx.min()).length()
    };
    let mesh_volume = |b: &Body| {
        assert_eq!(check(b), []);
        let m = tessellate(b, &MeshParams::new(size(&body) * 2e-4, 0.3)).unwrap();
        assert_eq!(m.faults, []);
        volume_and_area(&m).0
    };
    let whole = mesh_volume(&body);
    let rod = primitive::cylinder(OpKey(2), 0.004, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for (x, y) in [(0.1417, 0.0619), (-0.0812, -0.0739)] {
        let place = Rigid::translation(Vector3::new(x, y, -0.5));
        let left = cut(BooleanOp::Subtract, &body, &rod, place);
        let taken = cut(BooleanOp::Intersect, &body, &rod, place);
        let (l, t) = (mesh_volume(&left), mesh_volume(&taken));
        assert!(t > 0.0);
        assert!(
            (l + t - whole).abs() <= 1e-3 * whole,
            "{l} + {t} against {whole}"
        );
    }
}
