//! Test helpers behind the `testing` feature: an ODB++ features writer
//! whose output the reader reads back to the same model (`write`), jobs
//! broken on purpose (`broken`), the corpus manifest (`manifest`), and
//! KiCad's own outputs to compare boards with (`kicad`).

mod broken;
mod kicad;
mod manifest;
mod write;

pub use broken::{Breakage, Files, broken, folder, tgz};
pub use kicad::{
    KICAD_REPORT_CAP, TOLERANCE, compare_islands_with_d356, compare_with_drc, compare_with_kicad,
    d356_records, drc_unconnected,
};
pub use manifest::{Declared, check_copper, check_manifest, copper_summary, manifest, summary};
pub use write::{canonical, deg, mm, symbol_name, um, write_features};
