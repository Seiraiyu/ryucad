//! Hidden lines (design 326, phase 3): every edge and silhouette of
//! several placed bodies, cut wherever its visibility may change (its
//! image's crossings with every other image curve, its folds, its ends),
//! each piece judged by one certified ray towards the viewer: how many
//! faces it crosses in front of the piece.

use super::{ImageCurve, Silhouettes};
use crate::geom::Rigid;
use crate::name::Name;
use crate::topo::{Body, EdgeId, FaceId};

/// A body placed in the view's model space, as a boolean's tool is.
#[derive(Clone, Copy, Debug)]
pub struct Placed<'a> {
    pub body: &'a Body,
    pub place: Rigid,
}

/// What a view computes besides edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    /// Trace every face's silhouette (true by default). Off, only edges
    /// are drawn: a view of bodies whose outlines are all edges (prisms)
    /// traces nothing.
    pub silhouettes: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self { silhouettes: true }
    }
}

/// What a piece is part of. `body` is the placed body's index in the
/// slice the view was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Edge {
        body: u32,
        edge: EdgeId,
        name: Name,
    },
    /// The `ordinal`th silhouette stretch of `face` (10.A's numbering).
    Silhouette {
        body: u32,
        face: FaceId,
        face_name: Name,
        ordinal: u32,
    },
}

/// How a drawing draws a piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// An edge whose faces meet at an angle (convex or concave).
    Sharp,
    /// An edge whose faces are tangent (a fillet's rails).
    Smooth,
    /// A sheet's laminar edge.
    Boundary,
    /// A silhouette.
    Silhouette,
    /// An edge whose bend rounding couldn't settle.
    Uncertain,
}

/// Whether a piece is seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Visible,
    /// Behind this many faces (its quantitative invisibility).
    Hidden(u32),
    /// No point of the piece could be judged with a certificate.
    Uncertain,
}

impl Visibility {
    /// The visibility of a piece behind `n` faces.
    pub fn of(n: u32) -> Self {
        if n == 0 {
            Self::Visible
        } else {
            Self::Hidden(n)
        }
    }
}

/// One stretch of an edge or a silhouette along which visibility is
/// constant.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub source: Source,
    /// The exact image in the view's frame, and the stretch of the 3D
    /// curve's parameter it is the image of (one to one).
    pub image: ImageCurve,
    /// Encloses the piece's depth along `towards` (nearer is larger).
    pub depth: [f64; 2],
    pub visibility: Visibility,
    /// The pieces whose images this one's runs along (over more than 100
    /// tolerances), as indices into [`Projection::pieces`], nearest first
    /// by depth: a drawing draws the nearest.
    pub overlaps: Vec<u32>,
    pub class: Class,
    /// An edge a silhouette runs along (a half cylinder's boundary
    /// ruling): it draws the outline there.
    pub on_silhouette: bool,
}

/// What a view draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Projection {
    /// By placed body; in each, edges by ID, then silhouettes by face and
    /// ordinal; along each, by parameter.
    pub pieces: Vec<Piece>,
    /// Each placed body's silhouettes in the placed frame (their 3D
    /// curves, and the faces seen edge-on), in the order given.
    pub silhouettes: Vec<Silhouettes>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_trace_silhouettes_by_default_and_a_count_of_none_is_visible() {
        assert!(ViewOptions::default().silhouettes);
        assert_eq!(Visibility::of(0), Visibility::Visible);
        assert_eq!(Visibility::of(3), Visibility::Hidden(3));
        assert_eq!(Projection::default().pieces.len(), 0);
    }
}
