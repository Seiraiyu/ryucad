//! Benchmarks live in `benches/`. With the `occt` feature (empty until
//! roadmap phase 3), this crate will hold the OCCT comparison harness, so
//! the default build never needs a C++ toolchain.

#[cfg(feature = "occt")]
pub mod occt {}
