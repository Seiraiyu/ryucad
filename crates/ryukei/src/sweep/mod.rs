//! Sweeps: solids made by moving a planar profile.
//!
//! A [`Profile`] is a planar region in a [`crate::geom::Frame`]: closed
//! loops of trimmed [`crate::geom::Curve2`] segments, each carrying a
//! caller key. [`ValidProfile::new`] checks it (closure, no crossing or
//! touching, certified) and nests its loops into lumps with holes.
//! [`extrude`] and [`revolve`] sweep it into a solid per lump.

mod area;
mod build;
mod capped;
mod chain;
mod extrude;
mod intersect2;
mod names;
mod nest;
mod place;
mod profile;
mod revolve;

pub use extrude::extrude;
pub use profile::{Loop, LoopKey, Lump, Profile, Segment, SegmentKey, ValidLoop, ValidProfile};
pub use revolve::revolve;

use crate::SweepError;
