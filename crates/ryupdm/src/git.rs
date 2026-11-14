//! What Git's merge and diff do with ryupdm's files, for every product's
//! driver: which kinds merge whole, which pointers are one value, and
//! (below) their textconv lines. Settings (`ryupdm.json`) and properties
//! merge by the generic per-key walk: `approvals.rules` is an array, so
//! one value; `approvals.keys` per key; each property per key.

use crate::record::{OBSOLETE_KIND, RELEASE_KIND};
use serde_json::{Map, Value};

/// Whether a file of `kind` merges as one value: release and obsolete
/// records. Two different records at one path are one conflict.
pub fn whole(kind: &str) -> bool {
    matches!(kind, RELEASE_KIND | OBSOLETE_KIND)
}

/// A lock's label is one value: two branches locking one label at
/// different commits conflict there (`/projects/<ID>/labels/<label>`),
/// not in its commit and content apart.
pub fn lock_atom(at: &str) -> bool {
    let seg: Vec<&str> = at.split('/').collect();
    matches!(seg.as_slice(), ["", "projects", _, "labels", _])
}

/// A leaf as text: a string bare, anything else as JSON.
fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// The textconv lines of ryupdm's file kinds, for a product's textconv:
/// takes what it writes out of `rest` (the file's members besides
/// `kind`, `format` and `type`) and pushes lines sorted with the rest
/// onto `lines`, results onto `last`. `flatten` is the product's own
/// line writer, for members a newer file holds. False, touching nothing,
/// for a kind that isn't ryupdm's settings or records.
pub fn textconv(
    kind: &str,
    rest: &mut Map<String, Value>,
    lines: &mut Vec<String>,
    last: &mut Vec<String>,
    flatten: &dyn Fn(&str, &Value, &mut Vec<String>),
) -> bool {
    match kind {
        crate::config::KIND => {
            if let Some(Value::Object(decls)) = rest.remove("properties") {
                for (name, d) in &decls {
                    lines.push(format!("properties.{name} = {}", declaration(d)));
                }
            }
            if let Some(Value::Object(mut a)) = rest.remove("approvals") {
                if let Some(Value::Array(rules)) = a.remove("rules") {
                    for (i, r) in rules.iter().enumerate() {
                        let line = crate::approvals::Rule::from_json(r, "")
                            .map_or_else(|_| r.to_string(), |r| r.text());
                        lines.push(format!("approvals.rules.{i} = {line}"));
                    }
                }
                if let Some(Value::Object(keys)) = a.remove("keys") {
                    for (id, k) in &keys {
                        // A key, or `{key, revoked}` for one revoked on a day.
                        let line = match (k.as_str(), &k["key"], &k["revoked"]) {
                            (Some(hex), _, _) => hex.to_owned(),
                            (None, Value::String(hex), Value::String(day)) => {
                                format!("{hex} (revoked {day})")
                            }
                            _ => k.to_string(),
                        };
                        lines.push(format!("approvals.keys.{id} = {line}"));
                    }
                }
                // Anything else a newer file holds, field by field.
                if !a.is_empty() {
                    flatten("approvals", &Value::Object(a), lines);
                }
            }
            true
        }
        RELEASE_KIND | OBSOLETE_KIND => {
            if let Some(by) = rest.remove("released_by") {
                lines.push(format!("released_by = {}", released_by(&by)));
            }
            if let Some(Value::Object(results)) = rest.remove("results") {
                for (f, hash) in &results {
                    last.push(format!("results.{f} = {}", scalar(hash)));
                }
            }
            true
        }
        _ => false,
    }
}

/// A declared property: `enum anodized, bare (required, revising)`,
/// `quantity length`, `string`.
fn declaration(d: &Value) -> String {
    let mut s = d.get("type").map(scalar).unwrap_or_default();
    if let Some(Value::Array(values)) = d.get("values") {
        let list: Vec<String> = values.iter().map(scalar).collect();
        s.push_str(&format!(" {}", list.join(", ")));
    }
    if let Some(dim) = d.get("dim") {
        s.push_str(&format!(" {}", scalar(dim)));
    }
    let flags: Vec<&str> = ["required", "revising"]
        .into_iter()
        .filter(|f| d.get(*f) == Some(&Value::Bool(true)))
        .collect();
    if !flags.is_empty() {
        s.push_str(&format!(" ({})", flags.join(", ")));
    }
    if let Some(o) = d.as_object() {
        for (k, v) in o {
            if !matches!(
                k.as_str(),
                "type" | "values" | "dim" | "required" | "revising"
            ) {
                s.push_str(&format!(" {k}={}", scalar(v)));
            }
        }
    }
    s
}

/// Who wrote a record: `Ada <ada@example.com> as agent`, each part only
/// when it is there.
fn released_by(by: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(n) = by.get("name") {
        parts.push(scalar(n));
    }
    if let Some(e) = by.get("email") {
        parts.push(format!("<{}>", scalar(e)));
    }
    if let Some(p) = by.get("participant") {
        parts.push(format!("as {}", scalar(p)));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_merge_whole_and_a_locks_labels_are_one_value() {
        assert!(whole("ryupdm.release") && whole("ryupdm.obsolete"));
        assert!(!whole("ryupdm.config") && !whole("ryupdm.properties"));
        assert!(lock_atom("/projects/0123/labels/bolt~1A"));
        assert!(!lock_atom("/projects/0123/labels"));
        assert!(!lock_atom("/projects/0123/labels/bolt~1A/commit"));
        assert!(!lock_atom("/dependencies/lib"));
    }

    #[test]
    fn settings_and_records_read_a_line_each() {
        use serde_json::json;
        let flat = |path: &str, v: &Value, out: &mut Vec<String>| out.push(format!("{path} = {v}"));
        let mut rest: Map<String, Value> = json!({
            "properties": {"finish": {"type": "enum", "values": ["anodized", "bare"], "required": true}},
            "approvals": {"rules": [{"count": 1, "from": ["alice", "bob"], "parts": ["*"]}],
                          "keys": {"3f9a": {"key": "ab", "revoked": "2026-11-10"}}},
            "release_branch": "main"
        })
        .as_object()
        .unwrap()
        .clone();
        let (mut lines, mut last) = (Vec::new(), Vec::new());
        assert!(textconv(
            crate::config::KIND,
            &mut rest,
            &mut lines,
            &mut last,
            &flat
        ));
        assert_eq!(
            lines,
            [
                "properties.finish = enum anodized, bare (required)",
                "approvals.rules.0 = 1 of alice, bob for *",
                "approvals.keys.3f9a = ab (revoked 2026-11-10)",
            ]
        );
        assert_eq!(rest.keys().collect::<Vec<_>>(), ["release_branch"]);
        let mut rest: Map<String, Value> = json!({
            "released_by": {"name": "Ada", "email": "ada@example.com", "participant": "agent"},
            "results": {"pad": "c0ffee"}
        })
        .as_object()
        .unwrap()
        .clone();
        let (mut lines, mut last) = (Vec::new(), Vec::new());
        assert!(textconv(
            crate::record::RELEASE_KIND,
            &mut rest,
            &mut lines,
            &mut last,
            &flat
        ));
        assert_eq!(lines, ["released_by = Ada <ada@example.com> as agent"]);
        assert_eq!(last, ["results.pad = c0ffee"]);
        assert!(!textconv(
            "ryucad.part",
            &mut Map::new(),
            &mut lines,
            &mut last,
            &flat
        ));
    }
}
