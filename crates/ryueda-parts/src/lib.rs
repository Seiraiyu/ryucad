//! ryueda's part data (design 347): a fixed vocabulary of typed parameters
//! read from the text library components keep, categories, records of what
//! a source says of a part with the terms it may be kept under, the
//! interface every source of parts implements, and the cache records live
//! in on each machine, never in a project.
//!
//! It knows no project, session or file of ryueda's: ryueda builds a
//! library's records from its files and hands them to the `library`
//! source.

pub use ryu_doc::error;

pub mod dec;
pub mod param;
pub mod category;
pub mod record;
pub mod query;
pub mod source;
pub mod library;
