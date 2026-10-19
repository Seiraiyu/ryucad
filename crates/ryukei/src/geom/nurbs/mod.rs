//! Rational B-splines (NURBS): knot vectors, basis functions, curves and
//! surfaces.

mod basis;
mod bezier;
mod bound;
mod convert;
mod curve;
mod curve2;
mod eval;
mod knots;
mod project;
mod project_surface;
mod refine;
mod surface;

pub use basis::Basis;
pub(crate) use bezier::{BezierCurve, BezierPatch};
pub use convert::Reparam;
pub use curve::NurbsCurve;
pub use curve2::NurbsCurve2;
pub(crate) use eval::Coords;
pub use knots::{KnotVector, MAX_DEGREE};
pub(crate) use refine::Hom;
pub use surface::{Direction, NurbsSurface};
