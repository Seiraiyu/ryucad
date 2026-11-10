//! Blends read back from a body and edited (design 399, plan 13.C):
//! fillets, chamfers and their corners recognised on any body
//! ([`recognise_blends`]), deleted by size ([`delete_blends`]), made again
//! at a new size ([`resize_blends`]) or after a tweak that would break
//! them ([`Blends::Rebuild`]), their faces, edges and vertices keeping
//! their names.

pub(crate) mod recognise;

use crate::blend::BlendKind;
use crate::name::Name;

/// How close a face must be to a blend to be read as one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecogniseOptions {
    /// How far a spline may lie from the cylinder or torus it is read as,
    /// how far a chamfer's setback may vary, and how far two sizes may
    /// differ and be one, in meters: from `LINEAR_RESOLUTION` to
    /// `TOLERANCE_CAP`.
    pub tolerance: f64,
}

/// [`RecogniseOptions::tolerance`] by default: 10 µm, the most a healed
/// import's fitted pcurve may be off.
pub const DEFAULT_TOLERANCE: f64 = 1e-5;

impl Default for RecogniseOptions {
    fn default() -> Self {
        Self {
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

/// One blend: its faces (sorted), its kind and size, the two faces it
/// rolls or stands on (by name order; a `Distances` chamfer's first is
/// `supports[0]`), and the chain of blends joined tangentially it is in.
#[derive(Clone, Debug, PartialEq)]
pub struct Recognised {
    pub faces: Vec<Name>,
    pub kind: BlendKind,
    pub supports: [Name; 2],
    pub chain: usize,
}

/// A ball corner: its face and the blends it joins (indices into
/// [`Recognition::blends`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Corner {
    pub face: Name,
    pub blends: Vec<usize>,
}

/// A body's blends, by least face name, and its ball corners, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recognition {
    pub blends: Vec<Recognised>,
    pub corners: Vec<Corner>,
}

impl Recognition {
    /// The blend or corner holding face `face`: `Some(Ok(i))` for blend
    /// `i`, `Some(Err(j))` for corner `j`.
    pub fn holding(&self, face: Name) -> Option<Result<usize, usize>> {
        if let Some(i) = self.blends.iter().position(|b| b.faces.contains(&face)) {
            return Some(Ok(i));
        }
        self.corners.iter().position(|c| c.face == face).map(Err)
    }
}
