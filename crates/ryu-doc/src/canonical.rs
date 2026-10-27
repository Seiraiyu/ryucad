//! Canonical JSON: the one way ryucad writes a file, so that the same
//! document is always the same bytes and diffs show only real changes.
//!
//! UTF-8, LF, two-space indent, object keys sorted by bytes, one scalar per
//! line (arrays expanded too), a trailing newline. Numbers use the shortest
//! form that round-trips the f64, and `-0` is written as `0`.

use crate::error::{Code, Error, Result};
use serde_json::{Number, Value};

/// The canonical text of `v`.
pub fn to_string(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v, 0);
    out.push('\n');
    out
}

/// Parses JSON text; an error names the line and column.
pub fn parse(s: &str) -> Result<Value> {
    serde_json::from_str(s).map_err(|e| {
        Error::new(Code::InvalidJson, format!("invalid JSON: {e}"))
            .with("line", e.line())
            .with("column", e.column())
    })
}

/// Whether `s` is already in canonical form.
pub fn is_canonical(s: &str) -> bool {
    parse(s).is_ok_and(|v| to_string(&v) == s)
}

fn write(out: &mut String, v: &Value, depth: usize) {
    match v {
        Value::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(out, depth + 1);
                write(out, item, depth + 1);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            // serde_json's map is a BTreeMap: keys come out in byte order.
            out.push_str("{\n");
            for (i, (k, item)) in map.iter().enumerate() {
                indent(out, depth + 1);
                out.push_str(&Value::String(k.clone()).to_string());
                out.push_str(": ");
                write(out, item, depth + 1);
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push('}');
        }
        Value::Number(n) => out.push_str(&number(n)),
        // Scalars, `[]` and `{}`.
        _ => out.push_str(&v.to_string()),
    }
}

fn number(n: &Number) -> String {
    match n.as_f64() {
        Some(x) if x == 0.0 && n.is_f64() && x.is_sign_negative() => "0".to_owned(),
        _ => n.to_string(),
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn layout() {
        let v = json!({"b": [1, 2.5, "x"], "a": {"z": null, "y": true}, "e": [], "f": {}});
        assert_eq!(
            to_string(&v),
            "{\n  \"a\": {\n    \"y\": true,\n    \"z\": null\n  },\n  \"b\": [\n    1,\n    2.5,\n    \"x\"\n  ],\n  \"e\": [],\n  \"f\": {}\n}\n"
        );
    }

    #[test]
    fn numbers() {
        for (v, s) in [
            (json!(0.01), "0.01\n"),
            (json!(1e-7), "1e-7\n"),
            (json!(1.0), "1.0\n"),
            (json!(-0.0), "0\n"),
            (json!(-3), "-3\n"),
            (json!(u64::MAX), "18446744073709551615\n"),
        ] {
            assert_eq!(to_string(&v), s);
        }
    }

    #[test]
    fn keys_sort_by_bytes_and_strings_escape() {
        let v = json!({"b": "tab\there", "B": "é", "a\"q": 1});
        assert_eq!(
            to_string(&v),
            "{\n  \"B\": \"é\",\n  \"a\\\"q\": 1,\n  \"b\": \"tab\\there\"\n}\n"
        );
    }

    #[test]
    fn canonical_detection() {
        assert!(is_canonical("{\n  \"a\": 1\n}\n"));
        assert!(!is_canonical("{\"a\": 1}"));
        assert!(!is_canonical("{\n  \"a\": 1\n}"));
        assert!(!is_canonical("not json"));
    }

    #[test]
    fn parse_errors_name_the_position() {
        let e = parse("{\n  \"a\": ,\n}").unwrap_err();
        assert_eq!(e.code, Code::InvalidJson);
        assert_eq!(e.details["line"], 2);
    }
}
