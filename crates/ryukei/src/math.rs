//! Deterministic scalar math: the only place kernel code gets these
//! functions. They live in `ryu-math`, so crates that never touch the
//! kernel (the file formats) get the same bits; this is that crate, under
//! the path kernel code and `clippy.toml` have always named.

pub use ryu_math::*;
