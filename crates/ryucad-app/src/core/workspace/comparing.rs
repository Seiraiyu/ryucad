//! A compare with a Git revision, a sub-state of the window's workspace
//! (design 541, "The app's workspace"): the diff, its ghosts and which
//! side is drawn.

use super::*;

/// The window's compare, and the diffs it has asked for.
#[derive(Default)]
pub struct Comparing {
    /// This window's compare with a Git revision, while it is in one: it
    /// follows the window's participant's compare state.
    pub compare: Option<Compare>,
    /// Which side of the compare is drawn, and whether the other is too:
    /// the window's own, never the session's (design 447).
    pub blink: Blink,
    /// How many diffs were asked for, and whether one is on its way; one
    /// asked for meanwhile waits for it (`compare_again`).
    pub(super) compare_asked: u64,
    pub(super) compare_waiting: bool,
    pub(super) compare_again: bool,
}
