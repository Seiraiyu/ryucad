//! Sheet metal: recognizing a solid as sheet metal, and unfolding it.
//!
//! [`recognize`] reads any solid, built here or healed from a file, as a
//! sheet-metal part: its thickness, its flanges (rigid pieces between two
//! parallel planar sides `t` apart, holding anything certified to lie
//! between their planes), its bends (pairs of coaxial cylinders of radii
//! `r` and `r + t`, joined tangentially to two flanges along lines parallel
//! to their axis) and how they join. Where it isn't one, it says exactly
//! why, naming the faces ([`crate::SheetMetalError`]).
//!
//! [`unfold`] maps a recognized body face for face onto its flat: the same
//! faces, loops, edges and vertices with the same names, each `Modified`
//! in the journal, on new geometry. Flanges move rigidly; each unfolded
//! bend's region maps by its strip map, a point at angle `φ` from the
//! bend's start, `ρ` from its axis and `y` along it going to
//! `(φ · ba / θ, ρ - r, y)` in the strip, where `ba` is the developed
//! length the caller's [`Allowance`] gives. Bend lines stay as edges
//! between coplanar faces. Beside the flat solid it returns a
//! [`FlatPattern`] of exact plane curves and a [`FlatMap`], what refolding
//! needs.
//!
//! Everything either decides comes from closed forms, certified
//! enclosures and [`crate::tol`]; nothing is built until every image has
//! been found.

mod bends;
mod cells;
mod curve2;
mod flanges;
mod graph;
mod pattern;
mod radial;
mod recognize;
mod shape;
mod slab;
mod unfold;

#[cfg(test)]
pub(crate) mod tests;

pub use pattern::{BendLine, FlatPattern, PatternLoop, PatternSegment};
pub use recognize::recognize;
pub use unfold::unfold;

use crate::geom::{Frame, Plane, Rigid};
use crate::name::{Journal, Name};
use crate::topo::Body;

/// The tolerance recognition judges thickness, radii and walls within,
/// when the caller gives none: a tenth of a micron, wide enough for a
/// healed file written to six decimals in millimetres.
pub const DEFAULT_TOLERANCE: f64 = 1e-7;

/// How closely recognition holds geometry to sheet metal's rules. The
/// tolerance is never taken below [`crate::tol::LINEAR_RESOLUTION`], and
/// each comparison adds the tolerances of the edges and vertices
/// involved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetOptions {
    pub tolerance: f64,
}

impl Default for SheetOptions {
    fn default() -> Self {
        Self {
            tolerance: DEFAULT_TOLERANCE,
        }
    }
}

/// A body read as sheet metal.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetModel {
    pub thickness: f64,
    /// The tolerance it was recognized within: what unfolding adds to an
    /// edge of a wall recognition accepted as radial within it.
    pub tolerance: f64,
    /// By name.
    pub flanges: Vec<Flange>,
    /// By name.
    pub bends: Vec<Bend>,
    /// The flange unfolding holds still when the caller names none: the
    /// one holding the least-named side face.
    pub fixed: usize,
    /// The first loop the flanges and bends make, if they make one. A body
    /// with a loop is recognized, but can't be unfolded.
    pub cycle: Option<Cycle>,
}

impl SheetModel {
    /// Whether the bends join the flanges as a tree, so it unfolds.
    pub fn is_tree(&self) -> bool {
        self.cycle.is_none()
    }

    /// The bend of this name.
    pub fn bend(&self, name: Name) -> Option<&Bend> {
        self.bends.iter().find(|b| b.name == name)
    }
}

/// Flanges and bends joined in a loop: its bends, sorted, and the inner and
/// outer faces of the bend that closes it, sorted, where a rip would open
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cycle {
    pub bends: Vec<Name>,
    pub faces: Vec<Name>,
}

/// A rigid piece of sheet between two parallel planes.
#[derive(Clone, Debug, PartialEq)]
pub struct Flange {
    /// The least name among its side faces.
    pub name: Name,
    /// Every face of the piece, sides included, sorted. Walls shared with
    /// bends (the faces square to a bend's axis) are the bends'.
    pub faces: Vec<Name>,
    /// Its two sides, the one whose plane comes first in name order first.
    pub sides: [Side; 2],
}

/// One side of a flange: its plane, whose frame's `z` is the side's
/// outward normal, and the faces on it, sorted.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    pub plane: Plane,
    pub faces: Vec<Name>,
}

