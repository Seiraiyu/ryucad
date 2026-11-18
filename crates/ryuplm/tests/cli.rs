//! The CLI as an agent sees it: run the binary, parse stdout as JSON.
//!
//! Every scenario runs twice, in two identical projects: once in process
//! (`RYUPLM_NO_DAEMON=1`) and once through the project's daemon. Both must
//! give the same JSON and the same exit code.

use assert_cmd::Command;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Replaces the project root with `<root>` in every string of `v`.
fn replace_root(v: &mut Value, forms: &[&str]) {
    match v {
        Value::String(s) => {
            for f in forms {
                *s = s.replace(f, "<root>");
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| replace_root(x, forms)),
        Value::Object(o) => o.values_mut().for_each(|x| replace_root(x, forms)),
        _ => {}
    }
}

/// `ryuplm` with the variables that keep a test's daemons and settings
/// its own.
fn ryuplm(run: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ryuplm").unwrap();
    cmd.env("RYUPLM_RUNTIME_DIR", run)
        .env("RYUPLM_IDLE_TIMEOUT", "60")
        .env("RYUPLM_CONFIG_DIR", run.with_file_name("config"))
        .env("RYUPLM_ID_SEED", "cli")
        .env_remove("RYUPLM_NO_DAEMON")
        .env_remove("RYUPLM_PARTICIPANT");
    cmd
}

fn parse(out: std::process::Output, root: &Path) -> (i32, Value) {
    let text = String::from_utf8(out.stdout).unwrap();
    let mut json =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {text}"));
    let root = root.to_str().unwrap();
    let plain = root.strip_prefix(r"\\?\").unwrap_or(root);
    replace_root(&mut json, &[root, plain]);
    (out.status.code().unwrap(), json)
}

/// Two identical catalogue folders `p`: one served in process, one by a
/// daemon.
struct Pair {
    _dir: tempfile::TempDir,
    direct: PathBuf,
    daemon: PathBuf,
    run: PathBuf,
}

impl Pair {
    fn new() -> Pair {
        let dir = tempfile::tempdir().unwrap();
        let (direct, daemon) = (dir.path().join("direct"), dir.path().join("daemon"));
        for d in [&direct, &daemon] {
            fs::create_dir_all(d.join("p")).unwrap();
        }
        Pair {
            direct: direct.canonicalize().unwrap(),
            daemon: daemon.canonicalize().unwrap(),
            run: dir.path().join("run"),
            _dir: dir,
        }
    }

    /// A pair of catalogues: one made in process, its manifest (with its
    /// random project ID) copied to the other.
    fn project() -> Pair {
        let p = Pair::new();
        let (code, v) = p.run_in(&p.direct, &["init"]);
        assert_eq!(code, 0, "{v}");
        for f in ["ryuplm.json", ".gitattributes"] {
            fs::copy(p.direct.join("p").join(f), p.daemon.join("p").join(f)).unwrap();
        }
        p
    }

    fn run_in(&self, top: &Path, args: &[&str]) -> (i32, Value) {
        let mut cmd = ryuplm(&self.run);
        cmd.current_dir(top.join("p")).args(args);
        if top == self.direct {
            cmd.env("RYUPLM_NO_DAEMON", "1");
        }
        parse(cmd.output().unwrap(), top)
    }

    /// Runs `args` both ways and returns the common answer.
    fn both(&self, args: &[&str]) -> (i32, Value) {
        let a = self.run_in(&self.direct, args);
        let b = self.run_in(&self.daemon, args);
        assert_eq!(a, b, "{args:?}: in process vs daemon");
        a
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, v) = self.both(args);
        assert_eq!((code, &v["ok"]), (0, &json!(true)), "{args:?}: {v}");
        assert_eq!(v["warnings"], json!([]), "{args:?}");
        v["result"].clone()
    }

    fn fails(&self, args: &[&str], code: &str, exit: i32) -> Value {
        let (c, v) = self.both(args);
        assert_eq!(
            (c, v["error"]["code"].as_str().unwrap()),
            (exit, code),
            "{args:?}: {v}"
        );
        v["error"].clone()
    }

    fn files(&self, top: &Path) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else {
                    let rel = p.strip_prefix(root).unwrap().to_string_lossy();
                    out.push(rel.replace('\\', "/"));
                }
            }
        }
        let mut out = Vec::new();
        walk(&top.join("p"), &top.join("p"), &mut out);
        out.sort();
        out
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        let _ = ryuplm(&self.run)
            .current_dir(self.daemon.join("p"))
            .args(["daemon", "stop"])
            .output();
    }
}

