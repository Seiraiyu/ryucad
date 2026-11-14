//! The assembly tools, a sub-state of the window's workspace: drags, the
//! Move tool, the Mate dialog, interference and clearance (only when
//! asked), explodes and motions.

use super::*;

/// The shown assembly's tools.
#[derive(Default)]
pub struct Assembly {
    /// The shown assembly's interference, once asked for (Tools > Check
    /// interference); marked stale when the assembly changes, and asked
    /// again only by Check again (design 525).
    pub interference: Option<Interference>,
    /// Tools > Clearance, once asked for: the shown assembly's.
    pub clearance: Option<Clearance>,
    /// The instance being dragged, if any.
    pub drag: Option<Drag>,
    /// Why the last drag moved nothing, for the status line.
    pub drag_note: Option<String>,
    /// The Move tool, while it is on one instance.
    pub mover: Option<crate::core::mover::Mover>,
    /// When the Move tool's preview was last asked and for which value,
    /// and whether one is on its way.
    pub(super) mover_asked: Option<(Instant, Option<crate::core::mover::Grip>, f64)>,
    pub(super) mover_waiting: bool,
    /// The Mate dialog, while it is open.
    pub mating: Option<crate::core::mating::Mating>,
    /// How many mate previews the window has asked for.
    pub(super) mate_previews: u64,
    /// The shown assembly's explodes, the one shown, and a step being
    /// made.
    pub explodes: crate::core::explode::Explodes,
    /// The shown assembly's motions, the one shown, its frames, Play and
    /// the last sweep.
    pub motions: crate::core::motion::Motions,
}
