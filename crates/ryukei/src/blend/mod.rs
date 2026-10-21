//! Blends: fillets and chamfers on the edges of solid bodies (design 43).
//!
//! A blend takes any set of edges, each with its own fillet radius or
//! chamfer, and rounds or bevels each where it is convex and fills it
//! where it is concave. The pipeline makes every decision before it
//! changes anything: the edges are resolved and checked ([`spec`]).
//!
//! Only faces on the analytic surfaces (planes, cylinders, cones, spheres
//! and tori) and on earlier fillets' pipes may be blended; the rest is
//! refused with [`BlendError::Unsupported`], never guessed.

// Unused until Task 11 builds blends from them.
#[allow(dead_code)]
mod chain;
#[allow(dead_code)]
mod convex;
#[allow(dead_code)]
mod end;
#[allow(dead_code)]
mod fillet;
#[allow(dead_code)]
mod rail;
#[allow(dead_code)]
mod spec;
#[allow(dead_code)]
mod spine;

use crate::Error;
use crate::name::Name;
use crate::topo::{Body, EdgeId};

#[cfg(doc)]
use crate::BlendError;

/// What a blend does to one edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendKind {
    /// A rolling-ball fillet of this radius.
    Fillet { radius: f64 },
    /// A bevel set back from the edge along each face.
    Chamfer(Chamfer),
}

/// How far a chamfer is set back from its edge on each face: a sphere of
/// that radius swept along the edge meets the face there, so on a planar
/// face it is the distance from the edge along the face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Chamfer {
    /// The same distance on both faces.
    Equal { distance: f64 },
    /// `first` on `first_face`, one of the edge's faces, and `second` on
    /// the other.
    Distances {
        first: f64,
        second: f64,
        first_face: Name,
    },
    /// `distance` on `face`, one of the edge's faces, and the chamfer at
    /// `angle` (radians, in `(0, π/2]`) to that face.
    DistanceAngle {
        distance: f64,
        angle: f64,
        face: Name,
    },
}

/// One edge to blend, named by its stable name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeBlend {
    pub edge: Name,
    pub kind: BlendKind,
}

/// The edges reachable from `edge` through vertices where an edge meets
/// exactly one other tangentially, in order along the chain: from one end
/// to the other for an open chain, from `edge` along its direction for a
/// closed one. A blend refuses a blended edge that meets an unblended
/// one tangentially ([`BlendError::OpenTangentChain`]); ryucad offers to
/// blend the whole chain instead. Fails with `DeadEntity` for an edge
/// that is not live.
pub fn tangent_chain(body: &Body, edge: EdgeId) -> Result<Vec<EdgeId>, Error> {
    chain::tangent_chain(body, edge)
}

#[cfg(test)]
mod tests;
