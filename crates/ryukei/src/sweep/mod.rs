//! Sweeps: solids made by moving a planar profile.
//!
//! A [`Profile`] is a planar region in a [`crate::geom::Frame`]: closed
//! loops of trimmed [`crate::geom::Curve2`] segments, each carrying a
//! caller key. [`ValidProfile::new`] checks it (closure, no crossing or
//! touching, certified) and nests its loops into lumps with holes.
//! [`extrude`] and [`revolve`] sweep it into a solid per lump.

mod along;
mod area;
mod build;
mod capped;
mod extrude;
mod loft;
mod names;
mod nest;
mod path;
mod place;
mod planar;
mod profile;
mod revolve;
mod separate;
mod sheets;

pub use along::{AlongOptions, along};
pub use extrude::extrude;
pub use loft::{LoftOptions, Tangent, loft};
pub use path::{Path, PathSegment};
pub use planar::planar;
pub use profile::{
    Chain, ChainKey, Loop, LoopKey, Lump, Profile, Segment, SegmentKey, ValidChain, ValidLoop,
    ValidProfile,
};
pub use revolve::revolve;

use crate::SweepError;
