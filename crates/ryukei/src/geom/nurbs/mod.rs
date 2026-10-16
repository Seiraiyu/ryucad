//! Rational B-splines (NURBS): knot vectors, basis functions, curves and
//! surfaces.

mod basis;
mod knots;

pub use basis::Basis;
pub use knots::{KnotVector, MAX_DEGREE};
