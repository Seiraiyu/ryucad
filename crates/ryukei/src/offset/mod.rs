//! Offset bodies and shells.
//!
//! [`offset_body`] moves every face of a solid along its outward normal by
//! its distance (the spec's, or the face's own), positive outwards, and
//! closes what that opens and trims what that crosses, locally:
//!
//! - **Faces** become their surfaces' offsets (see
//!   [`crate::geom::Surface::offset`]), keeping their parameters, so every
//!   pcurve of a face is one on its offset. A face whose offset folds or
//!   collapses is refused (`FaceCollapses`).
//! - **Edges** (see `classify`): where the faces stay, the edge stays;
//!   where they are tangent all along it (within the gap a fit may close,
//!   `SMOOTH_GAP`), it moves with them; where they move apart, the gap is
//!   filled by the surface a ball of radius `|d|` sweeps rolling about the
//!   edge, a cylinder about a line, a torus about a circle, a pipe about
//!   any other curve, between the edge's two moved copies (its rails);
//!   where they cross, or move by different distances, the edge becomes
//!   the branch of their offsets' intersection near it (`EdgeVanishes`
//!   where there is none, or it no longer runs between where the edge's
//!   vertices go). An edge whose faces are tangent in some places only is
//!   refused (`Unsupported`, `PartlySmooth`).
//! - **Vertices** by the classes of the edges there (see `vertices`): a
//!   sphere where opening edges meet, one point where closing edges meet,
//!   and at three edges mixing both, fills ending on a face or trimming
//!   each other. Any other vertex is refused naming its case.
//!
//! The result is built through Euler operators from those faces (see
//! [`crate::layout`]), each shell of the body becoming one shell of it,
//! its voids still voids, and must pass [`crate::check::check`] and
//! [`crate::check::check_intersections`]: faces meeting away from their
//! edges are `SelfIntersecting`, naming them. Nothing is repaired.
//!
//! [`shell`] hollows a solid: the body less its inward offset, the open
//! faces offset by zero so they go (or, outwards, its outward offset less
//! the body).
//!
//! **Names.** Faces keep their names, as do edges and vertices that are
//! still one edge or one vertex (`Modified` in the journal). A fill is
//! named from the edge or vertex it replaces ([`OffsetRole::Edge`],
//! [`OffsetRole::Vertex`]), which the journal records as `Replaced`; new
//! edges from the faces they lie between (and the vertex, for those a
//! vertex makes), new vertices from the edges meeting there (`Created`).
//! Changing a distance renames nothing.
//!
//! [`OffsetRole::Edge`]: crate::name::OffsetRole::Edge
//! [`OffsetRole::Vertex`]: crate::name::OffsetRole::Vertex

mod build;
mod classify;
mod edges;
mod vertices;

#[cfg(test)]
mod tests;

use crate::Error;
use crate::name::{Journal, Name, OpKey};
use crate::topo::Body;

/// How far each face of an offset body moves: `distance` for every face
/// but those `faces` names, each by its own. Positive is outwards.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OffsetSpec {
    pub distance: f64,
    pub faces: Vec<(Name, f64)>,
}

/// `body` offset by `spec`: a new body and the journal of what happened
/// to names. See the module's documentation for what is built and what
/// is refused; refusals are `Error::InvalidOffset`, made before anything
/// is built, naming the face, edge or vertex.
pub fn offset_body(key: OpKey, body: &Body, spec: &OffsetSpec) -> Result<(Body, Journal), Error> {
    let plan = classify::Plan::new(body, spec)?;
    let geo = edges::curves(&plan)?;
    let vgeo = vertices::vertices(&plan, &geo)?;
    build::build(key, &plan, &geo, &vgeo)
}
