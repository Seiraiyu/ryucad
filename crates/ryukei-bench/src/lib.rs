//! Benchmarks live in `benches/`. With the `occt` feature this crate
//! also holds the OCCT comparison harness ([`occt`]); without it the
//! default build never needs a C++ toolchain.

#[cfg(feature = "occt")]
pub mod occt;
