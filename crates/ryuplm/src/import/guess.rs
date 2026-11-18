//! `import guess` (design 734): a mapping proposed from a source's header
//! and first rows, by column name and spelling only. No vendor's layout
//! is known here: the spellings are generic words.

use super::mapping::{Bom, Field, Mapping, Numbers, Reader, ReaderKind};
use super::rows::Table;
use super::text::Encoding;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryupdm::config::Config;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// Generic spellings, normalized (lower case, letters and digits only).
const SPELLINGS: &[(&str, &[&str])] = &[
    ("key", &["id", "itemid", "key", "partid"]),
    (
        "number",
        &[
            "partnumber",
            "pn",
            "partno",
            "number",
            "itemnumber",
            "partnum",
        ],
    ),
    ("name", &["name", "title", "shortdescription"]),
    ("description", &["description", "desc", "longdescription"]),
    ("unit", &["unit", "uom", "unitofmeasure"]),
    ("make_buy", &["makebuy", "makeorbuy", "procurement"]),
    ("bom.quantity", &["quantity", "qty", "count"]),
    ("bom.level", &["level", "lvl", "bomlevel"]),
    (
        "bom.parent",
        &["parent", "parentnumber", "parentpart", "parentid"],
    ),
    ("bom.find", &["find", "findnumber", "findno", "position"]),
    (
        "bom.refdes",
        &[
            "refdes",
            "designator",
            "designators",
            "reference",
            "references",
        ],
    ),
    (
        "aml.manufacturer",
        &["manufacturer", "mfr", "mfg", "manufacturername"],
    ),
    (
        "aml.mpn",
        &[
            "mpn",
            "manufacturerpartnumber",
            "mfrpartnumber",
            "mfrpn",
            "mfgpartnumber",
        ],
    ),
];

pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn edits(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(ca != cb))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Whether column name `n` spells `s`: equal, or one edit apart when both
/// have five letters or more.
fn spells(n: &str, s: &str, exact: bool) -> bool {
    n == s || (!exact && n.chars().count() >= 5 && s.chars().count() >= 5 && edits(n, s) <= 1)
}

/// The column's role and AML group: a name with a trailing number
/// (`Manufacturer 2`) is that group, 1-based in the name.
fn role(column: &str, config: &Config, exact: bool) -> Option<(String, usize)> {
    let n = norm(column);
    let digits = n.len() - n.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (base, group) = match n[n.len() - digits..].parse::<usize>() {
        Ok(g) if digits > 0 && (1..=10).contains(&g) => (&n[..n.len() - digits], g - 1),
        _ => (n.as_str(), 0),
    };
    for (r, list) in SPELLINGS {
        let b = if r.starts_with("aml.") {
            base
        } else {
            n.as_str()
        };
        if list.iter().any(|s| spells(b, s, exact)) {
            return Some((
                (*r).to_owned(),
                if r.starts_with("aml.") { group } else { 0 },
            ));
        }
    }
    for (p, _) in config.decls() {
        if spells(&n, &norm(&p), exact) {
            return Some((format!("properties.{p}"), 0));
        }
    }
    None
}

/// A tree reader's column as guess reads it (design 935): its last name,
/// and the AML group its nearest index gives (`/alts/1/mpn` is `mpn` in
/// group 1, `Alt[2]/MPN` is `MPN` in group 1).
fn leaf(column: &str, kind: ReaderKind) -> (String, Option<usize>) {
    let mut name = column.to_owned();
    let mut group = None;
    for step in column.split('/').filter(|s| !s.is_empty()) {
        match kind {
            ReaderKind::Json => match step.parse::<usize>() {
                Ok(i) => group = Some(i),
                Err(_) => name = step.replace("~1", "/").replace("~0", "~"),
            },
            _ => {
                let s = step.trim_start_matches('@');
                match s.strip_suffix(']').and_then(|x| x.split_once('[')) {
                    Some((n, i)) => {
                        name = n.to_owned();
                        group = i.parse::<usize>().ok().and_then(|i| i.checked_sub(1));
                    }
                    None => name = s.to_owned(),
                }
            }
        }
    }
    (name, group)
}

