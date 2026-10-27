//! Booleans of bodies on every surface: primitives with their curves and
//! surfaces swapped for exact NURBS forms, lofts, sweeps along paths,
//! spline revolutions and extrusions. Every result passes the checker and
//! the self-intersection check, meshes without faults, and encloses the
//! volume it should (exact where one is known, else the set identities).
//! The slow cases run in release only (`cargo test -p ryukei --release
//! --test general_booleans`).

use core::f64::consts::{PI, TAU};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{
    Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Rigid, Vector2, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{LoftOptions, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, loft};
use ryukei::topo::Body;
use ryukei_testkit::mesh::volume_and_area;
use ryukei_testkit::nurbs::convert_body;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// `b` with every curve and surface its exact NURBS form.
fn nurbs(mut b: Body) -> Body {
    convert_body(&mut b).unwrap();
    b
}

/// The volume a sound body's mesh encloses, at a chord of a millimetre.
fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-3, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    volume_and_area(&m).0
}

fn run(op: BooleanOp, target: &Body, tool: &Body, place: Rigid) -> Body {
    let tools = [Tool {
        key: ToolKey(1),
        body: tool,
        place,
    }];
    boolean(OpKey(100), op, target, &tools).unwrap().0
}

fn near(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 2e-3 * want.abs().max(1e-3),
        "{got} against {want}"
    );
}

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

/// A square of side `s` centred on its frame's origin.
fn square(first: u128, s: f64) -> Loop {
    let h = s / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            seg(
                first + i as u128,
                Curve2::Line2(Line2::through(p, q).unwrap()),
                0.0,
                p.distance(q),
            )
        })
        .collect();
    Loop {
        key: LoopKey(1),
        segments,
    }
}

/// A circle of radius `r` about its frame's origin.
fn circle(key: u128, r: f64) -> Loop {
    let f = Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap();
    Loop {
        key: LoopKey(1),
        segments: vec![seg(
            key,
            Curve2::Circle2(Circle2::new(f, r).unwrap()),
            0.0,
            TAU,
        )],
    }
}

/// A section in the plane `z = h`, turned by `turn` about `z`.
fn section(h: f64, turn: f64, l: Loop) -> ValidProfile {
    let (s, c) = ryukei::math::sin_cos(turn);
    ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::new(0.0, 0.0, h),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(c, s, 0.0),
        )
        .unwrap(),
        loops: vec![l],
    })
    .unwrap()
}

/// A plate with a hole, every way round: the plate, the rod or both in
/// NURBS form.
#[test]
fn a_hole_through_a_spline_plate() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for (p, r) in [
        (nurbs(plate.clone()), rod.clone()),
        (plate.clone(), nurbs(rod.clone())),
        (nurbs(plate.clone()), nurbs(rod.clone())),
    ] {
        let cut = run(BooleanOp::Subtract, &p, &r, at(2.0, 2.0, -1.0));
        near(volume(&cut), 16.0 - PI * 0.25);
        assert_eq!(cut.counts().faces, 7);
    }
}

