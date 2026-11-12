//! `diff` (design 518, plan 4.B): what changed between two revisions of a
//! project, as values. Files are compared as JSON, as written, so a file
//! that doesn't read shows as `invalid` and broken values show as they
//! are (ryucad's rule); each side's loaded project resolves names (a
//! pin's net, a component's designator). Every entry of a block carries
//! its file and its reference path ([`crate::schem::ref_path`]):
//! `<block>/<component>`, `<block>/<net>`, `<block>/port:<id>`,
//! `<block>/instance:<id>`, the paths stage 3's scene keys its items by.

use crate::error::Result;
use crate::project::{ERC, LINT, MANIFEST, PARTS, Project, STYLE};
use ryu_session::fresh::Files;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// One side of a diff: the revision as asked (`working` for the files on
/// disk), the commit it named (none for the working files), its files as
/// written and the project read from them.
pub struct Side<'a> {
    pub rev: &'a str,
    pub commit: Option<&'a str>,
    pub files: &'a Files,
    pub project: &'a Project,
}

impl Side<'_> {
    /// `{rev, commit}`, as an answer names a side.
    pub fn to_json(&self) -> Value {
        json!({"rev": self.rev, "commit": self.commit})
    }
}

/// What `diff` lists besides the structure.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Only this design's netlist.
    pub design: Option<String>,
    /// Only this block (by ID).
    pub block: Option<String>,
    /// Each design flattened and compared (`--netlist`).
    pub netlist: bool,
    /// Each block's layout by key (`--layout`), not only its counts.
    pub layout: bool,
    /// Each library object by member (`--library`), not only counts.
    pub library: bool,
    /// What is the same too.
    pub all: bool,
}

/// A file's JSON, or `None` when it is missing or doesn't parse.
pub(crate) fn json_at(files: &Files, path: &str) -> Option<Value> {
    let text = std::str::from_utf8(files.get(path)?).ok()?;
    crate::canonical::parse(text).ok()
}

/// `added`, `removed`, `changed` or `same`, for something on either side.
pub(crate) fn status(from: bool, to: bool, changed: bool) -> &'static str {
    match (from, to) {
        (false, true) => "added",
        (true, false) => "removed",
        _ if changed => "changed",
        _ => "same",
    }
}

/// A key as a JSON pointer's segment.
fn escape(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

/// Every leaf that differs between `a` and `b`, by JSON pointer: objects
/// by key, anything else (arrays included) as one value. A side the leaf
/// is missing from is left out.
pub(crate) fn changes(at: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Value>) {
    match (a, b) {
        (Some(Value::Object(x)), Some(Value::Object(y))) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                changes(&format!("{at}/{}", escape(k)), x.get(k), y.get(k), out);
            }
        }
        (a, b) if a == b => {}
        (a, b) => {
            let mut c = json!({ "at": at });
            if let Some(a) = a {
                c["from"] = a.clone();
            }
            if let Some(b) = b {
                c["to"] = b.clone();
            }
            out.push(c);
        }
    }
}

/// One file on both sides: `added`, `removed`, `changed` with its
/// `changes`, `same` (with `rewritten` when only its bytes differ), or
/// `invalid` when it is there but doesn't parse (`invalid` names the
/// sides).
pub(crate) fn file(from: &Files, to: &Files, path: &str) -> Value {
    let (ba, bb) = (from.get(path), to.get(path));
    let (ja, jb) = (json_at(from, path), json_at(to, path));
    let mut v = json!({"file": path, "status": status(ba.is_some(), bb.is_some(), ba != bb)});
    let bad: Vec<&str> = [
        ("from", ba.is_some() && ja.is_none()),
        ("to", bb.is_some() && jb.is_none()),
    ]
    .into_iter()
    .filter_map(|(s, b)| b.then_some(s))
    .collect();
    if !bad.is_empty() {
        v["status"] = json!("invalid");
        v["invalid"] = json!(bad);
        return v;
    }
    if ba.is_some() && bb.is_some() && ba != bb {
        let mut c = Vec::new();
        changes("", ja.as_ref(), jb.as_ref(), &mut c);
        if c.is_empty() {
            v["status"] = json!("same");
            v["rewritten"] = json!(true);
        } else {
            v["changes"] = json!(c);
        }
    }
    v
}

/// The IDs (file stems) of the `.json` files directly in `dir` on either
/// side, sorted.
pub(crate) fn ids_in(from: &Files, to: &Files, dir: &str) -> BTreeSet<String> {
    let prefix = format!("{dir}/");
    from.keys()
        .chain(to.keys())
        .filter_map(|p| p.strip_prefix(&prefix))
        .filter(|rest| !rest.contains('/'))
        .filter_map(|rest| rest.strip_suffix(".json"))
        .map(str::to_owned)
        .collect()
}