/// The mapping proposed for `table`, and each column's verdict.
pub fn propose(
    name: &str,
    table: &Table,
    reader: Reader,
    config: &Config,
) -> (Mapping, Vec<Value>) {
    let kind = reader.kind;
    let mut given: BTreeMap<String, String> = BTreeMap::new();
    let mut verdicts: Vec<Value> = table.columns.iter().map(|c| json!({"column": c})).collect();
    for exact in [true, false] {
        for (i, c) in table.columns.iter().enumerate() {
            if verdicts[i].get("target").is_some() {
                continue;
            }
            let found = if kind.tree() {
                let (n, g) = leaf(c, kind);
                role(&n, config, exact).map(|(r, g0)| {
                    let g = if r.starts_with("aml.") {
                        g.unwrap_or(g0)
                    } else {
                        0
                    };
                    (r, g)
                })
            } else {
                role(c, config, exact)
            };
            let Some((r, g)) = found.filter(|(_, g)| *g < 10) else {
                continue;
            };
            let t = match r.strip_prefix("aml.") {
                Some(part) => format!("aml.{g}.{part}"),
                None => r,
            };
            if let Some(other) = given.get(&t) {
                verdicts[i]["why"] = json!(format!("would be {t}, already given to {other:?}"));
                continue;
            }
            given.insert(t.clone(), c.clone());
            verdicts[i]["target"] = json!(t);
            verdicts[i]["why"] = json!(if exact {
                "its name"
            } else {
                "its spelling, one letter off"
            });
        }
    }
    let key = given
        .get("key")
        .or_else(|| given.get("number"))
        .or_else(|| given.get("aml.0.mpn"))
        .cloned()
        .unwrap_or_else(|| table.columns.first().cloned().unwrap_or_default());
    let mut fields = BTreeMap::new();
    for (t, c) in &given {
        if t == "key" || t.starts_with("bom.") {
            continue;
        }
        let mut f = Field::from(c);
        f.trim = true;
        fields.insert(t.clone(), f);
    }
    let bom = given.get("bom.quantity").map(|q| Bom {
        parent: given.get("bom.parent").cloned(),
        level: if given.contains_key("bom.parent") {
            None
        } else {
            given.get("bom.level").cloned()
        },
        root: None,
        quantity: q.clone(),
        find: given.get("bom.find").cloned(),
        refdes: given.get("bom.refdes").cloned(),
    });
    let numbers = if fields.contains_key("number") {
        Numbers::Keep
    } else {
        Numbers::Assign(None)
    };
    let m = Mapping {
        name: name.to_owned(),
        reader,
        key,
        fields,
        class: Vec::new(),
        bom,
        history: None,
        numbers,
    };
    (m, verdicts)
}

