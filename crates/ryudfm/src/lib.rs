//! ryudfm: design for manufacturing of printed circuit boards.
//!
//! `odb` reads ODB++ jobs (folders and archives, versions 7.0 to 8.1) into
//! `ryu_board`'s model, with faults and warnings for what it couldn't
//! read. The project, the CLI and the daemon come with plan 0.C.

pub use ryu_doc::{canonical, error, id, source, units};

pub mod boards;
pub mod codes;
pub mod doc;
pub mod import;
pub mod input;
pub mod inspect;
pub mod odb;
pub mod project;
pub mod quantity;
pub mod render;
pub mod session;
#[cfg(feature = "testing")]
pub mod testing;
