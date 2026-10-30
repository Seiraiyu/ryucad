//! The CLI as an agent sees it: run the binary, parse stdout as JSON.
//!
//! Every scenario runs twice, in two identical projects: once in process
//! (`RYUDFM_NO_DAEMON=1`) and once through the project's daemon. Both must
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

/// A committed corpus job, by its path.
fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

/// `ryudfm` with the variables that keep a test's daemons and settings
/// its own.
fn ryudfm(run: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ryudfm").unwrap();
    cmd.env("RYUDFM_RUNTIME_DIR", run)
        .env("RYUDFM_IDLE_TIMEOUT", "60")
        .env("RYUDFM_CONFIG_DIR", run.with_file_name("config"))
        .env_remove("RYUDFM_NO_DAEMON")
        .env_remove("RYUDFM_PARTICIPANT");
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

/// Two identical project folders `p`: one served in process, one by a
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

    /// A pair of projects: one made in process, its manifest (with its
    /// random project ID) copied to the other.
    fn project() -> Pair {
        let p = Pair::new();
        let (code, v) = p.run_in(&p.direct, &["init"]);
        assert_eq!(code, 0, "{v}");
        for f in ["ryudfm.json", ".gitattributes"] {
            fs::copy(p.direct.join("p").join(f), p.daemon.join("p").join(f)).unwrap();
        }
        p
    }

    fn run_in(&self, top: &Path, args: &[&str]) -> (i32, Value) {
        let mut cmd = ryudfm(&self.run);
        cmd.current_dir(top.join("p")).args(args);
        if top == self.direct {
            cmd.env("RYUDFM_NO_DAEMON", "1");
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
        let _ = ryudfm(&self.run)
            .current_dir(self.daemon.join("p"))
            .args(["daemon", "stop"])
            .output();
    }
}

#[test]
fn version_names_ryudfm() {
    let out = Command::cargo_bin("ryudfm")
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
    for k in ["ryudfm", "commit", "target", "protocol"] {
        assert!(r.get(k).is_some(), "{k}: {r}");
    }
    assert!(r.get("ryucad").is_none() && r.get("ryueda").is_none());
    let flag = Command::cargo_bin("ryudfm")
        .unwrap()
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(parse(flag, Path::new("/nowhere")), (0, v));
    let bad = Command::cargo_bin("ryudfm")
        .unwrap()
        .arg("no-such-command")
        .output()
        .unwrap();
    let (code, v) = parse(bad, Path::new("/nowhere"));
    assert_eq!((code, &v["error"]["code"]), (2, &json!("usage")));
}

#[test]
fn boards_from_the_cli() {
    let p = Pair::project();
    let (code, v) = p.run_in(&p.direct, &["init"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("project_exists")));
    assert_eq!(
        fs::read_to_string(p.direct.join("p/.gitattributes")).unwrap(),
        "*.json text eol=lf\nboards/*/input.* binary\nboards/*/input/** binary\n"
    );
    let info = p.ok(&["info"]);
    assert_eq!(info["boards"], json!({}));
    let rev = info["revision"].as_str().unwrap().to_owned();
    let tgz = corpus("minimal-clean.tgz");
    let tgz = tgz.to_str().unwrap();

    // Refused before anything is written.
    let junk = p.direct.join("junk.zip");
    fs::write(&junk, b"PK\x03\x04 not really").unwrap();
    fs::copy(&junk, p.daemon.join("junk.zip")).unwrap();
    p.fails(&["import", "../junk.zip"], "not_a_board", 1);
    p.fails(&["import", "../nothing.tgz"], "not_found", 1);
    p.fails(&["import", tgz, "--expect", "stale"], "conflict", 1);
    p.fails(&["import", tgz, "--board", "Bad Id"], "invalid_id", 1);
    let dry = p.ok(&["import", tgz, "--dry-run"]);
    assert_eq!(dry["dry_run"], json!(true));
    assert_eq!(p.files(&p.direct), [".gitattributes", "ryudfm.json"]);

    // A new board, its bytes as given.
    let made = p.ok(&["import", tgz, "--expect", &rev]);
    let id = made["board"].as_str().unwrap().to_owned();
    // Named from the file, not the job: exporters name jobs generically.
    assert_eq!(
        (id.as_str(), &made["file"]["name"]),
        ("minimal-clean", &json!("minimal-clean"))
    );
    assert_eq!(made["summary"]["components"], json!(11));
    assert_eq!(made["file"]["input"]["container"], json!("tgz"));
    assert_eq!(made["file"]["step"], json!("pcb"));
    assert_eq!(made["faults"], json!([]));
    assert_eq!(
        fs::read(p.direct.join(format!("p/boards/{id}/input.tgz"))).unwrap(),
        fs::read(tgz).unwrap()
    );
    let board_rev = made["revision"].as_str().unwrap().to_owned();
    p.fails(&["import", tgz, "--board", &id], "duplicate_id", 1);

    // The same job as a folder, a board of its own.
    let folder = corpus("minimal-clean");
    let made2 = p.ok(&["import", folder.to_str().unwrap(), "--board", "mc"]);
    assert_eq!(made2["file"]["input"]["container"], json!("folder"));
    assert!(
        p.files(&p.direct)
            .contains(&"boards/mc/input/matrix/matrix".to_owned())
    );

    let list = p.ok(&["board", "list"]);
    let boards = list["boards"].as_array().unwrap();
    assert_eq!(boards.len(), 2);
    assert!(boards.iter().all(|b| b.get("error").is_none()));
    let shown = p.ok(&["board", "show", &id]);
    assert_eq!(shown["revision"], json!(board_rev));
    assert_eq!(shown["found"], made["file"]["input"]["blake3"]);

    // Inspect: the summary, a page of a layer, a feature, components.
    let s = p.ok(&["inspect", &id]);
    assert_eq!(s["board"], json!(id));
    assert_eq!(s["layers"].as_array().unwrap().len(), 23);
    let layer = s["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["features"]["pad"].as_u64() > Some(0))
        .unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let page = p.ok(&["inspect", &id, "--layer", &layer, "--limit", "2"]);
    assert_eq!(page["features"].as_array().unwrap().len(), 2);
    let first = page["features"][0]["ref"].as_str().unwrap().to_owned();
    assert!(first.starts_with(&format!("{id}/pcb/{layer}/")), "{first}");
    let next = page["next"].as_str().unwrap().to_owned();
    let page2 = p.ok(&[
        "inspect", &id, "--layer", &layer, "--limit", "2", "--after", &next,
    ]);
    assert_ne!(page2["features"][0]["ref"], page["features"][0]["ref"]);
    let f = p.ok(&["inspect", &id, "--feature", &first]);
    assert_eq!(f["ref"], json!(first));
    assert!(f["shape"]["parts"].is_array() || f["shape_error"].is_string());
    let comps = p.ok(&["inspect", &id, "--components"]);
    assert_eq!(comps["total"], json!(11));
    p.fails(&["inspect", &id, "--layer", "nope"], "not_found", 1);

    // Renders: the bytes go to --out, the same both ways.
    let png = p.ok(&["render", &id, "--out", "top.png", "--size", "300"]);
    assert_eq!(png["format"], json!("png"));
    assert_eq!(png["binary"]["encoding"], json!("image/png"));
    let (a, b) = (
        fs::read(p.direct.join("p/top.png")).unwrap(),
        fs::read(p.daemon.join("p/top.png")).unwrap(),
    );
    assert_eq!(a, b);
    assert!(a.starts_with(b"\x89PNG"));
    let svg = p.ok(&["render", &id, "--out", "bottom.svg", "--side", "bottom"]);
    assert_eq!(svg["format"], json!("svg"));
    assert!(
        fs::read_to_string(p.direct.join("p/bottom.svg"))
            .unwrap()
            .starts_with("<svg")
    );
    let c = p.ok(&["check"]);
    assert_eq!(
        (c["schema"].clone(), c["errors"].clone()),
        (json!(1), json!(0))
    );

    // The next revision: the folder in place of the archive, keeping the
    // board's name and step.
    p.fails(
        &[
            "import",
            "--update",
            &id,
            folder.to_str().unwrap(),
            "--expect",
            "stale",
        ],
        "conflict",
        1,
    );
    p.fails(
        &["import", "--update", "nope", folder.to_str().unwrap()],
        "not_found",
        1,
    );
    p.fails(
        &[
            "import",
            "--update",
            &id,
            folder.to_str().unwrap(),
            "--step",
            "panel",
        ],
        "odb_step_not_found",
        1,
    );
    let up = p.ok(&[
        "import",
        "--update",
        &id,
        folder.to_str().unwrap(),
        "--expect",
        &board_rev,
    ]);
    assert_eq!(up["update"], json!(true));
    assert_eq!(up["file"]["name"], made["file"]["name"]);
    assert_eq!(up["file"]["input"]["container"], json!("folder"));
    let files = p.files(&p.direct);
    assert!(
        !files.contains(&format!("boards/{id}/input.tgz")),
        "{files:?}"
    );
    assert!(files.contains(&format!("boards/{id}/input/matrix/matrix")));

    // Edits and deletes; writes name boards, never paths.
    let e = p.ok(&["board", "edit", &id, "--name", "Minimal"]);
    assert_eq!(e["name"], json!("Minimal"));
    p.fails(
        &["board", "edit", &id, "--step", "panel"],
        "odb_step_not_found",
        1,
    );
    p.fails(
        &["board", "edit", "../junk.zip", "--name", "x"],
        "invalid_param",
        1,
    );
    let d = p.ok(&["board", "delete", "mc"]);
    assert_eq!(d["board"], json!("mc"));
    assert!(!p.direct.join("p/boards/mc").exists());
    p.fails(&["board", "show", "mc"], "not_found", 1);

    // An input changed by hand fails its board alone.
    for top in [&p.direct, &p.daemon] {
        fs::write(
            top.join(format!("p/boards/{id}/input/misc/info")),
            "changed",
        )
        .unwrap();
    }
    let e = p.fails(&["inspect", &id], "board_input_changed", 1);
    assert!(e["details"]["expected"].is_string());
    let e = p.fails(&["check"], "check_failed", 1);
    assert_eq!(
        e["details"]["report"]["findings"][0]["code"],
        json!("board_input_changed")
    );
}

#[test]
fn path_mode_reads_a_job_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let here = dir.path().canonicalize().unwrap();
    let go = |args: &[&str]| {
        let mut cmd = ryudfm(&run);
        cmd.current_dir(&here).args(args);
        parse(cmd.output().unwrap(), &here)
    };
    let job = corpus("vna-se-probe.zip");
    let job = job.to_str().unwrap();
    let (code, v) = go(&["inspect", job]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["board"], Value::Null);
    assert_eq!(v["result"]["path"], json!(job));
    let (code, v) = go(&["inspect", job, "--nets", "--limit", "1"]);
    assert_eq!((code, v["result"]["total"].clone()), (0, json!(3)), "{v}");
    let (code, v) = go(&["render", job, "--out", "v.svg", "--size", "200x100"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        (v["result"]["width"].clone(), v["result"]["height"].clone()),
        (json!(200), json!(100))
    );
    assert!(here.join("v.svg").is_file());
    let (code, v) = go(&["check", job]);
    assert_eq!((code, v["result"]["errors"].clone()), (0, json!(0)), "{v}");
    // Outside a project, nothing but paths.
    let (code, v) = go(&["inspect", "nothing-here"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("not_found")));
    let (code, v) = go(&["check"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (3, &json!("project_not_found"))
    );
    let (code, v) = go(&["import", job]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (3, &json!("project_not_found"))
    );
}

#[test]
fn writes_are_logged_and_undone_through_the_daemon() {
    let p = Pair::project();
    let run = |args: &[&str]| p.run_in(&p.daemon, args);
    let tgz = corpus("minimal-clean.tgz");
    let (code, v) = run(&["import", tgz.to_str().unwrap(), "--board", "mc"]);
    assert_eq!(code, 0, "{v}");
    let (_, h) = run(&["history"]);
    let entries = h["result"]["entries"].as_array().unwrap().clone();
    let change = entries.iter().find(|e| e["method"] == "import").unwrap();
    assert_eq!(change["participant"]["name"], "agent");
    let paths: Vec<&Value> = change["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| &f["path"])
        .collect();
    assert_eq!(
        paths,
        [
            &json!("boards/mc/board.json"),
            &json!("boards/mc/input.tgz")
        ]
    );
    let (code, u) = run(&["undo"]);
    assert_eq!(code, 0, "{u}");
    assert!(!p.daemon.join("p/boards").exists());
    assert!(u["result"]["revisions"].get("board/mc").is_none());
    let (code, r) = run(&["redo"]);
    assert_eq!(code, 0, "{r}");
    assert!(r["result"]["revisions"]["board/mc"].is_string());
    assert_eq!(
        fs::read(p.daemon.join("p/boards/mc/input.tgz")).unwrap(),
        fs::read(&tgz).unwrap()
    );
    // An update, undone, puts the old input back.
    let folder = corpus("minimal-clean");
    let (code, v) = run(&["import", "--update", "mc", folder.to_str().unwrap()]);
    assert_eq!(code, 0, "{v}");
    assert!(!p.daemon.join("p/boards/mc/input.tgz").exists());
    assert_eq!(run(&["undo"]).0, 0);
    assert!(p.daemon.join("p/boards/mc/input.tgz").is_file());
    assert!(!p.daemon.join("p/boards/mc/input").exists());
    let (code, v) = run(&["inspect", "mc"]);
    assert_eq!(code, 0, "{v}");
    // Another agent has nothing of its own to undo.
    let (code, v) = run(&["--as", "other", "undo"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("nothing_to_undo")));
    // In process there is no log.
    let (code, v) = run(&["--no-daemon", "undo"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("no_live_session")));
    let (code, s) = run(&["daemon", "status"]);
    assert_eq!(code, 0, "{s}");
    assert!(s["result"]["version"]["ryudfm"].is_string());
}

#[test]
fn watchers_hear_of_board_changes() {
    let p = Pair::project();
    // The daemon is started before the watcher, so the two don't race to
    // start one each, the watcher subscribing to one while the writes go
    // to the other (#246).
    let (code, v) = p.run_in(&p.daemon, &["info"]);
    assert_eq!(code, 0, "{v}");
    let mut watch = ryudfm(&p.run);
    watch.current_dir(p.daemon.join("p")).args([
        "--as",
        "watcher",
        "watch",
        "--topic",
        "project",
        "--timeout",
        "120",
    ]);
    let waiting = std::thread::spawn(move || watch.output().unwrap());
    let attached = || {
        let (_, v) = p.run_in(&p.daemon, &["participants"]);
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
    // Nothing says when the watcher has subscribed (it is attached first),
    // so the board is imported, and its import undone, until the watcher
    // has heard one change: it answers once it has, or at its deadline.
    let tgz = corpus("minimal-clean.tgz");
    let mut made = false;
    let mut writes = 0;
    while !waiting.is_finished() {
        let args: Vec<&str> = match made {
            false => vec!["import", tgz.to_str().unwrap(), "--board", "mc"],
            true => vec!["undo"],
        };
        assert_eq!(p.run_in(&p.daemon, &args).0, 0, "{args:?}");
        made = !made;
        writes += 1;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (code, v) = parse(waiting.join().unwrap(), &p.daemon);
    assert_eq!(code, 0, "{v}");
    let notes = v["result"]["notifications"].as_array().unwrap();
    let (_, status) = p.run_in(&p.daemon, &["daemon", "status"]);
    assert!(
        !notes.is_empty(),
        "the watcher heard nothing in 120 s of {writes} writes: {v}; daemon: {status}"
    );
    assert_eq!(notes[0]["method"], "project.changed", "{v}");
    assert_eq!(
        notes[0]["params"]["changed"],
        json!(["board/mc", "project"])
    );
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
        ("import", &["import"]),
        ("board.list", &["board", "list"]),
        ("board.show", &["board", "show"]),
        ("board.edit", &["board", "edit"]),
        ("board.delete", &["board", "delete"]),
        ("inspect", &["inspect"]),
        ("render", &["render"]),
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
        let out = Command::cargo_bin("ryudfm")
            .unwrap()
            .args(*args)
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success(), "ryudfm {args:?} --help");
    }
}
