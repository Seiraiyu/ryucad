//! The CLI as an agent sees it: run the binary, parse stdout as JSON.
//!
//! Every scenario runs twice, in two identical projects: once in process
//! (`RYUEDA_NO_DAEMON=1`) and once through the project's daemon. Both must
//! give the same JSON and the same exit code.

#[path = "support/breakout.rs"]
mod breakout;

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

/// `ryueda` with the variables that keep a test's daemons and settings
/// its own.
fn ryueda(run: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ryueda").unwrap();
    cmd.env("RYUEDA_RUNTIME_DIR", run)
        .env("RYUEDA_IDLE_TIMEOUT", "60")
        .env("RYUEDA_CONFIG_DIR", run.with_file_name("config"))
        // Random IDs a fixed sequence, the same in process and through a
        // daemon.
        .env("RYUEDA_ID_SEED", "cli")
        .env_remove("RYUEDA_NO_DAEMON")
        .env_remove("RYUEDA_PARTICIPANT");
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

/// Two identical folders, each a repository holding a project `board`
/// and a library project `passives`: one served in process, one by a
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
            fs::create_dir_all(d.join(".git")).unwrap();
            fs::create_dir_all(d.join("board")).unwrap();
            fs::create_dir_all(d.join("passives")).unwrap();
        }
        Pair {
            direct: direct.canonicalize().unwrap(),
            daemon: daemon.canonicalize().unwrap(),
            run: dir.path().join("run"),
            _dir: dir,
        }
    }

    fn run_in(&self, top: &Path, sub: &str, args: &[&str]) -> (i32, Value) {
        let mut cmd = ryueda(&self.run);
        cmd.current_dir(top.join(sub)).args(args);
        if top == self.direct {
            cmd.env("RYUEDA_NO_DAEMON", "1");
        }
        parse(cmd.output().unwrap(), top)
    }

    /// Runs `args` in `sub` both ways and returns the common answer.
    fn both_in(&self, sub: &str, args: &[&str]) -> (i32, Value) {
        let a = self.run_in(&self.direct, sub, args);
        let b = self.run_in(&self.daemon, sub, args);
        assert_eq!(a, b, "{args:?}: in process vs daemon");
        a
    }

    fn ok_in(&self, sub: &str, args: &[&str]) -> Value {
        let (code, v) = self.both_in(sub, args);
        assert_eq!((code, &v["ok"]), (0, &json!(true)), "{args:?}: {v}");
        assert_eq!(v["warnings"], json!([]), "{args:?}");
        v["result"].clone()
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.ok_in("board", args)
    }

    fn fails(&self, args: &[&str], code: &str, exit: i32) -> Value {
        let (c, v) = self.both_in("board", args);
        assert_eq!(
            (c, v["error"]["code"].as_str().unwrap()),
            (exit, code),
            "{args:?}: {v}"
        );
        v["error"].clone()
    }

    fn write(&self, rel: &str, text: &str) {
        for top in [&self.direct, &self.daemon] {
            let p = top.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        for d in ["board", "passives"] {
            let _ = ryueda(&self.run)
                .current_dir(self.daemon.join(d))
                .args(["daemon", "stop"])
                .output();
        }
    }
}

#[test]
fn version_names_ryueda() {
    let out = Command::cargo_bin("ryueda")
        .unwrap()
        .arg("version")
        .output()
        .unwrap();
    let (code, v) = parse(out, Path::new("/nowhere"));
    assert_eq!(
        (code, &v["ok"], &v["warnings"]),
        (0, &json!(true), &json!([]))
    );
    let r = &v["result"];
    for k in ["ryueda", "commit", "target", "protocol"] {
        assert!(r.get(k).is_some(), "{k}: {r}");
    }
    assert!(r.get("ryucad").is_none());
    let flag = Command::cargo_bin("ryueda")
        .unwrap()
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(parse(flag, Path::new("/nowhere")), (0, v));
}

#[test]
fn bad_arguments_are_a_usage_error_in_json() {
    let out = Command::cargo_bin("ryueda")
        .unwrap()
        .arg("no-such-command")
        .output()
        .unwrap();
    let (code, v) = parse(out, Path::new("/nowhere"));
    assert_eq!((code, &v["error"]["code"]), (2, &json!("usage")));
}

#[test]
fn projects_and_library_projects_from_the_cli() {
    let p = Pair::new();
    let made = p.run_in(&p.direct, "board", &["info"]);
    assert_eq!(
        (made.0, &made.1["error"]["code"]),
        (3, &json!("project_not_found"))
    );
    // Project IDs are random: the daemon's projects are copies.
    let (code, v) = p.run_in(&p.direct, "board", &["init"]);
    assert_eq!(
        (code, &v["result"]["project_kind"]),
        (0, &json!("design")),
        "{v}"
    );
    let (code, v) = p.run_in(
        &p.direct,
        "passives",
        &["init", "--library", "--unit", "in"],
    );
    assert_eq!(
        (code, &v["result"]["project_kind"]),
        (0, &json!("library")),
        "{v}"
    );
    let id = v["result"]["project"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 32);
    for d in ["board", "passives"] {
        let m = fs::read(p.direct.join(d).join("ryueda.json")).unwrap();
        fs::write(p.daemon.join(d).join("ryueda.json"), m).unwrap();
    }
    assert_ne!(
        p.run_in(&p.daemon, "board", &["info"]).1["result"]["project"],
        json!(id)
    );
    p.fails(&["init"], "project_exists", 1);
    // A ryueda project sits beside ryucad's, never in one.
    p.write("cad/ryucad.json", "{}");
    let (code, v) = p.both_in("cad", &["init"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("project_exists")));
    let info = p.ok(&["info"]);
    // The root is a path on this machine, written as its OS writes paths
    // (as ryucad's is); paths within a project are `/`-separated everywhere.
    assert_eq!(
        info["root"],
        format!("<root>{}board", std::path::MAIN_SEPARATOR)
    );
    assert_eq!(info["default_unit"], "mm");
    assert_eq!(info["libraries"]["self"]["path"], "");
    let rev = info["revision"].as_str().unwrap().to_owned();
    // Aliases name library projects beside the project, and nothing else.
    p.fails(&["lib", "add", "self", "../passives"], "invalid_param", 1);
    p.fails(&["lib", "add", "x", "../nowhere"], "library_missing", 1);
    p.fails(&["lib", "add", "x", "/abs"], "library_missing", 1);
    p.fails(&["lib", "add", "x", "../board"], "library_missing", 1);
    p.fails(&["lib", "add", "Bad", "../passives"], "invalid_id", 1);
    let e = p.fails(
        &["lib", "add", "passives", "../passives", "--expect", "stale"],
        "conflict",
        1,
    );
    assert_eq!(e["details"]["actual"], json!(rev));
    let added = p.ok(&["lib", "add", "passives", "../passives", "--expect", &rev]);
    assert_eq!(added["library"]["path"], "../passives");
    assert_ne!(added["revision"], json!(rev));
    p.fails(
        &["lib", "add", "passives", "../passives"],
        "duplicate_id",
        1,
    );
    p.write("passives/library/symbols/broken.json", "{");
    let list = p.ok(&["lib", "list"]);
    let libs = list["libraries"].as_array().unwrap();
    assert_eq!(libs.len(), 2);
    assert_eq!(libs[0]["alias"], "passives");
    assert_eq!(libs[0]["symbols"], 1);
    assert_eq!(libs[0]["faults"][0]["code"], "invalid_json");
    assert_eq!(
        libs[0]["faults"][0]["details"]["path"],
        "../passives/library/symbols/broken.json"
    );
    assert_eq!(libs[1]["alias"], "self");
    // A library project names no libraries of its own.
    let (code, v) = p.both_in("passives", &["lib", "add", "board", "../board"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("library_nested")));
    // The manifest is canonical, and names the alias by its path.
    let text = fs::read_to_string(p.direct.join("board/ryueda.json")).unwrap();
    assert!(
        text.contains(
            "\"libraries\": {\n    \"passives\": {\n      \"path\": \"../passives\"\n    }\n  },"
        ),
        "{text}"
    );
}

