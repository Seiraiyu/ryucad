//! ryukei phase 19's twin of #652's seams (design 642, plan 19.A): a
//! round bar of radius 0.3 along x by 2, bent a quarter turn of radius 1
//! towards y, on along y by 2 (`sweep::along`'s cylinder, torus and
//! cylinder, and two caps), its start cap's rim filleted 0.05. Deleting
//! the rim carries the first cylinder past the bend, where it crosses
//! the torus, which stays outside the region: a frontier seam with fixed
//! ends, met along it once the run stops there (an iso stop). The crate
//! builds the same rod (`ryukei::testing::bent_rod`, crate-internal);
//! [`ROD_DUMP_HASH`] pins both.

use core::f64::consts::{FRAC_PI_2, TAU};
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::geom::{
    Circle, Circle2, Curve, Curve2, Frame, Frame2, Line, Point2, Point3, Surface, Vector2, Vector3,
};
use ryukei::name::OpKey;
use ryukei::sweep::{
    AlongOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey, ValidProfile,
    along,
};
use ryukei::topo::{Body, FaceId};
use ryukei::tweak::{DeleteBlends, RecogniseOptions};

/// The rod's radius, its bend's and its rim fillet's.
pub const RADIUS: f64 = 0.3;
pub const BEND: f64 = 1.0;
pub const RIM: f64 = 0.05;

/// FNV-1a of the rod's `dump_unnamed`: the crate's and this one's are
/// the same rod. After a deliberate change to either, set it to the
/// value the failure prints, in both places it is checked.
pub const ROD_DUMP_HASH: u64 = 0x7032_8907_047f_8ef7;

/// FNV-1a over `bytes`.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The faces of `b` using edge `e`.
fn faces_of(b: &Body, e: ryukei::topo::EdgeId) -> Vec<FaceId> {
    b.edge_fins(e)
        .map(|fin| b.loop_(b.fin(fin).unwrap().loop_).unwrap().face)
        .collect()
}

/// The rod with its start rim filleted.
pub fn bent_rod() -> Body {
    let f2 = Frame2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0), true).unwrap();
    let round = Loop {
        key: LoopKey(1),
        segments: vec![Segment {
            key: SegmentKey(1),
            curve: Curve2::Circle2(Circle2::new(f2, RADIUS).unwrap()),
            t0: 0.0,
            t1: TAU,
        }],
    };
    let profile = ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::ORIGIN,
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
        loops: vec![round],
    })
    .unwrap();
    let bend = Frame::new(
        Point3::new(2.0, BEND, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let part = |key, curve, range| PathSegment {
        key: SegmentKey(key),
        curve,
        range,
    };
    let path = Path {
        segments: vec![
            part(
                1,
                Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0)).unwrap()),
                [0.0, 2.0],
            ),
            part(
                2,
                Curve::Circle(Circle::new(bend, BEND).unwrap()),
                [-FRAC_PI_2, 0.0],
            ),
            part(
                3,
                Curve::Line(
                    Line::new(
                        Point3::new(2.0 + BEND, BEND, 0.0),
                        Vector3::new(0.0, 1.0, 0.0),
                    )
                    .unwrap(),
                ),
                [0.0, 2.0],
            ),
        ],
    };
    let (rod, _) = along(OpKey(0x19a1), &profile, &path, &AlongOptions::default()).unwrap();
    // The start cap, the plane at x = 0, and its one edge, the rim.
    let cap = rod
        .faces()
        .find(|&f| {
            matches!(
                rod.surface(rod.face(f).unwrap().surface).unwrap(),
                Surface::Plane(_)
            ) && ryukei::intersect::face_box(&rod, f).max().x.abs() < 1e-6
        })
        .unwrap();
    let rim = rod
        .edges()
        .find(|&e| faces_of(&rod, e).contains(&cap))
        .unwrap();
    let (out, _) = blend(
        OpKey(0x19a2),
        &rod,
        &[EdgeBlend {
            edge: rod.edge(rim).unwrap().name,
            kind: BlendKind::Fillet { radius: RIM },
        }],
    )
    .unwrap();
    out
}

/// The delete that takes the rim and nothing else: blends no larger
/// than twice the rim (the bend's torus is a tube of the rod's radius).
pub fn spec() -> DeleteBlends {
    DeleteBlends {
        max_size: Some(2.0 * RIM),
        faces: Vec::new(),
        options: RecogniseOptions::default(),
    }
}
