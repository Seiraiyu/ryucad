//! Intersections of curves and surfaces, certified.
//!
//! Every topological decision here (whether a curve meets a surface,
//! where, and on which side it comes and goes) rests on interval
//! arithmetic and exact predicates. Coincidence and tangency are decided
//! by explicit tolerance rules, and what stays uncertain is reported as
//! [`Failure::Uncertain`], never guessed. Only the analytic surfaces
//! (plane, cylinder, cone, sphere, torus) are intersected; a surface with
//! no implicit form is [`Failure::Unsupported`].

mod cs;
mod implicit;

pub use cs::{CurveHit, Side, curve_surface};

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
