//! ryukei phase 21's twin of #652's untouched pairs (design 683, plan
//! 21.A): a block [`LENGTH`] × [`WIDTH`] × [`HEIGHT`] with its top edge
//! over +x rounded [`ROUND`], and a blind slot along x in its top,
//! [`SLOT_WIDTH`] wide and [`SLOT_DEPTH`] deep, from [`SLOT_START`] to
//! [`SHORT`] before the round. Deleting the slot's far end (healed by
//! growth) runs the slot out through the round and the +x side. Neither
//! shares a vertex with the end, so rungs 0 and 1 hold neither and the
//! walls reach their clipped sides; the ladder's rings bring in the
//! round, then the side, which touch no change and are tangent along
//! their shared edge. Pinned, they are cut by the walls and the floor and
//! never met with each other.

use core::f64::consts::FRAC_PI_4;
use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::geom::{Rigid, Surface, Vector3};
use ryukei::name::{FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::topo::{Body, EntityId, FaceId};
use ryukei::tweak::{DeleteSpec, Heal};

/// The block, its rounded edge, and the slot.
pub const LENGTH: f64 = 0.2;
pub const WIDTH: f64 = 0.1;
pub const HEIGHT: f64 = 0.05;
pub const ROUND: f64 = 0.01;
pub const SLOT_WIDTH: f64 = 0.02;
pub const SLOT_DEPTH: f64 = 0.02;
pub const SLOT_START: f64 = 0.02;
pub const SHORT: f64 = 0.02;

const BLOCK: OpKey = OpKey(0x21a0);

/// The block's face `role`.
fn face(b: &Body, role: FaceRole) -> FaceId {
    match b.by_name(Name::derive(BLOCK, Role::Face(role), &[])) {
        Some(EntityId::Face(f)) => f,
        _ => panic!("no face {role:?}"),
    }
}

/// The block with its top edge over +x rounded [`ROUND`].
pub fn rounded() -> Body {
    let (b, _) = primitive::block(BLOCK, LENGTH, WIDTH, HEIGHT, &Rigid::IDENTITY).unwrap();
    let (px, pz) = (face(&b, FaceRole::PlusX), face(&b, FaceRole::PlusZ));
    let edge = b
        .edges()
        .find(|&e| {
            let fs: Vec<FaceId> = b
                .edge_fins(e)
                .map(|fin| b.loop_(b.fin(fin).unwrap().loop_).unwrap().face)
                .collect();
            fs.contains(&px) && fs.contains(&pz)
        })
        .expect("the +x top edge");
    blend(
        OpKey(0x21a1),
        &b,
        &[EdgeBlend {
            edge: b.edge(edge).unwrap().name,
            kind: BlendKind::Fillet { radius: ROUND },
        }],
    )
    .unwrap()
    .0
}

/// Where the slot ends: [`SHORT`] before the round.
pub fn slot_end() -> f64 {
    LENGTH - ROUND - SHORT
}

/// The rounded block with the blind slot cut down from its top: x from
/// [`SLOT_START`] to [`slot_end`], y centred, z from `HEIGHT -
/// SLOT_DEPTH` up through the top.
pub fn slotted() -> Body {
    slotted_to(SLOT_START, slot_end(), [OpKey(0x21a2), OpKey(0x21a3)])
}

/// The rounded block with a slot like [`slotted`]'s from `start` to
/// `end`, the slot block and the cut keyed by `keys` (`near_room` shares
/// this).
pub fn slotted_to(start: f64, end: f64, keys: [OpKey; 2]) -> Body {
    let b = rounded();
    let (slot, _) = primitive::block(
        keys[0],
        end - start,
        SLOT_WIDTH,
        SLOT_DEPTH + 0.01,
        &Rigid::IDENTITY,
    )
    .unwrap();
    let place = Rigid::translation(Vector3::new(
        start,
        0.5 * (WIDTH - SLOT_WIDTH),
        HEIGHT - SLOT_DEPTH,
    ));
    boolean(
        keys[1],
        BooleanOp::Subtract,
        &b,
        &[Tool {
            key: ToolKey(1),
            body: &slot,
            place,
        }],
    )
    .unwrap()
    .0
}

/// The slot's far end: the plane face at x = [`slot_end`].
pub fn far_end(b: &Body) -> FaceId {
    far_end_at(b, slot_end())
}

/// The plane face at x = `x`: a slot's far end.
pub fn far_end_at(b: &Body, x: f64) -> FaceId {
    b.faces()
        .find(|&f| {
            let bx = ryukei::intersect::face_box(b, f);
            matches!(
                b.surface(b.face(f).unwrap().surface).unwrap(),
                Surface::Plane(_)
            ) && (bx.min().x - x).abs() < 1e-9
                && (bx.max().x - x).abs() < 1e-9
        })
        .expect("the slot's far end")
}

/// The delete of the slot's far end, healed by growth.
pub fn spec(b: &Body) -> DeleteSpec {
    DeleteSpec {
        faces: vec![b.face(far_end(b)).unwrap().name],
        heal: Heal::Grow,
    }
}

/// The volume once the slot runs out: the rounded block less the slot's
/// floor-to-top section over its run, the round's part included.
pub fn run_out_volume() -> f64 {
    let quarter = FRAC_PI_4 * ROUND * ROUND;
    let block = LENGTH * WIDTH * HEIGHT - (ROUND * ROUND - quarter) * WIDTH;
    // Under the flat top from SLOT_START to the round; under the round
    // (SLOT_DEPTH > ROUND, so the floor lies below it).
    let under = (LENGTH - ROUND - SLOT_START) * SLOT_DEPTH + ROUND * (SLOT_DEPTH - ROUND) + quarter;
    block - SLOT_WIDTH * under
}
