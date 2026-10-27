//! Booleans against exact volumes: every result passes the checker and
//! the self-intersection check, meshes without faults, and encloses the
//! volume it should. The cases at other scales run in release only
//! (`cargo test -p ryukei --release --test booleans`).

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Frame, Point3, Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use std::f64::consts::PI;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// The volume a sound body's mesh encloses.
fn volume(b: &Body) -> f64 {
    volume_at(b, 1.0)
}

/// [`volume`] of a body `scale` times the size, meshed as finely for
/// its size.
fn volume_at(b: &Body, scale: f64) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-3 * scale, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn run(op: BooleanOp, target: &Body, tools: &[(&Body, Rigid)]) -> Body {
    let tools: Vec<Tool> = tools
        .iter()
        .enumerate()
        .map(|(i, (b, place))| Tool {
            key: ToolKey(i as u128 + 1),
            body: b,
            place: *place,
        })
        .collect();
    boolean(OpKey(100), op, target, &tools).unwrap().0
}

fn near(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 2e-3 * want.abs().max(1e-3),
        "{got} against {want}"
    );
}

#[test]
fn a_hole_through_a_plate() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let r = run(BooleanOp::Subtract, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    near(volume(&r), 16.0 - PI * 0.25);
    let c = r.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (7, 14, 10));
    // The rod kept to the plate's thickness, and the two together.
    let i = run(BooleanOp::Intersect, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    near(volume(&i), PI * 0.25);
    let u = run(BooleanOp::Unite, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    near(volume(&u), 16.0 + PI * 0.25 * 2.0);
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn results_dont_depend_on_scale() {
    // A millimetre plate drilled once failed: a point found to classify
    // the hole's wall stepped off it by a fraction of its length round,
    // in radians, which carried it out of a wall a few millimetres high.
    for s in [1e-3, 1e-2, 10.0] {
        let at_s = |x: f64, y: f64, z: f64| at(x * s, y * s, z * s);
        let vol = |b: &Body| volume_at(b, s) / (s * s * s);
        let plate = primitive::block(OpKey(1), 4.0 * s, 4.0 * s, 1.0 * s, &Rigid::IDENTITY)
            .unwrap()
            .0;
        for (z, h) in [(-1.0, 3.0), (0.0, 1.0), (0.0, 3.0), (0.5, 3.0)] {
            let rod = primitive::cylinder(OpKey(2), 0.5 * s, h * s, &Rigid::IDENTITY)
                .unwrap()
                .0;
            let place = at_s(2.0, 2.0, z);
            // How far the rod runs inside the plate's thickness.
            let depth = (z + h).min(1.0) - z.max(0.0);
            let r = run(BooleanOp::Subtract, &plate, &[(&rod, place)]);
            near(vol(&r), 16.0 - PI * 0.25 * depth);
            let i = run(BooleanOp::Intersect, &plate, &[(&rod, place)]);
            near(vol(&i), PI * 0.25 * depth);
        }
        let c = primitive::cylinder(OpKey(1), s, 4.0 * s, &at_s(0.0, 0.0, -2.0))
            .unwrap()
            .0;
        let across = Rigid::rotation(Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(), PI / 2.0);
        near(
            vol(&run(BooleanOp::Intersect, &c, &[(&c, across)])),
            16.0 / 3.0,
        );
        let t = primitive::torus(OpKey(1), 3.0 * s, s, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let below = primitive::block(OpKey(2), 10.0 * s, 10.0 * s, 5.0 * s, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let half = run(BooleanOp::Subtract, &t, &[(&below, at_s(-5.0, -5.0, -5.0))]);
        near(vol(&half), PI * PI * 3.0);
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn cross_bores_at_any_scale() {
    // A 20-unit cube bored through along z (radius 3), then across along
    // x: smaller through the middle, equal, and off the middle. At a
    // millimetre the walls between bores were classified from points
    // stepped off their arcs in the parameter plane, where a radian round
    // a 3 mm bore is 3 mm and a metre along it a metre: they landed off
    // the wall, and the result was refused as non-manifold.
    let along_x = |s: f64, y: f64| {
        Rigid::from_frame(
            &Frame::new(
                Point3::new(-10.0 * s, y * s, 10.0 * s),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            )
            .unwrap(),
        )
    };
    for s in [1e-3, 1e-2, 1.0] {
        let cube = primitive::block(OpKey(1), 20.0 * s, 20.0 * s, 20.0 * s, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let bore = |r: f64| {
            primitive::cylinder(OpKey(2), r * s, 40.0 * s, &Rigid::IDENTITY)
                .unwrap()
                .0
        };
        let down = at(10.0 * s, 10.0 * s, -10.0 * s);
        let one = run(BooleanOp::Subtract, &cube, &[(&bore(3.0), down)]);
        let vol = |b: &Body| volume_at(b, s) / (s * s * s);
        near(vol(&one), 8000.0 - PI * 9.0 * 20.0);
        for (r, y) in [(2.0, 10.0), (3.0, 10.0), (2.0, 12.0)] {
            let two = boolean(
                OpKey(101),
                BooleanOp::Subtract,
                &one,
                &[Tool {
                    key: ToolKey(9),
                    body: &bore(r),
                    place: along_x(s, y),
                }],
            )
            .unwrap_or_else(|e| panic!("scale {s}, radius {r} at {y}: {e:?}"))
            .0;
            // Less than both bores apart, more than the larger alone.
            let v = vol(&two);
            assert!(
                v < vol(&one) - 1.0 && v > vol(&one) - PI * r * r * 20.0,
                "{s} {r} {y}: {v}"
            );
        }
    }
}

#[test]
fn crossing_blocks() {
    let a = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let b = primitive::block(OpKey(2), 2.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let place = at(1.0, 0.5, 0.5);
    near(volume(&run(BooleanOp::Unite, &a, &[(&b, place)])), 9.0);
    near(volume(&run(BooleanOp::Subtract, &a, &[(&b, place)])), 7.0);
    near(volume(&run(BooleanOp::Intersect, &a, &[(&b, place)])), 1.0);
}

#[test]
fn a_lens_and_a_steinmetz_solid() {
    let s = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let lens = run(BooleanOp::Intersect, &s, &[(&s, at(1.0, 0.0, 0.0))]);
    near(volume(&lens), 5.0 * PI / 12.0);
    // Two cylinders of radius 1 crossing at right angles.
    let c = primitive::cylinder(OpKey(1), 1.0, 4.0, &at(0.0, 0.0, -2.0))
        .unwrap()
        .0;
    let across = Rigid::rotation(Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(), PI / 2.0);
    let steinmetz = run(BooleanOp::Intersect, &c, &[(&c, across)]);
    near(volume(&steinmetz), 16.0 / 3.0);
}

#[test]
fn torus_and_cone_cuts() {
    let t = primitive::torus(OpKey(1), 3.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let below = primitive::block(OpKey(2), 10.0, 10.0, 5.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let half = run(BooleanOp::Subtract, &t, &[(&below, at(-5.0, -5.0, -5.0))]);
    near(volume(&half), PI * PI * 3.0);
    // A cone of radius 1 and height 2, its top half cut away: a frustum.
    let cone = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let top = primitive::block(OpKey(2), 4.0, 4.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let frustum = run(BooleanOp::Subtract, &cone, &[(&top, at(-2.0, -2.0, 1.0))]);
    near(volume(&frustum), PI * 2.0 / 3.0 - PI * 0.25 / 3.0);
}

#[test]
fn three_holes_at_once() {
    let plate = primitive::block(OpKey(1), 6.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.4, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools: Vec<(&Body, Rigid)> = (0..3)
        .map(|i| (&rod, at(1.0 + 2.0 * i as f64, 1.0, -1.0)))
        .collect();
    let r = run(BooleanOp::Subtract, &plate, &tools);
    near(volume(&r), 12.0 - 3.0 * PI * 0.16);
}

#[test]
fn faces_on_one_surface_merge() {
    // Two blocks overlapping side by side, flush top, bottom, front and
    // back: one box.
    let a = primitive::block(OpKey(1), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let u = run(BooleanOp::Unite, &a, &[(&a, at(1.0, 0.0, 0.0))]);
    near(volume(&u), 6.0);
    let c = u.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (6, 12, 8));
    // A pocket flush with the top.
    let block = primitive::block(OpKey(1), 4.0, 4.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let cutter = primitive::block(OpKey(2), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let pocket = run(BooleanOp::Subtract, &block, &[(&cutter, at(1.0, 1.0, 1.0))]);
    near(volume(&pocket), 28.0);
    let c = pocket.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (11, 24, 16));
    // Two cylinders end to end on one axis: one cylinder.
    let cyl = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let long = run(BooleanOp::Unite, &cyl, &[(&cyl, at(0.0, 0.0, 2.0))]);
    near(volume(&long), PI * 4.0);
    let c = long.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (3, 2, 2));
}

#[test]
fn a_box_cornered_on_a_sphere_s_centre() {
    // Issue #106: a box whose corner sits on a sphere's centre, one edge
    // along the sphere's polar axis, so both poles lie on its boundary.
    // The cut came back as the target unchanged.
    let cube = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = primitive::sphere(OpKey(2), 1.2, &at(1.0, 1.0, 1.0))
        .unwrap()
        .0;
    let corner = primitive::block(OpKey(3), 2.0, 2.0, 2.0, &at(1.0, 1.0, 1.0))
        .unwrap()
        .0;
    let lens = run(BooleanOp::Intersect, &cube, &[(&ball, Rigid::IDENTITY)]);
    let v = volume(&lens);
    let r = run(BooleanOp::Subtract, &lens, &[(&corner, Rigid::IDENTITY)]);
    near(volume(&r), v * 7.0 / 8.0);
    let r = run(BooleanOp::Subtract, &ball, &[(&corner, Rigid::IDENTITY)]);
    near(volume(&r), 4.0 / 3.0 * PI * (1.2 * 1.2 * 1.2) * 7.0 / 8.0);
}

#[test]
fn an_octant_off_a_ball_whatever_its_seam() {
    // Issue #106 again: the box's faces through the ball's poles meet it
    // in half circles cut at the poles, whose parameters run past the end
    // of the period wherever the seam lies between them. Turning the ball
    // about its axis, or laying the axis along another of the box's
    // edges, moves the seam round the cuts.
    let corner = primitive::block(OpKey(3), 2.0, 2.0, 2.0, &at(1.0, 1.0, 1.0))
        .unwrap()
        .0;
    let want = 4.0 / 3.0 * PI * (1.2 * 1.2 * 1.2) * 7.0 / 8.0;
    let c = Point3::new(1.0, 1.0, 1.0);
    let unit = |x: f64, y: f64, z: f64| Vector3::new(x, y, z);
    let mut frames = Vec::new();
    for deg in [30.0, 90.0, 135.0, 200.0, 300.0] {
        let a = deg * PI / 180.0;
        let x = unit(ryukei::math::cos(a), ryukei::math::sin(a), 0.0);
        frames.push(Frame::new(c, unit(0.0, 0.0, 1.0), x).unwrap());
    }
    frames.push(Frame::new(c, unit(1.0, 0.0, 0.0), unit(0.0, 1.0, 0.0)).unwrap());
    frames.push(Frame::new(c, unit(0.0, -1.0, 0.0), unit(0.0, 0.0, -1.0)).unwrap());
    for f in frames {
        let ball = primitive::sphere(OpKey(2), 1.2, &Rigid::from_frame(&f))
            .unwrap()
            .0;
        let r = run(BooleanOp::Subtract, &ball, &[(&corner, Rigid::IDENTITY)]);
        near(volume(&r), want);
        let i = run(BooleanOp::Intersect, &ball, &[(&corner, Rigid::IDENTITY)]);
        near(volume(&i), want / 7.0);
    }
}

#[test]
fn a_rim_touching_a_bore_at_a_point_is_refused() {
    // Issue #88: a block bored along x (radius 3), then cut by a pin
    // (radius 1) standing on the bore's middle plane, its bottom rim
    // touching the bore at one point. The voids would meet at that point
    // alone: the cut must be refused, as when the pin's wall touches.
    let block = primitive::block(OpKey(1), 20.0, 30.0, 10.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let along_x = Rigid::from_frame(
        &Frame::new(
            Point3::new(-10.0, 15.0, 4.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap(),
    );
    let rod = primitive::cylinder(OpKey(2), 3.0, 40.0, &along_x)
        .unwrap()
        .0;
    let bored = run(BooleanOp::Subtract, &block, &[(&rod, Rigid::IDENTITY)]);
    for (h, z) in [(6.0, 4.0), (8.0, 2.0)] {
        let pin = primitive::cylinder(OpKey(3), 1.0, h, &at(10.0, 19.0, z))
            .unwrap()
            .0;
        let tools = [Tool {
            key: ToolKey(1),
            body: &pin,
            place: Rigid::IDENTITY,
        }];
        match boolean(OpKey(100), BooleanOp::Subtract, &bored, &tools) {
            Err(ryukei::Error::InvalidBoolean(_)) => {}
            Err(e) => panic!("pin at {z}: {e:?}"),
            Ok((r, _)) => panic!("pin at {z}: built, crossing {:?}", check_intersections(&r)),
        }
    }
}

#[test]
fn a_ball_trimmed_through_its_poles_meshes() {
    // Issue #78, found by ryucad's feature fuzz target in millimetres: a
    // box cut by a ball whose pole lies on one of its faces, either the
    // face through the ball's centre and along its axis (a loop through
    // both poles) or the top touching the ball at its pole. Joining the
    // edges either side of a pole ran a pcurve past it, and the ball's
    // face was left out of the mesh as BadDomain.
    for s in [1.0, 1e-3] {
        let block = primitive::block(OpKey(1), 20.0 * s, 10.0 * s, 5.0 * s, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let tilted = Rigid::from_frame(
            &Frame::new(
                Point3::new(5.0 * s, 0.0, 2.0 * s),
                Vector3::new(5.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        );
        let cube = s * s * s;
        let ball = primitive::sphere(OpKey(2), 4.0 * s, &tilted).unwrap().0;
        let r = run(BooleanOp::Intersect, &block, &[(&ball, Rigid::IDENTITY)]);
        near(volume_at(&r, s) / cube, 205.0 * PI / 6.0);
        let ball = primitive::sphere(OpKey(2), 4.0 * s, &at(9.0 * s, 5.0 * s, s))
            .unwrap()
            .0;
        let r = run(BooleanOp::Intersect, &block, &[(&ball, Rigid::IDENTITY)]);
        near(volume_at(&r, s) / cube, 175.0 * PI / 3.0);
    }
}

/// A refusal names the same faces however the work is scheduled: the
/// boolean's parallel steps keep the first failure in their own order, not
/// whichever thread fails first. The `feature` fuzz target's interference
/// found it: a 20 × 10 × 5 mm block with a 2 mm pocket in its top edge,
/// against itself placed as a revolute mate at 12° leaves it, is `Uncertain`
/// at more than one pair of faces.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "minutes in a debug build: CI runs it in release"
)]
fn refusals_name_the_same_faces_on_any_number_of_threads() {
    use ryukei::geom::UnitVector3;
    let mm = 1e-3;
    let block = primitive::block(OpKey(1), 20.0 * mm, 10.0 * mm, 5.0 * mm, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let pocket = primitive::block(
        OpKey(2),
        2.0 * mm,
        2.0 * mm,
        2.0 * mm,
        &at(9.0 * mm, 4.0 * mm, 4.0 * mm),
    )
    .unwrap()
    .0;
    let b = run(BooleanOp::Subtract, &block, &[(&pocket, Rigid::IDENTITY)]);
    let turn = |deg: f64| Rigid::rotation(UnitVector3::Z, deg * PI / 180.0);
    let shift = |x: f64| at(x, 0.0, 0.0);
    let turned = shift(5.0 * mm).then_after(&turn(12.0)).inverse();
    let refusal = |threads: usize| {
        let tools = [Tool {
            key: ToolKey(7),
            body: &b,
            place: turned,
        }];
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| boolean(OpKey(8), BooleanOp::Intersect, &b, &tools))
            .map(|_| ())
            .expect_err("refused")
    };
    let first = refusal(1);
    assert!(
        matches!(
            first,
            ryukei::Error::InvalidBoolean(ryukei::BooleanError::Uncertain { .. })
        ),
        "{first:?}"
    );
    for threads in [1, 8, 1, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8] {
        assert_eq!(refusal(threads), first, "on {threads} threads");
    }
}

/// A face on a swept surface takes part in a boolean like any other: a
/// block through a spline blob's side cuts it, and what the cut takes and
/// what it leaves make up the blob. A block inside it, which only the
/// planar caps meet, cuts its exact volume out.
#[test]
fn a_swept_face_takes_part() {
    use ryukei::geom::{Curve2, KnotVector, NurbsCurve2, Point2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    use std::sync::Arc;
    // A blob: a periodic cubic round the origin, extruded 1 up.
    let n: u32 = 8;
    let pts: Vec<Point2> = (0..n)
        .map(|i| {
            let (s, c) = ryukei::math::sin_cos(2.0 * PI * f64::from(i) / f64::from(n));
            let k = [1.0, 0.8, 1.1, 0.9][i as usize % 4];
            Point2::new(k * c, k * s)
        })
        .collect();
    let knots: Vec<f64> = (0..=n).map(f64::from).collect();
    let kv = KnotVector::periodic(3, &knots, &vec![1; n as usize + 1]).unwrap();
    let blob = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(kv, pts, None).unwrap()));
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![Segment {
                key: SegmentKey(1),
                curve: blob,
                t0: 0.0,
                t1: f64::from(n),
            }],
        }],
    })
    .unwrap();
    let (body, _) = extrude(OpKey(1), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0).unwrap();
    let (through, _) = primitive::block(OpKey(2), 1.0, 1.0, 0.5, &at(0.5, -0.5, 0.25)).unwrap();
    let cut = run(BooleanOp::Subtract, &body, &[(&through, Rigid::IDENTITY)]);
    let kept = run(BooleanOp::Intersect, &body, &[(&through, Rigid::IDENTITY)]);
    near(volume(&cut) + volume(&kept), volume(&body));
    let (inside, _) = primitive::block(OpKey(4), 0.6, 0.6, 2.0, &at(-0.3, -0.3, -0.5)).unwrap();
    let r = run(BooleanOp::Subtract, &body, &[(&inside, Rigid::IDENTITY)]);
    assert!((volume(&body) - volume(&r) - 0.36).abs() <= 1e-3);
}
