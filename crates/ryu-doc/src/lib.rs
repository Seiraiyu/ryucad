//! The vocabulary every ryu product's files are written in: canonical
//! JSON, IDs and project IDs, sources (a folder, or a revision's files),
//! quantities with units, expressions, and the one error type with its
//! stable codes. Product neutral: ryucad's file kinds are in `ryucad-doc`,
//! which re-exports these under the paths ryucad has always used.

pub mod canonical;
pub mod error;
pub mod expr;
pub mod id;
pub mod source;
pub mod units;
