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
