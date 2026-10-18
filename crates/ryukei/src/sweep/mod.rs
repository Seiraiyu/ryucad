//! Sweeps: solids made by moving a planar profile.
//!
//! A [`Profile`] is a planar region in a [`crate::geom::Frame`]: closed
//! loops of trimmed [`crate::geom::Curve2`] segments, each carrying a
//! caller key. [`ValidProfile::new`] checks it (closure, no crossing or
//! touching, certified) and nests its loops into lumps with holes.

mod area;
mod chain;
mod intersect2;
mod nest;
mod profile;

pub use profile::{Loop, LoopKey, Lump, Profile, Segment, SegmentKey, ValidLoop, ValidProfile};
