//! The copper engine: exact distances between shapes, and an incremental
//! spatial index over a generic layout.
//!
//! Product-neutral: it knows `ryu-board`'s geometry and shapes and nothing
//! of ODB++, projects or sessions. ryudfm builds its layout from a board's
//! model; ryueda will build one from its own boards.

pub mod index;
pub mod layout;
pub mod measure;
pub mod poly;
pub mod surd;
pub mod tree;
pub mod wide;
