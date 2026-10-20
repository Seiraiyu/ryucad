//! Test-only helpers for ryukei. Never a dependency of the kernel itself.
//!
//! - [`exact`]: exact rational reference results for the robust predicates.
//! - [`Lcg`]: a seeded, platform-independent input generator.
//! - [`Golden`]: bit-exact output hashing checked against committed hashes.
//! - [`euler_script`]: Euler operator sequences played from bytes.
//! - [`nurbs`]: seeded random NURBS curves and surfaces.
//! - [`mesh`]: a NURBS-topped block, and measures of meshes.
//! - [`sweep_bytes`]: sweeps from bytes, for fuzzing and its regressions.
//! - [`boolean_bytes`]: booleans from bytes, for fuzzing and its
//!   regressions.

pub mod boolean_bytes;
pub mod euler_script;
pub mod exact;
mod golden;
mod lcg;
pub mod mesh;
pub mod nurbs;
pub mod sweep_bytes;

pub use golden::{Golden, Section};
pub use lcg::Lcg;
