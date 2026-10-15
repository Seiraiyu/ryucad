//! Test-only helpers for ryukei. Never a dependency of the kernel itself.
//!
//! - [`exact`]: exact rational reference results for the robust predicates.
//! - [`Lcg`]: a seeded, platform-independent input generator.
//! - [`Golden`]: bit-exact output hashing checked against committed hashes.

pub mod exact;
mod golden;
mod lcg;

pub use golden::Golden;
pub use lcg::Lcg;
