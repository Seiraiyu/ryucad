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
//! - [`sheet_bytes`]: sheets from bytes, for fuzzing and its regressions.
//! - [`tolerant_bytes`]: tolerant twins of bodies and booleans from bytes
//!   (design 299), for the tolerant tests and fuzzing.
//! - [`race`]: the bodies of the large-assembly STEP race.
//! - [`nurbs_race`]: the bodies of the NURBS race (design 94, plan 8.F).
//! - [`sheet_metal`]: sheet-metal parts extruded from bent strips.

/// The work budget the blend, sweep and offset fuzz targets give each
/// case, checks and meshes included: 8 times the most any case of the
/// sweep seed corpus spent, rounded up to a power of two (plan 15.A,
/// Task 21: davids-m4, an Apple M4, 2026-11-01; 6,228,952 units in 131 s
/// under ASAN). A unit's cost varies about 150 times between cases (a
/// lofted spline's enclosures in the four-parameter tracer take
/// milliseconds each), so this bounds runaways in units, not in seconds.
pub const FUZZ_BUDGET: u64 = 1 << 26;

/// Prints a fuzz case's work units when `RYUKEI_FUZZ_UNITS` is set, for
/// calibrating [`FUZZ_BUDGET`].
pub fn note_units(work: &ryukei::work::Work) {
    if std::env::var_os("RYUKEI_FUZZ_UNITS").is_some() {
        eprintln!("units {}", work.spent());
    }
}

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
pub mod sheet_bytes;
pub mod sheet_metal;
pub mod sweep_bytes;
pub mod tolerant_bytes;

pub use golden::{Golden, Section, unversioned};
pub use lcg::Lcg;
