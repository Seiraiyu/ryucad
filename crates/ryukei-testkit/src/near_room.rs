//! ryukei phase 22's twin of #658's clipped faces (design 724, plan
//! 22.A): `pinned_faces`' rounded block with its slot open through the -x
//! side (from [`START`]) and ending [`SHORT`] before the round, farther
//! than the deleted end's reach (its diagonal, about 0.028). Deleting the
//! end (healed by growth) runs the slot out through the round and the +x
//! side. In rung 3's narrowed build the near boxes of the faces round the
//! end stop short of the round, and the walk is clipped at the top, which
//! touches the change: CTC 02's shape. Given room, the top and the walls
//! run on, but the floor, given none, would be dropped (plan 22.A's
//! guard); given room too, the slot runs out.
//!
//! Plan 22.A's own shape, the slot starting inside the block as in
//! `pinned_faces`, never clips: the walls run past its pinned start plane
//! and the walk is ambiguous there first (census VII, #658).

use crate::pinned_faces::{
    HEIGHT, LENGTH, ROUND, SLOT_DEPTH, SLOT_WIDTH, WIDTH, far_end_at, slotted_to,
};
use core::f64::consts::FRAC_PI_4;
use ryukei::name::OpKey;
use ryukei::topo::{Body, FaceId};
use ryukei::tweak::{DeleteSpec, Heal};

/// Where the slot starts: past the block's -x side, so it is open there.
pub const START: f64 = -0.01;

/// How far before the round the slot ends.
pub const SHORT: f64 = 0.05;

/// Where the slot ends: [`SHORT`] before the round.
pub fn slot_end() -> f64 {
    LENGTH - ROUND - SHORT
}

/// The rounded block with the slot from [`START`] to [`slot_end`].
pub fn slotted() -> Body {
    slotted_to(START, slot_end(), [OpKey(0x22a2), OpKey(0x22a3)])
}

/// The slot's far end.
pub fn far_end(b: &Body) -> FaceId {
    far_end_at(b, slot_end())
}

/// The delete of the slot's far end, healed by growth.
pub fn spec(b: &Body) -> DeleteSpec {
    DeleteSpec {
        faces: vec![b.face(far_end(b)).unwrap().name],
        heal: Heal::Grow,
    }
}

/// The volume once the slot runs out: the rounded block less the slot's
/// floor-to-top section along the whole block, the round's part included.
pub fn run_out_volume() -> f64 {
    let quarter = FRAC_PI_4 * ROUND * ROUND;
    let block = LENGTH * WIDTH * HEIGHT - (ROUND * ROUND - quarter) * WIDTH;
    let under = (LENGTH - ROUND) * SLOT_DEPTH + ROUND * (SLOT_DEPTH - ROUND) + quarter;
    block - SLOT_WIDTH * under
}
