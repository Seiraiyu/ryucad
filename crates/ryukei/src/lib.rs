//! ryukei: B-rep geometry kernel.
//!
//! Operates on single bodies only; knows nothing about documents,
//! assemblies, or agents (those live in `ryucad`).

mod error;
pub mod geom;
pub mod math;
pub mod predicates;
pub mod tol;

pub use error::Error;
