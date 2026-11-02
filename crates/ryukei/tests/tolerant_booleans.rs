//! Booleans on tolerant bodies (design 299, plan 14.A): a box and a
//! cylinder written as a file describes them, every edge moved off its
//! faces by 1e-6 to 3e-5 m and every vertex by half that, healed with
//! tightening off, then drilled, slotted, and united and intersected with
//! a flush block. Each must succeed as it does on the exact body, check
//! clean and mesh without faults.

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::tolerant_bytes::tolerant_with;

/// The side of the box, and the cylinder's height and diameter.
const S: f64 = 0.01;

/// The probe's directions, fixed: Tasks 4 to 9 were verified with them.
fn dir(k: usize) -> Vector3 {
    let v = [
        Vector3::new(1.0, 2.0, -1.0),
        Vector3::new(-2.0, 1.0, 1.0),
        Vector3::new(1.0, -1.0, 2.0),
        Vector3::new(2.0, 1.0, 1.0),
        Vector3::new(-1.0, -2.0, 1.0),
    ][k % 5];
    v * (1.0 / v.length())
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Box,
    Cylinder,
}

fn exact(kind: Kind) -> Body {
    match kind {
        Kind::Box => {
            primitive::block(OpKey(1), S, S, S, &Rigid::IDENTITY)
                .unwrap()
                .0
        }
        Kind::Cylinder => {
            let at = Rigid::translation(Vector3::new(S / 2.0, S / 2.0, 0.0));
            primitive::cylinder(OpKey(1), S / 2.0, S, &at).unwrap().0
        }
    }
}

/// The body with its edges moved by `by` (none for the exact one).
fn body(kind: Kind, by: Option<f64>) -> Body {
    let b = exact(kind);
    match by {
        None => b,
        Some(by) => tolerant_with(OpKey(9), &b, by, dir, |v| dir(v + 2)).unwrap(),
    }
}

#[derive(Clone, Copy, Debug)]
enum Op {
    DrillMiddle,
    DrillCorner,
    DrillSide,
    Slot,
    UniteFlush,
    IntersectFlush,
}

const OPS: [Op; 6] = [
    Op::DrillMiddle,
    Op::DrillCorner,
    Op::DrillSide,
    Op::Slot,
    Op::UniteFlush,
    Op::IntersectFlush,
];

fn tool(op: Op) -> (BooleanOp, Body) {
    let rod = |x: f64, y: f64| {
        let at = Rigid::translation(Vector3::new(x, y, -S));
        primitive::cylinder(OpKey(2), S / 5.0, 3.0 * S, &at)
            .unwrap()
            .0
    };
    match op {
        Op::DrillMiddle => (BooleanOp::Subtract, rod(S / 2.0, S / 2.0)),
        Op::DrillCorner => (BooleanOp::Subtract, rod(0.0, 0.0)),
        Op::DrillSide => (BooleanOp::Subtract, rod(S, S / 2.0)),
        Op::Slot => {
            let at = Rigid::translation(Vector3::new(0.4 * S, -S / 2.0, S / 2.0));
            let slot = primitive::block(OpKey(5), S / 5.0, 2.0 * S, S / 2.0, &at);
            (BooleanOp::Subtract, slot.unwrap().0)
        }
        Op::UniteFlush | Op::IntersectFlush => {
            let at = Rigid::translation(Vector3::new(S / 2.0, 0.0, 0.0));
            let flush = primitive::block(OpKey(6), S, S, S, &at).unwrap().0;
            let op = if matches!(op, Op::UniteFlush) {
                BooleanOp::Unite
            } else {
                BooleanOp::Intersect
            };
            (op, flush)
        }
    }
}

/// Runs `op` on the body and asserts the result is sound: it builds,
/// checks clean, crosses nothing and meshes without faults.
fn sound(kind: Kind, by: Option<f64>, op: Op) {
    let target = body(kind, by);
    let (bop, t) = tool(op);
    let tools = [Tool {
        key: ToolKey(1),
        body: &t,
        place: Rigid::IDENTITY,
    }];
    let what = format!("{op:?} on {kind:?} moved by {by:?}");
    let (b, _) =
        boolean(OpKey(41), bop, &target, &tools).unwrap_or_else(|e| panic!("{what}: {e:?}"));
    assert_eq!(ryukei::check::check(&b), [], "{what}");
    assert_eq!(ryukei::check::check_intersections(&b), [], "{what}");
    let m = tessellate(&b, &MeshParams::new(1e-5, 0.3)).unwrap();
    assert_eq!(m.faults, [], "{what}");
}

