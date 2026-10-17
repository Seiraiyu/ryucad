//! The CLI as an agent sees it: run the binary, parse stdout as JSON.
//!
//! Every scenario runs twice, in two identical projects: once in process
//! (`RYUCAD_NO_DAEMON=1`) and once through the project's daemon. Both must
//! give the same JSON and the same exit code.

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Replaces the project root with `<root>` in every string of `v`, trying
/// the forms in order.
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

/// Two identical projects, one served in process and one by a daemon.
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
        fs::create_dir(&direct).unwrap();
        fs::create_dir(&daemon).unwrap();
        Pair {
            direct: direct.canonicalize().unwrap(),
            daemon: daemon.canonicalize().unwrap(),
            run: dir.path().join("run"),
            _dir: dir,
        }
    }

    fn run_in(&self, root: &Path, sub: &str, args: &[&str]) -> (i32, Value) {
        let mut cmd = Command::cargo_bin("ryucad").unwrap();
        cmd.current_dir(root.join(sub))
            .args(args)
            .env("RYUCAD_RUNTIME_DIR", &self.run)
            .env("RYUCAD_IDLE_TIMEOUT", "60")
            .env("RYUCAD_CONFIG_DIR", self.run.with_file_name("config"));
        if root == self.direct {
            cmd.env("RYUCAD_NO_DAEMON", "1");
        }
        let out = cmd.output().unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        let mut json = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {text}"));
        // Replace inside the parsed strings: on Windows the raw text has
        // escaped backslashes. The root may appear verbatim (`\\?\C:\…`) or
        // in the ordinary form.
        let root = root.to_str().unwrap();
        let plain = root.strip_prefix(r"\\?\").unwrap_or(root);
        replace_root(&mut json, &[root, plain]);
        (out.status.code().unwrap(), json)
    }

    /// Runs `args` from `sub` (relative to the project) both ways and
    /// returns the common answer.
    fn both_from(&self, sub: &str, args: &[&str]) -> (i32, Value) {
        let a = self.run_in(&self.direct, sub, args);
        let b = self.run_in(&self.daemon, sub, args);
        assert_eq!(a, b, "{args:?}: in process vs daemon");
        a
    }

    fn ok(&self, args: &[&str]) -> Value {
        self.ok_from("", args)
    }

    fn ok_from(&self, sub: &str, args: &[&str]) -> Value {
        let (code, v) = self.both_from(sub, args);
        assert_eq!((code, &v["ok"]), (0, &Value::Bool(true)), "{args:?}: {v}");
        assert_eq!(v["warnings"], serde_json::json!([]), "{args:?}");
        v["result"].clone()
    }

    fn fails(&self, args: &[&str], error: &str, exit: i32) -> Value {
        let (code, v) = self.both_from("", args);
        assert_eq!(v["ok"], false, "{args:?}: {v}");
        assert_eq!(
            (code, v["error"]["code"].as_str().unwrap()),
            (exit, error),
            "{args:?}: {v}"
        );
        v["error"].clone()
    }

    /// Writes a file into both projects, as an agent or git would.
    fn write(&self, rel: &str, contents: &str) {
        fs::write(self.direct.join(rel), contents).unwrap();
        fs::write(self.daemon.join(rel), contents).unwrap();
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        let _ = self.run_in(&self.daemon, "", &["daemon", "stop"]);
    }
}

