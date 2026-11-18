//! ryukei phase 23's twin of CTC 02 with PMI #295's wrong pin (design
//! 919, plan 23.A): `pinned_faces`' rounded block with its blind slot
//! from `SLOT_START` ending [`SHORT`] before the round, farther than the
//! deleted end's reach (plan 22.A's own shape for `near_room`). Deleting
//! the end (healed by growth) runs the slot out through the round and the
//! +x side. In rung 3's narrowed build the walk fails `Ambiguous` at
//! the slot's start plane, which touches no change and is pinned, and
//! main refuses `HealAmbiguous` there (census VIII, #658). Plan 23.A asks
//! the plane released and builds the rung again; the released build's
//! walk fails `Ambiguous` at the plane too (its sheet, carried over its
//! neighbours' near boxes, meets the top along the block's whole width),
//! so the delete still refuses, one capped build later. The twin guards
//! that release: its refusal and its units.

use crate::pinned_faces::{LENGTH, ROUND, SLOT_START, far_end_at, slotted_to};
use ryukei::name::OpKey;
use ryukei::topo::{Body, FaceId};
use ryukei::tweak::{DeleteSpec, Heal};

/// How far before the round the slot ends.
pub const SHORT: f64 = 0.05;

/// Where the slot ends: [`SHORT`] before the round.
pub fn slot_end() -> f64 {
    LENGTH - ROUND - SHORT
}

/// The rounded block with the slot from `SLOT_START` to [`slot_end`].
pub fn slotted() -> Body {
    slotted_to(SLOT_START, slot_end(), [OpKey(0x23a2), OpKey(0x23a3)])
}

/// The slot's far end.
pub fn far_end(b: &Body) -> FaceId {
    far_end_at(b, slot_end())
}

/// The slot's start plane, at x = `SLOT_START`: the pin released.
pub fn start_end(b: &Body) -> FaceId {
    far_end_at(b, SLOT_START)
}

/// The delete of the slot's far end, healed by growth.
pub fn spec(b: &Body) -> DeleteSpec {
    DeleteSpec {
        faces: vec![b.face(far_end(b)).unwrap().name],
        heal: Heal::Grow,
    }
}
