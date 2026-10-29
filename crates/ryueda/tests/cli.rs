//! The CLI as an agent sees it: run the binary, parse stdout as JSON.
//!
//! Every scenario runs twice, in two identical projects: once in process
//! (`RYUEDA_NO_DAEMON=1`) and once through the project's daemon. Both must
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
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        p.run_in(
            &p.daemon,
            "board",
            &["lib", "add", "passives", "../passives"]
        )
        .0,
        0
    );
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
