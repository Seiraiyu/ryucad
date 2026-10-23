//! ryucad's file format: a project folder of canonical JSON files (the
//! manifest, parts, their parameters and features), loaded and hashed the
//! same from a folder or from a Git revision, and the vocabulary those
//! files are written in: units, expressions and the one error type.
//!
//! It never calls the kernel's modeling: ryucad rebuilds, and ryupdm
//! manages releases, on top of it. Both re-use its paths (`ryucad::doc`
//! is this crate's `doc`).

pub mod doc;
pub mod error;
pub mod expr;
pub mod units;
