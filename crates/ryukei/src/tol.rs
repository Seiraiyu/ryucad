//! The kernel-wide tolerance model. Model space is SI meters.
//!
//! These are fixed for every body. Unit conversion and models larger than
//! the size box are ryucad's job at its boundary.

/// Two points closer than this are the same point (10 nm).
pub const LINEAR_RESOLUTION: f64 = 1e-8;

/// Two directions closer than this (in radians) are parallel.
pub const ANGULAR_RESOLUTION: f64 = 1e-11;

/// All model geometry lies in `[-SIZE_BOX_HALF, SIZE_BOX_HALF]` on each axis.
pub const SIZE_BOX_HALF: f64 = 500.0;
