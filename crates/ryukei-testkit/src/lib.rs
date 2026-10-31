//! Test-only helpers for ryukei. Never a dependency of the kernel itself.
//!
//! - [`exact`]: exact rational reference results for the robust predicates.
//! - [`Lcg`]: a seeded, platform-independent input generator.
//! - [`Golden`]: bit-exact output hashing checked against committed hashes.
//! - [`euler_script`]: Euler operator sequences played from bytes.
//! - [`nurbs`]: seeded random NURBS curves and surfaces.
//! - [`mesh`]: a NURBS-topped block, and measures of meshes.
//! - [`sweep_bytes`]: sweeps from bytes, for fuzzing and its regressions.
//! - [`along_bytes`]: sweeps along paths and lofts from bytes, likewise.
//! - [`boolean_bytes`]: booleans from bytes, for fuzzing and its
//!   regressions.
//! - [`blend_bytes`]: blends from bytes, for fuzzing and its regressions.
//! - [`heal_bytes`]: shapes with a file's faults from bytes, for fuzzing
//!   healing and its regressions.
//! - [`offset_bytes`]: offsets and shells from bytes, for fuzzing and its
//!   regressions.
//! - [`race`]: the bodies of the large-assembly STEP race.
//! - [`nurbs_race`]: the bodies of the NURBS race (design 94, plan 8.F).

pub mod along_bytes;
pub mod blend_bytes;
pub mod boolean_bytes;
pub mod euler_script;
pub mod exact;
mod golden;
pub mod heal_bytes;
mod lcg;
pub mod mesh;
pub mod nurbs;
pub mod nurbs_race;
pub mod offset_bytes;
pub mod race;
pub mod sweep_bytes;

pub use golden::{Golden, Section, unversioned};
pub use lcg::Lcg;
