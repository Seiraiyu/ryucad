//! Test helpers behind the `testing` feature: an ODB++ features writer
//! whose output the reader reads back to the same model (`write`), jobs
//! broken on purpose (`broken`), the corpus manifest (`manifest`), and
//! KiCad's own outputs to compare boards with (`kicad`).

mod manifest;
mod write;

pub use manifest::{Declared, check_manifest, manifest, summary};
pub use write::{canonical, deg, mm, symbol_name, um, write_features};