/// Runs `op` on the body and asserts it is refused as past the tolerance
/// cap: at a 3e-5 move the operands hold tolerances up to 9e-5, and the
/// result's clustered vertices and merged edges would pass 1e-4 (design
/// 299: no operation goes past the cap).
fn capped(kind: Kind, by: f64, op: Op) {
    use ryukei::{BooleanError, Error};
    let target = body(kind, Some(by));
    let (bop, t) = tool(op);
    let tools = [Tool {
        key: ToolKey(1),
        body: &t,
        place: Rigid::IDENTITY,
    }];
    match boolean(OpKey(41), bop, &target, &tools) {
        Err(Error::InvalidBoolean(BooleanError::ToleranceTooLarge { needed, .. })) => {
            assert!(needed > ryukei::tol::TOLERANCE_CAP, "{needed}");
        }
        other => panic!("{op:?} on {kind:?} moved by {by}: {:?}", other.map(|_| ())),
    }
}

#[test]
fn every_case_on_the_exact_bodies() {
    for kind in [Kind::Box, Kind::Cylinder] {
        for op in OPS {
            sound(kind, None, op);
        }
    }
}

#[test]
fn every_case_on_a_box_moved_by_the_resolution() {
    for op in OPS {
        sound(Kind::Box, Some(1e-8), op);
    }
}

#[test]
fn drills_through_the_middle_of_tolerant_bodies() {
    for by in [1e-6, 1e-5, 3e-5] {
        sound(Kind::Box, Some(by), Op::DrillMiddle);
    }
    for by in [1e-5, 3e-5] {
        sound(Kind::Cylinder, Some(by), Op::DrillMiddle);
        sound(Kind::Cylinder, Some(by), Op::DrillCorner);
    }
}

#[test]
fn drills_across_tolerant_edges() {
    sound(Kind::Box, Some(3e-5), Op::DrillCorner);
    sound(Kind::Box, Some(3e-5), Op::DrillSide);
    sound(Kind::Cylinder, Some(3e-5), Op::DrillSide);
    for by in [1e-6, 1e-5] {
        sound(Kind::Box, Some(by), Op::DrillCorner);
        sound(Kind::Box, Some(by), Op::DrillSide);
    }
    sound(Kind::Cylinder, Some(1e-5), Op::DrillSide);
}

#[test]
fn slots_across_tolerant_edges() {
    for by in [1e-6, 1e-5, 3e-5] {
        sound(Kind::Box, Some(by), Op::Slot);
    }
    sound(Kind::Cylinder, Some(1e-5), Op::Slot);
    capped(Kind::Cylinder, 3e-5, Op::Slot);
    sound(Kind::Box, Some(1e-5), Op::IntersectFlush);
}

#[test]
fn a_flush_intersect_at_a_micron() {
    sound(Kind::Box, Some(1e-6), Op::IntersectFlush);
}

#[test]
fn flush_unites_at_a_micron_and_ten() {
    sound(Kind::Box, Some(1e-6), Op::UniteFlush);
    sound(Kind::Box, Some(1e-5), Op::UniteFlush);
}

#[test]
fn flush_booleans_at_three_hundredths_of_a_millimetre() {
    sound(Kind::Box, Some(3e-5), Op::UniteFlush);
    capped(Kind::Box, 3e-5, Op::IntersectFlush);
}

#[test]
fn a_cylinder_flush_and_tangent_to_a_block() {
    sound(Kind::Cylinder, Some(1e-5), Op::UniteFlush);
    sound(Kind::Cylinder, Some(1e-5), Op::IntersectFlush);
    capped(Kind::Cylinder, 3e-5, Op::UniteFlush);
    capped(Kind::Cylinder, 3e-5, Op::IntersectFlush);
}

