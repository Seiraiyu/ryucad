//! Booleans of random placed bodies on every surface: primitives in NURBS
//! form, lofts, spline revolutions and extrusions, sweeps along splines,
//! against each other and against primitives. Every result passes the
//! checker and the self-intersection check and meshes without faults, and
//! the volumes keep the set identities. A pair the kernel refuses (an
//! intersection it can't certify, a non-manifold result) is skipped. A
//! third of the pairs are placed square to the axes on a coarse grid, so
//! flush faces and curves through the middles of the converted primitives
//! (their poles, seams and knot lines) come up. Release only (`cargo test
//! -p ryukei --release --test general_boolean_props`): minutes in a debug
//! build.

use core::f64::consts::{FRAC_PI_2, TAU};
use proptest::prelude::*;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{
    Axis, Circle2, Curve, Curve2, Frame, Frame2, KnotVector, Line2, NurbsCurve, NurbsCurve2,
    Point2, Point3, Rigid, Vector2, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{
    AlongOptions, LoftOptions, Loop, LoopKey, Path, PathSegment, Profile, Segment, SegmentKey,
    ValidProfile, along, extrude, loft, revolve,
};
use ryukei::topo::Body;
use ryukei::{BooleanError, Error};
use ryukei_testkit::mesh::volume_and_area;
use ryukei_testkit::nurbs::convert_body;
use std::sync::Arc;

/// The chord the volumes are measured at.
const CHORD: f64 = 1e-2;

/// How many kinds of body on spline and swept surfaces [`solid`] makes;
/// the kinds past them are primitives as they are.
const KINDS: usize = 10;

/// How many primitives follow them.
const PRIMITIVES: usize = 4;

/// A solid to build: a kind, two sizes, and a placement: a turn about an
/// axis then a move, or (`square`) a quarter turn about a coordinate axis
/// then a move on a grid of quarters.
#[derive(Clone, Copy, Debug)]
struct Spec {
    kind: usize,
    a: f64,
    b: f64,
    axis: (f64, f64, f64),
    angle: f64,
    at: (f64, f64, f64),
    square: Option<(u8, u8, (i8, i8, i8))>,
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        0usize..KINDS + PRIMITIVES,
        0.4f64..1.2,
        0.4f64..1.2,
        (-1.0f64..1.0, -1.0f64..1.0, 0.1f64..1.0),
        0.0f64..3.0,
        (-0.8f64..0.8, -0.8f64..0.8, -0.8f64..0.8),
        prop_oneof![
            2 => Just(None),
            1 => (0u8..4, 0u8..3, (-3i8..4, -3i8..4, -3i8..4)).prop_map(Some),
        ],
    )
        .prop_map(|(kind, a, b, axis, angle, at, square)| Spec {
            kind,
            a,
            b,
            axis,
            angle,
            at,
            square,
        })
}

fn seg(key: u128, curve: Curve2, t0: f64, t1: f64) -> Segment {
    Segment {
        key: SegmentKey(key),
        curve,
        t0,
        t1,
    }
}