/// A file's `name`, from the newer side where it reads.
fn name_of(from: &Files, to: &Files, path: &str) -> Option<String> {
    [json_at(to, path), json_at(from, path)]
        .into_iter()
        .flatten()
        .find_map(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
}

/// Each file of `dir` (`designs`, `rails`, `classes`) by ID, with its
/// name; the same left out unless `all`.
pub(crate) fn folder(from: &Files, to: &Files, dir: &str, all: bool) -> Vec<Value> {
    ids_in(from, to, dir)
        .into_iter()
        .filter_map(|id| {
            let path = format!("{dir}/{id}.json");
            let mut e = file(from, to, &path);
            if e["status"] == "same" && !all {
                return None;
            }
            e["id"] = json!(id);
            if let Some(n) = name_of(from, to, &path) {
                e["name"] = json!(n);
            }
            Some(e)
        })
        .collect()
}

/// The root files and the project's folders: `manifest`, `erc`, `lint`,
/// `parts`, `style` (each its file's entry, left out when the same unless
/// `all`), `designs`, `rails` and `classes`.
pub(crate) fn root(from: &Files, to: &Files, all: bool, out: &mut Map<String, Value>) {
    for (key, path) in [
        ("manifest", MANIFEST),
        ("erc", ERC),
        ("lint", LINT),
        ("parts", PARTS),
        ("style", STYLE),
    ] {
        let e = file(from, to, path);
        if e["status"] != "same" || all {
            out.insert(key.into(), e);
        }
    }
    for dir in [
        crate::conn::DESIGNS,
        crate::conn::RAILS,
        crate::conn::CLASSES,
    ] {
        out.insert(dir.into(), json!(folder(from, to, dir, all)));
    }
}

/// `diff` of `from` and `to`: the root files and folders, each block
/// (Task 2), its layout (Task 3), the library (Task 5) and with
/// `opts.netlist` each design's netlist (Task 4).
pub fn diff(from: &Side, to: &Side, opts: &Options) -> Result<Value> {
    let mut out = Map::new();
    out.insert("from".into(), from.to_json());
    out.insert("to".into(), to.to_json());
    root(from.files, to.files, opts.all, &mut out);
    Ok(Value::Object(out))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn files(list: &[(&str, &str)]) -> Files {
        list.iter()
            .map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec().into()))
            .collect()
    }

    #[test]
    fn changes_are_by_pointer_and_arrays_are_one_value() {
        let mut out = Vec::new();
        let a = json!({"a": 1, "b": {"c": [1, 2], "d/e": "x"}, "gone": true});
        let b = json!({"a": 1, "b": {"c": [1, 3], "d/e": "y"}, "new": 2});
        changes("", Some(&a), Some(&b), &mut out);
        assert_eq!(
            out,
            [
                json!({"at": "/b/c", "from": [1, 2], "to": [1, 3]}),
                json!({"at": "/b/d~1e", "from": "x", "to": "y"}),
                json!({"at": "/gone", "from": true}),
                json!({"at": "/new", "to": 2}),
            ]
        );
    }

    #[test]
    fn a_file_is_added_removed_changed_same_or_invalid() {
        let a = files(&[
            (
                "erc.json",
                r#"{"format": 1, "kind": "ryueda.erc", "severity": {"pin_conflict": "warning"}}"#,
            ),
            ("lint.json", "{"),
            ("style.json", "{\"format\": 1, \"kind\": \"ryueda.style\"}"),
        ]);
        let b = files(&[
            (
                "erc.json",
                r#"{"format": 1, "kind": "ryueda.erc", "severity": {"pin_conflict": "error"}}"#,
            ),
            ("lint.json", r#"{"format": 1, "kind": "ryueda.lint"}"#),
            ("parts.json", r#"{"format": 1, "kind": "ryueda.parts"}"#),
            (
                "style.json",
                "{\n  \"format\": 1,\n  \"kind\": \"ryueda.style\"\n}\n",
            ),
        ]);
        let e = file(&a, &b, "erc.json");
        assert_eq!(e["status"], "changed");
        assert_eq!(e["changes"][0]["at"], "/severity/pin_conflict");
        assert_eq!(e["changes"][0]["to"], "error");
        assert_eq!(file(&a, &b, "lint.json")["invalid"], json!(["from"]));
        assert_eq!(file(&a, &b, "parts.json")["status"], "added");
        let s = file(&a, &b, "style.json");
        assert_eq!(
            (&s["status"], &s["rewritten"]),
            (&json!("same"), &json!(true))
        );
        assert_eq!(file(&a, &b, "nope.json")["status"], "same");
    }

    #[test]
    fn a_folder_lists_its_files_by_id_with_names() {
        let a = files(&[(
            "rails/gnd.json",
            r#"{"format": 1, "kind": "ryueda.rail", "name": "GND"}"#,
        )]);
        let b = files(&[
            (
                "rails/gnd.json",
                r#"{"format": 1, "kind": "ryueda.rail", "name": "0V"}"#,
            ),
            (
                "rails/v3v3.json",
                r#"{"format": 1, "kind": "ryueda.rail", "name": "3V3"}"#,
            ),
            ("rails/deep/x.json", "{}"),
        ]);
        let l = folder(&a, &b, "rails", false);
        assert_eq!(l.len(), 2, "{l:?}");
        assert_eq!(
            (&l[0]["id"], &l[0]["name"], &l[0]["status"]),
            (&json!("gnd"), &json!("0V"), &json!("changed"))
        );
        assert_eq!(
            (&l[1]["id"], &l[1]["status"]),
            (&json!("v3v3"), &json!("added"))
        );
        assert!(folder(&b, &b, "rails", false).is_empty());
        assert_eq!(
            folder(&b, &b, "rails", true).len(),
            2,
            "--all lists the same too"
        );
    }
}
