//! ryukei: B-rep geometry kernel.
//!
//! Operates on single bodies only; knows nothing about documents,
//! assemblies, or agents (those live in `ryucad`).

pub mod blend;
pub mod boolean;
pub mod check;
pub mod codec;
mod error;
pub mod euler;
pub mod geom;
pub mod heal;
pub mod intersect;
pub mod interval;
pub(crate) mod layout;
pub mod math;
pub mod mesh;
pub mod name;
pub mod pcurve;
pub mod predicates;
pub mod primitive;
pub(crate) mod split;
pub mod sweep;
pub mod tol;
pub mod topo;

pub use error::{
    BlendError, BooleanError, Error, HealError, NurbsError, ProfileError, SweepError, TopologyError,
};

/// This kernel's version, for ryucad's `version` report and cache keys.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
