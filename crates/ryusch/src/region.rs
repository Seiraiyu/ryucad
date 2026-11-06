//! Functional regions: titled rectangles on a sheet, each holding one
//! function's parts (a recognised circuit, a dense IC with its
//! satellites, a connector with its breakout, a connected group). They
//! are layout, never connectivity: nothing electrical reads them.

use crate::geom::{GRID, Nm};

/// A region's margin inside its frame, on every side.
pub const FRAME: Nm = 2 * GRID;
/// The band at the top of a frame its title stands in.
pub const TITLE: Nm = 3 * GRID;
/// How far paper fit widens the channels between regions, at most.
pub const WIDEN: Nm = 4 * GRID;
