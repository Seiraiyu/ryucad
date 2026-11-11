//! The CLI as an agent sees it: run the binary, parse stdout as JSON.
//!
//! Every scenario runs twice, in two identical projects: once in process
//! (`RYUEDA_NO_DAEMON=1`) and once through the project's daemon. Both must
//! give the same JSON and the same exit code.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;

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
        // A parts cache of the test's own: no catalogue this machine holds.
        .env("RYUEDA_CACHE_DIR", run.with_file_name("cache"))
        // Random IDs a fixed sequence, the same in process and through a
        // daemon.
        .env("RYUEDA_ID_SEED", "cli")
        .env_remove("RYUEDA_NO_DAEMON")
        .env_remove("RYUEDA_PARTICIPANT");
    // No person's distributor keys or API: a scenario sets the fake's.
    for v in ryueda_parts::distributor::env_vars() {
        cmd.env_remove(v);
    }
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
    /// Variables every command, and so the daemon it starts, gets.
    env: Vec<(String, String)>,
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
            env: Vec::new(),
        }
    }

    /// [`Pair::new`] whose every command gets `env`.
    fn with_env(env: &[(&str, &str)]) -> Pair {
        let mut p = Pair::new();
        p.env = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        p
    }

    fn command_in(&self, top: &Path, sub: &str, args: &[&str]) -> Command {
        let mut cmd = ryueda(&self.run);
        cmd.current_dir(top.join(sub)).args(args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        if top == self.direct {
            cmd.env("RYUEDA_NO_DAEMON", "1");
        }
        cmd
    }

    fn run_in(&self, top: &Path, sub: &str, args: &[&str]) -> (i32, Value) {
        parse(self.command_in(top, sub, args).output().unwrap(), top)
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
        ("project.edit", &["project", "edit"]),
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
        ("parts.search", &["parts", "search"]),
        ("parts.sync", &["parts", "sync"]),
        ("parts.refresh", &["parts", "refresh"]),
        ("parts.sources", &["parts", "sources"]),
        ("parts.show", &["parts", "show"]),
        ("parts.configure", &["parts", "configure"]),
        ("import.kicad", &["import", "kicad"]),
        ("rail.add", &["rail", "add"]),
        ("rail.edit", &["rail", "edit"]),
        ("tag", &["tag"]),
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
        ("lint.get", &["lint"]),
        ("lint.configure", &["lint", "configure"]),
        ("lib.check", &["lib", "check"]),
        ("style.get", &["style"]),
        ("style.explain", &["style", "explain"]),
        ("style.set", &["style", "set"]),
        ("style.unset", &["style", "unset"]),
        ("style.rule", &["style", "rule", "add"]),
        ("style.palette", &["style", "palette", "set"]),
        ("standards.get", &["standards"]),
        ("standards.set", &["standards", "set"]),
        ("check", &["check"]),
        ("layout", &["layout"]),
        ("layout.place", &["place"]),
        ("layout.instructions", &["place"]),
        ("describe", &["describe"]),
        ("render", &["render"]),
        ("output", &["output"]),
        ("activity.log", &["history"]),
        ("undo", &["undo"]),
        ("redo", &["redo"]),
        ("session.hello", &["participants"]),
        ("participant.list", &["participants"]),
        ("subscribe", &["watch"]),
        ("request.cancel", &["cancel"]),
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
        Pair::projects_in(Pair::new())
    }

    fn projects_in(p: Pair) -> Pair {
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
    // A vocabulary parameter that doesn't read is a warning, not a
    // refusal, and the file stays at format 1 (design 347).
    let warned = p.ok(&["libcomp", "edit", "r_100k", "--param", "capacitance=lots"]);
    assert_eq!(warned["faults"][0]["code"], "parameter_invalid", "{warned}");
    assert_eq!(warned["faults"][0]["severity"], "warning");
    assert_eq!(
        warned["faults"][0]["details"]["pointer"],
        "/parameters/capacitance"
    );
    let text = fs::read_to_string(p.direct.join("board/library/components/r_100k.json")).unwrap();
    assert!(text.contains("\"format\": 1"), "{text}");
    p.ok(&["libcomp", "edit", "r_100k", "--unset-param", "capacitance"]);
    let shown = p.ok(&["libcomp", "show", "r_100k"]);
    assert_eq!(
        (&shown["category"], &shown["category_inferred"]),
        (&json!("resistor"), &json!(true))
    );
    let alt = p.ok(&[
        "libcomp",
        "edit",
        "r_100k",
        "--category",
        "resistor",
        "--alternate",
        "Vishay:CRCW0402100KFKED",
        "--alternate-offer",
        "CRCW0402100KFKED",
        "Mouser=71-CRCW0402100KFKED",
    ]);
    assert_eq!(
        alt["libcomp"]["alternates"],
        json!([{"manufacturer": "Vishay", "mpn": "CRCW0402100KFKED",
                "offers": [{"supplier": "Mouser", "sku": "71-CRCW0402100KFKED"}]}])
    );
    assert_eq!(alt["category_inferred"], json!(false));
    let at = p.direct.join("board/library/components/r_100k.json");
    assert!(fs::read_to_string(&at).unwrap().contains("\"format\": 2"));
    p.fails(
        &[
            "libcomp",
            "edit",
            "r_100k",
            "--alternate",
            "Yageo:RC0402FR-07100KL",
        ],
        "duplicate_id",
        1,
    );
    p.fails(
        &["libcomp", "edit", "r_100k", "--category", "valve"],
        "invalid_param",
        1,
    );
    p.fails(
        &["libcomp", "edit", "r_100k", "--alternate", "no-colon"],
        "usage",
        2,
    );
    p.ok(&[
        "libcomp",
        "edit",
        "r_100k",
        "--category",
        "",
        "--remove-alternate",
        "CRCW0402100KFKED",
    ]);
    assert!(fs::read_to_string(&at).unwrap().contains("\"format\": 1"));
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
        Pair::with_resistor_in(Pair::new())
    }

    /// [`Pair::with_resistor`] with a block `top` holding `R1` (`r1`)
    /// on a net `SDA` (plan 2.E's style scenarios).
    fn with_placed_resistor() -> Pair {
        let p = Pair::with_resistor();
        p.ok(&["block", "create", "Top", "--id", "top"]);
        p.ok(&["component", "add", "top", "--of", "r_100k", "--id", "r1"]);
        p.ok(&["connect", "top/r1/1", "SDA", "--new"]);
        p.ok(&["design", "create", "Board", "--root", "top", "--id", "d"]);
        p
    }

    fn with_resistor_in(p: Pair) -> Pair {
        let p = Pair::projects_in(p);
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
            "--manufacturer",
            "Yageo",
            "--mpn",
            "RC0402FR-07100KL",
            "--offer",
            "LCSC=C25741",
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
    assert_eq!(a["instance"]["refdes"], json!("{refdes}_A"));
    let b = p.ok(&["instance", "add", "top", "--of", "ch", "--offset", "100"]);
    assert_eq!(
        b["instance"]["refdes"],
        json!("{prefix}{number + 100}{rest}")
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
    // `check` reads every file and runs ERC on every design. The two
    // warnings: its parts have LCSC offers and this machine has no
    // catalogue to judge their supply by, and its resistor `r` is a box
    // under the built-in ANSI standards (sym_shape).
    let report = p.ok(&["check"]);
    assert_eq!(
        (&report["errors"], &report["warnings"]),
        (&json!(0), &json!(2)),
        "{report}"
    );
    assert_eq!(report["supply"][0]["details"]["reason"], "missing");
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

#[test]
fn properties_and_designator_formats_from_the_cli() {
    let p = Pair::breakout();
    let e = p.ok(&[
        "project",
        "edit",
        "--property",
        "company=Seiraiyu",
        "--sheet-size",
        "a4",
        "--orientation",
        "portrait",
    ]);
    assert_eq!(e["sheet_nm"], json!([210_000_000, 297_000_000]));
    assert_eq!(e["properties"], json!({"company": "Seiraiyu"}));
    let m = fs::read_to_string(p.direct.join("board/ryueda.json")).unwrap();
    assert!(m.contains("\"format\": 2"), "{m}");
    p.fails(
        &["project", "edit", "--property", "refdes=x"],
        "invalid_param",
        1,
    );
    p.fails(&["project", "edit", "--sheet-size", "a7"], "usage", 2);
    p.ok(&["block", "edit", "sensor", "--property", "base=100"]);
    p.ok(&[
        "instance",
        "add",
        "breakout",
        "--of",
        "sensor",
        "--id",
        "s2",
        "--port",
        "SDA=SDA",
        "--port",
        "SCL=SCL",
        "--refdes",
        "offset_hundreds",
    ]);
    p.ok(&[
        "instance",
        "edit",
        "breakout/s1",
        "--refdes",
        "{prefix}{number + base}{rest}",
        "--property",
        "base=500",
    ]);
    let refdes = |p: &Pair| -> Vec<(String, String)> {
        p.ok(&["netlist"])["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["reference"].as_str().unwrap().contains('/'))
            .map(|o| {
                (
                    o["reference"].as_str().unwrap().to_owned(),
                    o["refdes"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect()
    };
    let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    };
    // The instance's own property first; the second layer of the stack
    // takes the preset's hundreds.
    assert_eq!(
        refdes(&p),
        pairs(&[
            ("board:s1/c1", "C501"),
            ("board:s1/u1", "U501"),
            ("board:s2/c1", "C201"),
            ("board:s2/u1", "U201"),
        ])
    );
    // Without it, the block's default.
    p.ok(&[
        "instance",
        "edit",
        "breakout/s1",
        "--unset-property",
        "base",
    ]);
    assert_eq!(refdes(&p)[1], pairs(&[("board:s1/u1", "U101")])[0]);
    // A format that doesn't parse is refused; one naming a property no
    // level sets leaves no designator, and ERC says why.
    p.fails(
        &["instance", "edit", "breakout/s1", "--refdes", "{refdes"],
        "refdes_format_invalid",
        1,
    );
    p.ok(&[
        "instance",
        "edit",
        "breakout/s1",
        "--refdes",
        "{refdes}{nope}",
    ]);
    let erc = p.ok(&["erc"]);
    let missing: Vec<&Value> = erc["designs"][0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "refdes_missing")
        .collect();
    assert_eq!(missing.len(), 2, "{erc}");
    assert_eq!(
        missing[0]["details"]["error"]["code"],
        json!("property_unknown")
    );
}

/// Copies a file the daemon side of a pair wrote, for people to look at
/// (`target/ryueda-look`).
fn keep_for_looking(from: &Path, name: &str) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ryueda-look");
    fs::create_dir_all(&dir).unwrap();
    fs::copy(from, dir.join(name)).unwrap();
}

/// The project's own standards: read whole, changed by a merge patch,
/// written only where they differ from the built-in ones, undone.
#[test]
fn standards_from_the_cli() {
    let p = Pair::projects();
    let got = p.ok(&["standards"]);
    assert_eq!(got["from"], "built_in", "{got}");
    assert_eq!(got["standards"]["shapes"], "ansi");
    assert_eq!(got["standards"]["grid_nm"], 1_270_000);
    assert!(got["standards"]["classes"]["ic"]["sides"]["power_in"] == "up");
    let set = p.ok(&[
        "standards",
        "set",
        r#"{"shapes": "iec", "classes": {"ic": {"max_pins_per_unit": 100}}}"#,
    ]);
    assert_eq!(set["from"], "self", "{set}");
    assert_eq!(set["standards"]["shapes"], "iec");
    // A class patched keeps the rest of its rule.
    assert_eq!(set["standards"]["classes"]["ic"]["max_pins_per_unit"], 100);
    assert_eq!(set["standards"]["classes"]["ic"]["sides"]["power_in"], "up");
    let file: Value =
        serde_json::from_slice(&fs::read(p.direct.join("board/library/standards.json")).unwrap())
            .unwrap();
    assert_eq!(file["shapes"], "iec");
    assert!(file.get("grid_nm").is_none(), "{file}");
    // The library project has none of its own: the project's judge it.
    assert_eq!(
        p.ok(&["standards", "--library", "passives"])["from"],
        "self"
    );
    // Refusals: a bad value, a field no standards have.
    let e = p.fails(
        &["standards", "set", r#"{"grid_nm": 0}"#],
        "standards_invalid",
        1,
    );
    assert_eq!(e["details"]["faults"][0]["details"]["pointer"], "/grid_nm");
    p.fails(
        &["standards", "set", r#"{"colour": "red"}"#],
        "invalid_param",
        1,
    );
    // Back to the built-in shapes and class: the file goes.
    p.ok(&["standards", "set", r#"{"shapes": null, "classes": null}"#]);
    assert!(!p.direct.join("board/library/standards.json").exists());
}

/// A symbol's class: given, changed, taken off; a symbol with none stays
/// format 1.
#[test]
fn a_symbols_class_from_the_cli() {
    let p = Pair::projects();
    let made = p.ok(&[
        "symbol", "create", "R", "--ops", RESISTOR, "--class", "resistor",
    ]);
    assert_eq!(made["symbol"]["class"], "resistor", "{made}");
    let file = fs::read_to_string(p.direct.join("board/library/symbols/r.json")).unwrap();
    assert!(file.contains("\"format\": 2"), "{file}");
    let edited = p.ok(&["symbol", "edit", "r", "--class", "none"]);
    assert!(edited["symbol"].get("class").is_none(), "{edited}");
    let file = fs::read_to_string(p.direct.join("board/library/symbols/r.json")).unwrap();
    assert!(file.contains("\"format\": 1"), "{file}");
    let e = p.fails(
        &["symbol", "edit", "r", "--class", "gizmo"],
        "invalid_param",
        1,
    );
    assert!(
        e["details"]["classes"]
            .as_array()
            .unwrap()
            .contains(&json!("ic"))
    );
}

/// Symbols drawn from the standards' templates: a resistor to the
/// project's shapes, an IC from its pins, ops applied after.
#[test]
fn symbols_from_templates() {
    let p = Pair::projects();
    let r = p.ok(&["symbol", "create", "R", "--template", "resistor"]);
    assert_eq!(r["symbol"]["class"], "resistor", "{r}");
    assert_eq!(r["symbol"]["graphics"]["g1"]["type"], "polyline");
    p.ok(&["standards", "set", r#"{"shapes": "iec"}"#]);
    let box_r = p.ok(&["symbol", "create", "R box", "--template", "resistor"]);
    assert_eq!(box_r["symbol"]["graphics"]["g1"]["type"], "rect");
    let u = p.ok(&[
        "symbol",
        "create",
        "Sensor",
        "--template",
        "ic",
        "--pins",
        r#"[{"number": "1", "name": "VDD", "type": "power_in"},
            {"number": "2", "name": "GND", "type": "power_in"},
            {"number": "3", "name": "SDA", "type": "bidirectional"},
            {"number": "4", "name": "SCL", "type": "bidirectional"}]"#,
        "--ops",
        r#"[{"op": "add", "graphic": "text", "text": "BME", "at": [0, 0], "size": "1.27 mm"}]"#,
    ]);
    let pins = &u["symbol"]["pins"];
    assert_eq!(
        (&pins["1"]["side"], &pins["2"]["side"]),
        (&json!("up"), &json!("down"))
    );
    assert_eq!(u["added"], json!(["g2"]));
    // Refusals.
    p.fails(
        &["symbol", "create", "Q", "--template", "transistor"],
        "invalid_param",
        1,
    );
    p.fails(
        &[
            "symbol",
            "create",
            "X",
            "--pins",
            r#"[{"number": "1", "type": "input"}]"#,
        ],
        "invalid_param",
        1,
    );
    p.fails(
        &[
            "symbol",
            "create",
            "X",
            "--template",
            "ic",
            "--class",
            "led",
            "--pins",
            r#"[{"number": "1", "type": "input"}]"#,
        ],
        "invalid_param",
        1,
    );
}

/// A library's symbols judged: the box resistor under ANSI standards, its
/// class inferred from the component that places it, its fix applied.
#[test]
fn a_library_is_checked_from_the_cli() {
    let p = Pair::with_resistor();
    let c = p.ok(&["lib", "check"]);
    assert_eq!(c["library"], "self");
    assert_eq!(c["standards"]["from"], "built_in");
    let r = &c["symbols"][0];
    assert_eq!(
        (&r["reference"], &r["class"], &r["class_inferred"]),
        (
            &json!("lib/self/symbol/r"),
            &json!("resistor"),
            &json!(true)
        ),
        "{c}"
    );
    let shape = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "sym_shape")
        .unwrap()
        .clone();
    let points: i64 = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["points"].as_i64().unwrap())
        .sum();
    assert_eq!(r["score"].as_i64().unwrap(), (100 - points).max(0));
    let fix = &shape["fixes"][0];
    assert_eq!(fix["symbol"], "r");
    let ops = serde_json::to_string(&fix["ops"]).unwrap();
    p.ok(&["symbol", "edit", "r", "--ops", &ops]);
    let c = p.ok(&["lib", "check", "--symbol", "r"]);
    assert!(
        !c["symbols"][0]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "sym_shape"),
        "{c}"
    );
    // The library project, and a rule raised to an error.
    assert_eq!(
        p.ok(&["lib", "check", "--library", "passives"])["symbols"],
        json!([])
    );
    p.ok(&["lint", "configure", "--severity", "sym_pin_side=error"]);
    p.ok(&[
        "symbol",
        "create",
        "U",
        "--template",
        "ic",
        "--pins",
        r#"[{"number": "1", "name": "VDD", "type": "power_in"}]"#,
    ]);
    p.ok(&[
        "symbol",
        "edit",
        "u",
        "--ops",
        r#"[{"op": "set", "pin": "1", "side": "left"}]"#,
    ]);
    p.fails(&["lib", "check"], "check_failed", 1);
    p.fails(&["lib", "check", "--symbol", "nope"], "not_found", 1);
}

/// check judges the project's own symbols and those its designs place
/// from a library project, and fails on a rule raised to an error.
#[test]
fn check_judges_the_symbols_a_project_draws() {
    let p = Pair::with_resistor();
    let report = p.ok(&["check"]);
    let sym: Vec<&Value> = report["symbols"].as_array().unwrap().iter().collect();
    assert!(
        sym.iter()
            .any(|f| f["code"] == "sym_shape" && f["details"]["symbol"] == "lib/self/symbol/r"),
        "{report}"
    );
    assert!(sym.iter().all(|f| f["severity"] == "warning"));
    assert!(report["warnings"].as_u64().unwrap() >= sym.len() as u64);
    p.ok(&["lint", "configure", "--severity", "sym_shape=error"]);
    let e = p.fails(&["check"], "check_failed", 1);
    assert!(
        e["details"]["report"]["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "sym_shape" && f["severity"] == "error")
    );
}

#[test]
fn lint_preferences_from_the_cli() {
    let p = Pair::breakout();
    p.ok(&["layout"]);
    // The defaults: no file, every rule with its weight.
    let l = p.ok(&["lint"]);
    assert_eq!(
        l["prefs"]["prefer"],
        json!({"resistor": "us", "local_nets": "wires"})
    );
    assert!(
        l["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["code"] == "sch_topology")
    );
    // Every rule says what it finds, why, how to fix it and its evidence;
    // --explain gives one with the project's weight and severity for it.
    let e = p.ok(&["lint", "--explain", "sch_floating"]);
    for f in ["code", "weight", "unit", "what", "why", "fix", "evidence"] {
        assert!(
            e["rule"][f].is_string() || e["rule"][f].is_number(),
            "{f}: {e}"
        );
    }
    assert_eq!(e["weight_now"], json!(8));
    assert_eq!(e["severity_now"], json!("warning"));
    let u = p.fails(&["lint", "--explain", "sch_nonsense"], "not_found", 1);
    assert!(
        u["details"]["codes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "sch_floating"),
        "{u}"
    );
    let sheet = |d: &Value| d["blocks"][0]["sheets"][0].clone();
    let codes = |d: &Value| -> Vec<String> {
        sheet(d)["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["code"].as_str().unwrap().to_owned())
            .collect()
    };
    let before = p.ok(&["describe", "--block", "breakout"]);
    assert!(!codes(&before).contains(&"sch_resistor_style".to_owned()));
    // The box preferred: R1 and R2 are zigzags, and their symbol is judged
    // once, in the library, not on the sheet.
    let tried = p.ok(&["lint", "configure", "--prefer", "resistor=iec", "--dry-run"]);
    assert!(!p.direct.join("board/lint.json").exists());
    let came: Vec<&Value> = tried["symbols"]["came"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "sym_shape")
        .collect();
    assert_eq!(came.len(), 1, "{tried}");
    p.ok(&[
        "lint",
        "configure",
        "--prefer",
        "resistor=iec",
        "--weight",
        "sch_resistor_style=4",
    ]);
    let lint = p.ok(&["lint"]);
    assert_eq!(lint["prefs"]["weights"]["sym_shape"], 4, "{lint}");
    let after = p.ok(&["describe", "--block", "breakout"]);
    assert_eq!(
        sheet(&after)["score"],
        sheet(&before)["score"],
        "a resistor's drawing costs the sheet nothing"
    );
    // A rule made an error fails the check.
    p.ok(&["lint", "configure", "--severity", "sch_out_of_line=error"]);
    p.fails(&["check"], "check_failed", 1);
    // Every preference back to its default: the file goes, the check passes.
    p.ok(&[
        "lint",
        "configure",
        "--prefer",
        "resistor=default",
        "--weight",
        "sym_shape=default",
        "--severity",
        "sch_out_of_line=default",
    ]);
    assert!(!p.direct.join("board/lint.json").exists());
    p.ok(&["check"]);
    // Refusals name what they refuse.
    let e = p.fails(
        &["lint", "configure", "--weight", "sch_nonsense=1"],
        "invalid_param",
        1,
    );
    assert_eq!(e["details"]["param"], json!("weights/sch_nonsense"));
    p.fails(
        &["lint", "configure", "--prefer", "resistor=round"],
        "invalid_param",
        1,
    );
}

#[test]
fn schematics_from_the_cli() {
    let p = Pair::breakout();
    // Laid out from nothing: both blocks written, each one sheet.
    let l = p.ok(&["layout"]);
    let blocks: Vec<&str> = l["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["block"].as_str().unwrap())
        .collect();
    assert_eq!(blocks, ["breakout", "sensor"]);
    assert!(
        l["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["written"] == true)
    );
    assert!(p.direct.join("board/blocks/breakout/layout.json").is_file());
    // Again: nothing changes.
    let again = p.ok(&["layout"]);
    assert!(
        again["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["written"] == false)
    );
    // A description names idioms and neighbours.
    let d = p.ok(&["describe", "--block", "breakout"]);
    let idioms: Vec<&str> = d["blocks"][0]["idioms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert!(
        idioms.contains(&"pull_up") && idioms.contains(&"decoupling"),
        "{idioms:?}"
    );
    // Renders, SVG and PNG.
    let r = p.ok(&["render", "--out", "breakout.svg"]);
    assert_eq!(
        (&r["block"], &r["binary"]["encoding"]),
        (&json!("breakout"), &json!("image/svg+xml"))
    );
    let svg = fs::read_to_string(p.daemon.join("board/breakout.svg")).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains(">U2<"));
    assert_eq!(
        svg,
        fs::read_to_string(p.direct.join("board/breakout.svg")).unwrap()
    );
    p.ok(&[
        "render",
        "--block",
        "sensor",
        "--out",
        "sensor.png",
        "--scale",
        "6",
    ]);
    let png = fs::read(p.daemon.join("board/sensor.png")).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    keep_for_looking(&p.daemon.join("board/breakout.svg"), "breakout.svg");
    p.ok(&["render", "--out", "breakout.png", "--fit", "--scale", "12"]);
    keep_for_looking(&p.daemon.join("board/breakout.png"), "breakout.png");
    keep_for_looking(&p.daemon.join("board/sensor.png"), "sensor.png");
    // The output job: a page per sheet, the same bytes both ways.
    let o = p.ok(&["output", "--out", "breakout.pdf"]);
    assert_eq!(o["binary"]["encoding"], json!("application/pdf"));
    let pages: Vec<&str> = o["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["block"].as_str().unwrap())
        .collect();
    assert_eq!(pages, ["breakout", "sensor"]);
    let pdf = fs::read(p.daemon.join("board/breakout.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    assert_eq!(pdf, fs::read(p.direct.join("board/breakout.pdf")).unwrap());
    keep_for_looking(&p.daemon.join("board/breakout.pdf"), "breakout.pdf");
    p.fails(
        &["output", "--layers", "some", "--out", "x.pdf"],
        "usage",
        2,
    );
    // Instructions: the pull-ups beside the connector, a net as labels.
    let placed = p.ok(&["place", "breakout", "beside R1 J1 right", "net SDA label"]);
    assert_eq!(placed["added"], json!(["i1", "i2"]));
    let list = p.ok(&["place", "breakout", "--list"]);
    assert_eq!(list["instructions"][0]["text"], json!("beside R1 J1 right"));
    // R1 is on J1's right but past the lint's reach (round 1, F1: holds
    // judged the side alone), so it doesn't hold, and says why.
    assert_eq!(list["instructions"][0]["holds"], json!(false));
    assert!(
        list["instructions"][0]["why"]
            .as_str()
            .unwrap()
            .contains("mm from J1; beside means within"),
        "{list}"
    );
    // Repeating one changes nothing; a conflict is refused with fixes.
    assert_eq!(
        p.ok(&["place", "breakout", "net SDA label"])["added"],
        json!([])
    );
    let e = p.fails(
        &["place", "breakout", "edge R1 left"],
        "instruction_conflict",
        1,
    );
    assert_eq!(e["details"]["instructions"], json!(["i1", "i3"]));
    assert_eq!(
        e["details"]["fixes"][0],
        json!(["release R1", "edge R1 left"])
    );
    let e = p.fails(&["place", "breakout", "net SDA port"], "layout_rule", 1);
    assert!(
        e["details"]["fixes"].to_string().contains("net SDA label"),
        "{e}"
    );
    // A conflict inside one batch offers each instruction alone, never an
    // empty batch; a net's conflict names the net (round 1, F9).
    let e = p.fails(
        &[
            "place",
            "breakout",
            "--dry-run",
            "orient J1 90",
            "orient J1 180",
        ],
        "instruction_conflict",
        1,
    );
    assert_eq!(
        e["details"]["fixes"],
        json!([["orient J1 90"], ["orient J1 180"]]),
        "{e}"
    );
    let e = p.fails(
        &["place", "breakout", "net SDA wire"],
        "instruction_conflict",
        1,
    );
    assert!(
        e["message"].as_str().unwrap().contains("SDA's style"),
        "{e}"
    );
    // A net's instruction is released by its ID or its net (round 1, F9).
    assert_eq!(
        e["details"]["fixes"][0],
        json!(["release i2", "net SDA wire"]),
        "{e}"
    );
    assert!(
        !e["details"]["fixes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_array().unwrap().is_empty()),
        "{e}"
    );
    let dry = p.ok(&["place", "breakout", "--dry-run", "release i2"]);
    assert_eq!(dry["released"], json!(["i2"]));
    let dry = p.ok(&["place", "breakout", "--dry-run", "release net:SDA"]);
    assert_eq!(dry["released"], json!(["i2"]));
    let dry = p.ok(&["place", "breakout", "--dry-run", "release SDA"]);
    assert_eq!(dry["released"], json!(["i2"]));
    // A refusal in a batch quotes the instruction (round 1, F12).
    let e = p.fails(
        &[
            "place",
            "breakout",
            "--dry-run",
            "align R1 R2 row",
            "beside R9 J1 right",
        ],
        "not_found",
        1,
    );
    assert_eq!(e["details"]["text"], json!("beside R9 J1 right"), "{e}");
    assert_eq!(e["details"]["index"], json!(1), "{e}");
    let e = p.fails(
        &[
            "place",
            "breakout",
            "--dry-run",
            "--move",
            "beside R9 J1 right",
        ],
        "not_found",
        1,
    );
    assert_eq!(e["details"]["text"], json!("beside R9 J1 right"), "{e}");
    p.fails(&["place", "breakout", "release i9"], "not_found", 1);
    p.fails(&["place", "breakout", "release net:SCL"], "not_found", 1);
    p.fails(&["place", "breakout", "beside R9 J1 right"], "not_found", 1);
    p.fails(
        &["place", "breakout", "beside R1 J1 sideways"],
        "invalid_param",
        1,
    );
    // Sending the fix works.
    p.ok(&["place", "breakout", "release R1", "edge R1 left"]);
    // A move is held; a dry run writes nothing.
    let before = fs::read(p.direct.join("board/blocks/breakout/layout.json")).unwrap();
    let dry = p.ok(&[
        "place",
        "breakout",
        "--dry-run",
        "--move",
        "beside C2 U2 below",
    ]);
    assert!(dry["description"]["blocks"][0]["sheets"].is_array());
    assert_eq!(
        fs::read(p.direct.join("board/blocks/breakout/layout.json")).unwrap(),
        before
    );
    p.ok(&["place", "breakout", "--move", "beside C2 U2 below"]);
    let laid: Value = serde_json::from_slice(
        &fs::read(p.direct.join("board/blocks/breakout/layout.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(laid["placed"]["c2"]["held"], json!(true));
    // A connectivity write keeps the layout current in the same change,
    // and undo takes both back.
    p.ok(&[
        "component",
        "add",
        "breakout",
        "--of",
        "c_100n",
        "--id",
        "c9",
    ]);
    let laid: Value = serde_json::from_slice(
        &fs::read(p.daemon.join("board/blocks/breakout/layout.json")).unwrap(),
    )
    .unwrap();
    assert!(laid["placed"]["c9"].is_object(), "{laid}");
    let (code, _) = p.run_in(&p.daemon, "board", &["undo"]);
    assert_eq!(code, 0);
    let laid: Value = serde_json::from_slice(
        &fs::read(p.daemon.join("board/blocks/breakout/layout.json")).unwrap(),
    )
    .unwrap();
    assert!(laid["placed"].get("c9").is_none());
    // In process there is no undo: delete it there, so both sides agree.
    let (code, _) = p.run_in(&p.direct, "board", &["component", "delete", "breakout/c9"]);
    assert_eq!(code, 0);
    // A layout file edited by hand is stale: laid out in memory, written
    // only by `layout`.
    let path = "board/blocks/sensor/layout.json";
    let mut l: Value = serde_json::from_slice(&fs::read(p.direct.join(path)).unwrap()).unwrap();
    l["text"] = json!({});
    p.write(path, &ryueda::canonical::to_string(&l));
    let d = p.ok(&["describe", "--block", "sensor"]);
    assert_eq!(d["blocks"][0]["stale"], json!(true));
    let c = p.ok(&["check"]);
    let codes: Vec<&str> = c["schematic"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"layout_stale"), "{codes:?}");
    assert!(
        codes
            .iter()
            .all(|c| *c == "layout_stale" || c.starts_with("sch_")),
        "{codes:?}"
    );
    let l = p.ok(&["layout", "--block", "sensor"]);
    assert_eq!(l["blocks"][0]["written"], json!(true));
    assert_eq!(
        p.ok(&["describe", "--block", "sensor"])["blocks"][0].get("stale"),
        None
    );
}

/// Plan 2.A's regions: describe gives each sheet's regions, `region
/// rename` makes a lasting named region, and two regions naming one part
/// are refused.
#[test]
fn regions_from_the_cli() {
    let p = Pair::breakout();
    p.ok(&["layout", "--relayout", "sensor"]);
    let d = p.ok(&["describe", "--block", "sensor"]);
    let regions = d["blocks"][0]["sheets"][0]["regions"]
        .as_array()
        .unwrap()
        .clone();
    assert!(!regions.is_empty(), "{d}");
    let id = regions[0]["id"].as_str().unwrap().to_owned();
    p.ok(&[
        "place",
        "sensor",
        &format!("region rename {id} \"Sensor core\""),
    ]);
    let list = p.ok(&["place", "sensor", "--list"]);
    assert!(
        list["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["text"]
                .as_str()
                .unwrap()
                .starts_with("region \"Sensor core\" ")),
        "{list}"
    );
    let d = p.ok(&["describe", "--block", "sensor"]);
    assert!(
        d["blocks"][0]["sheets"][0]["regions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["title"] == "Sensor core" && r["named"] == true),
        "{d}"
    );
    let refused = p.fails(
        &["place", "sensor", "region Other U1", "region Again U1"],
        "instruction_conflict",
        1,
    );
    assert!(
        refused["message"].as_str().unwrap().contains("two regions"),
        "{refused}"
    );
    let (code, _) = p.run_in(&p.daemon, "board", &["undo"]);
    assert_eq!(code, 0);
}

/// Plan 2.A's paper fit: a small block takes A4, drawn and paged so.
#[test]
fn each_sheet_is_drawn_and_paged_at_its_own_paper() {
    let p = Pair::breakout();
    p.ok(&["layout", "--relayout", "sensor"]);
    let d = p.ok(&["describe", "--block", "sensor"]);
    assert_eq!(d["blocks"][0]["sheets"][0]["paper"]["size"], "a4", "{d}");
    p.ok(&["render", "--block", "sensor", "--out", "s.svg"]);
    let svg = fs::read_to_string(p.daemon.join("board/s.svg")).unwrap();
    // The A4 sheet's drawing area: 277 by 170 mm (the title band takes
    // 20 mm at its foot, plan 2.E).
    assert!(svg.contains("width=\"277\" height=\"170\""), "{svg}");
    p.ok(&["output", "--out", "s.pdf"]);
    let pdf = fs::read(p.daemon.join("board/s.pdf")).unwrap();
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("/MediaBox [0 0 841.8"), "an A4 page");
    // Its layers and its SemiBold designators and title (plan 2.E).
    assert!(text.contains("/OCProperties") && text.contains("+Inter-SemiBold"));
}

/// Parametric search over the project's libraries (design 347), in
/// process and through the daemon alike.
#[test]
fn parts_from_the_cli() {
    let p = Pair::with_resistor();
    for (name, value, params, mpn) in [
        (
            "R 4k7",
            "4k7",
            ["resistance=4.7 kOhm", "package=0402", "tolerance=1%"],
            "RC0402FR-074K7L",
        ),
        (
            "R 10k",
            "10k",
            ["resistance=10 kOhm", "package=0603", "tolerance=5%"],
            "RC0603JR-0710KL",
        ),
    ] {
        let mut args = vec![
            "libcomp",
            "create",
            name,
            "--symbol",
            "r",
            "--prefix",
            "R",
            "--value",
            value,
            "--footprint",
            "lib/passives/footprint/r_0402",
            "--mpn",
            mpn,
        ];
        for x in &params {
            args.push("--param");
            args.push(x);
        }
        p.ok(&args);
    }
    let keys = |v: &Value| -> Vec<String> {
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["key"].as_str().unwrap().to_owned())
            .collect()
    };
    let found = p.ok(&["parts", "search", "resistor", "resistance=4700 Ohm"]);
    assert_eq!(
        keys(&found),
        ["lib/self/libcomp/r_4k7", "lib/self/libcomp/r_100k"]
    );
    assert_eq!(found["results"][0]["meets"], 1);
    assert_eq!(found["results"][1]["unknown"], json!(["resistance"]));
    assert_eq!(found["query"]["conditions"], json!(["resistance=4700 Ohm"]));
    // Tolerances compare as fractions; the part that doesn't say comes last.
    let found = p.ok(&["parts", "search", "resistor", "tolerance<=5%"]);
    assert_eq!(
        keys(&found),
        [
            "lib/self/libcomp/r_10k",
            "lib/self/libcomp/r_4k7",
            "lib/self/libcomp/r_100k"
        ]
    );
    assert!(keys(&p.ok(&["parts", "search", "capacitor"])).is_empty());
    assert_eq!(keys(&p.ok(&["parts", "search", "--limit", "1"])).len(), 1);
    // Like a library component, or a placed one.
    let like = p.ok(&["parts", "search", "--like", "r_4k7"]);
    assert_eq!(
        keys(&like),
        ["lib/self/libcomp/r_4k7", "lib/self/libcomp/r_100k"]
    );
    p.ok(&["block", "create", "Top", "--id", "top"]);
    p.ok(&["component", "add", "top", "--of", "r_4k7", "--id", "r1"]);
    assert_eq!(
        keys(&p.ok(&["parts", "search", "--like", "top/r1"])),
        keys(&like)
    );
    p.fails(&["parts", "search", "--in", "acme"], "invalid_param", 1);
    p.fails(&["parts", "search", "colour=red"], "invalid_param", 1);
    p.fails(
        &["parts", "search", "capacitance=lots"],
        "invalid_quantity",
        1,
    );
    p.fails(&["parts", "search", "valve"], "invalid_param", 1);
    p.fails(&["parts", "search", "--like", "nope"], "not_found", 1);
}

#[test]
fn parts_configure_from_the_cli() {
    let p = Pair::projects();
    let v = p.ok(&["parts", "configure"]);
    assert_eq!(v["config"], json!({}));
    let v = p.ok(&[
        "parts",
        "configure",
        "--assembly",
        "jlcpcb",
        "--build-quantity",
        "10",
        "--source",
        "library",
        "--source",
        "jlc",
        "--severity",
        "part_extended=error",
    ]);
    assert_eq!(
        v["config"],
        json!({"assembly": "jlcpcb", "build_quantity": 10,
               "sources": ["library", "jlc"], "severity": {"part_extended": "error"}})
    );
    assert!(p.direct.join("board/parts.json").is_file());
    let e = p.fails(
        &["parts", "configure", "--severity", "pin_conflict=off"],
        "invalid_param",
        1,
    );
    assert_eq!(e["details"]["param"], "severity/pin_conflict");
    p.fails(
        &["parts", "configure", "--build-quantity", "0"],
        "invalid_param",
        1,
    );
    p.fails(
        &["parts", "configure", "--source", "acme"],
        "invalid_param",
        1,
    );
    let v = p.ok(&[
        "parts",
        "configure",
        "--assembly",
        "none",
        "--build-quantity",
        "default",
        "--no-sources",
        "--severity",
        "part_extended=default",
    ]);
    assert_eq!(v["config"], json!({}));
    assert!(
        !p.direct.join("board/parts.json").exists(),
        "nothing set, no file"
    );
}

#[path = "support/fake_jlc.rs"]
mod fake_jlc;

#[test]
fn the_jlc_catalogue_from_the_cli() {
    let site = fake_jlc::start("2026-11-06T11:33:30+00:00", std::time::Duration::ZERO);
    let p = Pair::with_resistor_in(Pair::with_env(&[
        ("RYUEDA_JLC_URL", &site.base),
        ("RYUEDA_NOW", "1793964810"),
    ]));
    let v = p.ok(&["parts", "sources"]);
    assert_eq!(v["sources"][1]["state"], "missing");
    let v = p.ok(&["parts", "search", "capacitor", "--in", "jlc"]);
    assert_eq!(v["warnings"][0]["reason"], "missing");
    p.fails(&["parts", "show", "lcsc:C1525"], "parts_not_found", 1);
    let v = p.ok(&["parts", "sync", "jlc"]);
    assert_eq!(
        (v["parts"].as_u64(), v["files"].as_u64()),
        (Some(5), Some(5))
    );
    assert_eq!(v["fetched_at"], 1_793_964_810u64);
    let v = p.ok(&[
        "parts",
        "search",
        "capacitor",
        "capacitance=100nF",
        "--in",
        "jlc",
        "--basic",
        "--stock",
        "1000",
    ]);
    assert_eq!(v["results"][0]["key"], "C1525");
    assert!(v["warnings"].as_array().unwrap().is_empty());
    let v = p.ok(&["parts", "show", "lcsc:C1525"]);
    assert_eq!(v["records"][0]["stock"], 1_200_000);
    p.fails(&["parts", "show", "lcsc:C9"], "parts_not_found", 1);
    // A component from the record: identifiers only, its parameters typed.
    p.ok(&[
        "libcomp",
        "create",
        "C 1u",
        "--symbol",
        "r",
        "--prefix",
        "C",
        "--footprint",
        "lib/passives/footprint/r_0402",
        "--category",
        "capacitor",
        "--param",
        "package=0402",
    ]);
    let v = p.ok(&[
        "libcomp",
        "create",
        "100n",
        "--from",
        "lcsc:C1525",
        "--param",
        "capacitance=100 nF",
    ]);
    assert_eq!(v["libcomp"]["mpn"], "CL05B104KO5NNNC");
    assert_eq!(v["libcomp"]["parameters"], json!({"capacitance": "100 nF"}));
    let v = p.ok(&["libcomp", "edit", "100n", "--from", "lcsc:C1525"]);
    assert_eq!(v["changed"], json!([]));
    // Supply: two of C52923 (40 in stock, Extended), for 100 boards.
    let v = p.ok(&[
        "libcomp",
        "create",
        "placed",
        "--id",
        "placed",
        "--from",
        "lcsc:C52923",
        "--param",
        "capacitance=1 uF",
    ]);
    assert_eq!(
        v["chose"]["symbol"]["reference"], "lib/self/symbol/r",
        "{v}"
    );
    p.ok(&["block", "create", "Top", "--id", "top"]);
    for i in 0..2 {
        p.ok(&[
            "component",
            "add",
            "top",
            "--of",
            "placed",
            "--id",
            &format!("c{i}"),
        ]);
    }
    p.ok(&[
        "design", "create", "Board", "--root", "top", "--id", "board",
    ]);
    p.ok(&[
        "parts",
        "configure",
        "--assembly",
        "jlcpcb",
        "--build-quantity",
        "100",
    ]);
    let (_, v) = p.both_in("board", &["check"]);
    let report = if v["ok"] == true {
        &v["result"]
    } else {
        &v["error"]["details"]["report"]
    };
    let codes: Vec<&str> = report["supply"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["part_extended", "part_out_of_stock"], "{report}");
    assert_eq!(report["supply"][1]["details"]["needed"], 200);
    let v = p.ok(&["parts", "refresh"]);
    assert_eq!(v["fresh"][0]["source"], "jlc");
    let v = p.ok(&["parts", "refresh", "--force"]);
    assert_eq!(v["synced"][0]["source"], "jlc");
}

#[test]
fn a_sync_is_cancelled_by_its_token_and_keeps_the_old_catalogue() {
    let fast = fake_jlc::start("2026-11-06T11:33:30+00:00", std::time::Duration::ZERO);
    let slow = fake_jlc::start(
        "2026-11-07T00:00:00+00:00",
        std::time::Duration::from_millis(800),
    );
    let p = Pair::projects_in(Pair::with_env(&[("RYUEDA_JLC_URL", &slow.base)]));
    // The catalogue there first, in process from the fast site.
    let mut first = p.command_in(&p.direct, "board", &["parts", "sync", "jlc"]);
    first.env("RYUEDA_JLC_URL", &fast.base);
    assert_eq!(parse(first.output().unwrap(), &p.direct).0, 0);
    let (_, before) = p.run_in(&p.daemon, "board", &["parts", "sources"]);
    assert_eq!(
        before["result"]["sources"][1]["created"],
        "2026-11-06T11:33:30+00:00"
    );
    // Started without waiting: assert_cmd's commands only run to the end.
    #[allow(deprecated)]
    let mut running = std::process::Command::new(assert_cmd::cargo::cargo_bin("ryueda"));
    running
        .current_dir(p.daemon.join("board"))
        .args(["parts", "sync", "jlc", "--token", "sync1", "--progress"])
        .env("RYUEDA_RUNTIME_DIR", &p.run)
        .env("RYUEDA_IDLE_TIMEOUT", "60")
        .env("RYUEDA_CONFIG_DIR", p.run.with_file_name("config"))
        .env("RYUEDA_CACHE_DIR", p.run.with_file_name("cache"))
        .env("RYUEDA_ID_SEED", "cli")
        .env("RYUEDA_JLC_URL", &slow.base)
        .env_remove("RYUEDA_NO_DAEMON")
        .env_remove("RYUEDA_PARTICIPANT");
    let running = running
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Once the manifest is in (so progress has begun), asked again until
    // the daemon has the request: the whole sync takes five files' waits.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let start = std::time::Instant::now();
    let v = loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let (code, v) = p.run_in(&p.daemon, "board", &["cancel", "sync1"]);
        if code == 0 && v["result"]["cancelled"] == true {
            break v;
        }
        assert!(start.elapsed().as_secs() < 3, "never cancelled: {v}");
    };
    assert_eq!(v["result"]["token"], "sync1");
    let out = running.wait_with_output().unwrap();
    let (code, v) = parse(out.clone(), &p.daemon);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (4, Some("cancelled")),
        "{v}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("fetch: "), "progress on stderr: {stderr}");
    let (_, after) = p.run_in(&p.daemon, "board", &["parts", "sources"]);
    assert_eq!(
        after["result"]["sources"][1]["created"],
        "2026-11-06T11:33:30+00:00"
    );
    // In process there is nothing to cancel.
    let (_, v) = p.run_in(&p.direct, "board", &["cancel", "sync1"]);
    assert_eq!(v["error"]["code"], "no_live_session");
}

/// `parts login` keeps keys owner-only beside the preferences, from
/// stdin, and `parts logout` forgets them; neither says what they are.
#[test]
fn parts_login_keeps_keys_out_of_sight() {
    let p = Pair::projects();
    let mut cmd = p.command_in(&p.direct, "board", &["parts", "login", "digikey"]);
    cmd.write_stdin("my-id\nmy-very-secret\n");
    let (code, v) = parse(cmd.output().unwrap(), &p.direct);
    assert_eq!(code, 0, "{v}");
    assert!(!v.to_string().contains("my-very-secret"));
    let file = p.run.with_file_name("config").join("parts-digikey.keys");
    assert!(file.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let v = p.ok(&["parts", "sources"]);
    let dk = v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "digikey")
        .unwrap()
        .clone();
    assert_eq!(dk["keys"], "file");
    let mut cmd = p.command_in(&p.direct, "board", &["parts", "login", "digikey"]);
    cmd.write_stdin("only-one\n");
    let (code, v) = parse(cmd.output().unwrap(), &p.direct);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("invalid_param"))
    );
    let (code, v) = p.run_in(&p.direct, "board", &["parts", "logout", "digikey"]);
    assert_eq!((code, &v["result"]["removed"]), (0, &json!(true)));
    assert!(!file.exists());
}

#[path = "support/fake_digikey.rs"]
mod fake_digikey;

/// DigiKey through a fake of its API (our own answers), with keys from the
/// environment: not configured, then searched, shown, kept, written into
/// the library as identifiers, and refused at its rate limit. In process
/// and through the daemon alike; nothing here is a key anyone has.
#[test]
fn digikey_from_the_cli() {
    use ryueda_parts::digikey::testing::{CLIENT_ID, CLIENT_SECRET};
    let site = fake_digikey::start();
    let bare = Pair::with_resistor_in(Pair::with_env(&[
        ("RYUEDA_DIGIKEY_URL", &site.base),
        ("RYUEDA_NOW", "1793964810"),
    ]));
    let v = bare.ok(&["parts", "sources"]);
    let dk = v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "digikey")
        .unwrap()
        .clone();
    assert_eq!(dk["state"], "not_configured");
    assert!(
        dk["how"]
            .as_str()
            .unwrap()
            .contains("ryueda parts login digikey")
    );
    let v = bare.ok(&["parts", "search", "capacitor", "--in", "digikey"]);
    assert_eq!(v["warnings"][0]["details"]["reason"], "not_configured");

    let p = Pair::with_resistor_in(Pair::with_env(&[
        ("RYUEDA_DIGIKEY_URL", &site.base),
        ("RYUEDA_NOW", "1793964810"),
        ("RYUEDA_DIGIKEY_CLIENT_ID", CLIENT_ID),
        ("RYUEDA_DIGIKEY_CLIENT_SECRET", CLIENT_SECRET),
    ]));
    let v = p.ok(&["parts", "sources"]);
    let dk = v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "digikey")
        .unwrap()
        .clone();
    assert_eq!(
        (dk["state"].as_str(), dk["keys"].as_str()),
        (Some("configured"), Some("environment"))
    );
    assert!(!v.to_string().contains(CLIENT_SECRET));
    let v = p.ok(&[
        "parts",
        "search",
        "capacitor",
        "capacitance=100nF",
        "--in",
        "digikey",
    ]);
    assert_eq!(v["results"][0]["key"], "1276-1001-1-ND");
    assert_eq!(v["results"][0]["attribution"], "Data from DigiKey");
    let v = p.ok(&["parts", "show", "digikey:296-1234-1-ND"]);
    assert_eq!(v["records"][0]["lifecycle"], "not_recommended");
    assert_eq!(v["records"][0]["fetched_at"], 1_793_964_810u64);
    p.fails(
        &["parts", "show", "digikey:nothing-ND"],
        "parts_not_found",
        1,
    );
    // Kept: offline still shows it.
    let v = p.ok(&["parts", "show", "digikey:296-1234-1-ND", "--offline"]);
    assert_eq!(v["records"][0]["mpn"], "TPS7A2033PDBVR");
    // A component from the record: identifiers only.
    p.ok(&["parts", "show", "digikey:1276-1001-1-ND"]);
    let v = p.ok(&[
        "libcomp",
        "create",
        "100n dk",
        "--from",
        "digikey:1276-1001-1-ND",
        "--symbol",
        "r",
        "--prefix",
        "C",
        "--footprint",
        "lib/passives/footprint/r_0402",
        "--param",
        "capacitance=100 nF",
    ]);
    assert_eq!(v["libcomp"]["mpn"], "CL05B104KO5NNNC");
    assert_eq!(
        v["libcomp"]["offers"],
        json!([{"supplier": "DigiKey", "sku": "1276-1001-1-ND"}])
    );
    assert_eq!(v["libcomp"]["parameters"], json!({"capacitance": "100 nF"}));
    assert_eq!(v["from"]["attribution"], "Data from DigiKey");
    // DigiKey says to wait: refused, saying when.
    site.limited
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let e = p.fails(
        &["parts", "show", "digikey:296-1234-1-ND", "--refresh"],
        "parts_not_found",
        1,
    );
    assert_eq!(e["details"]["warnings"][0]["code"], "parts_rate_limited");
    assert_eq!(e["details"]["warnings"][0]["details"]["retry_after_s"], 30);
}

/// `place --dry-run --render FILE --sheet S` and `layout --relayout B
/// --dry-run --render FILE --sheet S` draw that sheet alone; an unknown
/// sheet is not_found. Both ways (`Pair` runs every command in process
/// and through the daemon).
#[test]
fn a_dry_run_renders_one_sheet() {
    let p = Pair::breakout();
    p.ok(&["layout"]);
    let v = p.ok(&[
        "place",
        "breakout",
        "--dry-run",
        "--render",
        "one.svg",
        "--sheet",
        "s1",
    ]);
    assert_eq!(v["format"], json!("svg"));
    for top in [&p.direct, &p.daemon] {
        let svg = fs::read_to_string(top.join("board/one.svg")).unwrap();
        assert_eq!(svg.matches("<g id=\"sheet-").count(), 1, "{svg:.400}");
    }
    p.fails(
        &[
            "place",
            "breakout",
            "--dry-run",
            "--render",
            "one.svg",
            "--sheet",
            "nope",
        ],
        "not_found",
        1,
    );
    let v = p.ok(&[
        "layout",
        "--relayout",
        "breakout",
        "--dry-run",
        "--render",
        "two.svg",
        "--sheet",
        "s1",
    ]);
    assert_eq!(v["format"], json!("svg"));
}

/// `place "edge R1 left"` then `place "release R1"` puts R1 back where it
/// stood, and the layout file forgets what it remembered for it.
#[test]
fn release_restores_the_drawing() {
    let p = Pair::breakout();
    p.ok(&["layout"]);
    let read = || -> Value {
        serde_json::from_slice(
            &fs::read(p.direct.join("board/blocks/breakout/layout.json")).unwrap(),
        )
        .unwrap()
    };
    let before = read();
    p.ok(&["place", "breakout", "edge R1 left"]);
    let moved = read();
    let id = moved["instructions"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, i)| i["op"] == json!("edge"))
        .map(|(k, _)| k.clone())
        .unwrap();
    assert!(moved["before"][&id]["r1"].is_object(), "{moved:#}");
    assert_ne!(moved["placed"]["r1"], before["placed"]["r1"]);
    p.ok(&["place", "breakout", "release R1"]);
    let after = read();
    assert_eq!(after["placed"]["r1"]["at"], before["placed"]["r1"]["at"]);
    assert!(after.get("before").is_none_or(|b| b.get(&id).is_none()));
}

/// An instruction that doesn't hold says what stood where it wanted its
/// part: a designator, or that the sheet had no room there (plan 2.B
/// Task 18). Both ways.
#[test]
fn a_blocked_instruction_says_what_blocks_it() {
    let p = Pair::breakout();
    p.ok(&["layout"]);
    // J1 asked right of U2: something stands there.
    let v = p.ok(&["place", "breakout", "--dry-run", "beside J1 U2 right"]);
    let ins = &v["instructions"][0];
    assert_eq!(ins["holds"], json!(false), "{ins}");
    let why = ins["why"].as_str().unwrap();
    assert!(
        why.contains("where it would go") || why.contains("no free place"),
        "{why}"
    );
}

/// `text` reads in the agent's terms, holds, and the listing writes it
/// back the same; a field or side it doesn't know is invalid_param.
#[test]
fn text_from_the_cli() {
    let p = Pair::breakout();
    p.ok(&["layout"]);
    p.ok(&[
        "place",
        "breakout",
        "text R1 refdes left",
        "text R2 both below",
    ]);
    let list = p.ok(&["place", "breakout", "--list"]);
    let texts: Vec<&str> = list["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["text"].as_str().unwrap())
        .collect();
    assert!(texts.contains(&"text R1 refdes left"), "{list:#}");
    assert!(texts.contains(&"text R2 both below"), "{list:#}");
    for i in list["instructions"].as_array().unwrap() {
        if i["text"].as_str().unwrap().starts_with("text ") {
            assert_eq!(i["holds"], json!(true), "{i:#}");
        }
    }
    p.fails(
        &["place", "breakout", "text R1 colour left"],
        "invalid_param",
        1,
    );
    p.fails(&["place", "breakout", "text R1 refdes"], "invalid_param", 1);
}

/// Tags on a component, a net and a rail: added, removed, refused when
/// badly formed, each one logged change.
#[test]
fn objects_take_tags() {
    let p = Pair::with_placed_resistor();
    let block = "top";
    let t = p.ok(&[
        "tag", "R1", "--block", block, "--add", "dnp", "--add", "critical",
    ]);
    assert_eq!(t["tags"], json!(["critical", "dnp"]), "{t}");
    let t = p.ok(&["tag", "R1", "--block", block, "--remove", "critical"]);
    assert_eq!(t["tags"], json!(["dnp"]));
    assert_eq!(
        p.ok(&["tag", "SDA", "--block", block, "--add", "hv"])["tags"],
        json!(["hv"])
    );
    p.ok(&["rail", "add", "VBUS", "--voltage", "5 V"]);
    assert_eq!(
        p.ok(&["tag", "rail:VBUS", "--add", "hv"])["tags"],
        json!(["hv"])
    );
    p.fails(
        &["tag", "R1", "--block", block, "--add", "High Voltage"],
        "invalid_param",
        1,
    );
    p.fails(&["tag", "R1", "--add", "hv"], "invalid_param", 1);
    // Each one logged change: undo (the daemon's) takes the rail's back.
    let (code, u) = p.run_in(&p.daemon, "board", &["undo"]);
    assert_eq!(code, 0, "{u}");
    let rails: Vec<String> = fs::read_dir(p.daemon.join("board/rails"))
        .unwrap()
        .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert!(
        rails
            .iter()
            .any(|r| r.contains("VBUS") && !r.contains("tags")),
        "{rails:?}"
    );
}

/// The style revision moves when a style file does, and project edit names
/// an organisation's style.
#[test]
fn the_style_revision_follows_style_files() {
    let p = Pair::with_resistor();
    // A library project named for a style must hold one.
    p.fails(
        &["project", "edit", "--style-from", "passives"],
        "style_invalid",
        1,
    );
    p.write(
        "passives/style.json",
        "{\"kind\": \"ryueda.style\", \"format\": 1}\n",
    );
    let before = p.ok(&["info"])["revision"].clone();
    let e = p.ok(&["project", "edit", "--style-from", "passives"]);
    assert_eq!(e["schematic"]["style_from"], "passives", "{e}");
    p.fails(
        &["project", "edit", "--style-from", "nowhere"],
        "invalid_param",
        1,
    );
    assert_ne!(e["revision"], before);
    let e = p.ok(&["project", "edit", "--style-from", ""]);
    assert!(e["schematic"].get("style_from").is_none(), "{e}");
}

/// The style whole, and one object's properties with where each came
/// from.
#[test]
fn a_style_is_read_and_explained() {
    let p = Pair::with_placed_resistor();
    let block = "top";
    let s = p.ok(&["style"]);
    assert_eq!(s["builtin"]["palette"]["wire"]["light"], "#1f5fbf", "{s}");
    assert_eq!(s["project"], json!({}));
    p.ok(&["tag", "R1", "--block", block, "--add", "hv"]);
    let e = p.ok(&["style", "explain", "R1", "--block", block]);
    assert_eq!(e["object"]["kind"], "component");
    let colour = &e["properties"]["colour"];
    assert_eq!(colour["value"], "hv", "{e}");
    assert_eq!(colour["light"], "#c0261c");
    assert_eq!(colour["from"]["layer"], "built_in");
    assert_eq!(colour["from"]["select"]["tag"], "hv");
    assert_eq!(e["properties"]["fill"]["from"]["role"], "body");
    p.fails(
        &["style", "explain", "nope", "--block", block],
        "not_found",
        1,
    );
}

/// Style writes: an override by the agent's vocabulary, a rule by a
/// selector, a palette entry; a text size lays its block out again; a
/// colour moves no layout; undo takes each back; locks refuse.
#[test]
fn the_style_is_written_by_commands() {
    let p = Pair::with_placed_resistor();
    let block = "top";
    p.ok(&["layout"]);
    let layout = |top: &Path| fs::read_to_string(top.join("board/blocks/top/layout.json")).unwrap();
    let before = layout(&p.direct);
    p.ok(&["style", "set", "R1", "colour=hv", "--block", block]);
    assert_eq!(layout(&p.direct), before, "a colour moves no layout");
    let s = p.ok(&["style"]);
    let id = s["project"]["overrides"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    assert!(id.starts_with(&format!("component/{block}/")), "{s}");
    let svg = || {
        p.ok(&["render", "--block", block, "--out", "look.svg"]);
        fs::read_to_string(p.direct.join("board/look.svg")).unwrap()
    };
    assert!(!svg().contains("font-size=\"1.778\""));
    p.ok(&["style", "set", "R1", "text_size=large", "--block", block]);
    // The engine reads it: R1's text is drawn large, and stays where it
    // stood while it still fits there.
    assert!(svg().contains("font-size=\"1.778\""), "a larger text");
    assert_eq!(layout(&p.direct), before, "text that still fits stays");
    p.ok(&["style", "set", "tag=dnp", "dash=true"]);
    assert_eq!(
        p.ok(&["style"])["project"]["rules"][0]["select"]["tag"],
        "dnp"
    );
    p.ok(&["style", "rule", "add", "kind=net", "line_weight=heavy"]);
    p.ok(&["style", "rule", "remove", "1"]);
    p.ok(&["style", "palette", "set", "brand", "#0055aa/#66aaff"]);
    p.ok(&["style", "set", "R1", "fill=brand", "--block", block]);
    p.fails(
        &["style", "set", "R1", "colour=#ff0000", "--block", block],
        "invalid_param",
        1,
    );
    p.fails(
        &["style", "set", "R1", "fill=nowhere", "--block", block],
        "style_invalid",
        1,
    );
    p.ok(&["style", "unset", "R1", "text_size", "--block", block]);
    assert!(!svg().contains("font-size=\"1.778\""), "the text goes back");
    assert_eq!(layout(&p.direct), before);
    let style = |top: &Path| fs::read_to_string(top.join("board/style.json")).unwrap();
    assert!(!style(&p.daemon).contains("text_size"));
    let (code, u) = p.run_in(&p.daemon, "board", &["undo"]);
    assert_eq!(code, 0, "{u}");
    assert!(style(&p.daemon).contains("text_size"), "undo puts it back");
}

#[test]
fn an_organisations_locks_refuse_a_projects_writes() {
    let p = Pair::with_placed_resistor();
    p.write(
        "passives/style.json",
        r#"{"kind": "ryueda.style", "format": 1, "locks": ["colour", "palette.wire", "lint.severity.sch_jog"],
            "lint": {"severity": {"sch_jog": "error"}}}"#,
    );
    p.ok(&["project", "edit", "--style-from", "passives"]);
    let block = "top";
    let e = p.fails(
        &["style", "set", "R1", "colour=hv", "--block", block],
        "style_locked",
        1,
    );
    assert_eq!(e["details"]["lock"], "colour", "{e}");
    p.fails(
        &["style", "palette", "set", "wire", "#000000/#ffffff"],
        "style_locked",
        1,
    );
    p.ok(&["style", "set", "R1", "dash=true", "--block", block]);
    // The organisation's lint raises sch_jog, and lint configure can't
    // lower it.
    let l = p.ok(&["lint"]);
    assert_eq!(l["prefs"]["severity"]["sch_jog"], "error", "{l}");
    assert!(l["own"]["severity"].get("sch_jog").is_none());
    let e = p.fails(
        &["lint", "configure", "--severity", "sch_jog=warning"],
        "style_locked",
        1,
    );
    assert_eq!(e["details"]["lock"], "lint.severity.sch_jog");
    // Its standards judge the project's library once it pins them.
    p.write(
        "passives/style.json",
        r#"{"kind": "ryueda.style", "format": 1, "standards": true}"#,
    );
    p.write(
        "passives/library/standards.json",
        r#"{"kind": "ryueda.standards", "format": 1, "shapes": "iec"}"#,
    );
    let s = p.ok(&["standards"]);
    assert_eq!(
        (&s["from"], &s["standards"]["shapes"]),
        (&json!("passives"), &json!("iec")),
        "{s}"
    );
}

/// A render in either theme, through the project's style.
#[test]
fn renders_take_a_theme_and_the_style() {
    let p = Pair::with_placed_resistor();
    let block = "top";
    p.ok(&["layout"]);
    p.ok(&["tag", "R1", "--block", block, "--add", "hv"]);
    let svg = |theme: &str| {
        let out = format!("{theme}.svg");
        let a = p.ok(&["render", "--block", block, "--theme", theme, "--out", &out]);
        assert_eq!(a["theme"], theme);
        fs::read_to_string(p.direct.join("board").join(&out)).unwrap()
    };
    let light = svg("light");
    let dark = svg("dark");
    assert!(
        light.contains("fill=\"#ffffff\"") && light.contains("#c0261c"),
        "paper, and R1 in hv red"
    );
    assert!(dark.contains("fill=\"#1d2025\"") && dark.contains("#ff6a5c"));
    assert!(
        light.contains("font-weight=\"600\""),
        "the designator in SemiBold"
    );
    p.ok(&[
        "render", "--block", block, "--theme", "dark", "--format", "png", "--out", "dark.png",
    ]);
    assert!(
        fs::read(p.direct.join("board/dark.png"))
            .unwrap()
            .starts_with(b"\x89PNG")
    );
    p.fails(
        &[
            "render", "--block", block, "--theme", "sepia", "--out", "x.svg",
        ],
        "usage",
        2,
    );
}