#[test]
fn version_names_ryuplm() {
    let out = Command::cargo_bin("ryuplm")
        .unwrap()
        .arg("version")
        .output()
        .unwrap();
    let (code, v) = parse(out, Path::new("/nowhere"));
    assert_eq!((code, &v["ok"]), (0, &json!(true)));
    for k in ["ryuplm", "commit", "target", "protocol"] {
        assert!(v["result"].get(k).is_some(), "{k}");
    }
    let bad = Command::cargo_bin("ryuplm")
        .unwrap()
        .arg("no-such-command")
        .output()
        .unwrap();
    let (code, v) = parse(bad, Path::new("/nowhere"));
    assert_eq!((code, &v["error"]["code"]), (2, &json!("usage")));
}

#[test]
fn items_from_the_cli() {
    let p = Pair::project();
    p.ok(&[
        "settings",
        "edit",
        "--scheme",
        "mech",
        "--pattern",
        "MECH-\\d{5}",
        "--next-template",
        "MECH-{seq:5}",
    ]);
    p.ok(&["settings", "edit", "--class", "machined"]);
    let a = p.ok(&[
        "item", "create", "--next", "--class", "machined", "--name", "Bracket",
    ]);
    assert_eq!(a["item"]["number"], "MECH-00001");
    let id = a["item"]["id"].as_str().unwrap().to_owned();
    let b = p.ok(&[
        "item",
        "create",
        "100-0042",
        "--name",
        "Legacy",
        "--aml",
        r#"[{"manufacturer":"Acme","mpn":"X","status":"nrnd"}]"#,
    ]);
    assert_eq!(b["item"]["aml"][0]["status"], "nrnd");
    p.ok(&[
        "settings",
        "edit",
        "--property",
        "finish",
        "--declaration",
        r#"{"type":"string"}"#,
    ]);
    p.ok(&[
        "item",
        "edit",
        &id,
        "--description",
        "6061",
        "--property",
        "finish=\"anodized\"",
    ]);
    let shown = p.ok(&["item", "show", "MECH-00001"]);
    assert_eq!(shown["item"]["description"], "6061");
    p.ok(&["item", "renumber", &id, "MECH-00100"]);
    let listed = p.ok(&["item", "list", "--number", "MECH-00001"]);
    assert_eq!(listed["items"][0]["id"], id);
    // The legacy number warns; check passes with the warning listed.
    let (code, v) = p.both(&["check"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["warnings"], 1);
    assert_eq!(v["result"]["faults"][0]["code"], "part_number_unschemed");
    p.fails(&["item", "create", "100-0042"], "part_number_duplicate", 1);
    p.fails(&["item", "show", "NOPE"], "item_not_found", 1);
    p.ok(&["item", "delete", "100-0042"]);
    // Both ways wrote the same files.
    assert_eq!(p.files(&p.direct), p.files(&p.daemon));
}

#[test]
fn writes_are_logged_and_undone_through_the_daemon() {
    let p = Pair::project();
    let a = p.run_in(&p.daemon, &["item", "create", "A-1"]);
    assert_eq!(a.0, 0, "{}", a.1);
    let (_, h) = p.run_in(&p.daemon, &["history"]);
    assert_eq!(h["result"]["entries"][0]["method"], "item.create");
    let (code, _) = p.run_in(&p.daemon, &["undo"]);
    assert_eq!(code, 0);
    assert!(!p.daemon.join("p/items").exists());
    let (code, v) = p.run_in(&p.direct, &["undo"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("no_live_session")));
}

/// Every protocol method, as `proto.rs` names them.
fn protocol_methods() -> Vec<String> {
    let src = include_str!("../src/proto.rs");
    let start = src.find("pub enum Request {").unwrap();
    let end = start + src[start..].find("\n}\n").unwrap();
    src[start..end]
        .lines()
        .filter_map(|l| l.strip_prefix("    #[serde(rename = \""))
        .map(|s| s[..s.find('"').unwrap()].to_owned())
        .collect()
}

#[test]
fn every_protocol_method_has_a_command() {
    let commands: &[(&str, &[&str])] = &[
        ("project.info", &["info"]),
        ("item.create", &["item", "create"]),
        ("item.show", &["item", "show"]),
        ("item.list", &["item", "list"]),
        ("item.edit", &["item", "edit"]),
        ("item.renumber", &["item", "renumber"]),
        ("item.delete", &["item", "delete"]),
        ("item.link", &["item", "link"]),
        ("item.unlink", &["item", "unlink"]),
        ("bom.add", &["bom", "add"]),
        ("bom.edit", &["bom", "edit"]),
        ("bom.remove", &["bom", "remove"]),
        ("bom.get", &["bom", "show"]),
        ("bom.diff", &["bom", "diff"]),
        ("bom.where_used", &["where-used"]),
        ("settings.show", &["settings", "show"]),
        ("settings.edit", &["settings", "edit"]),
        ("check", &["check"]),
        ("release.propose", &["release", "propose"]),
        ("release.obsolete", &["release", "obsolete"]),
        ("release.list", &["release", "list"]),
        ("release.check", &["release", "check"]),
        ("release.sync", &["release", "sync"]),
        ("diff", &["diff"]),
        ("activity.log", &["history"]),
        ("undo", &["undo"]),
        ("redo", &["redo"]),
        ("session.hello", &["participants"]),
        ("participant.list", &["participants"]),
        ("subscribe", &["watch"]),
        ("daemon.status", &["daemon", "status"]),
        ("daemon.shutdown", &["daemon", "stop"]),
    ];
    let methods = protocol_methods();
    assert_eq!(methods.len(), commands.len(), "{methods:?}");
    for m in methods {
        let (_, args) = commands
            .iter()
            .find(|(n, _)| *n == m)
            .unwrap_or_else(|| panic!("{m} has no command"));
        let out = Command::cargo_bin("ryuplm")
            .unwrap()
            .args(*args)
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success(), "ryuplm {args:?} --help");
    }
}

