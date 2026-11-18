//! The tree readers (design 935) against 0.C's proven path: a nested BOM
//! in JSON and in XML reads as the indented BOM it is, and a CSV, a JSON
//! and an XML file saying the same read the same.

use ryuplm::import::{self, mapping::Mapping, records};
use ryuplm::units::Unit;
use serde_json::json;

fn config() -> ryupdm::config::Config {
    ryupdm::config::Config::from_json_in(&json!({"format": 5, "kind": "ryupdm.config"}), "ryuplm")
        .unwrap()
}

#[test]
fn nested_boms_read_as_indented_ones() {
    let json_text = r#"{"parts": [{"pn": "A", "children": [{"pn": "B", "qty": 2, "children": [{"pn": "C", "qty": 1}]}, {"pn": "D", "qty": 4}]}, {"pn": "E"}]}"#;
    let jm = Mapping::from_json(
        "nested",
        &json!({"format": 2, "kind": "ryuplm.import",
        "reader": {"type": "json", "records": "/parts", "children": "/children"},
        "key": "/pn", "fields": {"number": {"from": "/pn"}},
        "bom": {"level": "#level", "quantity": "/qty"}}),
    )
    .unwrap();
    let xml_text = r#"<Parts><Part pn="A"><Uses><Part pn="B" qty="2"><Uses><Part pn="C" qty="1"/></Uses></Part><Part pn="D" qty="4"/></Uses></Part><Part pn="E"/></Parts>"#;
    let xm = Mapping::from_json(
        "nested",
        &json!({"format": 2, "kind": "ryuplm.import",
        "reader": {"type": "xml", "records": "/Parts/Part", "children": "Uses/Part"},
        "key": "@pn", "fields": {"number": {"from": "@pn"}},
        "bom": {"level": "#level", "quantity": "@qty"}}),
    )
    .unwrap();
    for (text, m) in [(json_text, jm), (xml_text, xm)] {
        let table = import::read_table(text.as_bytes(), &m.reader).unwrap();
        let (rows, refusals) = records::map(&table, &m, &config(), Unit::Mm).unwrap();
        assert!(refusals.is_empty(), "{refusals:?}");
        let lines: Vec<(&str, Option<&str>, Option<&str>)> = rows
            .iter()
            .map(|r| {
                (
                    r.record.key.as_str(),
                    r.record.parent.as_deref(),
                    r.record.quantity.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                ("A", None, None),
                ("B", Some("A"), Some("2")),
                ("C", Some("B"), Some("1")),
                ("D", Some("A"), Some("4")),
                ("E", None, None)
            ],
            "{:?}",
            m.reader.kind
        );
    }
}

type Seen = (String, String, Vec<String>, Option<String>, Option<String>);

/// What a mapped row says, apart from its row number and the columns'
/// names (a CSV's `Note` is a JSON reader's `/Note`).
fn seen(rows: &[records::Mapped]) -> Vec<Seen> {
    rows.iter()
        .map(|r| {
            (
                r.record.key.clone(),
                serde_json::to_string(&r.values).unwrap(),
                r.extra.values().cloned().collect(),
                r.record.parent.clone(),
                r.record.quantity.clone(),
            )
        })
        .collect()
}

#[test]
fn csv_json_and_xml_saying_the_same_read_the_same() {
    use ryuplm::import::{mapping::ReaderKind, rows::Table};
    let (files, m, csv, _) = ryuplm::testing::import_source("trees", 120);
    let config = ryupdm::config::Config::read_in(&files, "ryuplm").unwrap();
    let unit = ryuplm::load::Catalogue::load(&files)
        .unwrap()
        .manifest
        .default_unit;
    let t = Table::from_raw(import::csv::parse(&csv, ',').unwrap(), 1).unwrap();
    let (want, want_refused) = records::map(&t, &m, &config, unit).unwrap();
    let (json_text, xml_text) = ryuplm::testing::csv_as_trees(&csv);
    for (kind, text) in [(ReaderKind::Json, json_text), (ReaderKind::Xml, xml_text)] {
        let tm = ryuplm::testing::tree_mapping(&m, kind);
        let table = import::read_table(text.as_bytes(), &tm.reader).unwrap();
        assert_eq!(table.rows.len(), t.rows.len(), "{kind:?}");
        let (got, refused) = records::map(&table, &tm, &config, unit).unwrap();
        assert_eq!(seen(&got), seen(&want), "{kind:?}");
        assert_eq!(refused.len(), want_refused.len(), "{kind:?}");
    }
}
