//! ryukei: B-rep geometry kernel.
//!
//! Operates on single bodies only; knows nothing about documents,
//! assemblies, or agents (those live in `ryucad`).

pub mod check;
mod error;
pub mod geom;
pub mod interval;
pub mod math;
pub mod name;
pub mod predicates;
pub mod tol;
pub mod topo;

pub use error::{Error, TopologyError};
