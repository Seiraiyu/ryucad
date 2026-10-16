//! Rational B-splines (NURBS): knot vectors, basis functions, curves and
//! surfaces.

mod basis;
mod bezier;
mod convert;
mod curve;
mod eval;
mod knots;
mod project;
mod refine;
mod surface;

pub use basis::Basis;
pub use convert::Reparam;
pub use curve::NurbsCurve;
pub use knots::{KnotVector, MAX_DEGREE};
pub use surface::{Direction, NurbsSurface};