#[test]
fn git_install_registers_the_driver_locally() {
    let p = Pair::project();
    let root = p.direct.join("p");
    std::process::Command::new("git")
        .current_dir(&root)
        .args(["init", "-q"])
        .status()
        .unwrap();
    let (code, v) = p.run_in(&p.direct, &["git", "install"]);
    assert_eq!(code, 0, "{v}");
    let cfg = &v["result"]["config"];
    assert!(
        cfg["merge.ryuplm.driver"]
            .as_str()
            .unwrap()
            .ends_with("git merge-driver %O %A %B %P"),
        "{cfg}"
    );
    assert!(
        cfg["diff.ryuplm.textconv"]
            .as_str()
            .unwrap()
            .ends_with("git textconv"),
        "{cfg}"
    );
}

/// `errors` lists every code with its exit code, title and declared keys,
/// from the build itself: no catalogue, the same both ways (design 698).
#[test]
fn errors_lists_every_code_and_its_keys() {
    let dir = tempfile::tempdir().unwrap();
    let run = |no_daemon: bool, args: &[&str]| {
        let mut cmd = ryuplm(&dir.path().join("run"));
        cmd.current_dir(dir.path()).args(args);
        if no_daemon {
            cmd.env("RYUPLM_NO_DAEMON", "1");
        }
        parse(cmd.output().unwrap(), dir.path())
    };
    let (code, v) = run(false, &["errors"]);
    assert_eq!(run(true, &["errors"]), (code, v.clone()));
    assert_eq!(code, 0, "{v}");
    let codes = v["result"]["codes"].as_array().unwrap();
    let item = codes
        .iter()
        .find(|c| c["code"] == "item_not_found")
        .unwrap();
    assert_eq!(
        (&item["declared"], &item["keys"][0]["name"]),
        (&json!(true), &json!("item"))
    );
    let (code, bad) = run(false, &["errors", "nope"]);
    assert_eq!((code, &bad["error"]["code"]), (1, &json!("invalid_param")));
}

