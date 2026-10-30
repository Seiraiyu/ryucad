//! `usdz`: a package of one layer, as usdc, in a zip archive the usdz
//! specification describes: stored (never compressed), each file's data
//! on a 64-byte boundary, the root layer first. The time in the archive
//! is the DOS epoch, so the same layer and name always give the same
//! bytes.

use super::layer::Layer;
use super::usdc;
use crate::zip::{Entry, write_stored};
use std::io::{self, Write};

/// Where the usdz specification puts each file's data.
pub const ALIGN: usize = 64;

/// Writes `layer` as a usdz package whose root layer is `<stem>.usdc`.
pub fn write(layer: &Layer, stem: &str, out: &mut dyn Write) -> io::Result<()> {
    let bytes = usdc::to_bytes(layer)?;
    let name = format!("{stem}.usdc");
    write_stored(
        &[Entry {
            name: &name,
            data: &bytes,
        }],
        ALIGN,
        out,
    )
}