#[test]
fn a_result_past_the_cap_is_refused() {
    use ryukei::heal::{HealOptions, heal, seamed};
    use ryukei::{BooleanError, Error};
    // Edges 6e-5 off, healed with a cap of a millimetre: tolerances of
    // about 1.4e-4, past the kernel's 1e-4.
    let mut s = seamed(&exact(Kind::Box), 1e-7).unwrap();
    for e in 0..s.edges.len() {
        let c = s.curves[s.edges[e].curve].transformed(&Rigid::translation(dir(e) * 6e-5));
        s.curves.push(c);
        s.edges[e].curve = s.curves.len() - 1;
    }
    for v in 0..s.vertices.len() {
        s.vertices[v].point += dir(v + 2) * 3e-5;
    }
    let wide = HealOptions {
        tolerance_cap: 1e-3,
        tighten: false,
    };
    let (target, _, _) = heal(OpKey(9), &s, &wide).unwrap();
    let (op, t) = tool(Op::DrillMiddle);
    let tools = [Tool {
        key: ToolKey(1),
        body: &t,
        place: Rigid::IDENTITY,
    }];
    match boolean(OpKey(41), op, &target, &tools) {
        Err(Error::InvalidBoolean(BooleanError::ToleranceTooLarge { needed, .. })) => {
            assert!(needed > ryukei::tol::TOLERANCE_CAP, "{needed}");
        }
        other => panic!("{other:?}"),
    }
}

/// A tool's face a micron above the tolerant box's top, within its
/// edges' tolerances: the result may hold no face narrower than its own
/// edges' tolerances (design 299's slivers).
#[test]
fn a_micron_step_leaves_no_sliver() {
    use ryukei::measure::{Entity, Properties, properties};
    let target = body(Kind::Box, Some(1e-5));
    let at = Rigid::translation(Vector3::new(S / 2.0, 0.0, 1e-6));
    let block = primitive::block(OpKey(6), S, S, S, &at).unwrap().0;
    let tools = [Tool {
        key: ToolKey(1),
        body: &block,
        place: Rigid::IDENTITY,
    }];
    let (b, _) = boolean(OpKey(41), BooleanOp::Unite, &target, &tools).unwrap();
    assert_eq!(ryukei::check::check(&b), []);
    assert_eq!(
        tessellate(&b, &MeshParams::new(1e-5, 0.3)).unwrap().faults,
        []
    );
    for f in b.faces() {
        let Properties::Face { area, .. } = properties(&b, Entity::Face(f)).unwrap() else {
            unreachable!()
        };
        let mut perimeter = 0.0;
        let mut tol = 0.0f64;
        for l in b.face_loops(f) {
            for fin in b.loop_fins(l) {
                let e = b.fin(fin).unwrap().edge;
                let Properties::Edge { length, .. } = properties(&b, Entity::Edge(e)).unwrap()
                else {
                    unreachable!()
                };
                perimeter += length.value();
                tol = tol.max(b.edge(e).unwrap().tolerance.get());
            }
        }
        // A strip as wide as its tolerance has area about tolerance times
        // half its perimeter.
        assert!(area.value() > tol * perimeter, "a sliver face {f:?}");
    }
}

/// A drill crossing a tolerant edge at 5°: a branch reaching the edge at
/// a shallow angle (design 299's branch ends).
#[test]
fn a_shallow_drill_across_a_tolerant_edge() {
    let target = body(Kind::Box, Some(3e-5));
    let along = Rigid::rotation(
        Vector3::new(0.0, 1.0, 0.0).normalize().unwrap(),
        core::f64::consts::FRAC_PI_2,
    );
    let tilt = Rigid::rotation(
        Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(),
        5.0 * core::f64::consts::PI / 180.0,
    );
    let place =
        Rigid::translation(Vector3::new(-S, 0.1 * S, S)).then_after(&tilt.then_after(&along));
    let rod = primitive::cylinder(OpKey(2), S / 5.0, 3.0 * S, &place)
        .unwrap()
        .0;
    let tools = [Tool {
        key: ToolKey(1),
        body: &rod,
        place: Rigid::IDENTITY,
    }];
    let (b, _) = boolean(OpKey(41), BooleanOp::Subtract, &target, &tools).unwrap();
    assert_eq!(ryukei::check::check(&b), []);
    assert_eq!(
        tessellate(&b, &MeshParams::new(1e-5, 0.3)).unwrap().faults,
        []
    );
}