fn square_loop(first: u128, s: f64) -> Loop {
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

fn circle_loop(key: u128, r: f64) -> Loop {
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

/// A profile in the plane `z = h` turned by `turn` about `z`.
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

fn nurbs(mut b: Body) -> Body {
    convert_body(&mut b).unwrap();
    b
}

/// A body of kind `s.kind`, centred near the origin, then placed.
fn solid(s: &Spec) -> Body {
    let (a, b, k) = (s.a, s.b, OpKey(1));
    let shift = |x: f64, y: f64, z: f64| Rigid::translation(Vector3::new(x, y, z));
    let body = match s.kind {
        0 => nurbs(
            primitive::block(k, a, b, 0.9, &shift(-a / 2.0, -b / 2.0, -0.45))
                .unwrap()
                .0,
        ),
        1 => nurbs(
            primitive::cylinder(k, a * 0.6, b * 1.5, &shift(0.0, 0.0, -b * 0.75))
                .unwrap()
                .0,
        ),
        2 => nurbs(
            primitive::frustum(k, a * 0.7, a * 0.3, b, &shift(0.0, 0.0, -b / 2.0))
                .unwrap()
                .0,
        ),
        3 => nurbs(primitive::sphere(k, a * 0.7, &Rigid::IDENTITY).unwrap().0),
        4 => nurbs(
            primitive::torus(k, a * 0.8 + 0.3, 0.25, &Rigid::IDENTITY)
                .unwrap()
                .0,
        ),
        5 => {
            let sections = [
                section(-b / 2.0, 0.0, circle_loop(1, a * 0.7)),
                section(b / 2.0, 0.0, circle_loop(2, a * 0.35)),
                section(b, 0.0, circle_loop(3, a * 0.5)),
            ];
            loft(OpKey(2), &sections, &LoftOptions::default())
                .unwrap()
                .0
        }
        6 => {
            let sections = [
                section(-b / 2.0, 0.0, square_loop(10, a)),
                section(b / 2.0, 0.6, square_loop(20, a)),
            ];
            loft(OpKey(3), &sections, &LoftOptions::default())
                .unwrap()
                .0
        }
        7 => {
            // A spline meridian from the axis round to the axis, turned
            // about z: a revolution with two poles.
            let kv = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
            let pts = vec![
                Point2::new(0.0, -b * 0.6),
                Point2::new(a * 0.9, -b * 0.6),
                Point2::new(a * 0.6, b * 0.6),
                Point2::new(0.0, b * 0.6),
            ];
            let m = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(kv, pts, None).unwrap()));
            let back = Curve2::Line2(
                Line2::through(Point2::new(0.0, b * 0.6), Point2::new(0.0, -b * 0.6)).unwrap(),
            );
            let profile = ValidProfile::new(Profile {
                frame: Frame::new(
                    Point3::ORIGIN,
                    Vector3::new(0.0, -1.0, 0.0),
                    Vector3::new(1.0, 0.0, 0.0),
                )
                .unwrap(),
                loops: vec![Loop {
                    key: LoopKey(1),
                    segments: vec![seg(1, m, 0.0, 1.0), seg(2, back, 0.0, b * 1.2)],
                }],
            })
            .unwrap();
            let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
            revolve(OpKey(6), &profile, axis, 0.0, TAU).unwrap().0
        }
        8 => {
            let blob = ryukei_testkit::nurbs::blob2(
                &mut ryukei_testkit::Lcg::new((a * 1e6) as u64),
                Point2::ORIGIN,
                a * 0.7,
            );
            let d = blob.domain();
            let profile = ValidProfile::new(Profile {
                frame: Frame::WORLD,
                loops: vec![Loop {
                    key: LoopKey(1),
                    segments: vec![seg(1, Curve2::Nurbs2(Arc::new(blob)), d[0], d[1])],
                }],
            })
            .unwrap();
            extrude(
                OpKey(5),
                &profile,
                Vector3::new(0.0, 0.0, 1.0),
                -b / 2.0,
                b / 2.0,
            )
            .unwrap()
            .0
        }
        10 => {
            primitive::block(k, a, b, 0.9, &shift(-a / 2.0, -b / 2.0, -0.45))
                .unwrap()
                .0
        }
        11 => {
            primitive::cylinder(k, a * 0.6, b * 1.5, &shift(0.0, 0.0, -b * 0.75))
                .unwrap()
                .0
        }
        12 => primitive::sphere(k, a * 0.7, &Rigid::IDENTITY).unwrap().0,
        13 => {
            primitive::torus(k, a * 0.8 + 0.3, 0.25, &Rigid::IDENTITY)
                .unwrap()
                .0
        }
        _ => {
            // A tube along a spline.
            let kv = KnotVector::clamped(3, &[0.0, 1.0, 2.0], &[4, 1, 4]).unwrap();
            let l = a + 0.6;
            let pts = vec![
                Point3::new(-l, 0.0, -0.2),
                Point3::new(-l / 2.0, 0.0, -0.2),
                Point3::new(0.0, b * 0.5, 0.0),
                Point3::new(l / 2.0, 0.2, 0.4),
                Point3::new(l, 0.0, 0.5),
            ];
            let n = NurbsCurve::new(kv, pts, None).unwrap();
            let path = Path {
                segments: vec![PathSegment {
                    key: SegmentKey(1),
                    curve: Curve::Nurbs(Arc::new(n)),
                    range: [0.0, 2.0],
                }],
            };
            let profile = ValidProfile::new(Profile {
                frame: Frame::new(
                    Point3::new(-l, 0.0, -0.2),
                    Vector3::new(1.0, 0.0, 0.0),
                    Vector3::new(0.0, 1.0, 0.0),
                )
                .unwrap(),
                loops: vec![circle_loop(1, 0.15)],
            })
            .unwrap();
            along(OpKey(4), &profile, &path, &AlongOptions::default())
                .unwrap()
                .0
        }
    };
    body.transformed(&place(s)).unwrap()
}

fn place(s: &Spec) -> Rigid {
    if let Some((quarter, axis, (x, y, z))) = s.square {
        let axis = [
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ][axis as usize];
        let step = |k: i8| 0.25 * f64::from(k);
        return Rigid::translation(Vector3::new(step(x), step(y), step(z))).then_after(
            &Rigid::rotation(axis.normalize().unwrap(), FRAC_PI_2 * f64::from(quarter)),
        );
    }
    let axis = Vector3::new(s.axis.0, s.axis.1, s.axis.2)
        .normalize()
        .unwrap();
    Rigid::translation(Vector3::new(s.at.0, s.at.1, s.at.2))
        .then_after(&Rigid::rotation(axis, s.angle))
}