/// A lofted frustum drilled down its middle, and a lofted cone cut by a
/// block: what the cut takes and what it leaves make up the cone.
#[test]
fn lofts_cut() {
    let (frustum, _) = loft(
        OpKey(5),
        &[
            section(0.0, 0.0, square(10, 2.0)),
            section(1.0, 0.0, square(20, 1.0)),
        ],
        &LoftOptions::default(),
    )
    .unwrap();
    let rod = primitive::cylinder(OpKey(2), 0.2, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let drilled = run(BooleanOp::Subtract, &frustum, &rod, at(0.0, 0.0, -1.0));
    near(volume(&drilled), 7.0 / 3.0 - PI * 0.04);
    let (cone, _) = loft(
        OpKey(6),
        &[
            section(0.0, 0.0, circle(1, 1.0)),
            section(1.0, 0.0, circle(2, 0.5)),
        ],
        &LoftOptions::default(),
    )
    .unwrap();
    let block = primitive::block(OpKey(7), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let place = at(0.3, -1.0, -0.5);
    let cut = run(BooleanOp::Subtract, &cone, &block, place);
    let kept = run(BooleanOp::Intersect, &cone, &block, place);
    near(volume(&cut) + volume(&kept), PI / 3.0 * (1.0 + 0.25 + 0.5));
}

/// A spline block and a block turned askew: pieces of the spline block's
/// bottom and side, which no longer meet, take boxes of their own, not
/// their whole surfaces' (whose sides would meet along a line neither
/// piece reaches).
#[test]
fn a_spline_block_and_a_block_askew() {
    let (a, b) = (0.978_264_102_398_909_7, 0.405_740_067_958_108_63);
    let spline = nurbs(
        primitive::block(OpKey(1), a, b, 0.9, &at(-a / 2.0, -b / 2.0, -0.45))
            .unwrap()
            .0,
    );
    let (c, d) = (0.719_139_516_023_649_6, 0.784_001_133_238_684_9);
    let axis = Vector3::new(
        -0.097_472_403_793_364_97,
        0.810_614_511_018_428,
        0.277_315_428_202_596_25,
    )
    .normalize()
    .unwrap();
    let place = at(
        0.444_509_347_737_580_33,
        0.105_675_578_460_901_14,
        -0.342_216_763_768_183_83,
    )
    .then_after(&Rigid::rotation(axis, 1.851_300_189_756_587))
    .then_after(&at(-c / 2.0, -d / 2.0, -0.45));
    let block = primitive::block(OpKey(2), c, d, 0.9, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let u = run(BooleanOp::Unite, &spline, &block, place);
    let i = run(BooleanOp::Intersect, &spline, &block, place);
    near(volume(&u) + volume(&i), a * b * 0.9 + c * d * 0.9);
}

/// A spline meridian from the axis round to the axis, turned about z: a
/// surface of revolution with a pole at each end.
fn spline_revolution(r: f64, h: f64) -> Body {
    use ryukei::geom::{Axis, KnotVector, NurbsCurve2};
    use ryukei::sweep::revolve;
    use std::sync::Arc;
    let kv = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
    let pts = vec![
        Point2::new(0.0, -h),
        Point2::new(r * 1.3, -h),
        Point2::new(r * 0.9, h),
        Point2::new(0.0, h),
    ];
    let meridian = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(kv, pts, None).unwrap()));
    let back = Curve2::Line2(Line2::through(Point2::new(0.0, h), Point2::new(0.0, -h)).unwrap());
    let profile = ValidProfile::new(Profile {
        frame: Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap(),
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![seg(1, meridian, 0.0, 1.0), seg(2, back, 0.0, 2.0 * h)],
        }],
    })
    .unwrap();
    let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
    revolve(OpKey(8), &profile, axis, 0.0, TAU).unwrap().0
}

/// A cap off a spline ball and off a spline revolution: the piece left
/// round the pole closes there, as on a sphere.
#[test]
fn caps_round_spline_poles() {
    let ball = nurbs(
        primitive::sphere(OpKey(3), 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let block = primitive::block(OpKey(7), 2.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let place = at(-1.0, -1.5, 0.5);
    let cap = run(BooleanOp::Intersect, &ball, &block, place);
    let rest = run(BooleanOp::Subtract, &ball, &block, place);
    let want = PI * 0.25 * (3.0 - 0.5) / 3.0;
    near(volume(&cap), want);
    near(volume(&rest), 4.0 / 3.0 * PI - want);
    let rev = spline_revolution(0.8, 0.6);
    let place = at(-1.0, -1.5, 0.3);
    let cap = run(BooleanOp::Intersect, &rev, &block, place);
    let rest = run(BooleanOp::Subtract, &rev, &block, place);
    near(volume(&cap) + volume(&rest), volume(&rev));
}

/// A spline ball halved through both its poles: the plane meets the
/// collapsed rows, where every crossing beside them is at the pole.
#[test]
fn a_spline_ball_halved_through_its_poles() {
    let ball = nurbs(
        primitive::sphere(OpKey(3), 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let block = primitive::block(OpKey(7), 2.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let place = at(0.0, -1.5, -1.5);
    let half = 2.0 / 3.0 * PI;
    near(
        volume(&run(BooleanOp::Intersect, &ball, &block, place)),
        half,
    );
    near(
        volume(&run(BooleanOp::Subtract, &ball, &block, place)),
        half,
    );
}

/// A block cornered at a spline ball's centre: its faces cut the ball
/// along its equator, a knot line of its spline form, and its edges
/// meet the ball on the seam and through the poles.
#[test]
fn an_octant_off_a_spline_ball() {
    let ball = nurbs(
        primitive::sphere(OpKey(3), 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0,
    );
    let block = primitive::block(OpKey(7), 2.0, 3.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let octant = PI / 6.0;
    let place = Rigid::IDENTITY;
    near(
        volume(&run(BooleanOp::Intersect, &ball, &block, place)),
        octant,
    );
    near(
        volume(&run(BooleanOp::Subtract, &ball, &block, place)),
        4.0 / 3.0 * PI - octant,
    );
}
