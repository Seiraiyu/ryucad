//! `diff` (design 518, plan 4.B): what changed between two revisions of a
//! project, as values. Files are compared as JSON, as written, so a file
//! that doesn't read shows as `invalid` and broken values show as they
//! are (ryucad's rule); each side's loaded project resolves names (a
//! pin's net, a component's designator). Every entry of a block carries
//! its file and its reference path ([`crate::schem::ref_path`]):
//! `<block>/<component>`, `<block>/<net>`, `<block>/port:<id>`,
//! `<block>/instance:<id>`, the paths stage 3's scene keys its items by.

use crate::conn::{BLOCKS, Node};
use crate::design::Part;
use crate::error::Result;
use crate::id::Id;
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
    out.insert("blocks".into(), json!(blocks(from, to, opts)));
    Ok(Value::Object(out))
}
/// What a pin or an instance's port connects to, named on one side:
/// `SDA` (a net of the block), `rail GND`, `port SDA`, `nc`, `—` when
/// left unconnected; the raw value when it doesn't read.
fn node_text(p: &Project, block: &Id, v: Option<&Value>) -> String {
    let Some(v) = v else {
        return "—".into();
    };
    let Ok(n) = serde_json::from_value::<Node>(v.clone()) else {
        return v.to_string();
    };
    let b = p.conn.blocks.get(block);
    match n {
        Node::Nc => "nc".into(),
        Node::Net(id) => b
            .and_then(|b| b.nets.get(&id))
            .and_then(|n| n.item.as_ref().ok())
            .map_or_else(|| id.to_string(), |n| n.name.clone()),
        Node::Rail(id) => format!(
            "rail {}",
            p.conn
                .rails
                .get(&id)
                .and_then(|r| r.item.as_ref().ok())
                .map_or_else(|| id.to_string(), |r| r.name.clone())
        ),
        Node::Port(id) => format!(
            "port {}",
            b.and_then(|b| b.block.item.as_ref().ok())
                .and_then(|x| x.ports.get(&id))
                .map_or_else(|| id.to_string(), |x| x.name.clone())
        ),
    }
}

