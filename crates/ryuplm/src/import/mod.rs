//! Imports (design 734): readers that turn a file into rows, knowing only
//! its format ([`csv`], [`xlsx`]); mapping files that say what the rows
//! mean ([`mapping`], committed as `imports/<name>.json`); and the
//! commands on top: `import guess`, `import check` and `import run`. No
//! shape is built in for any vendor: a new layout is a new mapping file,
//! and a reader is added only for a new file format.

pub mod csv;
pub mod guess;
pub mod mapping;
pub mod preview;
pub mod records;
pub mod rows;
pub mod run;
pub mod text;
pub mod transform;
pub mod xlsx;

use crate::error::{Code, Error, Result};
use std::path::Path;

/// The folder of mapping files, below the catalogue.
pub const IMPORTS: &str = "imports";
/// The largest source file read, and the largest XLSX part unpacked.
pub const MAX_SOURCE: u64 = 256 << 20;
/// The most rows and columns a source may have.
pub const MAX_ROWS: usize = 2_000_000;
pub const MAX_COLUMNS: usize = 16_384;

/// Where mapping `name` lives: `imports/<name>.json`.
pub fn mapping_path(name: &str) -> String {
    format!("{IMPORTS}/{name}.json")
}

/// The mapping a path names, when it is one: `imports/<name>.json`.
pub fn name_of(path: &str) -> Option<&str> {
    let n = path
        .strip_prefix(IMPORTS)?
        .strip_prefix('/')?
        .strip_suffix(".json")?;
    mapping::name_ok(n).then_some(n)
}

/// The bytes of the source file at `path`, refused past [`MAX_SOURCE`].
pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, &e))?;
    if meta.len() > MAX_SOURCE {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{} is larger than 256 MiB", path.display()),
        )
        .with("path", path));
    }
    std::fs::read(path).map_err(|e| Error::io(path, &e))
}

/// The table `bytes` hold, read as `reader` says.
pub fn read_table(
    bytes: &[u8],
    reader: &mapping::Reader,
) -> std::result::Result<rows::Table, rows::Refusal> {
    let raw = match reader.kind {
        mapping::ReaderKind::Csv => {
            let enc = reader.encoding.unwrap_or_else(|| text::sniff(bytes));
            let s = text::decode(bytes, enc)?;
            let d = reader.delimiter.unwrap_or_else(|| csv::sniff(&s));
            csv::parse(&s, d)?
        }
        mapping::ReaderKind::Xlsx => xlsx::read(bytes, reader.sheet.as_deref())?,
    };
    rows::Table::from_raw(raw, reader.header_row)
}
