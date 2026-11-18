//! Imports (design 734): readers that turn a file into rows, knowing only
//! its format ([`csv`], [`xlsx`]); mapping files that say what the rows
//! mean ([`mapping`], committed as `imports/<name>.json`); and the
//! commands on top: `import guess`, `import check` and `import run`. No
//! shape is built in for any vendor: a new layout is a new mapping file,
//! and a reader is added only for a new file format.

pub mod csv;
pub mod guess;
pub mod json;
pub mod mapping;
pub mod preview;
pub mod records;
pub mod rows;
pub mod run;
pub mod text;
pub mod transform;
pub mod xlsx;
pub mod xml;

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
        mapping::ReaderKind::Xlsx => {
            let (raw, d1904) = xlsx::read_book(bytes, reader.sheet.as_deref())?;
            let mut t = rows::Table::from_raw(raw, reader.header_row)?;
            t.date1904 = d1904;
            return Ok(t);
        }
        mapping::ReaderKind::Json => {
            return rows::Table::from_records(json::read(
                bytes,
                json::Shape {
                    records: reader.records.as_deref().unwrap_or(""),
                    children: reader.children.as_deref(),
                    lines: reader.lines,
                },
            )?);
        }
        mapping::ReaderKind::Xml => {
            return rows::Table::from_records(xml::read(
                bytes,
                reader.records.as_deref().unwrap_or("/"),
                reader.children.as_deref(),
            )?);
        }
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

/// What `import check` and `import run` start from: the mapping (judged
/// against the catalogue's declarations), its file guard, the settings,
/// and the source's rows through the mapping, refusals apart.
pub struct Prepared {
    pub mapping: Mapping,
    pub config: Config,
    pub table: rows::Table,
    pub rows: Vec<records::Mapped>,
    pub refusals: Vec<rows::Refusal>,
}

pub fn prepare(session: &crate::session::Session, name: &str, file: &Path) -> Result<Prepared> {
    let src = session.snapshot();
    let mapping = load(&src, name)?;
    let config = Config::read_in(&src, crate::catalogue::NAME)?;
    if let Some(e) = mapping
        .validate(&config, &session.catalogue().manifest)
        .into_iter()
        .next()
    {
        return Err(e);
    }
    if let Some(p) = &mapping.reader.file {
        let base = file.file_name().and_then(|f| f.to_str()).unwrap_or("");
        if !mapping::file_matches(p, base) {
            return Err(Error::new(
                Code::InvalidParam,
                format!("{base:?} doesn't match the mapping's file pattern {p:?}"),
            )
            .with("import", name)
            .with("path", file));
        }
    }
    let bytes = read_file(file)?;
    let table = read_table(&bytes, &mapping.reader).map_err(|r| refused(name, file, vec![r]))?;
    let (rows, mut refusals) = records::map(
        &table,
        &mapping,
        &config,
        session.catalogue().manifest.default_unit,
    )?;
    table.place(&mut refusals);
    Ok(Prepared {
        mapping,
        config,
        table,
        rows,
        refusals,
    })
}

/// The error for rows that don't read: `invalid_document`, every refusal
/// in `details.refusals`.
pub fn refused(name: &str, file: &Path, refusals: Vec<rows::Refusal>) -> Error {
    let first = refusals
        .first()
        .map(|r| r.message.clone())
        .unwrap_or_default();
    Error::new(
        Code::InvalidDocument,
        format!(
            "{} row(s) of {} don't read through {name}: {first}",
            refusals.len(),
            file.display()
        ),
    )
    .with("import", name)
    .with("path", file)
    .with("refusals", &refusals)
}

/// `import.list`: each mapping file, its reader and key, and how many
/// items carry its provenance; a mapping that doesn't read, with why.
pub fn list(session: &crate::session::Session) -> Result<serde_json::Value> {
    use serde_json::json;
    let src = session.snapshot();
    let mut out = Vec::new();
    for (file, _) in src.list(IMPORTS)? {
        let path = format!("{IMPORTS}/{file}");
        let Some(name) = name_of(&path) else { continue };
        let items = session
            .catalogue()
            .items
            .values()
            .filter(|i| i.source.contains_key(name))
            .count();
        let v = match load(&src, name) {
            Ok(m) => {
                json!({"name": name, "path": path, "reader": m.to_json()["reader"], "key": m.key, "bom": m.bom.is_some(), "items": items})
            }
            Err(e) => json!({"name": name, "path": path, "error": e, "items": items}),
        };
        out.push(v);
    }
    Ok(json!({"imports": out}))
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
    #[test]
    fn list_counts_each_mappings_items() {
        let (_d, root) = catalogue();
        let v = ask(&root, Request::ImportList {}).unwrap();
        assert_eq!(v, json!({"imports": []}));
    }

    #[test]
    fn a_json_refusal_says_where_its_record_is() {
        let (_d, root) = catalogue();
        std::fs::create_dir_all(root.join("imports")).unwrap();
        std::fs::write(
            root.join("imports/items.json"),
            crate::canonical::to_string(&json!({"format": 2, "kind": "ryuplm.import",
                "reader": {"type": "json", "records": "/items"},
                "key": "/id", "fields": {"number": {"from": "/pn"}}, "numbers": "keep"})),
        )
        .unwrap();
        let src = root.join("export.json");
        std::fs::write(
            &src,
            r#"{"items": [{"id": "1", "pn": "P-1"}, {"id": "2", "pn": ""}]}"#,
        )
        .unwrap();
        let check = || {
            ask(
                &root,
                Request::ImportCheck {
                    name: "items".into(),
                    file: src.display().to_string(),
                    limit: None,
                },
            )
            .unwrap_err()
        };
        let e = check();
        let r = &e.details["report"]["refusals"][0];
        assert_eq!(
            (r["row"].clone(), r["column"].clone(), r["place"].clone()),
            (json!(2), json!("/pn"), json!("/items/1")),
            "{e:?}"
        );
        // A record that doesn't parse is refused at the record.
        std::fs::write(&src, r#"{"items": [{"id": "1",}]}"#).unwrap();
        let e = check();
        assert_eq!(e.code, crate::error::Code::InvalidDocument, "{e:?}");
        assert_eq!(e.details["refusals"][0]["place"], "/items/0");
    }
}
