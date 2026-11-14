//! Intersections of curves and surfaces, certified.
//!
//! Every topological decision here (whether a curve meets a surface,
//! where, and on which side it comes and goes) rests on interval
//! arithmetic and exact predicates. Coincidence and tangency are decided
//! by explicit tolerance rules, and what stays uncertain is reported as
//! [`Failure::Uncertain`], never guessed. The analytic surfaces (plane,
//! cylinder, cone, sphere, torus) are intersected with each other by
//! their implicit forms, any other surface (a pipe, a NURBS, a swept
//! surface) with any of them in its own parameters, and any two others in
//! both their parameters together; a curve meets any surface. Faces that
//! touch along a curve are certified apart beside it instead
//! ([`beside`]).

mod beside;
pub(crate) mod broad;
pub(crate) mod cells;
pub(crate) mod coincide;
mod contour;
mod cs;
mod face;
mod fit;
mod general;
pub(crate) mod implicit;
mod inface;
mod known;
pub(crate) use known::{Seam, Seams, known_seam};
mod pipe;
mod pipe_cs;
mod seam;
mod sheet_cs;
mod sheet_trace;
pub(crate) use sheet_trace::trace_contour;
mod solid;
mod ss;
mod ss_exact;
mod ss_trace;
pub(crate) use ss_trace::Traced;
mod winding;

pub(crate) use beside::CELLS as BESIDE_CELLS;
pub use beside::beside;
pub(crate) use beside::{
    CONE_PARTS, face_beside, face_off_ball, faces_apart, normal_cones, parameter_box,
    section_plane, sine_apart,
};
#[cfg(test)]
pub(crate) use beside::{Tangency, tangency};
pub(crate) use coincide::{coincident, common, covering};
pub(crate) use contour::fit_contour;
pub(crate) use cs::curve_surface_in;
pub use cs::{CurveHit, Side, curve_surface};
pub(crate) use face::coarse_box;
pub(crate) use face::face_face_in;
pub use face::{Contact, FaceMeet, Rim, Stretch, face_box, face_face};
pub(crate) use face::{
    face_face_over, face_face_within, face_rect, max_tolerance, region_outside,
    region_outside_boxes, shifted, shifts,
};
pub(crate) use fit::{
    fit_along, fit_on, fit_on_feet, fit_pcurve, fit_pcurve_by, fit_pcurve_tolerant,
    fit_pcurve_within, pcurve_certified,
};
pub(crate) use implicit::curve_distance;
pub use inface::{Place, point_in_face};
pub use pipe::Cell;
pub use pipe_cs::curve_pipe;
pub(crate) use pipe_cs::curve_pipe_in;
pub(crate) use seam::reseamed;
pub(crate) use solid::face_tolerance;
pub(crate) use solid::point_in_body_in;
pub(crate) use solid::point_in_faces;
pub use solid::{Inclusion, point_in_body};
pub(crate) use ss::carried_by;
pub(crate) use ss::extended_curve;
pub use ss::{Branch, SurfaceHit, pipe_surface, surface_surface};
pub(crate) use ss::{pipe_surface_in, surface_surface_in};
pub use winding::winding;
pub(crate) use winding::winding_off;

/// Rounding slack added to every certified distance: the surfaces and
/// curves evaluate in floating point, and their frames are orthonormal
/// only to rounding, which moves points by a few ulps of the ±500 m size
/// box (about 1e-13 m) per operation. It is 1% of
/// [`crate::tol::LINEAR_RESOLUTION`].
pub(crate) const SLACK: f64 = 1e-10;

/// Why an intersection could not be computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Geometry no intersector covers.
    Unsupported,
    /// A root, an ordering or a classification that stayed uncertain
    /// after escalation.
    Uncertain,
}