/// The volume and area of a body's mesh, which must have no faults.
fn meshed(b: &Body) -> Result<(f64, f64), TestCaseError> {
    let m = tessellate(b, &MeshParams::new(CHORD, 0.3)).unwrap();
    prop_assert_eq!(&m.faults, &[]);
    prop_assert_eq!(check_mesh(b, &m), Ok(()));
    Ok(volume_and_area(&m))
}

/// The volume and area of a boolean's result, which must be sound.
fn measured(b: &Body) -> Result<(f64, f64), TestCaseError> {
    if b.counts().faces == 0 {
        return Ok((0.0, 0.0));
    }
    prop_assert_eq!(check(b), []);
    prop_assert_eq!(check_intersections(b), []);
    meshed(b)
}

fn run(op: BooleanOp, a: &Body, b: &Body) -> Result<Option<Body>, TestCaseError> {
    let tools = [Tool {
        key: ToolKey(1),
        body: b,
        place: Rigid::IDENTITY,
    }];
    match boolean(OpKey(50), op, a, &tools) {
        Ok((body, _)) => Ok(Some(body)),
        Err(Error::InvalidBoolean(
            BooleanError::Uncertain { .. } | BooleanError::NonManifold { .. },
        )) => Ok(None),
        Err(e) => Err(TestCaseError::fail(format!("{e:?}"))),
    }
}

/// Unite, intersect and subtract `sa` and `sb`: the results are sound and
/// their volumes keep the set identities.
fn identities(sa: Spec, sb: Spec) -> Result<(), TestCaseError> {
    let (a, b) = (solid(&sa), solid(&sb));
    let (Some(u), Some(i), Some(d)) = (
        run(BooleanOp::Unite, &a, &b)?,
        run(BooleanOp::Intersect, &a, &b)?,
        run(BooleanOp::Subtract, &a, &b)?,
    ) else {
        return Ok(());
    };
    let (va, sa) = meshed(&a)?;
    let (vb, sb) = meshed(&b)?;
    let (vu, su) = measured(&u)?;
    let (vi, si) = measured(&i)?;
    let (vd, sd) = measured(&d)?;
    // Each mesh is within the chord of its body: its volume within
    // area × chord.
    let slack = |s: f64| s * CHORD;
    prop_assert!(
        (vu + vi - va - vb).abs() <= slack(su + si + sa + sb),
        "{} + {} != {} + {}",
        vu,
        vi,
        va,
        vb
    );
    prop_assert!(
        (vd - (va - vi)).abs() <= slack(sd + sa + si),
        "{} != {} - {}",
        vd,
        va,
        vi
    );
    Ok(())
}

/// The seed the cases are drawn from. A release gate must not flip: a
/// random seed found a new case in the v0.7.0-pre.2 release run (issue
/// #180). New cases come from changing the seed on purpose, and from the
/// `boolean` fuzz target's general case.
const SEED: u64 = 0x7279_756b_6569_0001;

proptest! {
    #![proptest_config(ProptestConfig {
        rng_seed: proptest::test_runner::RngSeed::Fixed(SEED),
        ..ProptestConfig::with_cases(48)
    })]

    #[test]
    #[cfg_attr(debug_assertions, ignore = "minutes in a debug build: CI runs it in release")]
    fn set_identities_hold(sa in spec(), sb in spec()) {
        // A body on spline or swept surfaces, where it was made, against
        // any body placed.
        let sa = Spec { kind: sa.kind % KINDS, square: Some((0, 2, (0, 0, 0))), ..sa };
        identities(sa, sb)?;
    }
}

/// Issue #180: a spline torus against a spline blob extruded and
/// turned, which the v0.7.0-pre.2 release run found. Their union and
/// difference leave a torus face whose one loop spirals more than a turn
/// round the tube's long way: the mesher opened that way with its period
/// starting inside the loop, and the loop's edges, crossing it, never
/// fitted (`RefinementStalled`). Pinned by value, as a regression seed
/// means another case whenever the strategy changes.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_spline_torus_and_a_turned_blob_mesh() {
    let sa = Spec {
        kind: 4,
        a: 0.47198443097548365,
        b: 0.4,
        axis: (0.0, 0.0, 0.1),
        angle: 0.0,
        at: (0.0, 0.0, 0.0),
        square: Some((0, 2, (0, 0, 0))),
    };
    let sb = Spec {
        kind: 8,
        a: 1.0504938438984988,
        b: 0.9810909392426255,
        axis: (0.0, 0.48921730711540345, 0.7426524980579925),
        angle: 1.2922336779705372,
        at: (-0.18998684550387754, 0.0, 0.2646758891220622),
        square: None,
    };
    // All three build (a refusal would skip the meshing this pins).
    let (a, b) = (solid(&sa), solid(&sb));
    for op in [BooleanOp::Unite, BooleanOp::Intersect, BooleanOp::Subtract] {
        assert!(run(op, &a, &b).unwrap().is_some(), "{op:?}");
    }
    identities(sa, sb).unwrap();
}