/// `field`'s members (`pins`, `ports`) whose node differs, by key, as
/// `<what> <key>: <from> → <to>` in each side's names.
fn nodes(
    from: &Side,
    to: &Side,
    block: &Id,
    a: &Option<Value>,
    b: &Option<Value>,
    field: &str,
    what: &str,
) -> Vec<Value> {
    let get = |v: &Option<Value>| {
        v.as_ref()
            .and_then(|v| v.get(field))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    let (x, y) = (get(a), get(b));
    let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
    keys.into_iter()
        .filter(|k| x.get(*k) != y.get(*k))
        .map(|k| {
            json!(format!(
                "{what} {k}: {} → {}",
                node_text(from.project, block, x.get(k)),
                node_text(to.project, block, y.get(k))
            ))
        })
        .collect()
}

/// One object of block `block` (a component, net or instance) on both
/// sides, with its reference path and name; `None` when the same unless
/// `all`.
fn object(from: &Side, to: &Side, block: &Id, part: Part, id: &str, all: bool) -> Option<Value> {
    let path = format!("{BLOCKS}/{block}/{}/{id}.json", part.folder());
    let mut e = file(from.files, to.files, &path);
    if e["status"] == "same" && !all {
        return None;
    }
    let key = match part {
        Part::Instance => format!("instance:{id}"),
        _ => id.to_owned(),
    };
    e["id"] = json!(id);
    e["ref"] = json!(crate::schem::ref_path(block, &key));
    let (a, b) = (json_at(from.files, &path), json_at(to.files, &path));
    let field = |f: &str| {
        [&b, &a]
            .into_iter()
            .find_map(|v| v.as_ref()?.get(f)?.as_str().map(str::to_owned))
    };
    match part {
        Part::Component => {
            if let Some(r) = field("refdes") {
                e["name"] = json!(r);
            }
            let pins = nodes(from, to, block, &a, &b, "pins", "pin");
            if !pins.is_empty() {
                e["pins"] = json!(pins);
            }
        }
        Part::Net => {
            if let Some(n) = field("name") {
                e["name"] = json!(n);
            }
        }
        Part::Instance => {
            let ports = nodes(from, to, block, &a, &b, "ports", "port");
            if !ports.is_empty() {
                e["ports"] = json!(ports);
            }
        }
    }
    Some(e)
}

/// `block.json`'s `ports` or `buses` by ID, each `<block>/<kind>:<id>`.
fn members(
    block: &Id,
    a: &Option<Value>,
    b: &Option<Value>,
    field: &str,
    kind: &str,
    all: bool,
) -> Vec<Value> {
    let get = |v: &Option<Value>| {
        v.as_ref()
            .and_then(|v| v.get(field))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    let (x, y) = (get(a), get(b));
    let ids: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
    ids.into_iter()
        .filter_map(|id| {
            let (p, q) = (x.get(id), y.get(id));
            let st = status(p.is_some(), q.is_some(), p != q);
            if st == "same" && !all {
                return None;
            }
            let mut e = json!({"id": id, "ref": format!("{block}/{kind}:{id}"), "status": st});
            if let Some(n) = q.or(p).and_then(|v| v.get("name")).and_then(Value::as_str) {
                e["name"] = json!(n);
            }
            if st == "changed" {
                let mut c = Vec::new();
                changes("", p, q, &mut c);
                e["changes"] = json!(c);
            }
            Some(e)
        })
        .collect()
}

/// The blocks on either side, by ID: every folder under `blocks/`.
fn block_ids(from: &Files, to: &Files) -> BTreeSet<String> {
    let prefix = format!("{BLOCKS}/");
    from.keys()
        .chain(to.keys())
        .filter_map(|p| {
            p.strip_prefix(&prefix)?
                .split_once('/')
                .map(|(b, _)| b.to_owned())
        })
        .collect()
}

/// Each block: `block.json` by pointer (`ports` and `buses` apart, by
/// member), its components, nets and instances by object, and its layout
/// (Task 3). A block that is the same is left out unless `all`.
fn blocks(from: &Side, to: &Side, opts: &Options) -> Vec<Value> {
    let mut out = Vec::new();
    for b in block_ids(from.files, to.files) {
        if opts.block.as_ref().is_some_and(|x| *x != b) {
            continue;
        }
        let Ok(id) = Id::parse(&b, "block") else {
            continue;
        };
        let path = crate::design::block_path(&id);
        let mut e = file(from.files, to.files, &path);
        let (a, z) = (json_at(from.files, &path), json_at(to.files, &path));
        let prefix = format!("{BLOCKS}/{b}/");
        let here = |f: &Files| f.keys().any(|p| p.starts_with(&prefix));
        let (was, is) = (here(from.files), here(to.files));
        e["block"] = json!(b);
        e["ref"] = json!(b);
        if let Some(n) = name_of(from.files, to.files, &path) {
            e["name"] = json!(n);
        }
        let mut any = e["status"] != "same";
        for (key, part) in [
            ("components", Part::Component),
            ("nets", Part::Net),
            ("instances", Part::Instance),
        ] {
            let dir = format!("{BLOCKS}/{b}/{}", part.folder());
            let list: Vec<Value> = ids_in(from.files, to.files, &dir)
                .iter()
                .filter_map(|o| object(from, to, &id, part, o, opts.all))
                .collect();
            any |= list.iter().any(|x| x["status"] != "same");
            e[key] = json!(list);
        }
        for (key, kind) in [("ports", "port"), ("buses", "bus")] {
            let list = members(&id, &a, &z, key, kind, opts.all);
            any |= list.iter().any(|x| x["status"] != "same");
            e[key] = json!(list);
        }
        // The block's file's own changes, less the members listed above.
        if let Some(Value::Array(c)) = e.get_mut("changes") {
            c.retain(|x| {
                let at = x["at"].as_str().unwrap_or_default();
                !at.starts_with("/ports") && !at.starts_with("/buses")
            });
        }
        if e["status"] != "invalid" {
            e["status"] = json!(status(was, is, any));
        }
        if e["status"] == "same" && !opts.all {
            continue;
        }
        out.push(e);
    }
    out
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

    /// A project with its files as a session holds them.
    fn loaded(root: &std::path::Path) -> (Files, Project) {
        let files = crate::session::current(root).unwrap();
        (files, Project::load(root).unwrap())
    }

    #[test]
    fn a_block_lists_its_objects_by_reference_path() {
        use crate::project::ProjectKind;
        use crate::project::tests::{design_in, library_in, manifest};
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        library_in(&board);
        design_in(&board);
        let (fa, pa) = loaded(&board);
        // r1's value and its pin 1 moved to GND; an R2 added; sda renamed.
        let r1 = board.join("blocks/top/components/r1.json");
        let mut c: Value = serde_json::from_slice(&std::fs::read(&r1).unwrap()).unwrap();
        c["value"] = json!("10k");
        c["pins"]["1"] = json!({"rail": "gnd"});
        std::fs::write(&r1, crate::canonical::to_string(&c)).unwrap();
        std::fs::write(
            board.join("blocks/top/components/r2.json"),
            r#"{"format": 1, "kind": "ryueda.component", "of": {"id": "r_100k"}, "refdes": "R2"}"#,
        )
        .unwrap();
        let sda = board.join("blocks/top/nets/sda.json");
        let mut n: Value = serde_json::from_slice(&std::fs::read(&sda).unwrap()).unwrap();
        n["name"] = json!("SDA_3V3");
        std::fs::write(&sda, crate::canonical::to_string(&n)).unwrap();
        let (fb, pb) = loaded(&board);
        let (from, to) = (
            Side {
                rev: "HEAD",
                commit: None,
                files: &fa,
                project: &pa,
            },
            Side {
                rev: "working",
                commit: None,
                files: &fb,
                project: &pb,
            },
        );
        let v = diff(&from, &to, &Options::default()).unwrap();
        let top = &v["blocks"][0];
        assert_eq!(
            (&top["block"], &top["status"]),
            (&json!("top"), &json!("changed"))
        );
        let comps = top["components"].as_array().unwrap();
        let r1 = comps.iter().find(|c| c["id"] == "r1").unwrap();
        assert_eq!(r1["ref"], "top/r1");
        assert_eq!(r1["file"], "blocks/top/components/r1.json");
        assert!(
            r1["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["at"] == "/value")
        );
        assert_eq!(
            r1["pins"],
            json!(["pin 1: SDA → rail GND"]),
            "names as each side had them"
        );
        let r2 = comps.iter().find(|c| c["id"] == "r2").unwrap();
        assert_eq!(
            (&r2["status"], &r2["name"]),
            (&json!("added"), &json!("R2"))
        );
        let nets = top["nets"].as_array().unwrap();
        assert_eq!(nets.len(), 1, "{nets:?}");
        assert_eq!(
            (&nets[0]["ref"], &nets[0]["name"]),
            (&json!("top/sda"), &json!("SDA_3V3"))
        );
        assert_eq!(nets[0]["changes"][0]["at"], "/name");
        // The same files: nothing, unless --all.
        let same = diff(&to, &to, &Options::default()).unwrap();
        assert_eq!(same["blocks"], json!([]));
        let all = diff(
            &to,
            &to,
            &Options {
                all: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(all["blocks"][0]["status"], "same");
    }
}
