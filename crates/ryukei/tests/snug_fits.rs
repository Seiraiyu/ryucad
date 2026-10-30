//! Snug fits (issue #159): parts whose faces exactly touch, placed in an
//! assembly by poses and intersected in one part's frame as interference
//! does (`pose_a⁻¹ ∘ pose_b`), whose rounding moves the second part by a
//! few ulps. The issue's figures, in millimetres made meters as ryucad
//! makes them: a 72.41 mm line bore against a half shell of 33.775 to
//! 36.205 mm radius turned through 180°; a cap whose sides are its
//! pocket's sides; a bolt head standing on its seat, and a pair of bolts
//! on one cap. Every pose of every seed must give a clear result (nothing
//! shared) or a touch (`NonManifold`), never `Uncertain`. Release only
//! (`cargo test -p ryukei --release --test snug_fits`).

use core::f64::consts::PI;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Axis, Curve2, Frame, Line2, Point2, Point3, Rigid, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};
use ryukei::topo::Body;
use ryukei::{BooleanError, Error};
use ryukei_testkit::Lcg;

const MM: f64 = 1e-3;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

fn minus(a: &Body, b: &Body, place: Rigid, key: u128) -> Body {
    boolean(
        OpKey(key),
        BooleanOp::Subtract,
        a,
        &[Tool {
            key: ToolKey(1),
            body: b,
            place,
        }],
    )
    .unwrap()
    .0
}

/// A rectangle `[r0, r1] × [0, h]` in the xz plane turned about z through
/// `turn`: a shell (or with `r0` 0, a solid of revolution).
fn turned(key: u128, r0: f64, r1: f64, h: f64, turn: f64) -> Body {
    let pts = [
        Point2::new(r0, 0.0),
        Point2::new(r1, 0.0),
        Point2::new(r1, h),
        Point2::new(r0, h),
    ];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            Segment {
                key: SegmentKey(i as u128 + 1),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
    revolve(OpKey(key), &profile, axis, 0.0, turn).unwrap().0
}

/// A seeded rigid placement: a turn about a random axis and a move of up
/// to a metre, as an assembly's pose.
fn pose(g: &mut Lcg) -> Rigid {
    let mut x = || (g.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
    let axis = Vector3::new(x() - 0.5, x() - 0.5, x() - 0.5 + 1e-3)
        .normalize()
        .unwrap();
    Rigid::rotation(axis, 2.0 * PI * x()).then_after(&at(x() - 0.5, x() - 0.5, x() - 0.5))
}

/// The pairs: name, the first part and the second, both at the origin of
/// the assembly where they fit.
fn pairs() -> Vec<(&'static str, Body, Body)> {
    let len = 20.0 * MM;
    // A block with a 72.41 mm line bore along z, and the upper half shell.
    let block = primitive::block(
        OpKey(1),
        120.0 * MM,
        120.0 * MM,
        len,
        &at(-0.06, -0.06, 0.0),
    )
    .unwrap()
    .0;
    let bore = primitive::cylinder(
        OpKey(2),
        72.41 * MM / 2.0,
        2.0 * len,
        &at(0.0, 0.0, -len / 2.0),
    )
    .unwrap()
    .0;
    let bored = minus(&block, &bore, Rigid::IDENTITY, 10);
    let shell = turned(3, 33.775 * MM, 36.205 * MM, len, PI);
    // A block with a pocket 30 mm wide and 10 deep, and the cap filling it
    // side to side, standing proud of the top.
    let base = primitive::block(OpKey(4), 60.0 * MM, 40.0 * MM, 30.0 * MM, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let pocket = primitive::block(
        OpKey(5),
        30.0 * MM,
        50.0 * MM,
        20.0 * MM,
        &at(0.015, -0.005, 0.02),
    )
    .unwrap()
    .0;
    let pocketed = minus(&base, &pocket, Rigid::IDENTITY, 11);
    let cap = primitive::block(
        OpKey(6),
        30.0 * MM,
        40.0 * MM,
        15.0 * MM,
        &at(0.015, 0.0, 0.02),
    )
    .unwrap()
    .0;
    // A bolt (a 13 mm head on an 8 mm shank) standing on the cap's top
    // through a clearance hole, and a pair of them.
    let head = primitive::cylinder(OpKey(7), 6.5 * MM, 5.0 * MM, &at(0.0, 0.0, 0.0))
        .unwrap()
        .0;
    let shank = primitive::cylinder(OpKey(8), 4.0 * MM, 30.0 * MM, &at(0.0, 0.0, -0.03))
        .unwrap()
        .0;
    let bolt = boolean(
        OpKey(12),
        BooleanOp::Unite,
        &head,
        &[Tool {
            key: ToolKey(1),
            body: &shank,
            place: Rigid::IDENTITY,
        }],
    )
    .unwrap()
    .0;
    let hole = |x: f64| {
        primitive::cylinder(OpKey(9), 4.5 * MM, 40.0 * MM, &at(x, 0.02, 0.0))
            .unwrap()
            .0
    };
    let one = minus(&cap, &hole(0.03), Rigid::IDENTITY, 13);
    let two = minus(&one, &hole(0.022), Rigid::IDENTITY, 14);
    let seated = |x: f64| bolt.transformed(&at(x, 0.02, 0.035)).unwrap();
    vec![
        ("line bore and half shell", bored, shell),
        ("pocket and cap", pocketed, cap),
        ("cap and bolt", one, seated(0.03)),
        ("cap of two and its first bolt", two.clone(), seated(0.03)),
        ("cap of two and its second bolt", two, seated(0.022)),
    ]
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn snug_fits_are_clear_or_touching_under_rounded_poses() {
    let mut bad = Vec::new();
    for (name, a, b) in pairs() {
        for seed in 0..32 {
            let mut g = Lcg::new(seed);
            let pa = pose(&mut g);
            // The second part's pose reaches the same place another way (a
            // whole turn about another axis first, the identity but for
            // rounding), then the two are related as interference relates
            // them.
            let turn = pose(&mut g);
            let axis = Vector3::new(1.0, 2.0, 3.0).normalize().unwrap();
            let pb = pa.then_after(
                &turn
                    .inverse()
                    .then_after(&Rigid::rotation(axis, 2.0 * PI))
                    .then_after(&turn),
            );
            let place = pa.inverse().then_after(&pb);
            let r = boolean(
                OpKey(20),
                BooleanOp::Intersect,
                &a,
                &[Tool {
                    key: ToolKey(1),
                    body: &b,
                    place,
                }],
            );
            match r {
                Ok((body, _)) if body.faces().next().is_none() => {}
                Err(Error::InvalidBoolean(BooleanError::NonManifold { .. })) => {}
                Ok((body, _)) => bad.push(format!(
                    "{name}, seed {seed}: shares {} faces",
                    body.counts().faces
                )),
                Err(e) => bad.push(format!("{name}, seed {seed}: {e:?}")),
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