#[test]
fn version_reports_every_component() {
    let p = Pair::new();
    let r = p.ok(&["version"]);
    assert_eq!(r["ryucad"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["ryukei"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["protocol"], 2);
    assert!(r["target"].as_str().unwrap().contains('-'));
    assert!(!r["commit"].as_str().unwrap().is_empty());
}

#[test]
fn an_agent_session() {
    let p = Pair::new();
    assert_eq!(p.ok(&["init", "--unit", "mm"])["root"], "<root>");
    p.ok(&["part", "create", "bracket"]);
    let r = p.ok(&[
        "feature", "add", "bracket", "box", "--id", "plate", "--param", "x=100", "--param", "y=60",
        "--param", "z=5",
    ]);
    let rev = r["revision"].as_str().unwrap().to_owned();
    p.ok(&[
        "feature",
        "add",
        "bracket",
        "cylinder",
        "--param",
        "radius=12.5",
        "--param",
        "height=2 in",
        "--origin",
        "50,30,5mm",
        "--expect",
        &rev,
    ]);
    // From a subfolder, the project is found above.
    let list = p.ok_from("parts/bracket", &["feature", "list", "bracket"]);
    assert_eq!(list["features"][1]["id"], "cylinder-1");
    let r = p.ok(&["rebuild"]);
    assert_eq!(r["parts"][0]["failed"], 0);
    let inspect = p.ok(&["inspect", "bracket", "plate"]);
    let face = inspect["bodies"][0]["faces"][0]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    // Editing keeps names.
    p.ok(&["feature", "edit", "bracket", "plate", "--param", "x=120"]);
    let again = p.ok(&["inspect", "bracket", "plate", "--entity", &face]);
    assert_eq!(again["kind"], "face");
    p.ok(&["feature", "move", "bracket", "cylinder-1", "--first"]);
    p.ok(&["feature", "delete", "bracket", "plate"]);
    assert_eq!(p.ok(&["check-format"])["clean"], true);
    let shown = p.ok(&["feature", "show", "bracket", "cylinder-1"]);
    assert_eq!(
        shown["params"]["height"],
        serde_json::json!({"value": "2 in", "m": 0.0508})
    );
    assert_eq!(shown["placement"]["origin"][2]["value"], "5 mm");
    assert_eq!(p.ok(&["part", "list"])["parts"][0]["features"], 1);
}

#[test]
fn external_edits_are_seen() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=1",
    ]);
    p.ok(&["rebuild"]);
    let path = "parts/p/features/s.json";
    let text = fs::read_to_string(p.direct.join(path)).unwrap();
    p.write(path, &text.replace("\"1 mm\"", "\"2 mm\""));
    let shown = p.ok(&["feature", "show", "p", "s"]);
    assert_eq!(shown["params"]["radius"]["value"], "2 mm");
    // A new part made outside ryucad.
    fs::create_dir_all(p.direct.join("parts/q")).unwrap();
    fs::create_dir_all(p.daemon.join("parts/q")).unwrap();
    p.write(
        "parts/q/part.json",
        "{\"features\": [], \"format\": 1, \"kind\": \"ryucad.part\"}",
    );
    let parts = p.ok(&["part", "list"]);
    assert_eq!(parts["parts"][1]["part"], "q");
}

#[test]
fn every_error_code_has_its_exit_code() {
    let p = Pair::new();
    p.fails(&["part", "list"], "project_not_found", 3);
    p.ok(&["init"]);
    p.fails(&["init"], "project_exists", 1);
    p.fails(&["part", "create", "Bad"], "invalid_id", 1);
    p.ok(&["part", "create", "p"]);
    p.fails(&["part", "create", "p"], "duplicate_id", 1);
    p.fails(&["feature", "add", "p", "prism"], "unknown_feature_type", 1);
    p.fails(&["feature", "add", "p", "sphere"], "invalid_param", 1);
    p.fails(
        &[
            "feature",
            "add",
            "p",
            "sphere",
            "--param",
            "radius=1 parsec",
        ],
        "invalid_quantity",
        1,
    );
    p.fails(
        &[
            "feature", "add", "p", "sphere", "--param", "radius=1", "--z", "0,0,0",
        ],
        "invalid_placement",
        1,
    );
    p.fails(&["feature", "show", "p", "ghost"], "not_found", 1);
    let r = p.ok(&[
        "feature",
        "add",
        "p",
        "torus",
        "--id",
        "t",
        "--param",
        "major_radius=1",
        "--param",
        "minor_radius=1",
    ]);
    p.fails(&["rebuild", "p"], "rebuild_failed", 1);
    p.ok(&["feature", "edit", "p", "t", "--param", "major_radius=3"]);
    let stale = r["revision"].as_str().unwrap();
    p.fails(
        &["feature", "delete", "p", "t", "--expect", stale],
        "conflict",
        1,
    );
    p.fails(&["config", "set", "ui.colour", "x"], "unknown_setting", 1);
    p.fails(&["config", "set", "ui.theme", "neon"], "invalid_setting", 1);
    p.fails(&["feature", "bogus"], "usage", 2);
    p.fails(
        &["feature", "add", "p", "sphere", "--after", "t", "--first"],
        "usage",
        2,
    );
    p.write("parts/p/features/t.json", "{");
    p.fails(&["feature", "show", "p", "t"], "invalid_json", 1);
    p.write("parts/p/features/t.json", "{}");
    p.fails(&["feature", "show", "p", "t"], "invalid_document", 1);
    p.write(
        "ryucad.json",
        r#"{"default_unit":"mm","format":2,"kind":"ryucad.project"}"#,
    );
    p.fails(&["part", "list"], "format_too_new", 3);
}

