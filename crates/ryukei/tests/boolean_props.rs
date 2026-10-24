//! Booleans of random placed primitives and sweeps: every result passes
//! the checker and the self-intersection check and meshes without
//! faults, the volumes obey the set identities, and the result is the
//! same bits on one thread as on many. A pair the kernel refuses (an
//! intersection it can't certify, a non-manifold result) is skipped.
//! The property runs in release only (`cargo test -p ryukei --release
//! --test boolean_props`): checking every result for self-intersections
//! takes minutes in a debug build.

use proptest::prelude::*;
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Rigid, Vector2, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
use ryukei::topo::Body;
use ryukei::{BooleanError, Error};
use ryukei_testkit::mesh::volume_and_area;

/// The chord the volumes are measured at.
const CHORD: f64 = 1e-2;

/// A solid to build: a kind, two sizes, and a placement (a turn about an
/// axis, then a move).
#[derive(Clone, Copy, Debug)]
struct Spec {
    kind: usize,
    a: f64,
    b: f64,
    axis: (f64, f64, f64),
    angle: f64,
    at: (f64, f64, f64),
}

fn spec() -> impl Strategy<Value = Spec> {
    (
        0usize..7,
        0.4f64..1.2,
        0.4f64..1.2,
        (-1.0f64..1.0, -1.0f64..1.0, 0.1f64..1.0),
        0.0f64..3.0,
        (-0.8f64..0.8, -0.8f64..0.8, -0.8f64..0.8),
    )
        .prop_map(|(kind, a, b, axis, angle, at)| Spec {
            kind,
            a,
            b,
            axis,
            angle,
            at,
        })
}

/// A plate with a round hole, extruded: planes and cylinders from a
/// sweep.
fn holed_plate(w: f64, r: f64) -> Body {
    let c = [
        Point2::new(-w, -w),
        Point2::new(w, -w),
        Point2::new(w, w),
        Point2::new(-w, w),
    ];
    let outer: Vec<Segment> = (0..4)
        .map(|i| Segment {
            key: SegmentKey(i as u128 + 1),
            curve: Curve2::Line2(Line2::through(c[i], c[(i + 1) % 4]).unwrap()),
            t0: 0.0,
            t1: 2.0 * w,
        })
        .collect();
    let hole = Segment {
        key: SegmentKey(9),
        curve: Curve2::Circle2(
            Circle2::new(
                Frame2::new(Point2::ORIGIN, Vector2::new(1.0, 0.0), true).unwrap(),
                r,
            )
            .unwrap(),
        ),
        t0: 0.0,
        t1: core::f64::consts::TAU,
    };
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![
            Loop {
                key: LoopKey(1),
                segments: outer,
            },
            Loop {
                key: LoopKey(2),
                segments: vec![hole],
            },
        ],
    })
    .unwrap();
    let up = Vector3::new(0.0, 0.0, 1.0);
    extrude(OpKey(7), &profile, up, -0.4, 0.4).unwrap().0
}

/// A primitive or a sweep, centred near the origin, then placed.
fn solid(s: &Spec) -> Body {
    let axis = Vector3::new(s.axis.0, s.axis.1, s.axis.2)
        .normalize()
        .unwrap();
    let place = Rigid::translation(Vector3::new(s.at.0, s.at.1, s.at.2))
        .then_after(&Rigid::rotation(axis, s.angle));
    let (a, b, k) = (s.a, s.b, OpKey(1));
    let centred = |h: f64| place.then_after(&Rigid::translation(Vector3::new(0.0, 0.0, -h / 2.0)));
    let corner = place.then_after(&Rigid::translation(Vector3::new(-a / 2.0, -b / 2.0, -0.45)));
    match s.kind {
        0 => primitive::block(k, a, b, 0.9, &corner).unwrap().0,
        1 => {
            primitive::cylinder(k, a * 0.6, b * 1.5, &centred(b * 1.5))
                .unwrap()
                .0
        }
        2 => {
            primitive::cone(k, a * 0.7, b * 1.5, &centred(b * 1.5))
                .unwrap()
                .0
        }
        3 => {
            primitive::frustum(k, a * 0.7, a * 0.3, b, &centred(b))
                .unwrap()
                .0
        }
        4 => primitive::sphere(k, a * 0.7, &place).unwrap().0,
        5 => primitive::torus(k, a * 0.8 + 0.3, 0.25, &place).unwrap().0,
        _ => holed_plate(a * 0.8, a * 0.3).transformed(&place).unwrap(),
    }
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    #[cfg_attr(debug_assertions, ignore = "minutes in a debug build: CI runs it in release")]
    fn set_identities_hold(sa in spec(), sb in spec()) {
        let (a, b) = (solid(&sa), solid(&sb));
        let (Some(u), Some(i), Some(d)) = (
            run(BooleanOp::Unite, &a, &b)?,
            run(BooleanOp::Intersect, &a, &b)?,
            run(BooleanOp::Subtract, &a, &b)?,
        ) else {
            return Ok(());
        };
        // The operands' own soundness is the primitives' and sweeps' tests'.
        let (va, sa) = meshed(&a)?;
        let (vb, sb) = meshed(&b)?;
        let (vu, su) = measured(&u)?;
        let (vi, si) = if i.counts().faces == 0 { (0.0, 0.0) } else { measured(&i)? };
        let (vd, sd) = if d.counts().faces == 0 { (0.0, 0.0) } else { measured(&d)? };
        // Each mesh is within the chord of its body: its volume within
        // area × chord.
        let slack = |s: f64| s * CHORD;
        prop_assert!(
            (vu + vi - va - vb).abs() <= slack(su + si + sa + sb),
            "{} + {} != {} + {}", vu, vi, va, vb
        );
        prop_assert!(
            (vd - (va - vi)).abs() <= slack(sd + sa + si),
            "{} != {} - {}", vd, va, vi
        );
    }
}

