//! Test-only helpers for ryukei. Never a dependency of the kernel itself.
//!
//! - [`Lcg`]: a seeded, platform-independent input generator.
//! - [`Golden`]: bit-exact output hashing checked against committed hashes.

mod golden;
mod lcg;

pub use golden::Golden;
pub use lcg::Lcg;