#[test]
fn pretty_output_is_the_same_document() {
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .args(["version", "--pretty"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.lines().count() > 3);
    let compact = Command::cargo_bin("ryucad")
        .unwrap()
        .arg("version")
        .output()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        serde_json::from_slice::<Value>(&compact.stdout).unwrap()
    );
}

/// Every method in `proto::Request`, read from the variants' `rename`
/// attributes (indented once; fields are indented twice).
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
        ("project.init", &["init"]),
        ("project.info", &["version"]),
        ("part.create", &["part", "create"]),
        ("part.delete", &["part", "delete"]),
        ("part.list", &["part", "list"]),
        ("feature.add", &["feature", "add"]),
        ("feature.edit", &["feature", "edit"]),
        ("feature.delete", &["feature", "delete"]),
        ("feature.move", &["feature", "move"]),
        ("feature.list", &["feature", "list"]),
        ("feature.show", &["feature", "show"]),
        ("rebuild", &["rebuild"]),
        ("inspect", &["inspect"]),
        ("check_format", &["check-format"]),
        ("daemon.status", &["daemon", "status"]),
        ("daemon.shutdown", &["daemon", "stop"]),
        ("session.hello", &["participants"]),
        ("participant.list", &["participants"]),
        ("subscribe", &["watch"]),
        ("selection.get", &["selection"]),
        ("selection.set", &["select"]),
        ("camera.get", &["camera"]),
        ("camera.set", &["camera"]),
        ("highlight.set", &["highlight"]),
        ("highlight.clear", &["highlight"]),
        ("activity.set", &["status"]),
        ("activity.log", &["activity"]),
        ("undo", &["undo"]),
        ("history.log", &["history"]),
        ("mesh.get", &["mesh"]),
        ("render", &["render"]),
        ("doctor", &["doctor"]),
        ("doctor.repair", &["doctor"]),
        ("config.get", &["config", "get"]),
        ("config.set", &["config", "set"]),
        ("redo", &["redo"]),
        ("app.open", &["open"]),
    ];
    let methods = protocol_methods();
    assert!(methods.len() >= 26, "{methods:?}");
    for m in methods {
        let Some((_, args)) = commands.iter().find(|(name, _)| *name == m) else {
            panic!("protocol method {m} has no CLI command");
        };
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .args(*args)
            .arg("--help")
            .output()
            .unwrap();
        assert!(out.status.success(), "ryucad {args:?} --help");
    }
}

#[test]
fn settings_and_preferences() {
    let p = Pair::new();
    p.ok(&["init"]);
    let v = p.ok(&["config", "get", "ui.theme"]);
    assert_eq!(
        (&v["value"], &v["default"]),
        (&Value::from("system"), &Value::from(true))
    );
    p.ok(&["config", "set", "ui.theme", "dark"]);
    p.ok(&["config", "set", "display.edges", "false"]);
    let v = p.ok(&["config", "get", "--user"]);
    let theme = v["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "ui.theme")
        .unwrap()
        .clone();
    assert_eq!(theme["value"], "dark");
    assert_eq!(p.ok(&["config", "get", "display.edges"])["value"], false);
    // A project setting is a file change, guarded by the project revision.
    let rev = p.ok(&["config", "get", "default_unit"])["project_revision"].clone();
    p.fails(
        &["config", "set", "default_unit", "in", "--expect", "stale"],
        "conflict",
        1,
    );
    let v = p.ok(&[
        "config",
        "set",
        "default_unit",
        "in",
        "--expect",
        rev.as_str().unwrap(),
    ]);
    assert_eq!(v["value"], "in");
    assert_ne!(v["project_revision"], rev);
    p.fails(
        &["config", "set", "ui.theme", "dark", "--project"],
        "invalid_setting",
        1,
    );
    p.ok(&["part", "create", "q"]);
    let r = p.ok(&["feature", "add", "q", "sphere", "--param", "radius=1"]);
    assert_eq!(r["feature"]["params"]["radius"]["value"], "1 in");
}

#[test]
fn history_outside_and_inside_git() {
    let p = Pair::new();
    p.ok(&["init"]);
    let v = p.ok(&["history"]);
    assert_eq!(v["repository"], false);
    assert_eq!(v["warnings"][0]["code"], "not_a_git_repo");
    p.fails(&["history", "nope"], "not_found", 1);
}

#[test]
fn meshes_are_the_same_both_ways() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "torus",
        "--param",
        "major_radius=10",
        "--param",
        "minor_radius=2",
    ]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=1", "--param", "y=2", "--param",
        "z=3",
    ]);
    let m = p.ok(&["mesh", "p", "--quality", "coarse"]);
    assert_eq!(m["bodies"].as_array().unwrap().len(), 2);
    assert_eq!(m["bodies"][1]["faces"].as_array().unwrap().len(), 6);
    p.fails(&["mesh", "p", "--quality", "ultra"], "invalid_param", 1);
    // As bytes, through the daemon and in process alike.
    let r = p.ok(&["mesh", "p", "--quality", "coarse", "--out", "m.bin"]);
    assert_eq!(r["binary"]["encoding"], "ryucad.mesh.1");
    assert_eq!(r["out"], "m.bin");
    let a = fs::read(p.direct.join("m.bin")).unwrap();
    assert_eq!(a, fs::read(p.daemon.join("m.bin")).unwrap());
    assert_eq!(r["binary"]["length"], a.len());
    let wire = ryucad::mesh::decode(&a).unwrap();
    assert_eq!(wire[1].feature, "b");
    let tris = wire[1].mesh.as_ref().unwrap().triangles.len();
    assert_eq!(tris, m["bodies"][1]["triangles"].as_array().unwrap().len());
}

