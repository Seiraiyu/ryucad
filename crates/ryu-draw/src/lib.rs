//! 2D drawings every ryu product writes (design 417): exact curves, text
//! in Inter, line styles, and the PDF and SVG writers.
//!
//! It is product-neutral: no kernel, no `serde`, no project. ryueda's
//! schematic PDFs (design 236) are written through [`pdf`], and ryucad's
//! drawings are [`Item`]s written by [`pdf`] and [`svg`].
//!
//! Coordinates are `f64` in a unit the caller chooses, y up: a writer is
//! told how many points (PDF) or how many of its own units (SVG) one is.
//! ryueda draws in nanometres, ryucad in sheet millimetres.

pub mod dxf;
pub mod font;
pub mod geom;
pub mod hatch;
pub mod item;
pub mod pdf;
pub mod style;
pub mod svg;
pub mod symbol;

pub use geom::{P, Path, Segment};
pub use item::{Anchor, Item};
pub use style::{Colour, LineKind, Standard, Stroke};