#[test]
fn writes_are_logged_and_undone_through_the_daemon() {
    let p = Pair::new();
    for (sub, args) in [
        ("board", vec!["init"]),
        ("passives", vec!["init", "--library"]),
    ] {
        assert_eq!(p.run_in(&p.daemon, sub, &args).0, 0);
    }
    let run = |args: &[&str]| p.run_in(&p.daemon, "board", args);
    let before = fs::read(p.daemon.join("board/ryueda.json")).unwrap();
    assert_eq!(run(&["lib", "add", "passives", "../passives"]).0, 0);
    let (_, h) = run(&["history"]);
    let entries = h["result"]["entries"].as_array().unwrap().clone();
    let change = entries.iter().find(|e| e["method"] == "lib.add").unwrap();
    assert_eq!(change["participant"]["name"], "agent");
    // Only the project's own manifest: the library project it now names
    // isn't the change's, and undo leaves it be.
    let paths: Vec<&Value> = change["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| &f["path"])
        .collect();
    assert_eq!(paths, [&json!("ryueda.json")]);
    let (code, u) = run(&["undo"]);
    assert_eq!(code, 0, "{u}");
    assert_eq!(
        fs::read(p.daemon.join("board/ryueda.json")).unwrap(),
        before
    );
    assert!(u["result"]["revisions"].get("lib/passives").is_none());
    let (code, r) = run(&["redo"]);
    assert_eq!(code, 0, "{r}");
    assert!(r["result"]["revisions"]["lib/passives"].is_string());
    // Another agent has nothing of its own to undo.
    let (code, v) = run(&["--as", "other", "undo"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("nothing_to_undo")));
    // In process there is no log.
    let (code, v) = p.run_in(&p.daemon, "board", &["--no-daemon", "undo"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("no_live_session")));
    let (code, s) = run(&["daemon", "status"]);
    assert_eq!(code, 0, "{s}");
    assert!(s["result"]["version"]["ryueda"].is_string());
}

#[test]
fn watchers_hear_of_library_changes() {
    let p = Pair::new();
    for (sub, args) in [
        ("board", vec!["init"]),
        ("passives", vec!["init", "--library"]),
    ] {
        assert_eq!(p.run_in(&p.daemon, sub, &args).0, 0);
    }
    let board = p.daemon.join("board");
    let mut watch = ryueda(&p.run);
    watch.current_dir(&board).args([
        "--as",
        "watcher",
        "watch",
        "--topic",
        "project",
        "--timeout",
        "20",
    ]);
    let waiting = std::thread::spawn(move || watch.output().unwrap());
    // The watcher says hello, then subscribes at once.
    let attached = || {
        let (_, v) = p.run_in(&p.daemon, "board", &["participants"]);
        v["result"]["participants"]
            .as_array()
            .is_some_and(|l| l.iter().any(|x| x["name"] == "watcher"))
    };
    for _ in 0..200 {
        if attached() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // Nothing says when the watcher has subscribed (it is attached
    // first), so the library is named, and unnamed by undo, until the
    // watcher has heard one change: each moves the same revisions. (A
    // sleep before the write was a guess a busy machine broke, as it
    // once did ryucad's.)
    let mut named = false;
    while !waiting.is_finished() {
        let args: &[&str] = match named {
            false => &["lib", "add", "passives", "../passives"],
            true => &["undo"],
        };
        assert_eq!(p.run_in(&p.daemon, "board", args).0, 0, "{args:?}");
        named = !named;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (code, v) = parse(waiting.join().unwrap(), &p.daemon);
    assert_eq!(code, 0, "{v}");
    let notes = v["result"]["notifications"].as_array().unwrap();
    assert_eq!(notes[0]["method"], "project.changed", "{v}");
    let changed = &notes[0]["params"]["changed"];
    assert_eq!(changed, &json!(["lib/passives", "project"]));
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

/// Parity: nothing an agent can't do. Every protocol method has a CLI
/// command, and each command exists.
#[test]
fn every_protocol_method_has_a_command() {
    let commands: &[(&str, &[&str])] = &[
        ("project.info", &["info"]),
        ("lib.add", &["lib", "add"]),
        ("lib.list", &["lib", "list"]),
        ("symbol.create", &["symbol", "create"]),
        ("symbol.edit", &["symbol", "edit"]),
        ("symbol.show", &["symbol", "show"]),
        ("symbol.list", &["symbol", "list"]),
        ("symbol.delete", &["symbol", "delete"]),
        ("footprint.show", &["footprint", "show"]),
        ("footprint.list", &["footprint", "list"]),
        ("libcomp.create", &["libcomp", "create"]),
        ("libcomp.edit", &["libcomp", "edit"]),
        ("libcomp.show", &["libcomp", "show"]),
        ("libcomp.list", &["libcomp", "list"]),
        ("libcomp.delete", &["libcomp", "delete"]),
        ("import.kicad", &["import", "kicad"]),
        ("rail.add", &["rail", "add"]),
        ("rail.edit", &["rail", "edit"]),
        ("rail.delete", &["rail", "delete"]),
        ("rail.list", &["rail", "list"]),
        ("class.add", &["class", "add"]),
        ("class.edit", &["class", "edit"]),
        ("class.delete", &["class", "delete"]),
        ("class.list", &["class", "list"]),
        ("design.create", &["design", "create"]),
        ("design.edit", &["design", "edit"]),
        ("design.delete", &["design", "delete"]),
        ("design.list", &["design", "list"]),
        ("block.create", &["block", "create"]),
        ("block.edit", &["block", "edit"]),
        ("block.show", &["block", "show"]),
        ("block.list", &["block", "list"]),
        ("block.delete", &["block", "delete"]),
        ("component.add", &["component", "add"]),
        ("component.edit", &["component", "edit"]),
        ("component.delete", &["component", "delete"]),
        ("component.show", &["component", "show"]),
        ("connect", &["connect"]),
        ("disconnect", &["disconnect"]),
        ("nc", &["nc"]),
        ("net.add", &["net", "add"]),
        ("net.edit", &["net", "edit"]),
        ("net.rename", &["net", "rename"]),
        ("net.join", &["net", "join"]),
        ("net.delete", &["net", "delete"]),
        ("net.show", &["net", "show"]),
        ("instance.add", &["instance", "add"]),
        ("instance.edit", &["instance", "edit"]),
        ("instance.delete", &["instance", "delete"]),
        ("instance.show", &["instance", "show"]),
        ("netlist", &["netlist"]),
        ("erc", &["erc"]),
        ("erc.waive", &["erc", "waive"]),
        ("erc.configure", &["erc", "configure"]),
        ("check", &["check"]),
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
        let Some((_, args)) = commands.iter().find(|(name, _)| *name == m) else {
            panic!("protocol method {m} has no CLI command");
        };
        let out = Command::cargo_bin("ryueda")
            .unwrap()
            .args(*args)
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success(), "ryueda {args:?} --help");
    }
}

impl Pair {
    /// A design project `board` naming a library project `passives`, the
    /// same in both folders.
    fn projects() -> Pair {
        let p = Pair::new();
        assert_eq!(p.run_in(&p.direct, "board", &["init"]).0, 0);
        assert_eq!(p.run_in(&p.direct, "passives", &["init", "--library"]).0, 0);
        for d in ["board", "passives"] {
            let m = fs::read(p.direct.join(d).join("ryueda.json")).unwrap();
            fs::write(p.daemon.join(d).join("ryueda.json"), m).unwrap();
        }
        p.ok(&["lib", "add", "passives", "../passives"]);
        p
    }
}

/// A manifest written by hand with an alias named `self` would put the
/// library project in the project's own library's place, so writes would
/// land there, outside the activity log: it is refused, and nothing is
/// written anywhere.
#[test]
fn an_alias_named_self_is_refused() {
    let p = Pair::projects();
    for top in [&p.direct, &p.daemon] {
        let path = top.join("board/ryueda.json");
        let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let libs = m["libraries"].as_object_mut().unwrap();
        let alias = libs.remove("passives").unwrap();
        libs.insert("self".into(), alias);
        fs::write(&path, serde_json::to_string_pretty(&m).unwrap() + "\n").unwrap();
    }
    let (_, v) = p.both_in("board", &["symbol", "create", "R", "--ops", RESISTOR]);
    assert_eq!(v["error"]["code"], "invalid_document", "{v}");
    assert_eq!(v["error"]["details"]["pointer"], "/libraries/self", "{v}");
    for top in [&p.direct, &p.daemon] {
        for d in ["board", "passives"] {
            assert!(!top.join(d).join("library/symbols").exists(), "{d}");
        }
    }
}

/// A two-pin resistor drawn as a box, as ops.
const RESISTOR: &str = r#"[
    {"op": "add", "pin": "1", "type": "passive", "at": ["0", "3.81"], "side": "down", "length": "1.27 mm"},
    {"op": "add", "pin": "2", "type": "passive", "at": ["0", "-3.81"], "side": "up", "length": "1.27 mm"},
    {"op": "add", "graphic": "rect", "start": ["-1.016", "2.54"], "end": ["1.016", "-2.54"]}
]"#;

#[test]
fn symbols_from_the_cli() {
    let p = Pair::projects();
    let made = p.ok(&["symbol", "create", "R_Small", "--ops", RESISTOR]);
    assert_eq!(made["reference"], "lib/self/symbol/r_small");
    assert_eq!(made["path"], "library/symbols/r_small.json");
    assert_eq!(made["added"], json!(["g1"]));
    assert_eq!(made["symbol"]["pins"]["1"]["at"], json!([0, 3_810_000]));
    assert_eq!(made["symbol"]["pins"]["1"]["length_nm"], json!(1_270_000));
    assert_eq!(made["faults"], json!([]));
    // The next one by that name takes the next ID.
    let again = p.ok(&["symbol", "create", "R_Small", "--ops", RESISTOR]);
    assert_eq!(again["reference"], "lib/self/symbol/r_small-2");
    p.fails(
        &["symbol", "create", "X", "--id", "r_small"],
        "duplicate_id",
        1,
    );
    // A write that would leave a fault is refused, with every fault.
    let e = p.fails(
        &["symbol", "create", "Gate", "--units", "2", "--ops", r#"[{"op": "add", "pin": "1", "type": "input", "at": [0, 0], "side": "left", "unit": 1}]"#],
        "library_unit_invalid",
        1,
    );
    assert_eq!(e["details"]["faults"][0]["details"]["pointer"], "/units");
    let rev = made["revision"].as_str().unwrap();
    p.fails(
        &["symbol", "edit", "r_small", "--expect", rev],
        "conflict",
        1,
    );
    let rev = again["revision"].as_str().unwrap();
    let edited = p.ok(&[
        "symbol", "edit", "r_small", "--expect", rev, "--name", "R",
        "--ops", r#"[{"op": "set", "pin": "1", "name": "A"}, {"op": "move", "graphic": "g1", "by": ["0.1 mm", 0]}]"#,
    ]);
    assert_eq!(edited["symbol"]["name"], "R");
    assert_eq!(edited["symbol"]["pins"]["1"]["name"], "A");
    assert_eq!(
        edited["symbol"]["graphics"]["g1"]["start"],
        json!([-916_000, 2_540_000])
    );
    let e = p.fails(
        &[
            "symbol",
            "edit",
            "r_small",
            "--ops",
            r#"[{"op": "remove", "pin": "9"}]"#,
        ],
        "pin_not_found",
        1,
    );
    assert_eq!(e["details"]["pointer"], "/ops/0/pin");
    // Reads reach every library; writes only this project's.
    p.write(
        "passives/library/symbols/c.json",
        &fs::read_to_string(p.direct.join("board/library/symbols/r_small.json")).unwrap(),
    );
    let shown = p.ok(&["symbol", "show", "lib/passives/symbol/c"]);
    assert_eq!(shown["path"], "../passives/library/symbols/c.json");
    p.fails(
        &["symbol", "edit", "lib/passives/symbol/c"],
        "invalid_param",
        1,
    );
    p.fails(
        &["symbol", "show", "lib/nowhere/symbol/c"],
        "library_missing",
        1,
    );
    p.fails(&["symbol", "show", "nope"], "not_found", 1);
    let list = p.ok(&["symbol", "list"]);
    let ids: Vec<&Value> = list["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["id"])
        .collect();
    assert_eq!(ids, [&json!("r_small"), &json!("r_small-2")]);
    assert_eq!(list["symbols"][0]["pins"], 2);
    assert_eq!(
        p.ok(&["symbol", "list", "--library", "passives"])["symbols"][0]["id"],
        "c"
    );
    let gone = p.ok(&["symbol", "delete", "r_small-2"]);
    assert_eq!(gone["deleted"], "lib/self/symbol/r_small-2");
    assert!(
        !p.direct
            .join("board/library/symbols/r_small-2.json")
            .exists()
    );
    p.fails(&["symbol", "delete", "r_small-2"], "not_found", 1);
}

/// A STEP file that parses.
const STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('r','',(''),(''),'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";

/// An 0402 footprint with its model, as import (plan 0.D) would write it.
fn r0402(step: &str) -> String {
    let v = json!({
        "kind": "ryueda.footprint", "format": 1, "name": "R_0402_1005Metric", "mount": "smd",
        "pads": {
            "p1": {"number": "1", "kind": "smd", "shape": {"type": "roundrect", "ratio_ppm": 250_000},
                   "at": [-510_000, 0], "size": [540_000, 640_000], "layers": ["copper.top", "mask.top", "paste.top"]},
            "p2": {"number": "2", "kind": "smd", "shape": {"type": "roundrect", "ratio_ppm": 250_000},
                   "at": [510_000, 0], "size": [540_000, 640_000], "layers": ["copper.top", "mask.top", "paste.top"]}
        },
        "graphics": {
            "g1": {"layer": "courtyard.top", "type": "rect", "start": [-930_000, 470_000], "end": [930_000, -470_000], "width_nm": 50_000}
        },
        "models": [{"model": {"step": {"path": "library/models/r_0402.step", "blake3": blake3::hash(step.as_bytes()).to_hex().to_string()}}}]
    });
    ryueda::canonical::to_string(&v)
}

#[test]
fn footprints_and_library_components_from_the_cli() {
    let p = Pair::projects();
    p.write("passives/library/footprints/r_0402.json", &r0402(STEP));
    p.write("passives/library/models/r_0402.step", STEP);
    let fl = p.ok(&["footprint", "list", "--library", "passives"]);
    assert_eq!(
        fl["footprints"][0]["reference"],
        "lib/passives/footprint/r_0402"
    );
    assert_eq!(
        (&fl["footprints"][0]["pads"], &fl["footprints"][0]["faults"]),
        (&json!(2), &json!(0))
    );
    let shown = p.ok(&["footprint", "show", "lib/passives/footprint/r_0402"]);
    assert_eq!(shown["footprint"]["pads"]["p1"]["number"], "1");
    assert_eq!(p.ok(&["lib", "list"])["libraries"][0]["models"], 1);
    p.ok(&["symbol", "create", "R", "--ops", RESISTOR]);
    let made = p.ok(&[
        "libcomp",
        "create",
        "R 100k",
        "--symbol",
        "r",
        "--prefix",
        "R",
        "--value",
        "100k",
        "--footprint",
        "lib/passives/footprint/r_0402",
        "--param",
        "tolerance=1%",
        "--mpn",
        "RC0402FR-07100KL",
        "--offer",
        "LCSC=C25741",
    ]);
    assert_eq!(made["reference"], "lib/self/libcomp/r_100k");
    assert_eq!(made["faults"], json!([]));
    let c = &made["libcomp"];
    assert_eq!(c["symbol"], json!({"id": "r"}));
    assert_eq!(
        c["footprints"][0]["footprint"],
        json!({"library": "passives", "id": "r_0402"})
    );
    assert_eq!(c["offers"], json!([{"supplier": "LCSC", "sku": "C25741"}]));
    // What would be faulty is refused, with the fault where it lies.
    let e = p.fails(
        &[
            "libcomp",
            "create",
            "X",
            "--symbol",
            "r",
            "--prefix",
            "R",
            "--footprint",
            "nope",
        ],
        "not_found",
        1,
    );
    assert_eq!(
        e["details"]["faults"][0]["details"]["pointer"],
        "/footprints/0/footprint"
    );
    let e = p.fails(
        &[
            "libcomp",
            "create",
            "X",
            "--symbol",
            "r",
            "--prefix",
            "R",
            "--footprints",
            r#"[{"footprint": "lib/passives/footprint/r_0402", "pins": {"1": ["1"], "2": ["1"]}}]"#,
        ],
        "pad_mapped_twice",
        1,
    );
    assert_eq!(
        e["details"]["faults"][0]["details"]["pins"],
        json!(["1", "2"])
    );
    p.fails(
        &["libcomp", "create", "X", "--prefix", "R"],
        "invalid_param",
        1,
    );
    p.fails(
        &["libcomp", "create", "X", "--symbol", "r", "--prefix", "r5"],
        "invalid_param",
        1,
    );
    let edited = p.ok(&[
        "libcomp",
        "edit",
        "r_100k",
        "--value",
        "4k7",
        "--unset-param",
        "tolerance",
        "--param",
        "power=62.5 mW",
    ]);
    assert_eq!(edited["libcomp"]["value"], "4k7");
    assert_eq!(edited["libcomp"]["parameters"], json!({"power": "62.5 mW"}));
    // The symbol's pins are the component's now.
    let e = p.fails(
        &[
            "symbol",
            "edit",
            "r",
            "--ops",
            r#"[{"op": "remove", "pin": "2"}]"#,
        ],
        "has_dependents",
        1,
    );
    assert_eq!(
        e["details"]["dependents"],
        json!(["lib/self/libcomp/r_100k"])
    );
    p.fails(
        &[
            "symbol",
            "edit",
            "r",
            "--ops",
            r#"[{"op": "set", "pin": "2", "number": "B"}]"#,
        ],
        "has_dependents",
        1,
    );
    p.fails(&["symbol", "delete", "r"], "has_dependents", 1);
    // A pin added is the component's to map: the edit says what it did.
    let added = p.ok(&[
        "symbol",
        "edit",
        "r",
        "--ops",
        r#"[{"op": "add", "pin": "3", "type": "passive", "at": [0, 0], "side": "left"}]"#,
    ]);
    assert_eq!(
        added["components"][0]["reference"],
        "lib/self/libcomp/r_100k"
    );
    assert_eq!(
        added["components"][0]["faults"][0]["code"],
        "pin_without_pad"
    );
    let list = p.ok(&["libcomp", "list"]);
    assert_eq!(
        (
            &list["libcomps"][0]["value"],
            &list["libcomps"][0]["faults"]
        ),
        (&json!("4k7"), &json!(1))
    );
    // A model replaced by hand no longer matches its footprint.
    p.write(
        "passives/library/models/r_0402.step",
        &STEP.replace("0.,0.,0.", "1.,0.,0."),
    );
    let shown = p.ok(&["footprint", "show", "lib/passives/footprint/r_0402"]);
    assert_eq!(shown["faults"][0]["code"], "model_file_changed");
    p.write(
        "passives/library/footprints/r_0402.json",
        &r0402("not step"),
    );
    p.write("passives/library/models/r_0402.step", "not step");
    let shown = p.ok(&["footprint", "show", "lib/passives/footprint/r_0402"]);
    assert_eq!(shown["faults"][0]["code"], "model_unreadable");
    let gone = p.ok(&["libcomp", "delete", "r_100k"]);
    assert_eq!(gone["deleted"], "lib/self/libcomp/r_100k");
    p.ok(&["symbol", "delete", "r"]);
}

#[test]
fn blocks_rails_classes_and_designs_from_the_cli() {
    let p = Pair::projects();
    let power = p.ok(&["class", "add", "Power", "--id", "power"]);
    assert_eq!(power["reference"], "class/power");
    let gnd = p.ok(&["rail", "add", "GND", "--id", "gnd"]);
    assert_eq!(gnd["path"], "rails/gnd.json");
    let v3 = p.ok(&[
        "rail",
        "add",
        "3V3",
        "--voltage",
        "3.3 V",
        "--class",
        "Power",
    ]);
    // A random ID, the seed's: ten base32 characters.
    let v3_id = v3["reference"]
        .as_str()
        .unwrap()
        .strip_prefix("rail/")
        .unwrap();
    assert_eq!(v3_id.len(), 10);
    assert_eq!(
        v3["rail"],
        json!({"name": "3V3", "class": "power", "voltage": "3.3 V"})
    );
    p.fails(
        &["rail", "add", "X", "--voltage", "3.3 A"],
        "invalid_quantity",
        1,
    );
    p.fails(&["rail", "add", "GND"], "net_name_duplicate", 1);
    p.fails(&["rail", "add", "X", "--class", "nope"], "not_found", 1);
    let rev = v3["revision"].as_str().unwrap();
    p.fails(
        &["rail", "edit", "GND", "--expect", "stale", "--name", "0V"],
        "conflict",
        1,
    );
    let edited = p.ok(&["rail", "edit", "3V3", "--expect", rev, "--voltage", ""]);
    assert_eq!(edited["rail"].get("voltage"), None);
    let top = p.ok(&["block", "create", "Top", "--id", "top"]);
    assert_eq!(top["path"], "blocks/top/block.json");
    let sensor = p.ok(&[
        "block",
        "edit",
        "top",
        "--name",
        "Sensor",
        "--add-port",
        "SDA:bidirectional",
        "--add-port",
        "VDD:power",
    ]);
    let ports = sensor["block"]["ports"].as_object().unwrap();
    assert_eq!(ports.len(), 2);
    let sda = ports
        .iter()
        .find(|(_, v)| v["name"] == "SDA")
        .unwrap()
        .0
        .clone();
    p.fails(
        &["block", "edit", "top", "--add-port", "SDA:in"],
        "net_name_duplicate",
        1,
    );
    p.fails(
        &["block", "edit", "top", "--add-port", "X:sideways"],
        "usage",
        2,
    );
    p.fails(
        &["block", "edit", "top", "--remove-port", "nope"],
        "port_not_found",
        1,
    );
    p.fails(
        &["block", "edit", "top", "--add-bus", "I2C=nope"],
        "net_not_found",
        1,
    );
    let renamed = p.ok(&[
        "block",
        "edit",
        "top",
        "--rename-port",
        &format!("{sda}=SDA1"),
    ]);
    assert_eq!(renamed["block"]["ports"][&sda]["name"], "SDA1");
    let d = p.ok(&["design", "create", "Breakout", "--root", "top", "--id", "d"]);
    assert_eq!(d["design"], json!({"name": "Breakout", "root": "top"}));
    p.fails(&["design", "create", "X", "--root", "nope"], "not_found", 1);
    // A design's root, and what an instance places, stay.
    let e = p.fails(&["block", "delete", "top"], "has_dependents", 1);
    assert_eq!(e["details"]["dependents"], json!(["design/d"]));
    assert_eq!(p.ok(&["design", "list"])["designs"][0]["name"], "Breakout");
    assert_eq!(
        p.ok(&["rail", "list"])["rails"].as_array().unwrap().len(),
        2
    );
    assert_eq!(p.ok(&["class", "list"])["classes"][0]["id"], "power");
    let e = p.fails(&["class", "delete", "power"], "has_dependents", 1);
    assert_eq!(e["details"]["dependents"], json!([format!("rail/{v3_id}")]));
    p.ok(&["design", "delete", "Breakout"]);
    let list = p.ok(&["block", "list"]);
    assert_eq!(list["blocks"][0]["name"], "Sensor");
    p.ok(&["block", "delete", "top"]);
    for top in [&p.direct, &p.daemon] {
        assert!(!top.join("board/blocks").exists());
    }
    p.fails(&["block", "show", "top"], "not_found", 1);
}

impl Pair {
    /// [`Pair::projects`] with a resistor: symbol `r` and library
    /// component `r_100k` in the board's own library, on the library
    /// project's 0402 footprint.
    fn with_resistor() -> Pair {
        let p = Pair::projects();
        p.write("passives/library/footprints/r_0402.json", &r0402(STEP));
        p.write("passives/library/models/r_0402.step", STEP);
        p.ok(&["symbol", "create", "R", "--ops", RESISTOR]);
        p.ok(&[
            "libcomp",
            "create",
            "R 100k",
            "--symbol",
            "r",
            "--prefix",
            "R",
            "--value",
            "100k",
            "--footprint",
            "lib/passives/footprint/r_0402",
        ]);
        p
    }
}

/// The ID of what an answer's `reference` names last.
fn last(v: &Value) -> String {
    v["reference"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn components_and_connections_from_the_cli() {
    let p = Pair::with_resistor();
    p.ok(&["block", "create", "Top", "--id", "top"]);
    p.ok(&["rail", "add", "GND", "--id", "gnd"]);
    let r1 = p.ok(&["component", "add", "top", "--of", "r_100k"]);
    assert_eq!(r1["component"]["refdes"], "R1");
    assert_eq!(r1["component"]["of"], json!({"id": "r_100k"}));
    let r1 = last(&r1);
    let r7 = p.ok(&[
        "component",
        "add",
        "top",
        "--of",
        "r_100k",
        "--refdes",
        "R7",
        "--id",
        "r7",
    ]);
    assert_eq!(r7["reference"], "top/r7");
    let r2 = p.ok(&[
        "component",
        "add",
        "top",
        "--of",
        "r_100k",
        "--value",
        "4k7",
    ]);
    assert_eq!(r2["component"]["refdes"], "R2");
    let r2 = last(&r2);
    p.fails(&["component", "add", "top", "--of", "nope"], "not_found", 1);
    p.fails(
        &[
            "component",
            "add",
            "top",
            "--of",
            "r_100k",
            "--refdes",
            "7R",
        ],
        "invalid_param",
        1,
    );
    p.fails(
        &["component", "add", "top", "--of", "r_100k", "--id", "r7"],
        "duplicate_id",
        1,
    );
    let pin = |c: &str, n: &str| format!("top/{c}/{n}");
    let made = p.ok(&["connect", &pin(&r1, "1"), "SDA", "--new"]);
    let sda = made["net"].as_str().unwrap().to_owned();
    assert!(sda.starts_with("top/"), "{made}");
    assert_eq!(p.ok(&["connect", &pin(&r2, "1"), "SDA"])["net"], json!(sda));
    p.fails(
        &["connect", &pin(&r2, "2"), "SDA", "--new"],
        "net_name_duplicate",
        1,
    );
    assert_eq!(
        p.ok(&["connect", &pin(&r1, "2"), "rail:GND"])["net"],
        "rail/gnd"
    );
    p.fails(&["connect", &pin(&r1, "3"), "SDA"], "pin_not_found", 1);
    p.fails(&["connect", &pin(&r2, "2"), "nope"], "net_not_found", 1);
    p.fails(
        &["connect", &pin(&r2, "2"), "port:VDD"],
        "port_not_found",
        1,
    );
    p.fails(&["connect", "top/nope/1", "SDA"], "not_found", 1);
    assert_eq!(p.ok(&["nc", &pin(&r2, "2")])["net"], "nc");
    let again = p.ok(&["nc", &pin(&r2, "2")]);
    assert_eq!(again["changed"], false);
    p.ok(&["block", "edit", "top", "--add-port", "VDD:power"]);
    assert_eq!(
        p.ok(&["connect", "top/r7/1", "port:VDD"])["net"]
            .as_str()
            .unwrap()
            .split_once("port:")
            .unwrap()
            .0,
        "top/"
    );
    let shown = p.ok(&["component", "show", &format!("top/{r1}")]);
    assert_eq!(shown["connections"], json!({"1": sda, "2": "rail/gnd"}));
    assert_eq!(shown["component"]["pins"]["2"], json!({"rail": "gnd"}));
    let off = p.ok(&["disconnect", &pin(&r1, "1")]);
    assert_eq!((&off["net"], &off["changed"]), (&Value::Null, &json!(true)));
    // Footprints are the library component's, by ID or in full.
    let fp = p.ok(&[
        "component",
        "edit",
        &format!("top/{r1}"),
        "--footprint",
        "r_0402",
    ]);
    assert_eq!(
        fp["component"]["footprint"],
        json!({"library": "passives", "id": "r_0402"})
    );
    p.fails(
        &[
            "component",
            "edit",
            &format!("top/{r1}"),
            "--footprint",
            "nope",
        ],
        "not_found",
        1,
    );
    let rev = fp["revision"].as_str().unwrap();
    p.fails(
        &[
            "component",
            "edit",
            &format!("top/{r1}"),
            "--refdes",
            "R9",
            "--expect",
            "stale",
        ],
        "conflict",
        1,
    );
    let e = p.ok(&[
        "component",
        "edit",
        &format!("top/{r1}"),
        "--refdes",
        "R9",
        "--param",
        "dnp=no",
        "--expect",
        rev,
    ]);
    assert_eq!(e["component"]["parameters"], json!({"dnp": "no"}));
    // The next free number fills the gap R1 left.
    let r3 = p.ok(&["component", "add", "top", "--of", "r_100k"]);
    assert_eq!(r3["component"]["refdes"], "R1");
    let e = p.fails(&["rail", "delete", "GND"], "has_dependents", 1);
    assert_eq!(e["details"]["dependents"], json!([pin(&r1, "2")]));
    p.ok(&["component", "delete", &format!("top/{r1}")]);
    p.ok(&["rail", "delete", "GND"]);
    let text = fs::read_to_string(p.direct.join("board/blocks/top/components/r7.json")).unwrap();
    assert!(
        text.contains("\"pins\": {\n    \"1\": {\n      \"port\""),
        "{text}"
    );
}

#[test]
fn nets_and_instances_from_the_cli() {
    let p = Pair::with_resistor();
    p.ok(&["rail", "add", "GND", "--id", "gnd"]);
    p.ok(&["block", "create", "Top", "--id", "top"]);
    p.ok(&["block", "create", "Channel", "--id", "ch"]);
    p.ok(&["block", "edit", "ch", "--add-port", "SDA:bidirectional"]);
    p.ok(&["component", "add", "ch", "--of", "r_100k", "--id", "r"]);
    p.ok(&["connect", "ch/r/1", "port:SDA"]);
    p.ok(&["connect", "ch/r/2", "rail:GND"]);
    let sda = p.ok(&[
        "net",
        "add",
        "top",
        "SDA",
        "--id",
        "sda",
        "--voltage",
        "3.3 V",
    ]);
    assert_eq!(sda["net"], json!({"name": "SDA", "voltage": "3.3 V"}));
    p.fails(&["net", "add", "top", "SDA"], "net_name_duplicate", 1);
    p.fails(&["net", "add", "top", "X", "--id", "nc"], "invalid_id", 1);
    p.ok(&["net", "add", "top", "SCL", "--id", "scl"]);
    // Pairs name each other.
    p.ok(&["net", "add", "top", "D_P", "--id", "dp"]);
    p.ok(&["net", "add", "top", "D_N", "--id", "dn"]);
    let dp = p.ok(&["net", "edit", "top/D_P", "--pair-with", "D_N"]);
    assert_eq!(dp["net"]["pair"], json!({"with": "dn", "polarity": "p"}));
    assert_eq!(
        p.ok(&["net", "show", "top/dn"])["net"]["pair"],
        json!({"with": "dp", "polarity": "n"})
    );
    let e = p.fails(&["net", "delete", "top/dp"], "has_dependents", 1);
    assert_eq!(e["details"]["dependents"], json!(["top/dn"]));
    p.ok(&["net", "edit", "top/dp", "--no-pair"]);
    assert_eq!(p.ok(&["net", "show", "top/dn"])["net"].get("pair"), None);
    p.ok(&["net", "delete", "top/dp"]);
    // Instances, with channel rules.
    let a = p.ok(&[
        "instance", "add", "top", "--of", "ch", "--id", "ch_a", "--port", "SDA=SDA", "--suffix",
        "_A",
    ]);
    assert_eq!(
        a["connections"],
        json!({p.ok(&["block", "show", "ch"])["block"]["ports"].as_object().unwrap().keys().next().unwrap().as_str(): "top/sda"})
    );
    assert_eq!(
        a["instance"]["refdes"],
        json!({"rule": "suffix", "value": "_A"})
    );
    let b = p.ok(&["instance", "add", "top", "--of", "ch", "--offset", "100"]);
    assert_eq!(
        b["instance"]["refdes"],
        json!({"rule": "offset", "value": 100})
    );
    let b = last(&b);
    p.fails(&["instance", "add", "top", "--of", "nope"], "not_found", 1);
    p.fails(
        &["instance", "add", "top", "--of", "ch", "--port", "nope=SDA"],
        "port_not_found",
        1,
    );
    // No block contains itself.
    let e = p.fails(&["instance", "add", "ch", "--of", "top"], "block_cycle", 1);
    assert_eq!(
        e["details"]["faults"][0]["details"]["chain"],
        json!(["ch", "top"])
    );
    p.fails(&["net", "delete", "top/sda"], "net_has_pins", 1);
    let e = p.fails(
        &["block", "edit", "ch", "--remove-port", "SDA"],
        "has_dependents",
        1,
    );
    assert_eq!(e["details"]["dependents"].as_array().unwrap().len(), 2);
    let o = p.ok(&[
        "instance",
        "edit",
        &format!("top/{b}"),
        "--override",
        "r=R42",
        "--port",
        "SDA=SCL",
    ]);
    assert_eq!(o["instance"]["overrides"], json!({"r": "R42"}));
    p.fails(
        &[
            "instance",
            "edit",
            &format!("top/{b}"),
            "--override",
            "x/r=R1",
        ],
        "not_found",
        1,
    );
    p.fails(
        &[
            "instance",
            "edit",
            &format!("top/{b}"),
            "--override",
            "r=1R",
        ],
        "invalid_param",
        1,
    );
    // Joining moves every pin and port, in one change.
    p.ok(&["component", "add", "top", "--of", "r_100k", "--id", "pull"]);
    p.ok(&["connect", "top/pull/1", "SCL"]);
    let j = p.ok(&["net", "join", "top/SCL", "SDA"]);
    assert_eq!(
        j["moved"],
        json!([
            "top/pull/1",
            format!(
                "top/{b}/{}",
                a["connections"].as_object().unwrap().keys().next().unwrap()
            )
        ])
    );
    assert_eq!(j["into"], "top/sda");
    assert!(!p.direct.join("board/blocks/top/nets/scl.json").exists());
    p.fails(&["net", "join", "top/sda", "SDA"], "invalid_param", 1);
    let r = p.ok(&["net", "join", "top/sda", "rail:GND"]);
    assert_eq!(r["into"], "rail/gnd");
    assert_eq!(
        p.ok(&["component", "show", "top/pull"])["connections"]["1"],
        "rail/gnd"
    );
    let shown = p.ok(&["instance", "show", "top/ch_a"]);
    assert_eq!(shown["instance"]["of"], "ch");
    p.ok(&["instance", "delete", "top/ch_a"]);
    p.ok(&[
        "instance",
        "edit",
        &format!("top/{b}"),
        "--no-refdes",
        "--unoverride",
        "r",
    ]);
    let renamed = p.ok(&["net", "rename", "top/dn", "USB_N"]);
    assert_eq!(renamed["net"]["name"], "USB_N");
}

/// What a design places stays in the library: the library component, and
/// the pins of its symbol the design connects.
#[test]
fn library_objects_a_design_places_stay() {
    let p = Pair::with_resistor();
    p.ok(&["block", "create", "Top", "--id", "top"]);
    p.ok(&["component", "add", "top", "--of", "r_100k", "--id", "r1"]);
    p.ok(&["connect", "top/r1/1", "N1", "--new"]);
    let e = p.fails(
        &[
            "symbol",
            "edit",
            "r",
            "--ops",
            r#"[{"op": "remove", "pin": "1"}]"#,
        ],
        "has_dependents",
        1,
    );
    assert_eq!(
        e["details"]["dependents"],
        json!(["lib/self/libcomp/r_100k", "top/r1/1"])
    );
    // A pin the design leaves alone names only the library component.
    let e = p.fails(
        &[
            "symbol",
            "edit",
            "r",
            "--ops",
            r#"[{"op": "remove", "pin": "2"}]"#,
        ],
        "has_dependents",
        1,
    );
    assert_eq!(
        e["details"]["dependents"],
        json!(["lib/self/libcomp/r_100k"])
    );
    let e = p.fails(&["libcomp", "delete", "r_100k"], "has_dependents", 1);
    assert_eq!(e["details"]["dependents"], json!(["top/r1"]));
    p.ok(&["component", "delete", "top/r1"]);
    p.ok(&["libcomp", "delete", "r_100k"]);
    p.ok(&["symbol", "delete", "r"]);
}

#[test]
fn netlists_from_the_cli() {
    let p = Pair::with_resistor();
    p.fails(&["netlist"], "not_found", 1);
    p.ok(&["rail", "add", "GND", "--id", "gnd"]);
    p.ok(&["block", "create", "Top", "--id", "top"]);
    p.ok(&["block", "create", "Pull-up", "--id", "pu"]);
    p.ok(&["block", "edit", "pu", "--add-port", "IO:bidirectional"]);
    let port = p.ok(&["block", "show", "pu"])["block"]["ports"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    p.ok(&[
        "component",
        "add",
        "pu",
        "--of",
        "r_100k",
        "--id",
        "r",
        "--refdes",
        "R1",
    ]);
    p.ok(&["connect", "pu/r/1", "port:IO"]);
    p.ok(&["connect", "pu/r/2", "rail:GND"]);
    p.ok(&["net", "add", "top", "SDA", "--id", "sda"]);
    p.ok(&[
        "instance", "add", "top", "--of", "pu", "--id", "a", "--port", "IO=SDA", "--suffix", "_A",
    ]);
    p.ok(&[
        "instance", "add", "top", "--of", "pu", "--id", "b", "--port", "IO=SDA", "--suffix", "_B",
    ]);
    p.ok(&[
        "design", "create", "Board", "--root", "top", "--id", "board",
    ]);
    let n = p.ok(&["netlist"]);
    assert_eq!(n["design"], "board");
    let refdes: Vec<&Value> = n["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| &o["refdes"])
        .collect();
    assert_eq!(refdes, [&json!("R1_A"), &json!("R1_B")]);
    let sda = n["nets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "SDA")
        .unwrap();
    assert_eq!(sda["members"].as_array().unwrap().len(), 2);
    assert_eq!(sda["ports"][0]["port"], format!("board:a/port:{port}"));
    assert_eq!(p.ok(&["netlist", "Board"]), n);
    // ERC: the pull-ups hold SDA to GND, so nothing is wrong but a lone
    // net: SDA has two pins, GND two.
    let erc = p.ok(&["erc"]);
    assert_eq!(
        (&erc["errors"], &erc["warnings"]),
        (&json!(0), &json!(0)),
        "{erc}"
    );
    p.ok(&["net", "add", "top", "SPARE", "--id", "spare"]);
    p.ok(&[
        "component",
        "add",
        "top",
        "--of",
        "r_100k",
        "--id",
        "r9",
        "--refdes",
        "R9",
    ]);
    p.ok(&["connect", "top/r9/1", "SPARE"]);
    let erc = p.ok(&["erc", "board"]);
    let found = &erc["designs"][0]["findings"];
    let codes: Vec<&Value> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|f| &f["code"])
        .collect();
    assert_eq!(codes, [&json!("pin_unconnected"), &json!("net_single_pin")]);
    assert_eq!(found[0]["refs"], json!(["board:r9/2"]));
    let single = found[1]["id"].as_str().unwrap().to_owned();
    assert_eq!(single.len(), 16);
    // Waivers and severities are erc.json's.
    p.fails(
        &["erc", "waive", "0123456789abcdef", "--reason", "x"],
        "not_found",
        1,
    );
    let w = p.ok(&["erc", "waive", &single, "--reason", "a test point"]);
    assert_eq!(
        w["erc"]["waivers"][&single],
        json!({"reason": "a test point", "by": "agent"})
    );
    let erc = p.ok(&["erc"]);
    assert_eq!(
        (&erc["warnings"], &erc["designs"][0]["waived"]),
        (&json!(1), &json!(1))
    );
    p.ok(&["erc", "configure", "--severity", "pin_unconnected=off"]);
    let erc = p.ok(&["erc"]);
    assert_eq!(erc["warnings"], 0, "{erc}");
    p.fails(
        &["erc", "configure", "--severity", "nope=off"],
        "invalid_param",
        1,
    );
    p.fails(
        &["erc", "configure", "--severity", "net_single_pin=loud"],
        "invalid_param",
        1,
    );
    // A waiver whose finding is gone says so.
    p.ok(&["connect", "top/r9/2", "SPARE"]);
    let erc = p.ok(&["erc"]);
    assert_eq!(erc["findings"][0]["code"], "waiver_unused", "{erc}");
    p.ok(&["erc", "waive", &single, "--remove"]);
    p.ok(&["erc", "configure", "--severity", "pin_unconnected=default"]);
    // With nothing left in it, erc.json goes.
    assert!(!p.direct.join("board/erc.json").exists());
    // `check` reads every file and runs ERC on every design.
    let report = p.ok(&["check"]);
    assert_eq!(
        (&report["errors"], &report["warnings"]),
        (&json!(0), &json!(0)),
        "{report}"
    );
    p.ok(&[
        "component",
        "add",
        "top",
        "--of",
        "r_100k",
        "--id",
        "r8",
        "--refdes",
        "R9",
    ]);
    let e = p.fails(&["check"], "check_failed", 1);
    let erc = &e["details"]["report"]["erc"]["designs"][0]["findings"];
    assert!(
        erc.as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "refdes_duplicate"),
        "{e}"
    );
    p.write("board/rails/x.json", "{");
    let e = p.fails(&["check"], "check_failed", 1);
    assert_eq!(
        e["details"]["report"]["faults"][0]["details"]["path"],
        "rails/x.json"
    );
    // A broken file refuses the design, naming it.
    p.write("board/blocks/pu/components/r.json", "{");
    let e = p.fails(&["netlist"], "invalid_json", 1);
    assert_eq!(
        e["details"]["faults"][0]["details"]["path"],
        "blocks/pu/components/r.json"
    );
}

/// Every file under `dir`, by path relative to it, with its bytes.
fn tree(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// The paths that differ between two trees.
fn changed(
    a: &std::collections::BTreeMap<String, Vec<u8>>,
    b: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Vec<String> {
    a.keys()
        .chain(b.keys())
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

impl Pair {
    /// The sensor breakout, entered by its script in both projects.
    fn breakout() -> Pair {
        let p = Pair::projects();
        for (path, text) in breakout::footprints() {
            p.write(&format!("board/{path}"), &text);
        }
        for args in breakout::script() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            p.ok(&args);
        }
        p
    }
}

#[test]
fn the_sensor_breakout_from_the_cli() {
    let p = Pair::breakout();
    let n = p.ok(&["netlist"]);
    let refdes: Vec<(&str, &str)> = n["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            (
                o["reference"].as_str().unwrap(),
                o["refdes"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        refdes,
        [
            ("board:c2", "C2"),
            ("board:c3", "C3"),
            ("board:j1", "J1"),
            ("board:r1", "R1"),
            ("board:r2", "R2"),
            ("board:u2", "U2"),
            ("board:s1/c1", "C1"),
            ("board:s1/u1", "U1"),
        ]
    );
    let names: Vec<&str> = n["nets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["3V3", "GND", "SCL", "SDA", "VIN"]);
    let sda = &n["nets"][3];
    let pins: Vec<&Value> = sda["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| &m["pin"])
        .collect();
    assert_eq!(
        pins,
        [
            &json!("board:j1/4"),
            &json!("board:r1/1"),
            &json!("board:s1/u1/3")
        ]
    );
    let erc = p.ok(&["erc"]);
    assert_eq!(
        (&erc["errors"], &erc["warnings"]),
        (&json!(0), &json!(0)),
        "{erc}"
    );
    let report = p.ok(&["check"]);
    assert_eq!(report["errors"], 0, "{report}");
    // Each edit touches only what it must.
    let board = p.direct.join("board");
    let before = tree(&board);
    p.ok(&[
        "component",
        "add",
        "breakout",
        "--of",
        "c_100n",
        "--id",
        "c4",
    ]);
    p.ok(&["connect", "breakout/c4/1", "rail:VIN"]);
    let after = tree(&board);
    assert_eq!(
        changed(&before, &after),
        ["blocks/breakout/components/c4.json"]
    );
    let before = after;
    p.ok(&["net", "rename", "breakout/SDA", "I2C_SDA"]);
    let after = tree(&board);
    assert_eq!(changed(&before, &after), ["blocks/breakout/nets/sda.json"]);
    let before = after;
    p.ok(&["net", "add", "breakout", "TP", "--id", "tp"]);
    p.ok(&["connect", "breakout/c4/2", "TP"]);
    let middle = tree(&board);
    let j = p.ok(&["net", "join", "breakout/TP", "rail:GND"]);
    assert_eq!(j["moved"], json!(["breakout/c4/2"]));
    let after = tree(&board);
    assert_eq!(
        changed(&before, &after),
        ["blocks/breakout/components/c4.json"]
    );
    assert_eq!(
        changed(&middle, &after),
        [
            "blocks/breakout/components/c4.json",
            "blocks/breakout/nets/tp.json"
        ]
    );
    let erc = p.ok(&["erc"]);
    assert_eq!(
        (&erc["errors"], &erc["warnings"]),
        (&json!(0), &json!(0)),
        "{erc}"
    );
    // The join is one change: undone and redone through the daemon.
    let daemon = p.daemon.join("board");
    let joined = tree(&daemon);
    let (code, u) = p.run_in(&p.daemon, "board", &["undo"]);
    assert_eq!(code, 0, "{u}");
    assert_eq!(
        changed(&joined, &tree(&daemon)),
        [
            "blocks/breakout/components/c4.json",
            "blocks/breakout/nets/tp.json"
        ]
    );
    let (code, r) = p.run_in(&p.daemon, "board", &["redo"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(tree(&daemon), joined);
    // A block created and undone leaves no folder behind.
    assert_eq!(
        p.run_in(
            &p.daemon,
            "board",
            &["block", "create", "Spare", "--id", "spare"]
        )
        .0,
        0
    );
    assert_eq!(p.run_in(&p.daemon, "board", &["undo"]).0, 0);
    assert!(!daemon.join("blocks/spare").exists());
}

/// Copies `from` (a folder) to `to`, byte for byte.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let at = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &at);
        } else {
            fs::copy(e.path(), at).unwrap();
        }
    }
}

/// Every file under `dir`, relative, with `/`.
fn files_under(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = tree(dir).into_keys().collect();
    out.sort();
    out
}

#[test]
fn kicad_import_from_the_cli() {
    let p = Pair::projects();
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/kicad");
    for top in [&p.direct, &p.daemon] {
        copy_tree(&data, &top.join("kicad"));
    }
    let args = [
        "import",
        "kicad",
        "../kicad/symbols/Ryu_Test.kicad_sym",
        "../kicad/footprints/Ryu_Test.pretty",
        "--models",
        "../kicad/3dmodels",
    ];
    // A dry run says what would change, and writes nothing.
    let mut dry = args.to_vec();
    dry.push("--dry-run");
    let r = p.ok(&dry);
    assert_eq!(r["dry_run"], true);
    assert_eq!(
        r["footprints"]["added"],
        json!([
            "lib/self/footprint/ryu_test_every_pad",
            "lib/self/footprint/ryu_test_r_0402"
        ])
    );
    assert!(!p.daemon.join("board/library").exists());

    // Into the library project the board names: its files, guarded by
    // its revision.
    let rev = p.ok(&["lib", "list"])["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["alias"] == "passives")
        .unwrap()["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut into = args.to_vec();
    into.extend(["--into", "passives", "--expect", "stale"]);
    p.fails(&into, "conflict", 1);
    let n = into.len();
    into[n - 1] = &rev;
    let r = p.ok(&into);
    assert_eq!(r["library"], "passives");
    assert_eq!(
        r["files"],
        json!([
            "../passives/.gitattributes",
            "../passives/library/components/ryu_test_nand2.json",
            "../passives/library/components/ryu_test_r.json",
            "../passives/library/components/ryu_test_r_10k.json",
            "../passives/library/components/ryu_test_sensor.json",
            "../passives/library/footprints/ryu_test_every_pad.json",
            "../passives/library/footprints/ryu_test_r_0402.json",
            "../passives/library/models/every_pad.step",
            "../passives/library/models/r_0402.step",
            "../passives/library/symbols/ryu_test_nand2.json",
            "../passives/library/symbols/ryu_test_r.json",
            "../passives/library/symbols/ryu_test_sensor.json"
        ])
    );
    assert_eq!(r["refused"], json!([]));
    assert_eq!(r["notices"].as_array().unwrap().len(), 7);
    assert!(!p.daemon.join("board/library").exists());
    let listed = p.ok(&["symbol", "list", "--library", "passives"]);
    assert_eq!(listed["symbols"].as_array().unwrap().len(), 3);
    // A design project reads the component through the alias.
    p.ok(&["libcomp", "show", "lib/passives/libcomp/ryu_test_r_10k"]);

    // Through the daemon the import is one entry with exactly the library
    // project's files it wrote, undone and redone whole.
    let passives = p.daemon.join("passives");
    let written = files_under(&passives);
    let run = |args: &[&str]| p.run_in(&p.daemon, "board", args);
    let (_, h) = run(&["history"]);
    let entry = h["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["method"] == "import.kicad")
        .unwrap()
        .clone();
    let paths: Vec<&str> = entry["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths.len(), 12, "{paths:?}");
    assert!(
        paths.iter().all(|f| f.starts_with("../passives/")),
        "{paths:?}"
    );
    let (code, u) = run(&["undo"]);
    assert_eq!(code, 0, "{u}");
    assert_eq!(files_under(&passives), ["ryueda.json"]);
    let (code, r) = run(&["redo"]);
    assert_eq!(code, 0, "{r}");
    assert_eq!(files_under(&passives), written);

    // Into this project's own library, from the board.
    let r = p.ok(&args);
    assert_eq!(r["library"], "self");
    assert_eq!(
        r["models"]["added"],
        json!(["every_pad.step", "r_0402.step"])
    );
    p.ok(&["check"]);
    // Again: nothing.
    assert_eq!(p.ok(&args)["files"], json!([]));
    // A model that isn't where the import looks is said, by the file
    // that names it.
    let r = p.ok(&[
        "import",
        "kicad",
        "../kicad/footprints/Ryu_Test.pretty",
        "--dry-run",
    ]);
    let unresolved: Vec<&str> = r["notices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["code"] == "import_unresolved")
        .map(|n| n["details"]["path"].as_str().unwrap())
        .collect();
    assert_eq!(unresolved.len(), 2, "{r}");
    for at in unresolved {
        assert!(
            at.starts_with("<root>") && at.ends_with(".kicad_mod"),
            "{at}"
        );
    }
    // Legacy files and unknown aliases are refused.
    p.write("kicad/old.lib", "EESchema-LIBRARY Version 2.4\n");
    p.fails(
        &["import", "kicad", "../kicad/old.lib"],
        "kicad_legacy_unsupported",
        1,
    );
    let mut nowhere = args.to_vec();
    nowhere.extend(["--into", "nowhere"]);
    p.fails(&nowhere, "library_missing", 1);
}
