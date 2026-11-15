//! Design 603's bars against #810: the scripted project (56 parts, 209
//! instances, about 1,000 single requests) driven through one `ryucad
//! session --stdio` from the release binary, as a build script drives
//! it: from scratch (one batch of ensures, materials and properties),
//! the same again, one constant changed, and #810's review round (one
//! constant, 16 nuts and washers, one part gone with `prune`, two
//! materials). Each case's statuses are checked exactly, a second run of
//! it is `unchanged`, and `undo` takes the review round back. Release
//! builds only.
//!
//! With `RYUCAD_BARS=1` the bars are asserted as they are (Linux, 8
//! cores); without, ten times looser, as the other bars are.

mod common;

use common::scripted::{Script, leaf_name};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

fn machine() -> String {
    format!(
        "{} {}, {} cores",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get())
    )
}

/// The script's one session.
struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    id: u64,
}

fn ryucad(root: &Path, scratch: &Path) -> Command {
    let mut c = Command::new(assert_cmd::cargo::cargo_bin("ryucad"));
    c.current_dir(root)
        .env("RYUCAD_RUNTIME_DIR", scratch.join("run"))
        .env("RYUCAD_CONFIG_DIR", scratch.join("config"))
        .env("RYUCAD_CACHE_DIR", scratch.join("cache"))
        .env("RYUCAD_IDLE_TIMEOUT", "600");
    c
}

impl Session {
    fn start(root: &Path, scratch: &Path) -> Session {
        let mut child = ryucad(root, scratch)
            .args(["session", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Session {
            child,
            stdin,
            stdout,
            id: 0,
        }
    }

    /// One request's result, and how long it took in seconds.
    fn timed(&mut self, method: &str, params: Value) -> (Value, f64) {
        self.id += 1;
        let line = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let t = Instant::now();
        writeln!(self.stdin.as_mut().unwrap(), "{line}").unwrap();
        let mut answer = String::new();
        self.stdout.read_line(&mut answer).unwrap();
        let secs = t.elapsed().as_secs_f64();
        let v: Value = serde_json::from_str(&answer).unwrap();
        let r = v
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("{method}: {v}"));
        (r, secs)
    }

    fn must(&mut self, method: &str, params: Value) -> Value {
        self.timed(method, params).0
    }

    fn end(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success());
    }
}

fn statuses(v: &Value) -> BTreeMap<String, String> {
    v["parts"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(p, s)| (p.clone(), s["status"].as_str().unwrap().to_owned()))
        .collect()
}

/// Every part `unchanged` but those named, which are as said.
fn only(v: &Value, changed: &[(&str, &str)]) {
    for (p, s) in statuses(v) {
        let want = changed
            .iter()
            .find(|(c, _)| *c == p)
            .map_or("unchanged", |(_, s)| s);
        assert_eq!(s, want, "{p}: {v}");
    }
}

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(root.join(rel)).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let r = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if e.file_type().unwrap().is_dir() {
                walk(root, &r, out);
            } else {
                out.insert(r, std::fs::read(e.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

#[test]
#[cfg_attr(debug_assertions, ignore = "a release build's bar")]
fn a_build_script_rebuilds_810s_project() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("project");
    std::fs::create_dir(&root).unwrap();
    assert!(
        ryucad(&root, scratch.path())
            .args(["init", "--unit", "mm"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let mut script = Script::new(56, 209);
    let singles = script.singles().len();
    let mut s = Session::start(&root, scratch.path());
    s.must("pdm.init", json!({}));
    let entries = |s: &mut Session| {
        s.must("activity.log", json!({}))["entries"]
            .as_array()
            .unwrap()
            .len()
    };
    let before = entries(&mut s);

    // From scratch: one batch, one process, one activity entry.
    let (v, scratch_s) = s.timed("batch", script.batch());
    assert!(statuses(&v).values().all(|x| x == "created"), "{v}");
    assert_eq!(v["rebuild"]["failed"], 0, "{v}");
    assert_eq!(entries(&mut s), before + 1);
    // Again, nothing changed: nothing written, no entry.
    let (v, again_s) = s.timed("batch", script.batch());
    only(&v, &[]);
    assert_eq!(v["written"], 0);
    assert_eq!(entries(&mut s), before + 1);
    // One constant: that part, and the assemblies over it rebuilt.
    script
        .part_mut("p08")
        .params
        .as_mut()
        .unwrap()
        .insert("k".into(), json!("31 mm"));
    let (v, constant_s) = s.timed("batch", script.batch());
    only(&v, &[("p08", "updated")]);
    let rebuilt: Vec<&str> = v["rebuild"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["part"].as_str().unwrap())
        .collect();
    assert_eq!(rebuilt[0], "p08", "{rebuilt:?}");
    assert!(rebuilt.contains(&"top"), "{rebuilt:?}");
    assert_eq!(
        s.must("batch", script.batch())["written"],
        0,
        "the second run"
    );
    // The review round, with prune.
    let undone = files(&root);
    script.review();
    let mut review = script.batch();
    review["prune"] = json!(true);
    let (v, review_s) = s.timed("batch", review.clone());
    only(
        &v,
        &[
            ("p04", "updated"),
            ("m0", "updated"),
            ("top", "updated"),
            (&leaf_name(55), "deleted"),
        ],
    );
    let m0 = v["ops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["result"]["part"] == "m0")
        .unwrap();
    let added = m0["result"]["features"]
        .as_object()
        .unwrap()
        .values()
        .filter(|x| *x == "created")
        .count();
    assert_eq!(added, 16, "{m0}");
    assert_eq!(s.must("batch", review)["written"], 0, "the second run");
    // One undo takes the round back.
    s.must("undo", json!({}));
    assert!(
        files(&root) == undone,
        "undo restores the files before the review round"
    );
    s.end();

    println!(
        "#810's project (56 parts, 209 instances, {singles} single requests), on {}: \
         from scratch {scratch_s:.2} s, again {again_s:.2} s, one constant {constant_s:.2} s, \
         the review round {review_s:.2} s",
        machine()
    );
    let k = slack();
    assert!(scratch_s <= 10.0 * k, "from scratch {scratch_s:.2} s");
    assert!(again_s <= 1.0 * k, "again {again_s:.2} s");
    assert!(constant_s <= 2.0 * k, "one constant {constant_s:.2} s");
    assert!(review_s <= 3.0 * k, "the review round {review_s:.2} s");
}