/// The pixels of a PNG file.
fn pixels(path: &Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(fs::File::open(path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn renders_are_the_same_both_ways() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "cylinder",
        "--id",
        "c",
        "--param",
        "radius=5",
        "--param",
        "height=10",
    ]);
    let face = {
        let v = p.ok(&["inspect", "p", "c"]);
        format!(
            "p/c/{}",
            v["bodies"][0]["faces"][0]["name"].as_str().unwrap()
        )
    };
    let r = p.ok(&[
        "render",
        "--out",
        "a.png",
        "--size",
        "160x120",
        "--highlight",
        &face,
        "--labels",
    ]);
    assert_eq!(r["binary"]["encoding"], "image/png");
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(160), Some(120))
    );
    let a = pixels(&p.direct.join("a.png"));
    assert_eq!(a, pixels(&p.daemon.join("a.png")));
    assert_eq!((a.0, a.1), (160, 120));
    // The highlight shows: there is note-blue in the image.
    assert!(a.2.chunks(4).any(|px| px[2] > 200 && px[0] < 140));
    let r = p.ok(&[
        "render",
        "p",
        "--out",
        "top.png",
        "--view",
        "top",
        "--no-edges",
    ]);
    assert_eq!(r["camera"]["up"], serde_json::json!([0.0, 1.0, 0.0]));
    p.fails(
        &["render", "--out", "x.png", "--size", "5000x10"],
        "render_too_large",
        1,
    );
    p.fails(
        &["render", "--out", "x.png", "--view", "sideways"],
        "invalid_param",
        1,
    );
}

#[test]
fn a_project_that_lost_its_manifest_is_damaged() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    fs::remove_file(p.direct.join("ryucad.json")).unwrap();
    fs::remove_file(p.daemon.join("ryucad.json")).unwrap();
    let e = p.fails(&["part", "list"], "project_damaged", 3);
    assert_eq!(e["details"]["repair"], "ryucad doctor");
    let r = p.ok(&["doctor"]);
    assert_eq!(r["issues"][0]["code"], "missing_manifest");
}

#[test]
fn doctor_on_a_healthy_project() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=1",
    ]);
    assert_eq!(p.ok(&["doctor"])["healthy"], true);
    p.write("parts/p/features/t.json", "{");
    let r = p.ok(&["doctor"]);
    let codes: Vec<&str> = r["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["invalid_json", "orphan_feature"]);
}

#[test]
fn doctor_repairs_a_damaged_project() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "in"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=1",
    ]);
    fs::remove_file(p.direct.join("ryucad.json")).unwrap();
    fs::remove_file(p.daemon.join("ryucad.json")).unwrap();
    let r = p.ok(&["doctor"]);
    let id = r["issues"][0]["repairs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(id.starts_with("missing_manifest:recreate:ryucad.json@"));
    let out = p.ok(&["doctor", "--repair", &id, "--param", "unit=in"]);
    assert_eq!(out["report"]["healthy"], true);
    // The project works again, through the daemon too.
    let v = p.ok(&["feature", "show", "p", "s"]);
    assert_eq!(v["params"]["radius"]["value"], "1 in");
    p.fails(&["doctor", "--repair", &id], "repair_conflict", 1);
}
