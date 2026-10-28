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
    assert_eq!(info["root"], "<root>/board");
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
