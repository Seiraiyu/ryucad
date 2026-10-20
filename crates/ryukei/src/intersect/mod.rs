//! Intersections of curves and surfaces, certified.
//!
//! Every topological decision here (whether a curve meets a surface,
//! where, and on which side it comes and goes) rests on interval
//! arithmetic and exact predicates. Coincidence and tangency are decided
//! by explicit tolerance rules, and what stays uncertain is reported as
//! [`Failure::Uncertain`], never guessed. The analytic surfaces (plane,
//! cylinder, cone, sphere, torus) are intersected with each other, and a
//! pipe with any of them ([`pipe_surface`]); a surface with no implicit
//! form is otherwise [`Failure::Unsupported`].

pub(crate) mod broad;
mod coincide;
mod cs;
mod face;
mod fit;
mod implicit;
mod inface;
mod pipe;
mod pipe_cs;
mod pipe_trace;
mod solid;
mod ss;
mod ss_exact;
mod ss_trace;

pub(crate) use coincide::coincident;
pub use cs::{CurveHit, Side, curve_surface};
pub use face::{Contact, FaceMeet, Rim, Stretch, face_box, face_face};
pub(crate) use face::{shifted, shifts};
pub(crate) use fit::fit_pcurve;
pub use inface::{Place, point_in_face};
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
    /// A surface with no implicit form (a NURBS or a swept surface) that
    /// might meet the other geometry.
    Unsupported,
    /// A root, an ordering or a classification that stayed uncertain
    /// after escalation.
    Uncertain,
}