/// A ryucad project at `p/mech` in both catalogues of the pair: an
/// assembly `box` placing four `screw`s. Its project ID.
fn mech(p: &Pair) -> &'static str {
    const PID: &str = "33333333333333333333333333333333";
    for top in [&p.direct, &p.daemon] {
        let dir = top.join("p/mech");
        let put = |rel: &str, v: Value| {
            let f = dir.join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, ryuplm::canonical::to_string(&v)).unwrap();
        };
        put(
            "ryucad.json",
            json!({"default_unit": "mm", "format": 2, "kind": "ryucad.project", "project": PID}),
        );
        put(
            "parts/screw/part.json",
            json!({"features": ["b"], "format": 1, "kind": "ryucad.part"}),
        );
        put(
            "parts/screw/features/b.json",
            json!({"format": 1, "kind": "ryucad.feature", "type": "box", "x": "1", "y": "1", "z": "1"}),
        );
        let ids: Vec<String> = (0..4).map(|k| format!("s{k}")).collect();
        put(
            "parts/box/part.json",
            json!({"features": ids, "format": 2, "kind": "ryucad.part", "part_kind": "assembly"}),
        );
        for id in &ids {
            put(
                &format!("parts/box/features/{id}.json"),
                json!({"format": 4, "kind": "ryucad.feature", "type": "instance", "part": "screw",
                       "rev": "working", "pose": {"origin_m": [0, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}}),
            );
        }
    }
    PID
}

#[test]
fn links_boms_and_where_used_from_the_cli() {
    let p = Pair::project();
    let pid = mech(&p);
    for n in ["BOX-1", "SCREW-1", "GLUE-1"] {
        p.ok(&["item", "create", n]);
    }
    // By the project's folder, then by ID.
    let linked = p.ok(&["item", "link", "BOX-1", "ryucad", "mech", "box"]);
    assert_eq!(linked["item"]["links"][0]["project"], pid);
    p.ok(&["item", "link", "SCREW-1", "ryucad", pid, "screw"]);
    p.ok(&["bom", "add", "BOX-1", "GLUE-1", "0.5", "--unit", "kg"]);
    let twice = p.ok(&["bom", "add", "BOX-1", "SCREW-1", "4", "--find", "10"]);
    let line = twice["line"].as_str().unwrap().to_owned();
    let bom = p.ok(&["bom", "show", "BOX-1"]);
    let from: Vec<&str> = bom["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["from"].as_str().unwrap())
        .collect();
    assert_eq!(from, ["own", "own", "ryucad"], "{bom}");
    assert_eq!(bom["lines"][2]["number"], "SCREW-1");
    assert_eq!(bom["lines"][2]["quantity"], "4");
    assert_eq!(bom["findings"][0]["code"], "bom_line_twice");
    let check = p.ok(&["check"]);
    assert!(
        check["faults"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "bom_line_twice"),
        "{check}"
    );
    // A loop, and a delete that would leave a line naming nothing, are refused.
    p.fails(&["bom", "add", "SCREW-1", "BOX-1", "1"], "bom_cycle", 1);
    p.fails(&["item", "delete", "GLUE-1"], "bom_item_missing", 1);
    // The line the CAD now models comes off, and the warning with it.
    p.ok(&["bom", "remove", "BOX-1", &line]);
    assert_eq!(p.ok(&["bom", "show", "BOX-1"])["findings"], json!([]));
    // A link no folder holds is a warning, never a failure.
    p.ok(&[
        "item",
        "link",
        "GLUE-1",
        "ryueda",
        "44444444444444444444444444444444",
        "main",
    ]);
    let check = p.ok(&["check"]);
    assert!(
        check["faults"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "item_link_unresolved"),
        "{check}"
    );
    let used = p.ok(&["where-used", "SCREW-1", "--working", "--no-dependents"]);
    let via: Vec<&str> = used["working"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["via"].as_str().unwrap())
        .collect();
    assert_eq!(via, ["ryucad"], "{used}");
    p.ok(&["item", "unlink", "GLUE-1", "ryueda"]);
}
