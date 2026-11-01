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
    for by in [1e-5, 3e-5] {
        sound(Kind::Cylinder, Some(by), Op::Slot);
    }
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
    sound(Kind::Box, Some(3e-5), Op::IntersectFlush);
}

#[test]
fn a_cylinder_flush_and_tangent_to_a_block() {
    for by in [1e-5, 3e-5] {
        sound(Kind::Cylinder, Some(by), Op::UniteFlush);
        sound(Kind::Cylinder, Some(by), Op::IntersectFlush);
    }
}
