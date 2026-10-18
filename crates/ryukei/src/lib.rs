//! ryukei: B-rep geometry kernel.
//!
//! Operates on single bodies only; knows nothing about documents,
//! assemblies, or agents (those live in `ryucad`).

pub mod check;
mod error;
pub mod euler;
pub mod geom;
pub mod interval;
pub mod math;
pub mod mesh;
pub mod name;
pub mod predicates;
pub mod primitive;
pub mod sweep;
pub mod tol;
pub mod topo;

pub use error::{Error, NurbsError, ProfileError, SweepError, TopologyError};

/// This kernel's version, for ryucad's `version` report and cache keys.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
