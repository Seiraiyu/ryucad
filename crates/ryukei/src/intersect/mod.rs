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
mod cells;
pub(crate) mod coincide;
mod cs;
mod face;
mod fit;
mod general;
mod implicit;
mod inface;
mod pipe;
mod pipe_cs;
mod sheet_cs;
mod sheet_trace;
mod solid;
mod ss;
mod ss_exact;
mod ss_trace;

pub use beside::beside;
pub(crate) use beside::face_beside;
pub(crate) use coincide::coincident;
pub use cs::{CurveHit, Side, curve_surface};
pub(crate) use face::coarse_box;
pub use face::{Contact, FaceMeet, Rim, Stretch, face_box, face_face};
pub(crate) use face::{face_face_within, shifted, shifts};
pub(crate) use fit::{
    fit_along, fit_on, fit_pcurve, fit_pcurve_by, fit_pcurve_within, pcurve_certified,
};
pub(crate) use implicit::curve_distance;
pub use inface::{Place, point_in_face};
pub use pipe::Cell;
pub use pipe_cs::curve_pipe;
pub(crate) use solid::point_in_faces;
pub use solid::{Inclusion, point_in_body};
pub use ss::{Branch, SurfaceHit, pipe_surface, surface_surface};

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
