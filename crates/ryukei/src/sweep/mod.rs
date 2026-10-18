//! Sweeps: solids made by moving a planar profile.
//!
//! A [`Profile`] is a planar region in a [`crate::geom::Frame`]: closed
//! loops of trimmed [`crate::geom::Curve2`] segments, each carrying a
//! caller key. [`ValidProfile::new`] checks it and orients its loops.

mod area;
mod profile;

pub use profile::{Loop, LoopKey, Lump, Profile, Segment, SegmentKey, ValidLoop, ValidProfile};