#[test]
fn the_same_on_one_thread_and_many() {
    let plate = primitive::block(OpKey(1), 6.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.4, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = primitive::sphere(OpKey(3), 0.6, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools: Vec<Tool> = (0..3)
        .map(|i| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: Rigid::translation(Vector3::new(1.0 + 2.0 * i as f64, 1.0, -1.0)),
        })
        .chain([Tool {
            key: ToolKey(9),
            body: &ball,
            place: Rigid::translation(Vector3::new(3.0, 1.0, 1.0)),
        }])
        .collect();
    let on = |n: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap();
        pool.install(|| boolean(OpKey(5), BooleanOp::Subtract, &plate, &tools).unwrap())
    };
    let (one, j1) = on(1);
    let (many, j8) = on(8);
    assert_eq!(one.dump(), many.dump());
    assert_eq!(j1, j8);
}

#[test]
fn refusals_name_their_cause() {
    let a = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let twice = [
        Tool {
            key: ToolKey(1),
            body: &a,
            place: Rigid::IDENTITY,
        },
        Tool {
            key: ToolKey(1),
            body: &a,
            place: Rigid::translation(Vector3::new(0.5, 0.0, 0.0)),
        },
    ];
    assert_eq!(
        boolean(OpKey(1), BooleanOp::Unite, &a, &twice).err(),
        Some(Error::InvalidBoolean(BooleanError::DuplicateToolKey {
            key: ToolKey(1)
        }))
    );
    let far = [Tool {
        key: ToolKey(4),
        body: &a,
        place: Rigid::translation(Vector3::new(499.5, 0.0, 0.0)),
    }];
    assert_eq!(
        boolean(OpKey(1), BooleanOp::Unite, &a, &far).err(),
        Some(Error::InvalidBoolean(BooleanError::OutsideSizeBox {
            tool: ToolKey(4)
        }))
    );
}

/// A torus and a cone the property drew (work item #639): each boolean
/// of the pair is sound and meshes without faults.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_torus_and_a_cone_mesh() {
    let sa = Spec {
        kind: 5,
        a: 1.091689180436402,
        b: 0.4,
        axis: (0.7934173986103373, 0.5887877433409116, 0.4233599872174678),
        angle: 0.6831438557681597,
        at: (0.09653434897503403, 0.3764130794660872, 0.5995984573443072),
    };
    let sb = Spec {
        kind: 2,
        a: 0.5646845820134708,
        b: 0.5119787626174719,
        axis: (0.8970463648484467, 0.5667061376105614, 0.6042353694013554),
        angle: 0.6331565792233591,
        at: (
            0.037940514823881964,
            -0.24715793815791573,
            -0.2895137011468087,
        ),
    };
    let (a, b) = (solid(&sa), solid(&sb));
    for op in [BooleanOp::Unite, BooleanOp::Intersect, BooleanOp::Subtract] {
        let r = run(op, &a, &b).unwrap().unwrap();
        if r.counts().faces > 0 {
            measured(&r).unwrap_or_else(|e| panic!("{op:?}: {e}"));
        }
    }
}