/// `import.guess`: the proposal for the file at `file`, saved as
/// `imports/<save>.json` when asked.
pub fn answer(
    session: &mut Session,
    file: &Path,
    sheet: Option<String>,
    delimiter: Option<String>,
    encoding: Option<String>,
    records: Option<String>,
    save: Option<String>,
) -> Result<Value> {
    let bytes = super::read_file(file)?;
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (kind, lines) = match ext.as_str() {
        "xlsx" => (ReaderKind::Xlsx, false),
        "json" => (ReaderKind::Json, false),
        "jsonl" | "ndjson" => (ReaderKind::Json, true),
        "xml" => (ReaderKind::Xml, false),
        _ => (ReaderKind::Csv, false),
    };
    let usage = |m: &str| Error::new(Code::Usage, m);
    if kind.tree() && (sheet.is_some() || delimiter.is_some() || encoding.is_some()) {
        return Err(usage(
            "--sheet, --delimiter and --encoding are for CSV and XLSX files",
        ));
    }
    if !kind.tree() && records.is_some() {
        return Err(usage("--records is for JSON and XML files"));
    }
    let encoding = encoding
        .map(|e| {
            Encoding::parse(&e).ok_or_else(|| {
                usage("--encoding is utf-8, utf-16le, utf-16be, latin1 or windows-1252")
            })
        })
        .transpose()?;
    let delimiter = delimiter
        .map(|d| {
            let d = if d == "\\t" || d == "tab" {
                "\t".to_owned()
            } else {
                d
            };
            let mut cs = d.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => Ok(c),
                _ => Err(usage("--delimiter is one character (or tab)")),
            }
        })
        .transpose()?;
    let mut reader = Reader {
        kind,
        file: (!ext.is_empty()).then(|| format!("*.{ext}")),
        encoding: None,
        delimiter: None,
        sheet,
        header_row: 1,
        records: None,
        children: None,
        lines: false,
    };
    let mut sheets = Value::Null;
    match kind {
        ReaderKind::Csv => {
            let enc = encoding.unwrap_or_else(|| super::text::sniff(&bytes));
            let text = super::text::decode(&bytes, enc).map_err(Error::from)?;
            reader.encoding = Some(enc);
            reader.delimiter = Some(delimiter.unwrap_or_else(|| super::csv::sniff(&text)));
        }
        ReaderKind::Xlsx => {
            sheets = json!(super::xlsx::sheets(&bytes).map_err(Error::from)?);
        }
        ReaderKind::Json => {
            if lines && records.is_some() {
                return Err(usage(
                    "--records is for JSON and XML documents; JSON Lines' records are its lines",
                ));
            }
            let (r, c) =
                super::json::survey(&bytes, lines, records.as_deref()).map_err(Error::from)?;
            reader.records = (!lines).then_some(r);
            reader.children = c;
            reader.lines = lines;
        }
        ReaderKind::Xml => {
            if let Some(r) = &records
                && !super::xml::path_ok(r, true)
            {
                return Err(usage(
                    "--records for XML is the elements' path from the root by local name: /Export/Items/Item",
                ));
            }
            let (r, c) = super::xml::survey(&bytes, records.as_deref()).map_err(Error::from)?;
            reader.records = Some(r);
            reader.children = c;
        }
    }
    let table = super::read_table(&bytes, &reader).map_err(Error::from)?;
    let config = Config::read_in(&session.snapshot(), crate::catalogue::NAME)?;
    let name = save.clone().unwrap_or_else(|| "proposed".to_owned());
    let (m, columns) = propose(&name, &table, reader, &config);
    let sample: Vec<Value> = table
        .rows
        .iter()
        .take(5)
        .map(|r| json!({"row": r.at, "cells": table.cells(r)}))
        .collect();
    let mut todo = Vec::new();
    if m.bom
        .as_ref()
        .is_some_and(|b| b.parent.is_none() && b.level.is_none())
    {
        todo.push("bom: no parent or level column; set bom.root to the number of the item this is the BOM of");
    }
    let mut v = json!({"mapping": m.to_json(), "columns": columns, "rows": table.rows.len(), "sample": sample, "todo": todo});
    if !sheets.is_null() {
        v["sheets"] = sheets;
    }
    if let Some(name) = save {
        if !super::mapping::name_ok(&name) {
            return Err(usage(
                "--save names a mapping: 1 to 63 of a-z, 0-9, _ and -",
            ));
        }
        let path = super::mapping_path(&name);
        if session.files().contains_key(&path) {
            return Err(Error::new(
                Code::Conflict,
                format!("{path} exists; edit it, or save under another name"),
            )
            .with("path", &path));
        }
        if !todo.is_empty() {
            return Err(Error::new(
                Code::InvalidParam,
                format!(
                    "the proposal is incomplete ({}); save it after fixing that",
                    todo.join("; ")
                ),
            )
            .with("todo", &todo));
        }
        crate::write::commit(
            session,
            &format!("saving mapping {name}"),
            crate::write::Writes(vec![(path.clone(), Some(m.to_text()))]),
        )?;
        v["saved"] = json!(path);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::rows::Table;

    fn table(header: &[&str]) -> Table {
        Table::from_raw(
            vec![(1, header.iter().map(|s| (*s).to_owned()).collect())],
            1,
        )
        .unwrap()
    }

    fn csv() -> Reader {
        Reader {
            kind: ReaderKind::Csv,
            file: None,
            encoding: None,
            delimiter: Some(','),
            sheet: None,
            header_row: 1,
            records: None,
            children: None,
            lines: false,
        }
    }

    #[test]
    fn columns_are_matched_by_name_and_spelling() {
        let config = Config::from_json_in(&json!({"format": 5, "kind": "ryupdm.config", "properties": {"finish": {"type": "string"}}}), "ryuplm").unwrap();
        let t = table(&[
            "Item ID",
            "Part No.",
            "Descripton",
            "Qty",
            "Parent",
            "Manufacturer 1",
            "Mfr. Part Number 1",
            "Manufacturer 2",
            "MPN 2",
            "Finish",
            "Colour",
        ]);
        let (m, cols) = propose("x", &t, csv(), &config);
        assert_eq!(m.key, "Item ID");
        let targets: Vec<Option<&str>> = cols.iter().map(|c| c["target"].as_str()).collect();
        assert_eq!(
            targets,
            [
                Some("key"),
                Some("number"),
                Some("description"),
                Some("bom.quantity"),
                Some("bom.parent"),
                Some("aml.0.manufacturer"),
                Some("aml.0.mpn"),
                Some("aml.1.manufacturer"),
                Some("aml.1.mpn"),
                Some("properties.finish"),
                None
            ]
        );
        assert_eq!(cols[2]["why"], "its spelling, one letter off");
        assert_eq!(m.numbers, Numbers::Keep);
        assert!(m.fields["number"].trim);
        let b = m.bom.clone().unwrap();
        assert_eq!(
            (b.parent.as_deref(), b.quantity.as_str()),
            (Some("Parent"), "Qty")
        );
        // The proposal is itself a valid mapping file.
        Mapping::read("x", m.to_text().as_bytes()).unwrap();
    }

    #[test]
    fn without_a_number_column_numbers_are_assigned() {
        let config =
            Config::from_json_in(&json!({"format": 5, "kind": "ryupdm.config"}), "ryuplm").unwrap();
        let (m, _) = propose(
            "x",
            &table(&[
                "Comment",
                "Designator",
                "Quantity",
                "Manufacturer Part Number 1",
            ]),
            csv(),
            &config,
        );
        assert_eq!(m.numbers, Numbers::Assign(None));
        assert_eq!(m.key, "Manufacturer Part Number 1");
    }

    #[test]
    fn tree_columns_are_named_by_their_last_step() {
        assert_eq!(
            leaf("/alts/1/mpn", ReaderKind::Json),
            ("mpn".to_owned(), Some(1))
        );
        assert_eq!(leaf("/a~1b", ReaderKind::Json), ("a/b".to_owned(), None));
        assert_eq!(
            leaf("Alt[2]/MPN", ReaderKind::Xml),
            ("MPN".to_owned(), Some(1))
        );
        assert_eq!(leaf("@id", ReaderKind::Xml), ("id".to_owned(), None));
        assert_eq!(
            leaf("#level", ReaderKind::Json),
            ("#level".to_owned(), None)
        );
        let config =
            Config::from_json_in(&json!({"format": 5, "kind": "ryupdm.config"}), "ryuplm").unwrap();
        let t = Table::from_records(vec![crate::import::rows::Leaves {
            place: "/items/0".into(),
            cells: vec![
                ("#level".into(), "0".into()),
                ("/id".into(), "1".into()),
                ("/pn".into(), "A".into()),
                ("/qty".into(), "1".into()),
                ("/sources/0/mpn".into(), "M".into()),
                ("/sources/1/manufacturer".into(), "Acme".into()),
                ("/sources/12/mpn".into(), "Z".into()),
            ],
        }])
        .unwrap();
        let mut r = csv();
        r.kind = ReaderKind::Json;
        r.delimiter = None;
        r.records = Some("/items".into());
        r.children = Some("/parts".into());
        let (m, cols) = propose("t", &t, r, &config);
        let targets: Vec<Option<&str>> = cols.iter().map(|c| c["target"].as_str()).collect();
        assert_eq!(
            targets,
            [
                Some("bom.level"),
                Some("key"),
                Some("number"),
                Some("bom.quantity"),
                Some("aml.0.mpn"),
                Some("aml.1.manufacturer"),
                None
            ]
        );
        assert_eq!(m.bom.clone().unwrap().level.as_deref(), Some("#level"));
        Mapping::read("t", m.to_text().as_bytes()).unwrap();
    }

    #[test]
    fn json_and_xml_files_get_tree_proposals() {
        use crate::proto::Request;
        use crate::server::tests::{ask, catalogue};
        let (_d, root) = catalogue();
        let guess = |file: &std::path::Path, records: Option<&str>, sheet: Option<&str>| {
            ask(
                &root,
                Request::ImportGuess {
                    file: file.display().to_string(),
                    sheet: sheet.map(str::to_owned),
                    delimiter: None,
                    encoding: None,
                    records: records.map(str::to_owned),
                    save: None,
                },
            )
        };
        let j = root.join("export.json");
        std::fs::write(
            &j,
            r#"{"items": [{"id": "1", "pn": "A", "sources": [{"mpn": "M1"}, {"mpn": "M2"}]}, {"id": "2", "pn": "B"}]}"#,
        )
        .unwrap();
        let v = guess(&j, None, None).unwrap();
        assert_eq!(v["mapping"]["format"], 2);
        assert_eq!(
            v["mapping"]["reader"],
            json!({"type": "json", "file": "*.json", "records": "/items"})
        );
        assert_eq!(v["mapping"]["key"], "/id");
        assert_eq!(
            v["mapping"]["fields"]["aml.1.mpn"]["from"],
            "/sources/1/mpn"
        );
        let l = root.join("dump.jsonl");
        std::fs::write(&l, "{\"id\": \"1\", \"pn\": \"A\"}\n").unwrap();
        assert_eq!(
            guess(&l, None, None).unwrap()["mapping"]["reader"],
            json!({"type": "json", "file": "*.jsonl", "lines": true})
        );
        let x = root.join("export.xml");
        std::fs::write(
            &x,
            r#"<E><I id="1"><PN>A</PN></I><I id="2"><PN>B</PN></I></E>"#,
        )
        .unwrap();
        let v = guess(&x, None, None).unwrap();
        assert_eq!(
            v["mapping"]["reader"],
            json!({"type": "xml", "file": "*.xml", "records": "/E/I"})
        );
        assert_eq!(v["mapping"]["key"], "@id");
        assert_eq!(
            guess(&x, Some("/E"), None).unwrap()["mapping"]["reader"]["records"],
            "/E"
        );
        assert_eq!(guess(&x, None, Some("S")).unwrap_err().code, Code::Usage);
        let c = root.join("items.csv");
        std::fs::write(&c, "ID,PN\n1,A\n").unwrap();
        assert_eq!(guess(&c, Some("/x"), None).unwrap_err().code, Code::Usage);
        // XML records are an absolute path, as the saved mapping must say,
        // and JSON Lines' records are its lines.
        let e = guess(&x, Some("E/I"), None).unwrap_err();
        assert_eq!(e.code, Code::Usage, "{e:?}");
        assert_eq!(guess(&l, Some("/x"), None).unwrap_err().code, Code::Usage);
    }
}
