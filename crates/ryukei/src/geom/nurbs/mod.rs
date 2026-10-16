//! Rational B-splines (NURBS): knot vectors, basis functions, curves and
//! surfaces.

mod basis;
mod curve;
mod eval;
mod knots;
mod refine;
mod surface;

pub use basis::Basis;
pub use curve::NurbsCurve;
pub use knots::{KnotVector, MAX_DEGREE};
pub use surface::{Direction, NurbsSurface};
