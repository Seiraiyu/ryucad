//! Gerber X2 layers and Excellon drill files written from `ryu-board`'s
//! shapes (design 732). Product-neutral: it knows no project, product or
//! fabricator, and depends on `ryu-board` alone. ryueda's `board export`
//! writes through it; ryudfm's stage 6 will add its readers here.
//!
//! Everything is integer nanometres in and exact text out: the same
//! input gives the same bytes on every platform, with no dates.

pub mod excellon;
mod fmt;
pub mod gerber;
#[cfg(feature = "testing")]
pub mod read;
mod recognise;

pub use fmt::{LIMIT, mm};
pub use recognise::{Aperture, Flash, flash_of, same_contour};

/// Who wrote a file: `.GenerationSoftware`'s vendor, application and
/// version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Software {
    pub vendor: String,
    pub application: String,
    pub version: String,
}

/// Why a file can't be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A coordinate past what the formats hold here (±9,999.999999 mm),
    /// with where it is.
    OutOfRange(ryu_board::geom::Point),
    /// A region with holes: Gerber's cut-ins aren't written, so the
    /// caller gives hole-free pieces.
    Holes,
    /// A piece the writer doesn't draw, saying which.
    Unsupported(&'static str),
    /// A drill of no diameter.
    NoDiameter,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::OutOfRange(p) => write!(
                f,
                "({}, {}) nm is past what Gerber and Excellon hold here (±9,999.999999 mm)",
                p.x, p.y
            ),
            Error::Holes => write!(f, "a region with holes (give hole-free pieces)"),
            Error::Unsupported(what) => write!(f, "{what} isn't written"),
            Error::NoDiameter => write!(f, "a drill of no diameter"),
        }
    }
}

impl std::error::Error for Error {}