/// A block turned 30° whose corner sits on the tolerant box's corner
/// vertex: three of its edges leave that vertex (design 299's dart
/// order).
#[test]
fn a_fan_of_edges_at_a_tolerant_corner() {
    let target = body(Kind::Box, Some(3e-5));
    let turn = Rigid::rotation(
        Vector3::new(0.0, 0.0, 1.0).normalize().unwrap(),
        core::f64::consts::PI / 6.0,
    );
    let place = Rigid::translation(Vector3::new(0.0, 0.0, S - S / 2.0)).then_after(&turn);
    let block = primitive::block(OpKey(7), S / 2.0, S / 2.0, S / 2.0, &place)
        .unwrap()
        .0;
    let tools = [Tool {
        key: ToolKey(1),
        body: &block,
        place: Rigid::IDENTITY,
    }];
    let (b, _) = boolean(OpKey(41), BooleanOp::Subtract, &target, &tools).unwrap();
    assert_eq!(ryukei::check::check(&b), []);
    assert_eq!(
        tessellate(&b, &MeshParams::new(1e-5, 0.3)).unwrap().faults,
        []
    );
}

/// Seed `seed`'s case of `tolerant_boolean_props`, its bytes drawn as
/// that test draws them, its move `MOVES[pick]` where `pick` is given
/// (the first byte picks it).
fn seeded_by(seed: usize, pick: Option<u8>) -> ryukei_testkit::tolerant_bytes::TolerantCase {
    let mut g = ryukei_testkit::Lcg::new(0x746f_6c65_7261_6e74);
    let mut data: Vec<u8> = Vec::new();
    for _ in 0..=seed {
        data = (0..96).map(|_| (g.next_u64() >> 56) as u8).collect();
    }
    if let Some(p) = pick {
        data[0] = p;
    }
    ryukei_testkit::tolerant_bytes::tolerant_case(&data).expect("a case")
}

/// The seeded case's tolerant boolean builds, checks clean and meshes at
/// a millimetre (the bodies are about a metre across).
fn seeded_sound(seed: usize) {
    seeded_sound_by(seed, None);
}

fn seeded_sound_by(seed: usize, pick: Option<u8>) {
    let case = seeded_by(seed, pick);
    let c = &case.tolerant;
    let (b, _) = boolean(OpKey(9), c.op, &c.target, &c.tools())
        .unwrap_or_else(|e| panic!("seed {seed}: {e:?}"));
    assert_eq!(ryukei::check::check(&b), [], "seed {seed}");
    let m = tessellate(&b, &MeshParams::new(1e-3, 0.3)).unwrap();
    assert_eq!(m.faults, [], "seed {seed}");
}

/// A sphere minus a cylinder whose top rim, moved 1e-5 off, passes inside
/// its tube under the sphere's tangency with the cylinder: the face box
/// the tolerant rims give put the trace's cells just wrong.
#[test]
fn a_tangency_under_a_tolerant_rim() {
    seeded_sound(60);
}

/// A cylinder and a cone united with a block, edges a micron off: where a
/// tolerant edge meets an exact one, the pcurve's image at the joint
/// vertex lies farther from it than the edge's curve does.
#[test]
fn pcurves_reach_their_joint_vertices() {
    seeded_sound(70);
}

/// A torus united with two blocks standing on one plane, one turned:
/// where the turned block's tolerant top edge crosses the other's back
/// face, the edge's hit lies farther from the stretch below it than the
/// stretch's reach, though the stretch passes through the edge's tube,
/// and the stretches meeting there ended at two vertices. At the 3e-5
/// move the result would pass the tolerance cap.
#[test]
fn stretches_ending_in_a_tolerant_edges_tube() {
    for pick in 0..3 {
        seeded_sound_by(27, Some(pick));
    }
    let case = seeded_by(27, Some(3));
    let c = &case.tolerant;
    match boolean(OpKey(9), c.op, &c.target, &c.tools()) {
        Err(ryukei::Error::InvalidBoolean(ryukei::BooleanError::ToleranceTooLarge { .. })) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
}