/// Which way a bend folds, seen in the flat from the side away from the
/// first bend's centre (the side [`FlatPattern`] draws): `Up` towards the
/// viewer, `Down` away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BendDirection {
    Up,
    Down,
}

/// A cylindrical bend between two flanges.
#[derive(Clone, Debug, PartialEq)]
pub struct Bend {
    /// The least name among its inner and outer faces.
    pub name: Name,
    /// Its faces on the inner cylinder (radius `r`), sorted.
    pub inner: Vec<Name>,
    /// Its faces on the outer cylinder (radius `r + t`), sorted.
    pub outer: Vec<Name>,
    /// The other faces meeting it: planes square to its axis, planes
    /// through it and radial walls, sorted.
    pub walls: Vec<Name>,
    /// On the axis: `z` along it, `x` towards the bend line on
    /// `flanges[0]`, so the bend's angle runs from 0 there to `angle` at
    /// `flanges[1]`'s.
    pub axis: Frame,
    /// The inner radius `r`.
    pub radius: f64,
    /// Its angle `θ`, in `(0, π]`.
    pub angle: f64,
    /// The flange nearer [`SheetModel::fixed`] first.
    pub flanges: [usize; 2],
    /// Its bend-line edges on each flange's side, as `flanges` orders
    /// them, sorted.
    pub lines: [Vec<Name>; 2],
    pub direction: BendDirection,
}

/// How long a bend's region is in the flat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Allowance {
    /// The neutral radius is `r + k t`, with `k` in `[0, 1]`: the developed
    /// length is `θ (r + k t)`.
    KFactor(f64),
    /// The developed length itself, positive.
    Length(f64),
}

impl Allowance {
    /// The developed length for a bend of inner radius `r` and angle `θ`
    /// in sheet of thickness `t`, or `None` when the allowance is out of
    /// range.
    pub fn length(self, r: f64, t: f64, theta: f64) -> Option<f64> {
        let ba = match self {
            Self::KFactor(k) => {
                if !(0.0..=1.0).contains(&k) {
                    return None;
                }
                theta * (r + k * t)
            }
            Self::Length(ba) => ba,
        };
        (ba.is_finite() && ba >= crate::tol::LINEAR_RESOLUTION).then_some(ba)
    }
}

/// What to unfold and how far.
#[derive(Clone, Debug, PartialEq)]
pub struct UnfoldSpec {
    /// A side face of the flange to hold still, by name; else
    /// [`SheetModel::fixed`].
    pub fixed: Option<Name>,
    /// The bends to unfold, by name; `None` unfolds them all. A bend left
    /// folded is carried rigidly with the flange beyond it.
    pub bends: Option<Vec<Name>>,
    /// Every bend's allowance, unless `overrides` names it.
    pub allowance: Allowance,
    pub overrides: Vec<(Name, Allowance)>,
}

impl UnfoldSpec {
    /// Every bend unfolded about the default flange with one allowance.
    pub fn all(allowance: Allowance) -> Self {
        Self {
            fixed: None,
            bends: None,
            allowance,
            overrides: Vec::new(),
        }
    }
}

/// An unfolded body, its journal, its flat pattern and what refolding
/// needs.
#[derive(Clone, Debug)]
pub struct Unfolded {
    pub body: Body,
    pub journal: Journal,
    pub pattern: FlatPattern,
    pub map: FlatMap,
}

/// How a flat was made from its folded body, by name only, so it stays
/// valid after the flat has been cut. It has a codec
/// ([`crate::codec::encode_flat_map`]).
#[derive(Clone, Debug, PartialEq)]
pub struct FlatMap {
    pub thickness: f64,
    /// Each unfolded bend's strip, in the order unfolding met them.
    pub strips: Vec<Strip>,
    /// Each flange's motion from its folded place to its flat one, by
    /// name.
    pub flanges: Vec<(Name, Rigid)>,
}

/// An unfolded bend's region in the flat.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub bend: Name,
    /// The flange nearer the fixed one, and the other, by name.
    pub near: Name,
    pub far: Name,
    /// The bend's axis frame where it is folded ([`Bend::axis`]).
    pub axis: Frame,
    /// The strip in the flat: its origin on the inner side at the near
    /// bend line, `x` across the strip (its bend lines at `x = 0` and
    /// `x = length`), `y` along the axis and `z` through the sheet (the
    /// sides at `z = 0` and `z = thickness`).
    pub frame: Frame,
    pub radius: f64,
    pub angle: f64,
    /// The developed length.
    pub length: f64,
    pub allowance: Allowance,
}
