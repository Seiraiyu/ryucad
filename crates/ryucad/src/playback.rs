//! Motions shown, played and drawn (design 529 phase 6, plan 26.C). A
//! window shows at most one motion (`live::View::motion`): which one, how
//! far through, and whether it plays. `motion.frames` gives where a
//! motion puts each moved body at evenly spaced fractions, solved by
//! continuation as a sweep is, so a window fetches them once and scrubs
//! and plays locally. A render draws a motion at any fraction, and with
//! `interference` paints the faces that collide there (against rest, as
//! a sweep finds them). Nothing here writes, and only a render's
//! `interference` ever intersects a pair.

use crate::doc::id::Id;

/// The motion a participant's window shows: an assembly's, how far
/// through (0 its start, 1 its end), and whether the window plays it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Shown {
    pub part: Id,
    pub motion: Id,
    pub fraction: f64,
    pub playing: bool,
}
