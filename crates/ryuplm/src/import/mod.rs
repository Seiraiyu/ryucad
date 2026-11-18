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
use crate::load::Catalogue;
use crate::source::{Kind, Source};
use mapping::Mapping;
use ryupdm::config::Config;
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

/// Mapping `name` from `src`, or `not_found` naming the mappings there are.
pub fn load(src: &dyn Source, name: &str) -> Result<Mapping> {
    let path = mapping_path(name);
    let bytes = if mapping::name_ok(name) {
        src.read(&path)?
    } else {
        None
    };
    match bytes {
        Some(b) => Mapping::read(name, &b),
        None => {
            let there: Vec<String> = src
                .list(IMPORTS)?
                .into_iter()
                .filter_map(|(n, _)| n.strip_suffix(".json").map(str::to_owned))
                .collect();
            Err(Error::new(
                Code::NotFound,
                format!("no mapping {path}; the mappings are: {}", there.join(", ")),
            )
            .with("import", name)
            .with("imports", there))
        }
    }
}

/// `import_mapping_invalid` for each file under `imports/` that isn't a
/// mapping, doesn't read, or names what the catalogue doesn't declare.
pub fn judge(src: &dyn Source, cat: &Catalogue, config: Option<&Config>) -> Vec<Error> {
    let mut out = Vec::new();
    for (file, kind) in src.list(IMPORTS).unwrap_or_default() {
        let path = format!("{IMPORTS}/{file}");
        let name = match (kind, name_of(&path)) {
            (Kind::File, Some(n)) => n.to_owned(),
            _ => {
                out.push(Error::new(Code::ImportMappingInvalid, format!("{path} isn't a mapping file: imports/<name>.json, the name 1 to 63 of a-z, 0-9, _ and -")).with("path", &path));
                continue;
            }
        };
        match src
            .read(&path)
            .ok()
            .flatten()
            .map(|b| Mapping::read(&name, &b))
        {
            Some(Ok(m)) => {
                if let Some(c) = config {
                    out.extend(m.validate(c, &cat.manifest));
                }
            }
            Some(Err(e)) => out.push(e),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::proto::Request;
    use crate::server::tests::{ask, catalogue};
    use serde_json::json;

    #[test]
    fn a_bad_mapping_fails_check_and_a_good_one_is_tracked() {
        let (_d, root) = catalogue();
        std::fs::create_dir_all(root.join("imports")).unwrap();
        let good = crate::import::mapping::tests::erp();
        // erp() names class "fastener" and "machined": declare them.
        for c in ["fastener", "machined"] {
            ask(
                &root,
                Request::SettingsEdit {
                    change: serde_json::from_value(json!({"class": c})).unwrap(),
                    expected_revision: None,
                },
            )
            .unwrap();
        }
        ask(
            &root,
            Request::SettingsEdit {
                change: serde_json::from_value(
                    json!({"property": "thread", "declaration": {"type": "string"}}),
                )
                .unwrap(),
                expected_revision: None,
            },
        )
        .unwrap();
        std::fs::write(
            root.join("imports/erp.json"),
            crate::canonical::to_string(&good),
        )
        .unwrap();
        let ok = ask(&root, Request::Check { within: vec![] }).unwrap();
        assert_eq!(ok["errors"], 0, "{ok}");
        let info = ask(&root, Request::ProjectInfo {}).unwrap();
        std::fs::write(root.join("imports/bad.json"), "{\"format\": 1}").unwrap();
        let info2 = ask(&root, Request::ProjectInfo {}).unwrap();
        assert_ne!(info["revision"], info2["revision"], "imports/ is tracked");
        let e = ask(&root, Request::Check { within: vec![] }).unwrap_err();
        let r = &e.details["report"];
        assert_eq!(r["faults"][0]["code"], "import_mapping_invalid");
        assert_eq!(r["faults"][0]["details"]["path"], "imports/bad.json");
    }
}
