//! The CLI as an agent sees it: run the binary, parse stdout as JSON.
//!
//! Every scenario runs twice, in two identical projects: once in process
//! (`RYUCAD_NO_DAEMON=1`) and once through the project's daemon. Both must
//! give the same JSON and the same exit code.

mod fake_review;

use assert_cmd::Command;
use serde_json::{Value, json};
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
            .env("RYUCAD_CONFIG_DIR", self.run.with_file_name("config"))
            // Each project its own disk cache, as two machines would have.
            .env("RYUCAD_CACHE_DIR", root.with_extension("cache"));
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
        // A panic here while a failed test unwinds would abort the whole
        // binary and lose the failure's message.
        if std::thread::panicking() {
            return;
        }
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
fn parameters_drive_features() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    let r = p.ok(&["param", "set", "p", "wall=3mm", "n=4"]);
    assert_eq!(r["parameters"][1]["value"], "3 mm");
    let rev = r["revision"].as_str().unwrap().to_owned();
    p.ok(&[
        "feature",
        "add",
        "p",
        "box",
        "--id",
        "b",
        "--param",
        "x=wall*n",
        "--param",
        "y=10",
        "--param",
        "z=wall",
        "--origin",
        "max(wall, 1 mm),0,-wall",
        "--expect",
        &rev,
    ]);
    let shown = p.ok(&["feature", "show", "p", "b"]);
    assert_eq!(
        shown["params"]["x"],
        serde_json::json!({"value": "wall * n", "m": 0.012, "expr": true})
    );
    assert_eq!(shown["placement"]["origin"][0]["value"], "max(wall, 1 mm)");
    let before = p.ok(&["rebuild"])["parts"][0]["hash"].clone();
    p.ok(&["param", "set", "p", "wall=4 mm"]);
    let after = p.ok(&["rebuild"]);
    assert_ne!(after["parts"][0]["hash"], before);
    assert_eq!(
        p.ok(&["feature", "show", "p", "b"])["params"]["x"]["m"],
        0.016
    );
    let list = p.ok(&["param", "list", "p"]);
    assert_eq!(
        list["parameters"][1]["used_by"]["features"],
        serde_json::json!(["b"])
    );
    assert_eq!(p.ok(&["check-format"])["clean"], true);
    p.fails(&["param", "set", "p", "wall=1 deg"], "unit_mismatch", 1);
    p.fails(&["param", "set", "p", "wall=wall"], "parameter_cycle", 1);
    p.fails(
        &["feature", "edit", "p", "b", "--param", "y=gap"],
        "unknown_parameter",
        1,
    );
    p.fails(&["param", "set", "p"], "usage", 2);
    p.fails(&["param", "delete", "p", "wall"], "has_dependents", 1);
    let r = p.ok(&["param", "rename", "p", "wall", "skin"]);
    assert_eq!(r["parameters"][1]["name"], "skin");
    let shown = p.ok(&["feature", "show", "p", "b"]);
    assert_eq!(shown["params"]["x"]["value"], "skin * n");
    p.ok(&["feature", "delete", "p", "b"]);
    p.ok(&["param", "delete", "p", "skin"]);
    p.ok(&["param", "delete", "p", "n"]);
    assert_eq!(
        p.ok(&["param", "list", "p"])["parameters"],
        serde_json::json!([])
    );
}

#[test]
fn sketches_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&["param", "set", "p", "width=20 mm"]);
    let r = p.ok(&[
        "feature", "add", "p", "sketch", "--id", "s", "--plane", "zx",
    ]);
    assert_eq!(r["feature"]["plane"], "zx");
    let ops = r#"[
        {"op": "add", "type": "point", "at": [0, 0]},
        {"op": "add", "type": "point", "at": [19, 1]},
        {"op": "add", "type": "point", "at": [21, 9]},
        {"op": "add", "type": "point", "at": [1, 11]},
        {"op": "add", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "type": "line", "start": "p2", "end": "p3"},
        {"op": "add", "type": "line", "start": "p3", "end": "p4"},
        {"op": "add", "type": "line", "start": "p4", "end": "p1"},
        {"op": "add", "type": "horizontal", "a": "l1"},
        {"op": "add", "type": "horizontal", "a": "l3"},
        {"op": "add", "type": "vertical", "a": "l2"},
        {"op": "add", "type": "vertical", "a": "l4"},
        {"op": "add", "type": "fix", "a": "p1"},
        {"op": "add", "type": "length", "a": "l1", "value": "width"}
    ]"#;
    p.write("ops.json", ops);
    let r = p.ok(&["sketch", "edit", "p", "s", "@ops.json"]);
    assert_eq!(r["report"]["state"], "under");
    assert_eq!(r["report"]["dof"], 1);
    let r = p.ok(&[
        "sketch",
        "edit",
        "p",
        "s",
        r#"[{"op": "add", "type": "distance", "a": "p1", "b": "l3", "value": "width / 2"}]"#,
    ]);
    assert_eq!(r["added"], serde_json::json!(["d2"]));
    assert_eq!(r["report"]["state"], "well");
    let g = p.ok(&["sketch", "get", "p", "s"]);
    assert_eq!(g["entities"]["p3"]["state"], "well");
    assert_eq!(g["constraints"]["d2"]["value"]["value"], "width / 2");
    // A proposal, solved but not written.
    let dry = p.ok(&[
        "sketch",
        "solve",
        "p",
        "s",
        r#"[{"op": "set", "id": "d2", "value": "width"}]"#,
    ]);
    assert_eq!(dry["changed"], true);
    assert_eq!(p.ok(&["sketch", "solve", "p", "s"])["changed"], false);
    let r = p.ok(&["rebuild", "p"]);
    assert_eq!(r["parts"][0]["features"][0]["sketch"]["state"], "well");
    // The parameter moves the sketch, written in the same change.
    let r = p.ok(&["param", "set", "p", "width=30 mm"]);
    assert_eq!(r["sketches"][0]["ok"], true);
    assert_eq!(p.ok(&["check-format"])["clean"], true);
    // Refused edits, by code.
    p.fails(
        &[
            "sketch",
            "edit",
            "p",
            "s",
            r#"[{"op": "add", "type": "parallel", "a": "l1", "b": "l3"}]"#,
        ],
        "sketch_overconstrained",
        1,
    );
    p.fails(
        &[
            "sketch",
            "edit",
            "p",
            "s",
            r#"[{"op": "add", "type": "length", "a": "l2", "value": "1 m"}]"#,
        ],
        "sketch_unsolved",
        1,
    );
    p.fails(&["sketch", "edit", "p", "s", "[nope"], "usage", 2);
    // Stale from outside: a warning in the envelope, and a repair.
    p.write(
        "parts/p/parameters.json",
        "{\n  \"format\": 1,\n  \"kind\": \"ryucad.parameters\",\n  \"parameters\": {\n    \"width\": \"25 mm\"\n  }\n}\n",
    );
    let (code, v) = p.both_from("", &["rebuild", "p"]);
    assert_eq!((code, &v["ok"]), (0, &serde_json::json!(true)), "{v}");
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1);
    assert!(v["warnings"][0].as_str().unwrap().contains("sketch s"));
    let d = p.ok(&["doctor"]);
    let issue = &d["issues"][0];
    assert_eq!(issue["code"], "sketch_stale");
    let id = issue["repairs"][0]["id"].as_str().unwrap().to_owned();
    p.ok(&["doctor", "--repair", &id]);
    assert_eq!(p.ok(&["doctor"])["healthy"], true);
}

#[test]
fn sketches_are_drawn_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&["feature", "add", "p", "sketch", "--id", "s"]);
    let ops = r#"[
        {"op": "add", "type": "point", "at": [0, 0]},
        {"op": "add", "type": "point", "at": [20, 0]},
        {"op": "add", "type": "point", "at": [10, 8]},
        {"op": "add", "type": "line", "start": "p1", "end": "p2"},
        {"op": "add", "type": "circle", "center": "p3", "r": 4},
        {"op": "add", "type": "length", "a": "l1", "value": "20 mm"},
        {"op": "add", "type": "radius", "a": "ci1", "value": "4 mm"}
    ]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    // Curves as polylines, finer or coarser, and dimensions' labels.
    let pieces = |q: &str| {
        let g = p.ok(&["sketch", "get", "p", "s", "--quality", q]);
        assert_eq!(g["quality"], q);
        g["entities"]["ci1"]["polyline"].as_array().unwrap().len()
    };
    assert!(pieces("coarse") < pieces("normal") && pieces("normal") < pieces("fine"));
    let g = p.ok(&["sketch", "get", "p", "s"]);
    assert_eq!(g["entities"]["l1"]["polyline"].as_array().unwrap().len(), 2);
    assert_eq!(g["constraints"]["d1"]["label"]["text"], "20 mm");
    p.fails(
        &["sketch", "get", "p", "s", "--quality", "best"],
        "invalid_param",
        1,
    );
    // Renders draw sketches unless told not to, the same both ways.
    for (file, extra) in [("with.png", None), ("without.png", Some("--no-sketches"))] {
        let mut args = vec!["render", "--out", file, "--size", "120x90", "--view", "top"];
        args.extend(extra);
        p.ok(&args);
    }
    let with = pixels(&p.direct.join("with.png"));
    assert_eq!(with, pixels(&p.daemon.join("with.png")));
    let without = pixels(&p.direct.join("without.png"));
    assert_eq!(without, pixels(&p.daemon.join("without.png")));
    assert_ne!(with, without);
    // Without sketches, only the axis triad is off the background.
    let bg = [245, 246, 248];
    let drawn = |img: &(u32, u32, Vec<u8>)| img.2.chunks(4).filter(|c| c[..3] != bg).count();
    assert!(drawn(&with) > drawn(&without) + 100);
}

#[test]
fn sketches_become_bodies_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&["feature", "add", "p", "sketch", "--id", "s"]);
    p.ok(&[
        "sketch",
        "edit",
        "p",
        "s",
        r#"[{"op": "add", "type": "point", "at": [10, 0]},
            {"op": "add", "type": "point", "at": [20, 0]},
            {"op": "add", "type": "point", "at": [20, 8]},
            {"op": "add", "type": "point", "at": [10, 8]},
            {"op": "add", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "type": "line", "start": "p4", "end": "p1"},
            {"op": "add", "type": "point", "at": [0, 0]},
            {"op": "add", "type": "point", "at": [0, 10]},
            {"op": "add", "type": "line", "start": "p5", "end": "p6", "construction": true}]"#,
    ]);
    let r = p.ok(&[
        "feature", "add", "p", "extrude", "--id", "e", "--sketch", "s", "--length", "3",
    ]);
    assert_eq!(r["feature"]["extent"]["type"], "blind");
    p.ok(&[
        "feature",
        "add",
        "p",
        "revolve",
        "--id",
        "r",
        "--sketch",
        "s",
        "--axis",
        "l5",
        "--angle",
        "90",
        "--symmetric",
    ]);
    let r = p.ok(&["rebuild", "p"]);
    let features = r["parts"][0]["features"].as_array().unwrap();
    assert_eq!(features.len(), 3);
    assert!(features[1]["hash"].is_string() && features[2]["hash"].is_string());
    let v = p.ok(&["inspect", "p", "e"]);
    assert_eq!(v["bodies"][0]["faces"][0]["origin"]["sketch"], "s");
    p.ok(&[
        "feature",
        "edit",
        "p",
        "e",
        "--extent",
        "symmetric",
        "--reverse",
    ]);
    p.ok(&[
        "feature",
        "edit",
        "p",
        "r",
        "--angle",
        "full",
        "--symmetric",
        "false",
    ]);
    let r = p.ok(&["render", "--out", "s.png", "--size", "160x120"]);
    assert_eq!(r["width"], 160);
    assert_eq!(
        pixels(&p.direct.join("s.png")),
        pixels(&p.daemon.join("s.png"))
    );
    // Refused, by code.
    p.fails(&["feature", "delete", "p", "s"], "has_dependents", 1);
    p.fails(
        &["feature", "move", "p", "e", "--first"],
        "reference_forward",
        1,
    );
    p.fails(
        &[
            "feature", "add", "p", "revolve", "--sketch", "s", "--axis", "p1", "--angle", "9",
        ],
        "reference_wrong_kind",
        1,
    );
    p.fails(
        &[
            "feature", "add", "p", "extrude", "--sketch", "s", "--length", "0",
        ],
        "invalid_extent",
        1,
    );
    p.ok(&[
        "feature", "add", "p", "extrude", "--id", "bad", "--sketch", "s", "--length", "1",
        "--loop", "l5",
    ]);
    let (code, v) = p.both_from("", &["rebuild", "p"]);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("rebuild_failed")),
        "{v}"
    );
    let bad = &v["error"]["details"]["parts"][0]["features"][3];
    assert_eq!(bad["error"]["code"], "profile_invalid");
    p.ok(&["feature", "delete", "p", "bad"]);
    // A sketch that fails takes its sweeps with it: by hand, l1 is given two
    // lengths at once.
    let path = p.direct.join("parts/p/features/s.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["constraints"]["x1"] = serde_json::json!({"a": "l1", "type": "length", "value": "10 mm"});
    v["constraints"]["x2"] =
        serde_json::json!({"a": "p1", "b": "p2", "type": "distance", "value": "12 mm"});
    p.write(
        "parts/p/features/s.json",
        &serde_json::to_string_pretty(&v).unwrap(),
    );
    let (_, v) = p.both_from("", &["rebuild", "p"]);
    let e = &v["error"]["details"]["parts"][0]["features"][1];
    assert_eq!(e["error"]["code"], "dependency_failed", "{v}");
}

/// The name of the entity of `kind` in an `inspect` body that `pick`
/// chooses.
fn named(body: &Value, kind: &str, pick: impl Fn(&Value) -> bool) -> String {
    let e = body[kind].as_array().unwrap().iter().find(|e| pick(e));
    e.unwrap_or_else(|| panic!("no such entity in {kind}"))["name"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Whether `v` (three numbers) is `want`.
fn at(v: &Value, want: [f64; 3]) -> bool {
    (0..3).all(|k| (v[k].as_f64().unwrap() - want[k]).abs() < 1e-12)
}

#[test]
fn references_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=20", "--param", "y=10", "--param",
        "z=5",
    ]);
    let body = p.ok(&["inspect", "p", "b"])["bodies"][0].clone();
    let top = named(&body, "faces", |f| {
        let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
        f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == 1.0
    });
    let corner = named(&body, "vertices", |v| at(&v["point"], [0.02, 0.01, 0.005]));
    let (o, x) = (
        named(&body, "vertices", |v| at(&v["point"], [0.0, 0.0, 0.0])),
        named(&body, "vertices", |v| at(&v["point"], [0.02, 0.0, 0.0])),
    );
    let along_x = named(&body, "edges", |e| {
        (e["start"] == o.as_str() && e["end"] == x.as_str())
            || (e["start"] == x.as_str() && e["end"] == o.as_str())
    });
    let r = |n: &str| format!("p/b/{n}");
    // A sketch on the top, with a corner projected into it, extruded.
    let v = p.ok(&[
        "feature",
        "add",
        "p",
        "sketch",
        "--id",
        "s",
        "--face",
        &r(&top),
    ]);
    assert_eq!(v["feature"]["face"], r(&top));
    let ops = format!(
        r#"[{{"op": "add", "type": "point", "at": [2, 2]}},
            {{"op": "add", "type": "point", "projected": "{}"}},
            {{"op": "add", "type": "point", "at": [2, 8]}},
            {{"op": "add", "type": "line", "start": "p1", "end": "p2"}},
            {{"op": "add", "type": "line", "start": "p2", "end": "p3"}},
            {{"op": "add", "type": "line", "start": "p3", "end": "p1"}}]"#,
        r(&corner)
    );
    p.ok(&["sketch", "edit", "p", "s", &ops]);
    p.ok(&[
        "feature", "add", "p", "extrude", "--id", "e", "--sketch", "s", "--length", "3",
    ]);
    // A second extrude up to the first's lid, a millimetre short.
    let lid = named(&p.ok(&["inspect", "p", "e"])["bodies"][0], "faces", |f| {
        f["origin"]["cap"] == "end"
    });
    p.ok(&["feature", "add", "p", "sketch", "--id", "t"]);
    p.ok(&[
        "sketch",
        "edit",
        "p",
        "t",
        r#"[{"op": "add", "type": "point", "at": [40, 0]},
            {"op": "add", "type": "circle", "center": "p1", "r": 2}]"#,
    ]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "extrude",
        "--id",
        "f",
        "--sketch",
        "t",
        "--up-to",
        &format!("p/e/{lid}"),
        "--offset",
        "-1",
    ]);
    // A rectangle on zx, revolved about the box's bottom edge.
    p.ok(&[
        "feature", "add", "p", "sketch", "--id", "u", "--plane", "zx",
    ]);
    p.ok(&[
        "sketch",
        "edit",
        "p",
        "u",
        r#"[{"op": "add", "type": "point", "at": [-20, 2]},
            {"op": "add", "type": "point", "at": [-12, 2]},
            {"op": "add", "type": "point", "at": [-12, 8]},
            {"op": "add", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "type": "line", "start": "p3", "end": "p1"}]"#,
    ]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "revolve",
        "--id",
        "r",
        "--sketch",
        "u",
        "--axis",
        &r(&along_x),
        "--angle",
        "90",
    ]);
    let rb = p.ok(&["rebuild", "p"]);
    assert_eq!(rb["parts"][0]["failed"], 0, "{rb}");
    let list = p.ok(&["feature", "list", "p"]);
    assert_eq!(list["features"][1]["uses"], serde_json::json!(["b"]));
    assert_eq!(list["features"][4]["uses"], serde_json::json!(["t", "e"]));
    let f = &p.ok(&["inspect", "p", "f"])["bodies"][0]["bounds"]["max"][2];
    assert!((f.as_f64().unwrap() - 0.007).abs() < 1e-12, "{f}");
    // Widening the box moves the corner: s is written solved with it.
    let v = p.ok(&["feature", "edit", "p", "b", "--param", "x=25"]);
    assert_eq!(
        v["sketches"],
        serde_json::json!([{"feature": "s", "ok": true}])
    );
    let v = p.ok(&["sketch", "get", "p", "s"]);
    assert_eq!(v["stale"], false);
    // Refused, by code.
    p.fails(&["feature", "delete", "p", "b"], "has_dependents", 1);
    p.fails(
        &["feature", "move", "p", "b", "--after", "s"],
        "reference_forward",
        1,
    );
    p.fails(
        &["feature", "add", "p", "sketch", "--face", &r(&along_x)],
        "reference_wrong_kind",
        1,
    );
    p.fails(
        &[
            "feature",
            "add",
            "p",
            "sketch",
            "--face",
            "p/b/0123456789abcdef0123456789abcdef",
        ],
        "reference_unresolved",
        1,
    );
    p.fails(
        &["feature", "add", "p", "sketch", "--face-x", &r(&along_x)],
        "usage",
        2,
    );
    p.fails(
        &[
            "sketch",
            "edit",
            "p",
            "t",
            &format!(
                r#"[{{"op": "add", "type": "line", "projected": "{}"}}]"#,
                r(&top)
            ),
        ],
        "reference_wrong_kind",
        1,
    );
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
    p.fails(&["config", "set", "ui.style", "neon"], "invalid_setting", 1);
    p.fails(&["feature", "bogus"], "usage", 2);
    // A group with no subcommand is a usage error too, in JSON, for agents.
    p.fails(&["config"], "usage", 2);
    p.fails(&["daemon"], "usage", 2);
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
        "parts/p/features/t.json",
        r#"{"conflicts":{"/params/x":{"kind":"value"}},"format":1,"kind":"ryucad.feature","type":"box"}"#,
    );
    p.fails(&["feature", "show", "p", "t"], "merge_conflict", 1);
    // It still says what it is, for the tree.
    let rebuilt = p.fails(&["rebuild"], "rebuild_failed", 1);
    let listed = p.ok(&["feature", "list", "p"]);
    for all in [
        &rebuilt["details"]["parts"][0]["features"],
        &listed["features"],
    ] {
        let t = all
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "t")
            .unwrap();
        assert_eq!(
            (t["ok"].as_bool(), t["type"].as_str()),
            (Some(false), Some("box")),
            "{t}"
        );
    }
    p.write(
        "ryucad.json",
        r#"{"default_unit":"mm","format":3,"kind":"ryucad.project"}"#,
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

#[test]
fn version_flags_are_the_version_command() {
    let run = |args: &[&str]| {
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let v = run(&["version"]);
    assert_eq!(run(&["--version"]), v);
    assert_eq!(run(&["-V"]), v);
    assert_eq!(run(&["--version", "--pretty"]), v);
    // With anything else it is still a usage error, as before.
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .args(["part", "list", "--version"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn vectors_may_start_with_a_minus() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    let v = p.ok(&[
        "feature", "add", "p", "box", "--param", "x=1", "--param", "y=1", "--param", "z=1",
        "--origin", "-5,0,0", "--x", "-0,1,0", "--z", "0,0,1",
    ]);
    assert_eq!(
        v["feature"]["placement"]["origin"][0]["value"], "-5 mm",
        "{v}"
    );
    assert_eq!(
        v["feature"]["placement"]["x"],
        serde_json::json!([0, 1, 0]),
        "{v}"
    );
}

#[test]
fn sketch_edit_help_lists_every_type() {
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .args(["sketch", "edit", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    for t in ryucad::doc::sketch::ConstraintType::ALL {
        assert!(help.contains(t.as_str()), "{}: {help}", t.as_str());
    }
    for t in ["point", "line", "arc", "circle", "\"r\": Q"] {
        assert!(help.contains(t), "{t}: {help}");
    }
}

#[test]
fn a_reader_closing_early_is_no_panic() {
    use std::process::Stdio;
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("ryucad"))
        .args(["version", "--pretty"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Closed before the process gets to write: `| head -c 0`.
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
    assert!(out.status.success());
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
        ("param.list", &["param", "list"]),
        ("param.set", &["param", "set"]),
        ("param.delete", &["param", "delete"]),
        ("param.rename", &["param", "rename"]),
        ("pdm.init", &["pdm", "init"]),
        ("pdm.config", &["pdm", "config"]),
        ("pdm.configure", &["pdm", "configure"]),
        ("props.get", &["props", "show"]),
        ("props.set", &["props", "set"]),
        ("release.list", &["release", "list"]),
        ("bom.get", &["bom"]),
        ("bom.diff", &["bom", "diff"]),
        ("bom.where_used", &["where-used"]),
        ("bom.renumber", &["bom", "renumber"]),
        ("release.check", &["release", "check"]),
        ("release.sync", &["release", "sync"]),
        ("release.propose", &["release", "propose"]),
        ("release.obsolete", &["release", "obsolete"]),
        ("sketch.edit", &["sketch", "edit"]),
        ("sketch.solve", &["sketch", "solve"]),
        ("sketch.get", &["sketch", "get"]),
        ("assembly.move", &["assembly", "move"]),
        ("assembly.tree", &["assembly", "tree"]),
        ("assembly.solve", &["assembly", "solve"]),
        ("interference", &["interference"]),
        ("measure", &["measure"]),
        ("mass", &["mass"]),
        ("clearance", &["clearance"]),
        ("materials.list", &["materials", "list"]),
        ("materials.set", &["materials", "set"]),
        ("materials.delete", &["materials", "delete"]),
        ("view.list", &["view", "list"]),
        ("view.save", &["view", "save"]),
        ("view.delete", &["view", "delete"]),
        ("view.rename", &["view", "rename"]),
        ("import", &["import"]),
        ("export", &["export"]),
        ("rebuild", &["rebuild"]),
        ("inspect", &["inspect"]),
        ("check_format", &["check-format"]),
        ("diff", &["diff"]),
        ("check", &["check"]),
        ("merge.status", &["merge", "status"]),
        ("merge.resolve", &["merge", "resolve"]),
        ("worktree.list", &["worktree", "list"]),
        ("daemon.status", &["daemon", "status"]),
        ("daemon.shutdown", &["daemon", "stop"]),
        ("session.hello", &["participants"]),
        ("participant.list", &["participants"]),
        ("subscribe", &["watch"]),
        ("selection.get", &["selection"]),
        ("selection.set", &["select"]),
        ("camera.get", &["camera"]),
        ("camera.set", &["camera"]),
        ("compare.get", &["compare"]),
        ("compare.set", &["compare"]),
        ("view.focus", &["focus"]),
        ("visibility.set", &["hide"]),
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
        ("cache.stats", &["cache", "stats"]),
        ("cache.clear", &["cache", "clear"]),
        ("cache.login", &["cache", "login"]),
        ("cache.logout", &["cache", "logout"]),
        ("cache.warm", &["cache", "warm"]),
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
fn the_disk_cache_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init"]);
    let empty = p.ok(&["cache", "stats"]);
    assert_eq!(
        (&empty["enabled"], &empty["entries"]),
        (&json!(true), &json!(0))
    );
    assert_eq!(empty["max_bytes"], 10u64 << 30);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    p.ok(&["rebuild"]);
    p.ok(&["mesh", "p"]);
    let s = p.ok(&["cache", "stats"]);
    assert_eq!(s["kinds"]["features"]["entries"], 1, "{s}");
    assert_eq!(s["kinds"]["meshes"]["entries"], 1, "{s}");
    assert_eq!(s["kinds"]["tools"]["entries"], 0, "{s}");
    let c = p.ok(&["cache", "clear"]);
    assert_eq!(c["removed"]["entries"], 2, "{c}");
    assert_eq!(c["entries"], 0);
    // The bound is a preference; setting it trims.
    p.ok(&["config", "set", "cache.max_bytes", "67108864"]);
    assert_eq!(p.ok(&["cache", "stats"])["max_bytes"], 67108864);
    p.fails(
        &["config", "set", "cache.max_bytes", "10"],
        "invalid_setting",
        1,
    );
}

#[test]
fn unreadable_preferences_are_an_envelope_warning() {
    let p = Pair::new();
    p.ok(&["init"]);
    let config = p.run.with_file_name("config");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("preferences.json"), "{ not json").unwrap();
    for args in [&["config", "get"][..], &["config", "get", "ui.theme"]] {
        let (code, v) = p.both_from("", args);
        assert_eq!(code, 0, "{v}");
        assert!(v["result"].get("warnings").is_none(), "{v}");
        assert!(
            v["warnings"][0].as_str().unwrap().contains("preferences"),
            "{v}"
        );
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
    assert!(v.get("warnings").is_none(), "{v}");
    assert!(p.ok(&["config", "get"]).get("warnings").is_none());
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
    // Set back to its default, it is the default again.
    p.ok(&["config", "set", "display.edges", "true"]);
    let v = p.ok(&["config", "get", "display.edges"]);
    assert_eq!(
        (&v["value"], &v["default"]),
        (&Value::from(true), &Value::from(true))
    );
    p.ok(&["config", "set", "display.edges", "false"]);
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
    // The warning is the envelope's, like every other command's.
    let (code, v) = p.both_from("", &["history"]);
    assert_eq!(code, 0);
    assert_eq!(v["result"]["repository"], false);
    assert!(v["result"].get("warnings").is_none(), "{v}");
    assert!(
        v["warnings"][0]
            .as_str()
            .unwrap()
            .contains("is not in a Git repository"),
        "{v}"
    );
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
    // Compact, the same meshes in fewer bytes.
    let r = p.ok(&[
        "mesh",
        "p",
        "--quality",
        "coarse",
        "--out",
        "m2.bin",
        "--compact",
    ]);
    assert_eq!(r["binary"]["encoding"], "ryucad.mesh.2");
    let b = fs::read(p.direct.join("m2.bin")).unwrap();
    assert_eq!(b, fs::read(p.daemon.join("m2.bin")).unwrap());
    assert!(b.len() < a.len());
    let compact = ryucad::mesh::decode(&b).unwrap();
    assert_eq!(
        compact[1].mesh.as_ref().unwrap().triangles,
        wire[1].mesh.as_ref().unwrap().triangles
    );
    p.fails(&["mesh", "p", "--compact"], "usage", 2);
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
        "view_not_found",
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

/// Runs git in `dir` with a fixed identity and time, so the same commits
/// in two repositories have the same IDs, and its output.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_AUTHOR_DATE", "2026-10-21T12:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-21T12:00:00Z")
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn git_diff_reads_values_through_textconv() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    let root = &p.direct;
    let file = root.join("parts/p/features/b.json");
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .args(["git", "textconv"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("ryucad.feature box (format 1)\n"),
        "{text}"
    );
    assert!(text.contains("\nparams.x = 10 mm\n"), "{text}");
    // Registered by hand here; `git install` does it for real.
    fs::write(root.join(".gitattributes"), "*.json diff=ryucad\n").unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "a box"]);
    p.ok(&["feature", "edit", "p", "b", "--param", "x=12"]);
    // Git runs it through a shell: forward slashes, even on Windows.
    let bin = assert_cmd::cargo::cargo_bin("ryucad");
    let bin = bin.to_str().unwrap().replace('\\', "/");
    let driver = format!("diff.ryucad.textconv='{bin}' git textconv");
    let diff = git(root, &["-c", &driver, "diff", "--no-color", "-U0"]);
    assert!(
        diff.contains("\n-params.x = 10 mm\n+params.x = 12 mm\n"),
        "{diff}"
    );
    // A file that isn't there is an error, in JSON.
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .args(["git", "textconv", "nowhere.json"])
        .current_dir(root)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "io");
}

#[test]
fn the_merge_driver_merges_three_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str, radius: &str| {
        let v = serde_json::json!({
            "format": 1, "kind": "ryucad.feature", "type": "sphere",
            "params": {"radius": radius},
            "placement": {"origin": ["0 mm", "0 mm", "0 mm"], "x": [1, 0, 0], "z": [0, 0, 1]},
        });
        let path = dir.path().join(name);
        fs::write(&path, ryucad::doc::canonical::to_string(&v)).unwrap();
        path
    };
    let driver = |base: &Path, ours: &Path, theirs: &Path| -> (i32, String) {
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .args(["git", "merge-driver"])
            .args([base, ours, theirs])
            .arg("parts/p/features/s.json")
            .output()
            .unwrap();
        assert!(out.stdout.is_empty());
        (
            out.status.code().unwrap(),
            fs::read_to_string(ours).unwrap(),
        )
    };
    let base = file("base", "1 mm");
    // Theirs changed it, ours didn't: theirs, and exit 0.
    let (code, text) = driver(&base, &file("ours", "1 mm"), &file("theirs", "2 mm"));
    assert_eq!(code, 0);
    assert!(text.contains("\"radius\": \"2 mm\""), "{text}");
    // Both changed it: ours in place, the record, exit 1.
    let (code, text) = driver(&base, &file("ours", "3 mm"), &file("theirs", "2 mm"));
    assert_eq!(code, 1);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["params"]["radius"], "3 mm");
    assert_eq!(v["conflicts"]["/params/radius"]["theirs"], "2 mm");
    // What isn't a ryucad file merges as lines, and always exits 1, so Git
    // leaves it for someone to look at.
    let theirs = dir.path().join("plain");
    fs::write(&theirs, "not json\n").unwrap();
    let (code, text) = driver(&base, &file("ours", "1 mm"), &theirs);
    assert_eq!((code, text.as_str()), (1, "not json\n"));
    let (code, text) = driver(&base, &file("ours", "3 mm"), &theirs);
    assert_eq!(code, 1);
    assert!(text.starts_with("<<<<<<< ours\n"), "{text}");
    // A file that isn't there: exit 4, a message for Git to show.
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .args(["git", "merge-driver", "nowhere", "nor", "here"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("ryucad: merging nor"));
}

#[test]
fn git_install_registers_the_drivers_and_the_hook() {
    // Git's own config is the test's: a home of its own, no system file.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&root).unwrap();
    let run_in = |at: &Path, args: &[&str]| -> (i32, Value) {
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .args(args)
            .current_dir(at)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env("RYUCAD_NO_DAEMON", "1")
            .env("RYUCAD_CONFIG_DIR", dir.path().join("config"))
            .env("RYUCAD_CACHE_DIR", dir.path().join("cache"))
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        (out.status.code().unwrap(), v)
    };
    let run = |args: &[&str]| run_in(&root, args);
    let codes = |v: &Value| -> Vec<String> {
        v["result"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["code"].as_str().unwrap().to_owned())
            .collect()
    };
    let (code, v) = run(&["init"]);
    assert_eq!(code, 0, "{v}");
    assert!(v["result"].get("git").is_none(), "{v}");
    let attributes = fs::read_to_string(root.join(".gitattributes")).unwrap();
    assert!(
        attributes.contains("\nparts/**/*.json merge=ryucad diff=ryucad text eol=lf\n"),
        "{attributes}"
    );
    // Not a repository: nothing to register in, and nothing to report.
    let (code, v) = run(&["git", "install"]);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("not_a_git_repo"))
    );
    git(&root, &["init", "-q", "-b", "main"]);
    let (_, v) = run(&["doctor"]);
    assert_eq!(
        codes(&v),
        [
            "git_drivers_missing",
            "git_hook_missing",
            "git_hook_missing"
        ]
    );
    // The drivers in the user's config; the clone's own setup is per clone.
    let (code, v) = run(&["git", "install", "--global"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["scope"], "global");
    let config = &v["result"]["config"];
    assert!(
        config["diff.ryucad.textconv"]
            .as_str()
            .unwrap()
            .ends_with("' git textconv")
    );
    assert!(
        config["merge.ryucad.driver"]
            .as_str()
            .unwrap()
            .ends_with("' git merge-driver %O %A %B %P")
    );
    let global = fs::read_to_string(home.join(".gitconfig")).unwrap();
    assert!(global.contains("[diff \"ryucad\"]"), "{global}");
    assert!(global.contains("[merge \"ryucad\"]"), "{global}");
    // merge.renames stays the clone's: every other repository reads the
    // global config too.
    let (_, v) = run(&["doctor"]);
    assert_eq!(
        codes(&v),
        [
            "git_drivers_missing",
            "git_hook_missing",
            "git_hook_missing"
        ]
    );
    let message = v["result"]["issues"][0]["message"].as_str().unwrap();
    assert!(
        message.starts_with("merge.renames isn't false"),
        "{message}"
    );
    assert!(!global.contains("renames"), "{global}");
    // In the repository's own config: the drivers, renames off and the
    // hooks.
    fs::remove_file(home.join(".gitconfig")).unwrap();
    let (code, v) = run(&["git", "install"]);
    assert_eq!(
        (code, v["result"]["scope"].as_str()),
        (0, Some("local")),
        "{v}"
    );
    for h in ["pre-merge-commit", "pre-commit"] {
        assert_eq!(v["result"]["hooks"][h]["status"], "installed", "{h}");
    }
    let local = fs::read_to_string(root.join(".git/config")).unwrap();
    assert!(
        local.contains("textconv") && local.contains("merge-driver"),
        "{local}"
    );
    assert!(local.contains("renames = false"), "{local}");
    let hook = root.join(".git/hooks/pre-merge-commit");
    let script = fs::read_to_string(&hook).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".git/hooks/pre-commit")).unwrap(),
        script
    );
    assert!(script.ends_with("' merge status --hook\n"), "{script}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&hook).unwrap().permissions().mode() & 0o111,
            0o111
        );
    }
    let (_, v) = run(&["doctor"]);
    assert_eq!(v["result"]["healthy"], true, "{v}");
    // Someone else's hook is left alone, with the line to add.
    fs::write(&hook, "#!/bin/sh\necho mine\n").unwrap();
    let (code, v) = run(&["git", "install"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        v["result"]["hooks"]["pre-merge-commit"]["status"],
        "foreign"
    );
    assert_eq!(v["result"]["hooks"]["pre-commit"]["status"], "installed");
    assert!(
        v["result"]["hooks"]["pre-merge-commit"]["line"]
            .as_str()
            .unwrap()
            .ends_with("' merge status --hook || exit 1")
    );
    assert_eq!(fs::read_to_string(&hook).unwrap(), "#!/bin/sh\necho mine\n");
    let (_, v) = run(&["doctor"]);
    assert_eq!(codes(&v), ["git_hook_foreign"]);
    // A project made inside a repository sets the clone up as it starts.
    let other = dir.path().join("other");
    fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    let (code, v) = run_in(&other, &["init"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        v["result"]["git"]["hooks"]["pre-commit"]["status"],
        "installed"
    );
    let (_, v) = run_in(&other, &["doctor"]);
    assert_eq!(v["result"]["healthy"], true, "{v}");
}

impl Pair {
    /// Runs git in both projects.
    fn git(&self, args: &[&str]) {
        git(&self.direct, args);
        git(&self.daemon, args);
    }
}

#[test]
fn diff_and_check_through_the_cli() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    p.ok(&[
        "feature", "add", "p", "sphere", "--id", "c", "--param", "radius=1",
    ]);
    p.git(&["init", "-q", "-b", "main"]);
    // Registered in each repository, so the user's own config doesn't count.
    let installed = p.ok(&["git", "install"]);
    assert_eq!(installed["hooks"]["pre-commit"]["status"], "installed");
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "start"]);
    let head = git(&p.direct, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_eq!(p.ok(&["diff"])["parts"], serde_json::json!({}));
    p.ok(&["feature", "edit", "p", "b", "--param", "x=12"]);
    let d = p.ok(&["diff"]);
    assert_eq!(d["from"]["commit"], head.as_str());
    assert_eq!(
        d["parts"]["p"]["features"]["b"]["changes"][0],
        serde_json::json!({"at": "/params/x", "from": "10 mm", "to": "12 mm"})
    );
    let m = p.ok(&["diff", "--model", "--part", "p"]);
    let b = &m["parts"]["p"]["features"]["b"]["model"];
    assert_eq!(b["body"]["faces"]["changed"].as_array().unwrap().len(), 5);
    // The committed revision rebuilds, inspects and meshes as it was.
    let r = p.ok(&["rebuild", "--rev", "HEAD"]);
    assert_eq!(r["commit"], head.as_str());
    assert_eq!(r["parts"][0]["features"][0]["hash"], b["from"]["hash"]);
    let i = p.ok(&["inspect", "p", "b", "--rev", "HEAD"]);
    assert_eq!(i["bodies"][0]["bounds"]["max"][0], 0.01);
    let mesh = p.ok(&["mesh", "p", "c", "--rev", "HEAD", "--quality", "coarse"]);
    assert_eq!(mesh["commit"], head.as_str());
    // Check, and against a revision.
    let c = p.ok(&["check"]);
    assert_eq!(c["passed"], true, "{c}");
    let c = p.ok(&["check", "--against", "HEAD"]);
    assert_eq!(
        c["against"]["parts"]["p"]["features"]["b"]["status"],
        "changed"
    );
    p.ok(&[
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
    let e = p.fails(&["check"], "check_failed", 1);
    assert_eq!(e["details"]["findings"][0]["code"], "feature_failed");
    assert_eq!(e["details"]["findings"][0]["feature"], "t");
    p.fails(&["diff", "--from", "nowhere"], "rev_not_found", 1);
    p.fails(&["rebuild", "--rev", "HEAD~5"], "rev_not_found", 1);
    p.fails(&["check", "--against", "nowhere"], "rev_not_found", 1);
}

#[test]
fn merges_through_the_cli() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    p.git(&["init", "-q", "-b", "main"]);
    p.ok(&["git", "install"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "start"]);
    p.fails(&["merge", "status"], "not_merging", 1);
    p.git(&["checkout", "-q", "-b", "theirs"]);
    p.ok(&["feature", "edit", "p", "b", "--param", "x=15"]);
    p.git(&["commit", "-q", "-am", "theirs"]);
    p.git(&["checkout", "-q", "main"]);
    p.ok(&["feature", "edit", "p", "b", "--param", "x=12"]);
    p.git(&["commit", "-q", "-am", "ours"]);
    for root in [&p.direct, &p.daemon] {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["merge", "--no-edit", "theirs"])
            .current_dir(root)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
    }
    let id = "parts/p/features/b.json#/params/x";
    let v = p.ok(&["merge", "status"]);
    assert_eq!(v["step"], "merge");
    assert_eq!(v["conflicts"][0]["id"], id);
    assert_eq!(v["conflicts"][0]["theirs"], "15 mm");
    p.fails(&["feature", "show", "p", "b"], "merge_conflict", 1);
    p.fails(&["merge", "resolve", id], "usage", 2);
    p.fails(
        &["merge", "resolve", "nowhere#", "--ours"],
        "conflict_not_found",
        1,
    );
    p.fails(
        &["merge", "resolve", id, "--theirs", "--expect", "0000"],
        "conflict",
        1,
    );
    let r = p.ok(&["merge", "resolve", id, "--value", "13 mm"]);
    assert_eq!(r["status"]["clean"], true);
    assert_eq!(r["status"]["unstaged"][0], "parts/p/features/b.json");
    let b = p.ok(&["feature", "show", "p", "b"]);
    assert_eq!(b["params"]["x"]["value"], "13 mm");
    // The daemon logged it: undo brings the conflict back.
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!(code, 0, "{v}");
    let (_, v) = p.run_in(&p.daemon, "", &["merge", "status"]);
    assert_eq!(v["result"]["conflicts"][0]["id"], id);
}

#[test]
fn pdm_settings_and_properties_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "a"]);
    p.ok(&["part", "create", "b"]);
    let c = p.ok(&["pdm", "config"]);
    assert_eq!(
        (&c["configured"], &c["revisions"]),
        (&json!(false), &json!("letters"))
    );
    // Both projects get one ID, so the two ways answer alike; pdm init
    // keeps an ID a project has.
    let id = "0123456789abcdef0123456789abcdef";
    p.write(
        "ryucad.json",
        &format!(
            "{{\n  \"default_unit\": \"mm\",\n  \"format\": 2,\n  \"kind\": \"ryucad.project\",\n  \"project\": \"{id}\"\n}}\n"
        ),
    );
    let c = p.ok(&["pdm", "init"]);
    assert_eq!(
        (&c["configured"], &c["project"]),
        (&json!(true), &json!(id))
    );
    let c = p.ok(&[
        "pdm",
        "configure",
        "--pattern",
        r"\d{3}-\d{4}",
        "--property",
        r#"finish={"type":"enum","values":["bare","red"]}"#,
        "--property",
        r#"count={"type":"integer","revising":false}"#,
        "--expect",
        c["project_revision"].as_str().unwrap(),
    ]);
    assert_eq!(c["properties"]["count"]["revising"], false);
    let v = p.ok(&[
        "props",
        "set",
        "a",
        "number=100-0001",
        "count=3",
        "finish=bare",
    ]);
    let count = v["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "count")
        .unwrap();
    assert_eq!(count["value"], 3);
    let shown = p.ok(&["props", "show", "a"]);
    assert_eq!(shown, v);
    p.fails(
        &["props", "set", "b", "number=100-0001"],
        "part_number_duplicate",
        1,
    );
    p.fails(&["props", "set", "b", "colour=red"], "property_unknown", 1);
    p.fails(&["props", "set", "b", "finish=blue"], "property_invalid", 1);
    p.fails(&["props", "set", "b", "number=1"], "part_number_invalid", 1);
    p.fails(&["props", "set", "b"], "usage", 2);
    p.fails(
        &["pdm", "configure", "--property", "x={not json"],
        "usage",
        2,
    );
    p.fails(
        &["pdm", "configure", "--remove-property", "count"],
        "property_unknown",
        1,
    );
    p.fails(
        &["props", "set", "a", "count=4", "--expect", "0000"],
        "conflict",
        1,
    );
    p.ok(&["props", "set", "a", "count="]);
    assert_eq!(p.ok(&["check"])["passed"], true);
    // The daemon logged the last write: undo puts count back.
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!(code, 0, "{v}");
    let (_, v) = p.run_in(&p.daemon, "", &["props", "show", "a"]);
    assert!(
        v["result"]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["name"] == "count" && x["value"] == 3),
        "{v}"
    );
    // A released part's ID can't come back.
    fs::create_dir_all(p.direct.join("releases/old")).unwrap();
    fs::create_dir_all(p.daemon.join("releases/old")).unwrap();
    p.write("releases/old/A.json", "{}");
    p.fails(&["part", "create", "old"], "part_id_retired", 1);
    p.fails(
        &["pdm", "configure", "--revisions", "numeric"],
        "revision_scheme_locked",
        1,
    );
}

#[test]
fn solids_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    let unit = ["--param", "x=10", "--param", "y=10", "--param", "z=10"];
    let mut base = vec!["feature", "add", "p", "box", "--id", "a"];
    base.extend(unit);
    p.ok(&base);
    // A drilled hole through all of a, sketched on its bottom.
    p.ok(&["feature", "add", "p", "sketch", "--id", "s"]);
    let ops = r#"[
        {"op": "add", "type": "point", "at": [5, 5]},
        {"op": "add", "type": "fix", "a": "p1"},
        {"op": "add", "type": "circle", "center": "p1", "r": 2},
        {"op": "add", "type": "radius", "a": "ci1", "value": "2 mm"}
    ]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    let hole = p.ok(&[
        "feature",
        "add",
        "p",
        "extrude",
        "--id",
        "h",
        "--sketch",
        "s",
        "--operation",
        "cut",
        "--extent",
        "through_all",
    ]);
    assert_eq!(hole["feature"]["targets"], serde_json::json!(["a"]));
    assert_eq!(
        hole["feature"]["extent"],
        serde_json::json!({"type": "through_all"})
    );
    // A second solid, joined to the first by a boolean.
    let mut second = vec![
        "feature", "add", "p", "box", "--id", "b", "--origin", "10,0,0",
    ];
    second.extend(unit);
    p.ok(&second);
    p.fails(
        &[
            "feature",
            "add",
            "p",
            "box",
            "--id",
            "j",
            "--operation",
            "join",
            "--param",
            "x=1",
            "--param",
            "y=1",
            "--param",
            "z=1",
        ],
        "invalid_param",
        1,
    );
    let u = p.ok(&[
        "feature", "add", "p", "boolean", "--id", "u", "--op", "unite", "--target", "a", "--tool",
        "b",
    ]);
    assert_eq!(u["feature"]["target"], "a");
    assert_eq!(u["feature"]["tools"], serde_json::json!(["b"]));
    p.ok(&["rebuild", "p"]);
    // b is the boolean's now.
    let e = p.fails(
        &[
            "feature",
            "add",
            "p",
            "box",
            "--id",
            "late",
            "--operation",
            "cut",
            "--target",
            "b",
            "--param",
            "x=1",
            "--param",
            "y=1",
            "--param",
            "z=1",
        ],
        "body_consumed",
        1,
    );
    assert_eq!(e["details"]["consumed_by"], "u");
    // Through all both ways from the sketch.
    p.ok(&["feature", "edit", "p", "h", "--both"]);
    let show = p.ok(&["feature", "show", "p", "h"]);
    assert_eq!(
        show["extent"],
        serde_json::json!({"type": "through_all", "both": true})
    );
    p.ok(&["rebuild", "p"]);
}

#[test]
fn blends_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=20", "--param", "y=10", "--param",
        "z=5",
    ]);
    let body = p.ok(&["inspect", "p", "b"])["bodies"][0].clone();
    let vertex = |want| named(&body, "vertices", |v| at(&v["point"], want));
    let edge = |a: [f64; 3], b: [f64; 3]| {
        let (a, b) = (vertex(a), vertex(b));
        let n = named(&body, "edges", |e| {
            (e["start"] == a.as_str() && e["end"] == b.as_str())
                || (e["start"] == b.as_str() && e["end"] == a.as_str())
        });
        format!("p/b/{n}")
    };
    let up = edge([0.02, 0.01, 0.0], [0.02, 0.01, 0.005]);
    let front = edge([0.0, 0.0, 0.005], [0.02, 0.0, 0.005]);
    let top = named(&body, "faces", |f| {
        let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
        f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == 1.0
    });
    let top = format!("p/b/{top}");
    let f = p.ok(&[
        "feature", "add", "p", "fillet", "--id", "f", "--radius", "1", "--edge", &up,
    ]);
    assert_eq!(f["feature"]["groups"][0]["edges"], serde_json::json!([up]));
    p.ok(&[
        "feature",
        "edit",
        "p",
        "f",
        "--radius",
        "2 mm",
        "--propagate",
        "false",
    ]);
    let show = p.ok(&["feature", "show", "p", "f"]);
    assert_eq!(show["groups"][0]["radius"]["value"], "2 mm");
    assert_eq!(show["propagate"], false);
    let c = p.ok(&[
        "feature",
        "add",
        "p",
        "chamfer",
        "--id",
        "c",
        "--form",
        "distances",
        "--d1",
        "0.5",
        "--d2",
        "1",
        "--face",
        &top,
        "--edge",
        &front,
    ]);
    assert_eq!(c["feature"]["groups"][0]["face"], top);
    p.ok(&["rebuild", "p"]);
    // Several sizes in one fillet, clear of the others.
    let bottom = edge([0.0, 0.0, 0.0], [0.02, 0.0, 0.0]);
    let back = edge([0.0, 0.01, 0.0], [0.0, 0.01, 0.005]);
    let groups = format!(
        r#"[{{"radius": "0.4 mm", "edges": ["{bottom}"]}}, {{"radius": "0.5 mm", "edges": ["{back}"]}}]"#
    );
    let g = p.ok(&[
        "feature", "add", "p", "fillet", "--id", "g", "--after", "b", "--groups", &groups,
    ]);
    assert_eq!(g["feature"]["groups"][1]["radius"]["value"], "0.5 mm");
    p.ok(&["rebuild", "p"]);
    p.fails(
        &["feature", "edit", "p", "g", "--radius", "2"],
        "invalid_param",
        1,
    );
    p.fails(
        &["feature", "add", "p", "fillet", "--groups", "{}"],
        "usage",
        2,
    );
    // The corner edge is the fillet's now.
    let e = p.fails(
        &["feature", "add", "p", "chamfer", "--d", "1", "--edge", &up],
        "reference_unresolved",
        1,
    );
    assert_eq!(e["details"]["removed_by"], "f");
    p.fails(&["feature", "delete", "p", "b"], "has_dependents", 1);
}

#[test]
fn releases_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    // One ID for both projects, so the two ways write the same records.
    p.write(
        "ryucad.json",
        "{\n  \"default_unit\": \"mm\",\n  \"format\": 2,\n  \"kind\": \"ryucad.project\",\n  \"project\": \"0123456789abcdef0123456789abcdef\"\n}\n",
    );
    p.ok(&["props", "set", "p", "number=100"]);
    p.fails(&["release", "propose", "p"], "not_a_git_repo", 1);
    p.git(&["init", "-q", "-b", "main"]);
    p.ok(&["git", "install"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "start"]);
    let v = p.ok(&["release", "list"]);
    assert_eq!(
        (&v["branch"]["name"], &v["parts"][0]["text"]),
        (&json!("main"), &json!("in work"))
    );
    let v = p.ok(&["release", "propose", "p", "--note", "first"]);
    assert_eq!(
        (&v["path"], &v["record"]["note"]),
        (&json!("releases/p/A.json"), &json!("first"))
    );
    p.fails(&["release", "propose", "p"], "release_pending", 1);
    let dry = p.ok(&["release", "propose", "p", "--dry-run"]);
    assert_eq!(
        (&dry["ready"], &dry["problems"][0]["code"]),
        (&json!(false), &json!("release_pending"))
    );
    p.fails(&["release", "obsolete", "p", "A"], "release_pending", 1);
    assert_eq!(p.ok(&["release", "check"])["findings"], json!([]));
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "release p A"]);
    p.fails(&["release", "propose", "p"], "nothing_to_release", 1);
    let v = p.ok(&["release", "list", "p"]);
    assert_eq!(
        (
            &v["records"][0]["status"],
            &v["records"][0]["tag"]["status"]
        ),
        (&json!("official"), &json!("missing"))
    );
    let v = p.ok(&["release", "check"]);
    assert_eq!(v["findings"][0]["code"], "release_tag_missing");
    let v = p.ok(&["release", "sync"]);
    assert_eq!(v["created"][0]["tag"], "ryupdm/p/A");
    let v = p.ok(&["release", "check", "--results"]);
    assert_eq!((&v["schema"], &v["findings"]), (&json!(1), &json!([])));
    assert_eq!(p.ok(&["props", "show", "p"])["state"]["text"], "released A");
    // An edit is work towards B; a stale proposal fails the checks.
    p.ok(&["feature", "edit", "p", "b", "--param", "x=12"]);
    assert_eq!(p.ok(&["props", "show", "p"])["state"]["text"], "A modified");
    p.ok(&["release", "propose", "p"]);
    p.ok(&["feature", "edit", "p", "b", "--param", "x=13"]);
    let e = p.fails(&["release", "check"], "check_failed", 1);
    assert_eq!(e["details"]["findings"][0]["code"], "release_stale");
    let e = p.fails(&["check"], "check_failed", 1);
    assert!(
        e["details"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "release_stale" && f["source"] == "release")
    );
    fs::remove_file(p.direct.join("releases/p/B.json")).unwrap();
    fs::remove_file(p.daemon.join("releases/p/B.json")).unwrap();
    p.fails(&["release", "obsolete", "p", "B"], "invalid_param", 1);
    let v = p.ok(&["release", "obsolete", "p", "A", "--note", "replaced by q"]);
    assert_eq!(v["path"], "releases/p/A.obsolete.json");
    // The daemon logged it: undo takes the record away, redo puts it back.
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!(code, 0, "{v}");
    assert!(!p.daemon.join("releases/p/A.obsolete.json").exists());
    let (code, v) = p.run_in(&p.daemon, "", &["redo"]);
    assert_eq!(code, 0, "{v}");
    assert!(p.daemon.join("releases/p/A.obsolete.json").exists());
}

#[test]
fn pdm_in_diff_and_through_textconv() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    p.write(
        "ryucad.json",
        "{\n  \"default_unit\": \"mm\",\n  \"format\": 2,\n  \"kind\": \"ryucad.project\",\n  \"project\": \"0123456789abcdef0123456789abcdef\"\n}\n",
    );
    p.ok(&["props", "set", "p", "number=100"]);
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "start"]);
    p.ok(&["release", "propose", "p"]);
    p.ok(&["props", "set", "p", "name=Pin"]);
    let v = p.ok(&["diff"]);
    assert_eq!(
        v["parts"]["p"]["pdm"],
        json!({"properties": {"name": {"revising": false, "to": "Pin"}},
               "records": {"added": [{"label": "A", "kind": "release"}],
                           "removed": [], "changed": []},
               "state": {"from": "in work", "to": "in work, A proposed"}})
    );
    // Git's own diff reads the record and the properties by value.
    let root = &p.direct;
    git(root, &["add", "."]);
    let bin = assert_cmd::cargo::cargo_bin("ryucad");
    let bin = bin.to_str().unwrap().replace('\\', "/");
    let driver = format!("diff.ryucad.textconv='{bin}' git textconv");
    fs::write(root.join(".gitattributes"), "*.json diff=ryucad\n").unwrap();
    let diff = git(
        root,
        &["-c", &driver, "diff", "--cached", "--no-color", "-U0"],
    );
    assert!(diff.contains("\n+name = Pin\n"), "{diff}");
    assert!(diff.contains("\n+ryupdm.release (format 1)\n"), "{diff}");
    assert!(diff.contains("\n+released_by = "), "{diff}");
    assert!(diff.contains("\n+\n+results.b = "), "{diff}");
}

#[test]
fn renders_a_committed_revision() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=10", "--param",
        "z=10",
    ]);
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "start"]);
    let head = git(&p.direct, &["rev-parse", "HEAD"]).trim().to_owned();
    let before = p.ok(&["render", "--out", "head.png", "--size", "160x120"]);
    p.ok(&["feature", "edit", "p", "b", "--param", "x=30"]);
    let r = p.ok(&[
        "render", "--out", "rev.png", "--size", "160x120", "--rev", "HEAD",
    ]);
    assert_eq!(r["commit"], head.as_str());
    assert_eq!(r["camera"], before["camera"]);
    assert_eq!(
        pixels(&p.direct.join("rev.png")),
        pixels(&p.direct.join("head.png"))
    );
    assert_eq!(
        pixels(&p.direct.join("rev.png")),
        pixels(&p.daemon.join("rev.png"))
    );
    // Framed together, both renders share one camera, whatever changed.
    let a = p.ok(&[
        "render", "--out", "a.png", "--rev", "HEAD", "--frame", "working",
    ]);
    let b = p.ok(&["render", "--out", "b.png", "--frame", "HEAD"]);
    assert_eq!(a["camera"], b["camera"]);
    assert_ne!(a["camera"], before["camera"]);
    p.fails(
        &["render", "--out", "x.png", "--frame", "nowhere"],
        "rev_not_found",
        1,
    );
    let c = p.ok(&[
        "render",
        "--out",
        "c.png",
        "--size",
        "160x120",
        "--compare",
        "HEAD",
    ]);
    assert_eq!(
        c["compare"],
        serde_json::json!({"rev": "HEAD", "commit": head})
    );
    let (_, _, px) = pixels(&p.direct.join("c.png"));
    // The box's changed faces in the compare's orange.
    assert!(
        px.chunks(4).any(|q| q[0] > 200 && q[1] > 120 && q[2] < 90),
        "changed orange"
    );
    assert_eq!(
        pixels(&p.direct.join("c.png")),
        pixels(&p.daemon.join("c.png"))
    );
    // Against itself, nothing differs: the render is the plain one.
    let same = p.ok(&[
        "render",
        "--out",
        "s.png",
        "--size",
        "160x120",
        "--compare",
        "working",
    ]);
    let plain = p.ok(&["render", "--out", "w.png", "--size", "160x120"]);
    assert_eq!(same["camera"], plain["camera"]);
    assert_eq!(
        pixels(&p.direct.join("s.png")),
        pixels(&p.direct.join("w.png"))
    );
    // A solid only the working files have is new as a whole; a solid only
    // the revision has is a red ghost.
    p.ok(&[
        "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=4", "--origin", "40,0,0",
    ]);
    let n = p.ok(&[
        "render",
        "--out",
        "n.png",
        "--size",
        "200x120",
        "--compare",
        "HEAD",
    ]);
    assert_eq!(n["compare"]["rev"], "HEAD");
    let (_, _, px) = pixels(&p.direct.join("n.png"));
    assert!(
        px.chunks(4).any(|q| q[1] > 150 && q[0] < 120 && q[2] < 120),
        "new green"
    );
    let back = p.ok(&[
        "render",
        "--out",
        "r.png",
        "--size",
        "200x120",
        "--rev",
        "HEAD",
        "--compare",
        "working",
    ]);
    assert_eq!(back["commit"], head.as_str());
    // The ghost is translucent: red over the background's grey, its blue
    // near its green (the compare's orange has little blue).
    let (_, _, px) = pixels(&p.direct.join("r.png"));
    assert!(
        px.chunks(4).any(|q| {
            let (r, g, b) = (i32::from(q[0]), i32::from(q[1]), i32::from(q[2]));
            r > g + 20 && b > 120 && (g - b).abs() < 12
        }),
        "removed red ghost"
    );
    assert_eq!(
        pixels(&p.direct.join("r.png")),
        pixels(&p.daemon.join("r.png"))
    );
    // A camera given explicitly isn't framed.
    p.fails(
        &[
            "render", "--out", "x.png", "--frame", "HEAD", "--of", "agent",
        ],
        "usage",
        2,
    );
    p.fails(
        &["render", "--out", "x.png", "--compare", "nowhere"],
        "rev_not_found",
        1,
    );
    p.fails(
        &["render", "--out", "x.png", "--rev", "nowhere"],
        "rev_not_found",
        1,
    );
}

#[test]
fn outputs_carry_their_schema() {
    let p = Pair::new();
    p.ok(&["init"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "sphere", "--id", "s", "--param", "radius=1",
    ]);
    p.git(&["init", "-q", "-b", "main"]);
    p.git(&["add", "."]);
    p.git(&["commit", "-q", "-m", "start"]);
    for args in [
        &["diff"][..],
        &["inspect", "p"],
        &["mesh", "p"],
        &["mesh", "p", "--out", "m.bin"],
        &["release", "list"],
        &["check"],
    ] {
        assert_eq!(p.ok(args)["schema"], 1, "{args:?}");
    }
}

#[test]
fn review_needs_a_github_clone() {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let out = Command::cargo_bin("ryucad")
            .unwrap()
            .current_dir(dir.path())
            .args(args)
            .env("RYUCAD_REVIEW_TOKEN", "t")
            .env("RYUCAD_REVIEW_URL", "http://127.0.0.1:9")
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        )
    };
    let (code, v) = run(&["review", "status"]);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("not_a_git_repo")),
        "{v}"
    );
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let (code, v) = run(&["review", "status", "3"]);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("review_no_remote")),
        "{v}"
    );
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widget.git",
        ],
    );
    let (code, v) = run(&["review", "status", "3"]);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (4, Some("review_unreachable")),
        "{v}"
    );
    let (code, _) = run(&["review"]);
    assert_eq!(code, 2);
}

#[test]
fn review_against_a_fake_service() {
    use fake_review::{Fake, json};
    let thread = serde_json::json!({"id": "t1", "ref": "p/b/0123456789abcdef0123456789abcdef", "state": "open"});
    let fake = Fake::start(vec![
        json(
            "GET",
            "/api/v1/repos/acme/widget/pulls?branch=feature%2Farm",
            200,
            serde_json::json!({"pulls": [{"number": 7}]}),
        ),
        json(
            "GET",
            "/api/v1/repos/acme/widget/pulls/7",
            200,
            serde_json::json!({
                "checks": {"check": "success", "merge": "action_required"},
                "renders": [
                    {"project": "", "part": "p", "side": "before", "url": "/api/v1/artifacts/k1"},
                    {"project": "models/arm", "part": "p", "side": "after", "url": "/api/v1/artifacts/k2"}]}),
        ),
        json(
            "GET",
            "/api/v1/repos/acme/widget/pulls/7/threads?state=open",
            200,
            serde_json::json!({"threads": [thread]}),
        ),
        json(
            "POST",
            "/api/v1/repos/acme/widget/pulls/7/threads",
            201,
            thread.clone(),
        ),
        json(
            "POST",
            "/api/v1/threads/t1/resolve",
            200,
            serde_json::json!({"id": "t1", "state": "resolved"}),
        ),
        json(
            "POST",
            "/api/v1/repos/acme/widget/pulls/7/merge-in",
            409,
            serde_json::json!({"error": {"code": "head_moved",
                "message": "the head moved since the merge was checked"}}),
        ),
        (
            "GET",
            "/api/v1/artifacts/k1".to_owned(),
            200,
            "image/png",
            vec![1],
        ),
        (
            "GET",
            "/api/v1/artifacts/k2".to_owned(),
            200,
            "image/png",
            vec![2],
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(
        dir.path(),
        &["remote", "add", "origin", "git@github.com:acme/widget.git"],
    );
    git(dir.path(), &["checkout", "-q", "-b", "feature/arm"]);
    let empty = tempfile::tempdir().unwrap();
    let run = |args: &[&str], token: Option<&str>| {
        let mut cmd = Command::cargo_bin("ryucad").unwrap();
        cmd.current_dir(dir.path())
            .args(args)
            .env("RYUCAD_REVIEW_URL", &fake.url)
            .env_remove("RYUCAD_REVIEW_TOKEN")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env("PATH", empty.path());
        if let Some(t) = token {
            cmd.env("RYUCAD_REVIEW_TOKEN", t);
        }
        let out = cmd.output().unwrap();
        (
            out.status.code().unwrap(),
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        )
    };
    let ok = |args: &[&str]| {
        let (code, v) = run(args, Some("tok"));
        assert_eq!((code, &v["ok"]), (0, &Value::Bool(true)), "{args:?}: {v}");
        v["result"].clone()
    };
    let s = ok(&["review", "status"]);
    assert_eq!(
        (s["repo"].as_str(), s["pull"].as_u64()),
        (Some("acme/widget"), Some(7))
    );
    assert_eq!(s["checks"]["merge"], "action_required");
    assert_eq!(
        ok(&["review", "threads", "--open"])["threads"][0]["id"],
        "t1"
    );
    let c = ok(&[
        "review",
        "comment",
        "p/b/0123456789abcdef0123456789abcdef",
        "too thin here",
    ]);
    assert_eq!(c["id"], "t1");
    assert_eq!(ok(&["review", "resolve", "t1"])["state"], "resolved");
    let r = ok(&["review", "renders", "7", "--out", "shots"]);
    assert_eq!(r["files"].as_array().unwrap().len(), 2);
    assert_eq!(
        fs::read(dir.path().join("shots/root-p-before.png")).unwrap(),
        vec![1]
    );
    assert_eq!(
        fs::read(dir.path().join("shots/models_arm-p-after.png")).unwrap(),
        vec![2]
    );
    let (code, v) = run(&["review", "update", "7"], Some("tok"));
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("review_head_moved")),
        "{v}"
    );
    let (code, v) = run(&["review", "status", "7"], None);
    assert_eq!(
        (code, v["error"]["code"].as_str()),
        (1, Some("review_unauthenticated")),
        "{v}"
    );
    // Every request carried the token; the comment its reference and text.
    let seen = fake.seen.lock().unwrap().clone();
    assert!(
        seen.iter().all(|s| s.auth.as_deref() == Some("Bearer tok")),
        "{seen:?}"
    );
    let posted = seen
        .iter()
        .find(|s| s.path == "/api/v1/repos/acme/widget/pulls/7/threads")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&posted.body).unwrap(),
        serde_json::json!({"ref": "p/b/0123456789abcdef0123456789abcdef", "body": "too thin here"})
    );
}

#[test]
fn holes_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=20", "--param", "y=10", "--param",
        "z=5",
    ]);
    // A sketch on the box's top with two points to drill at.
    p.ok(&[
        "feature", "add", "p", "sketch", "--id", "s", "--origin", "0,0,5",
    ]);
    let ops = r#"[
        {"op": "add", "type": "point", "at": [5, 5]},
        {"op": "add", "type": "fix", "a": "p1"},
        {"op": "add", "type": "point", "at": [15, 5]},
        {"op": "add", "type": "fix", "a": "p2"}
    ]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    let h = p.ok(&[
        "feature",
        "add",
        "p",
        "hole",
        "--id",
        "h",
        "--sketch",
        "s",
        "--form",
        "counterbore",
        "--diameter",
        "3",
        "--counterbore-diameter",
        "6",
        "--counterbore-depth",
        "1",
        "--depth",
        "3",
        "--bottom",
        "118",
    ]);
    let f = &h["feature"];
    assert_eq!(f["targets"], serde_json::json!(["b"]));
    assert_eq!(f["counterbore"]["depth"]["value"], "1 mm");
    assert_eq!(f["bottom"]["drill"]["value"], "118 deg");
    assert!(f.get("points").is_none());
    p.ok(&["rebuild", "p"]);
    // Countersunk, through everything, at one point.
    p.ok(&[
        "feature",
        "edit",
        "p",
        "h",
        "--form",
        "countersink",
        "--countersink-diameter",
        "6",
        "--countersink-angle",
        "90",
        "--depth",
        "through_all",
        "--bottom",
        "flat",
        "--point",
        "p2",
    ]);
    let show = p.ok(&["feature", "show", "p", "h"]);
    assert_eq!(show["points"], serde_json::json!(["p2"]));
    assert_eq!(show["depth"], "through_all");
    assert!(show.get("counterbore").is_none() && show.get("bottom").is_none());
    p.ok(&["rebuild", "p"]);
    // A drill point only ends a blind hole.
    p.fails(
        &["feature", "edit", "p", "h", "--bottom", "118"],
        "invalid_extent",
        1,
    );
    // Up to the bottom face, stopping 1 mm short; then every point again.
    let body = p.ok(&["inspect", "p", "b"])["bodies"][0].clone();
    let bottom = named(&body, "faces", |f| {
        let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
        f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign == -1.0
    });
    let bottom = format!("p/b/{bottom}");
    p.ok(&[
        "feature",
        "edit",
        "p",
        "h",
        "--depth",
        "up_to",
        "--up-to",
        &bottom,
        "--offset",
        "-1",
        "--all-points",
    ]);
    let show = p.ok(&["feature", "show", "p", "h"]);
    assert_eq!(show["depth"]["up_to"]["target"], bottom);
    assert!(show.get("points").is_none());
    p.ok(&["rebuild", "p"]);
    p.fails(
        &["feature", "edit", "p", "h", "--point", "p9"],
        "not_found",
        1,
    );
    p.fails(&["feature", "delete", "p", "s"], "has_dependents", 1);
}

#[test]
fn patterns_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=20", "--param", "y=10", "--param",
        "z=5", "--origin", "-10,-5,0",
    ]);
    p.ok(&[
        "feature", "add", "p", "sketch", "--id", "s", "--origin", "0,0,5",
    ]);
    let ops = r#"[
        {"op": "add", "type": "point", "at": [3, 2]},
        {"op": "add", "type": "fix", "a": "p1"}
    ]"#;
    p.ok(&["sketch", "edit", "p", "s", ops]);
    p.ok(&[
        "feature",
        "add",
        "p",
        "hole",
        "--id",
        "h",
        "--sketch",
        "s",
        "--diameter",
        "1",
        "--depth",
        "2",
    ]);
    // Three along x, then a second row along -y, one skipped.
    let pat = p.ok(&[
        "feature",
        "add",
        "p",
        "pattern",
        "--id",
        "pat",
        "--seed",
        "h",
        "--direction",
        "x",
        "--spacing",
        "2",
        "--count",
        "3",
    ]);
    assert_eq!(pat["feature"]["targets"], serde_json::json!(["b"]));
    assert_eq!(pat["feature"]["count"]["n"], 3.0);
    p.ok(&["rebuild", "p"]);
    p.ok(&[
        "feature",
        "edit",
        "p",
        "pat",
        "--direction2",
        "y",
        "--spacing2",
        "2",
        "--count2",
        "2",
        "--reverse2",
        "--skip",
        "2,1",
    ]);
    let show = p.ok(&["feature", "show", "p", "pat"]);
    assert_eq!(show["skip"], serde_json::json!([[2, 1]]));
    assert_eq!(show["reverse2"], true);
    p.ok(&["rebuild", "p"]);
    // Round the z axis, then its mirror image in the yz plane.
    p.ok(&[
        "feature",
        "edit",
        "p",
        "pat",
        "--form",
        "circular",
        "--axis",
        "z",
        "--count",
        "4",
        "--angle",
        "360",
        "--no-skip",
    ]);
    let show = p.ok(&["feature", "show", "p", "pat"]);
    assert_eq!(show["axis"], "z");
    assert!(show.get("skip").is_none() && show.get("direction").is_none());
    p.ok(&["rebuild", "p"]);
    p.ok(&[
        "feature", "edit", "p", "pat", "--form", "mirror", "--plane", "yz",
    ]);
    p.ok(&["rebuild", "p"]);
    p.fails(
        &["feature", "edit", "p", "pat", "--skip", "1"],
        "invalid_document",
        1,
    );
    p.fails(
        &["feature", "edit", "p", "pat", "--seed", "b"],
        "reference_wrong_kind",
        1,
    );
    p.fails(&["feature", "delete", "p", "h"], "has_dependents", 1);
}

#[test]
fn assemblies_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    let made = p.ok(&["part", "create", "frame", "--assembly"]);
    assert_eq!(made["kind"], "assembly");
    let added = p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b1", "--of", "bolt", "--origin", "10,0,0",
    ]);
    assert_eq!(added["feature"]["part"], "bolt");
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b2", "--of", "bolt", "--fixed",
    ]);
    // A part takes no instance, an assembly no solid.
    p.fails(
        &["feature", "add", "bolt", "instance", "--of", "bolt"],
        "wrong_part_kind",
        1,
    );
    p.fails(
        &["feature", "add", "frame", "instance", "--of", "frame"],
        "assembly_cycle",
        1,
    );
    let moved = p.ok(&[
        "assembly",
        "move",
        "frame",
        "b1",
        "--translate",
        "0,5mm,0",
        "--axis",
        "0,0,1",
        "--angle",
        "90deg",
    ]);
    assert_eq!(
        moved["pose"]["origin_m"],
        serde_json::json!([0.01, 0.005, 0])
    );
    p.fails(
        &["assembly", "move", "frame", "b2", "--translate", "1,0,0"],
        "instance_fixed",
        1,
    );
    let tree = p.ok(&["assembly", "tree", "frame"]);
    assert_eq!(tree["instances"][0]["path"], "b1");
    assert_eq!(tree["instances"][1]["fixed"], true);
    let built = p.ok(&["rebuild", "frame"]);
    assert_eq!(
        built["parts"][0]["occurrences"].as_array().unwrap().len(),
        2
    );
    let shown = p.ok(&["inspect", "frame", "b1"]);
    let face = shown["occurrences"][0]["bodies"][0]["faces"][0]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let one = p.ok(&[
        "inspect",
        "frame",
        "--entity",
        &format!("frame/b1/shank/{face}"),
    ]);
    assert_eq!(one["kind"], "face");
    p.fails(&["part", "delete", "bolt"], "has_dependents", 1);
    // Undone through the daemon (undo needs one), the move is gone there.
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!(code, 0, "{v}");
    let (_, tree) = p.run_in(&p.daemon, "", &["assembly", "tree", "frame"]);
    assert_eq!(
        tree["result"]["instances"][0]["pose"]["origin_m"],
        serde_json::json!([0.01, 0, 0])
    );
}

#[test]
fn measure_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=30",
    ]);
    let body = p.ok(&["inspect", "p"])["bodies"][0].clone();
    let face = |k: usize| format!("p/b/{}", body["faces"][k]["name"].as_str().unwrap());
    let one = p.ok(&["measure", &face(0)]);
    assert_eq!(one["kind"], "face", "{one}");
    assert!(one["area"]["m2"].as_f64().unwrap() > 0.0);
    let two = p.ok(&["measure", &face(0), &face(1)]);
    assert!(two["distance"]["m"].is_number(), "{two}");
    let solid = p.ok(&["measure", "p/b"]);
    assert_eq!(solid["volume"]["value"], "6000 mm^3", "{solid}");
    p.fails(&["measure", "p/nothing"], "not_found", 1);
    let (code, _) = p.both_from("", &["measure"]);
    assert_eq!(code, 2);
}

#[test]
fn materials_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "bracket"]);
    let set = p.ok(&[
        "materials",
        "set",
        "steel",
        "--density",
        "7850 kg/m^3",
        "--default",
    ]);
    assert_eq!(set["created"], true, "{set}");
    let rev = set["project_revision"].as_str().unwrap().to_owned();
    p.ok(&[
        "materials",
        "set",
        "aluminium 6061",
        "--density",
        "2.7 g/cm^3",
        "--colour",
        "#b8bcc2",
        "--expect",
        &rev,
    ]);
    p.fails(
        &[
            "materials",
            "set",
            "brass",
            "--density",
            "8.5 g/cm^3",
            "--expect",
            &rev,
        ],
        "conflict",
        1,
    );
    p.fails(
        &["materials", "set", "brass", "--density", "8.5"],
        "invalid_quantity",
        1,
    );
    // As an agent would write it: `props set` would give each project an
    // ID of its own, and the two answers would differ.
    p.write(
        "parts/bracket/properties.json",
        &ryucad::doc::canonical::to_string(&json!({
            "format": 1, "kind": "ryupdm.properties", "material": "aluminium 6061"
        })),
    );
    let list = p.ok(&["materials", "list"]);
    assert_eq!(list["default"], "steel", "{list}");
    assert_eq!(list["materials"][0]["parts"], json!(["bracket"]), "{list}");
    p.fails(
        &["materials", "delete", "aluminium 6061"],
        "has_dependents",
        1,
    );
    // The bracket's mass at aluminium's density; with no material and no
    // default a part gives its geometry and a warning.
    p.ok(&[
        "feature", "add", "bracket", "box", "--id", "b", "--param", "x=10", "--param", "y=10",
        "--param", "z=10",
    ]);
    let m = p.ok(&["mass", "bracket"]);
    assert_eq!(m["mass"]["value"], "0.0027 kg", "{m}");
    assert_eq!(m["parts"][0]["material"], "aluminium 6061");
    p.ok(&["part", "create", "loose"]);
    p.ok(&[
        "feature", "add", "loose", "box", "--id", "b", "--param", "x=10", "--param", "y=10",
        "--param", "z=10",
    ]);
    p.ok(&["materials", "delete", "steel"]);
    assert_eq!(p.ok(&["materials", "list"])["default"], json!(null));
    let (code, v) = p.both_from("", &["mass", "loose"]);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["warnings"][0].as_str().unwrap().contains("no material"),
        "{v}"
    );
    assert!(v["result"].get("mass").is_none(), "{v}");
    // In a session, the delete is one undoable change.
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!((code, &v["ok"]), (0, &json!(true)), "{v}");
    let (_, v) = p.run_in(&p.daemon, "", &["materials", "list"]);
    assert_eq!(v["result"]["default"], "steel", "{v}");
}

#[test]
fn named_views_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=40", "--param", "y=20", "--param",
        "z=10",
    ]);
    let rev = p.ok(&["view", "list", "p"])["revisions"]["p"]
        .as_str()
        .unwrap()
        .to_owned();
    let saved = p.ok(&[
        "view",
        "save",
        "p",
        "detail",
        "--eye",
        "100,-80,60",
        "--target",
        "0,0,0",
        "--up",
        "0,0,1",
        "--ortho",
        "70",
        "--expect",
        &rev,
    ]);
    assert_eq!(saved["camera"]["eye"], json!([0.1, -0.08, 0.06]), "{saved}");
    assert_eq!(
        saved["camera"]["projection"],
        json!({"orthographic": {"height": 0.07}})
    );
    assert!(p.direct.join("parts/p/views/detail.json").is_file());
    p.fails(
        &[
            "view", "save", "p", "other", "--eye", "1,1,1", "--target", "0,0,0", "--up", "0,0,1",
            "--expect", &rev,
        ],
        "conflict",
        1,
    );
    p.fails(
        &[
            "view", "save", "p", "iso", "--eye", "1,1,1", "--target", "0,0,0", "--up", "0,0,1",
        ],
        "invalid_id",
        1,
    );
    // A render restores it, and draws the origin when asked.
    let r = p.ok(&[
        "render", "p", "--out", "a.png", "--size", "160x120", "--view", "detail", "--origin",
    ]);
    assert_eq!(r["camera"], saved["camera"], "{r}");
    let r = p.ok(&[
        "render",
        "--out",
        "b.png",
        "--size",
        "160x120",
        "--view",
        "back_bottom_left",
    ]);
    assert!(r["camera"]["eye"][0].as_f64().unwrap() < 0.0, "{r}");
    p.ok(&["view", "rename", "p", "detail", "close"]);
    let list = p.ok(&["view", "list"]);
    assert_eq!(list["views"][0]["view"], "close", "{list}");
    p.fails(&["view", "delete", "p", "detail"], "view_not_found", 1);
    // In a session, the camera restores it too; with no daemon there is
    // no camera to set.
    let (code, v) = p.run_in(&p.daemon, "", &["camera", "--view", "close"]);
    assert_eq!((code, &v["result"]["camera"]), (0, &saved["camera"]), "{v}");
    let (code, v) = p.run_in(&p.daemon, "", &["view", "save", "p", "mine"]);
    assert_eq!((code, &v["result"]["camera"]), (0, &saved["camera"]), "{v}");
    let (_, v) = p.run_in(&p.daemon, "", &["inspect", "p"]);
    let top = v["result"]["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["surface"]["type"] == "plane"
                && f["surface"]["frame"]["z"][2].as_f64().unwrap()
                    * if f["reversed"] == true { -1.0 } else { 1.0 }
                    == 1.0
        })
        .map(|f| format!("p/b/{}", f["name"].as_str().unwrap()))
        .unwrap();
    let (code, v) = p.run_in(&p.daemon, "", &["camera", "--normal-to", &top]);
    assert_eq!(code, 0, "{v}");
    let eye = &v["result"]["camera"]["eye"];
    let target = &v["result"]["camera"]["target"];
    assert!(
        eye[2].as_f64().unwrap() > target[2].as_f64().unwrap(),
        "{v}"
    );
    let (code, v) = p.run_in(&p.daemon, "", &["view", "delete", "p", "close"]);
    assert_eq!(code, 0, "{v}");
    // One undoable change.
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!((code, &v["ok"]), (0, &json!(true)), "{v}");
    assert!(p.daemon.join("parts/p/views/close.json").is_file());
}

#[test]
fn interference_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    p.ok(&["part", "create", "plate"]);
    p.ok(&[
        "feature", "add", "plate", "box", "--id", "slab", "--param", "x=100", "--param", "y=50",
        "--param", "z=5",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "p1", "--of", "plate",
    ]);
    // Through the plate, standing on it, and away from it.
    for (id, at) in [("b1", "10,10,0"), ("b2", "30,10,5"), ("b3", "500,0,0")] {
        p.ok(&[
            "feature", "add", "frame", "instance", "--id", id, "--of", "bolt", "--origin", at,
        ]);
    }
    let all = p.ok(&["interference", "frame"]);
    assert_eq!(
        (&all["pairs"], &all["clear"]),
        (&json!(2), &json!(1)),
        "{all}"
    );
    let i = &all["interferences"][0];
    assert_eq!(
        (&i["a"], &i["b"]),
        (&json!("frame/p1/slab"), &json!("frame/b1/shank"))
    );
    let v = i["volume_m3"].as_f64().unwrap();
    assert!((v - std::f64::consts::PI * 45e-9).abs() < 1e-3 * v, "{v}");
    let one = p.ok(&["interference", "frame", "--instance", "b2"]);
    assert_eq!(
        (&one["pairs"], &one["interferences"]),
        (&json!(1), &json!([]))
    );
    p.fails(
        &["interference", "frame", "--instance", "b9"],
        "not_found",
        1,
    );
    p.fails(&["interference", "bolt"], "wrong_part_kind", 1);
    // The same assembly's clearances: b2 stands on the plate (0), b3 is
    // half a metre away (only within a reach that long).
    let c = p.ok(&["clearance", "frame", "--below", "1000"]);
    let list = c["clearances"].as_array().unwrap();
    assert_eq!(list[0]["distance"]["m"], json!(0.0), "{c}");
    assert!(list.iter().any(|x| x["b"] == "frame/b3/shank"), "{c}");
    let near = p.ok(&["clearance", "frame", "--instance", "b3"]);
    assert_eq!(near["clearances"], json!([]), "{near}");
    p.fails(&["clearance", "bolt"], "wrong_part_kind", 1);
}

#[test]
fn mates_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    let params: [(&str, &str, &str, &[&str]); 2] = [
        ("plate", "box", "slab", &["x=100", "y=50", "z=5"]),
        ("bolt", "cylinder", "shank", &["radius=3", "height=20"]),
    ];
    for (part, ty, id, params) in params {
        p.ok(&["part", "create", part]);
        let mut args = vec!["feature", "add", part, ty, "--id", id];
        for x in params {
            args.extend(["--param", x]);
        }
        p.ok(&args);
    }
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "base", "--of", "plate", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "pin", "--of", "bolt", "--origin", "50,0,0",
    ]);
    let face = |part: &str, solid: &str, z: f64| -> String {
        let v = p.ok(&["inspect", part, solid]);
        v["bodies"][0]["faces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
                f["surface"]["type"] == "plane"
                    && (f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign - z).abs() < 1e-12
            })
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .unwrap()
    };
    let top = format!("frame/base/slab/{}", face("plate", "slab", 1.0));
    let foot = format!("frame/pin/shank/{}", face("bolt", "shank", -1.0));
    let added = p.ok(&[
        "feature",
        "add",
        "frame",
        "mate",
        "--id",
        "m1",
        "--form",
        "revolute",
        "--a",
        &top,
        "--a-offset",
        r#"{"origin": ["20 mm", "10 mm", "0 mm"]}"#,
        "--b",
        &foot,
        "--b-offset",
        r#"{"flip": true}"#,
    ]);
    // The write keeps the assembly current: the pin's solved pose stored.
    assert_eq!(added["assemblies"][0]["moved"], serde_json::json!(["pin"]));
    let solved = p.ok(&["assembly", "solve", "frame"]);
    assert_eq!(solved["converged"], true);
    assert_eq!(solved["solve"]["instances"]["pin"]["dof"], 1);
    // A turn about the hinge, previewed, then made.
    let preview = p.ok(&[
        "assembly", "solve", "frame", "pin", "--axis", "0,0,1", "--angle", "30deg",
    ]);
    assert_eq!(preview["held"], true);
    // As a drag previews it: only what the pin's mates reach, the fixed
    // plate left out, the pin where the whole solve puts it.
    let part = p.ok(&[
        "assembly",
        "solve",
        "frame",
        "pin",
        "--axis",
        "0,0,1",
        "--angle",
        "30deg",
        "--component",
    ]);
    assert_eq!(part["component"], true);
    let ids: Vec<&String> = part["instances"].as_object().unwrap().keys().collect();
    assert_eq!(ids, ["pin"]);
    assert_eq!(
        part["instances"]["pin"]["pose"],
        preview["instances"]["pin"]["pose"]
    );
    p.fails(&["assembly", "solve", "frame", "--component"], "usage", 2);
    p.ok(&[
        "assembly", "move", "frame", "pin", "--axis", "0,0,1", "--angle", "30deg",
    ]);
    // Driven: no freedom left; a second hinge elsewhere is refused.
    p.ok(&["feature", "edit", "frame", "m1", "--angle", "45deg"]);
    let tree = p.ok(&["assembly", "tree", "frame"]);
    assert_eq!(tree["instances"][1]["solve"]["dof"], 0);
    p.fails(
        &[
            "feature",
            "add",
            "frame",
            "mate",
            "--form",
            "revolute",
            "--a",
            &top,
            "--b",
            &foot,
            "--b-offset",
            r#"{"flip": true}"#,
        ],
        "assembly_overconstrained",
        1,
    );
    // Copies round the plate's z, one named by a path.
    p.ok(&[
        "feature",
        "add",
        "frame",
        "instance_pattern",
        "--id",
        "ring",
        "--instance",
        "pin",
        "--axis",
        "z",
        "--count",
        "4",
        "--angle",
        "360deg",
    ]);
    let built = p.ok(&["rebuild", "frame"]);
    assert_eq!(
        built["parts"][0]["occurrences"].as_array().unwrap().len(),
        5
    );
    let on_copy = foot.replace("frame/pin/", "frame/ring:2/pin/");
    let one = p.ok(&["inspect", "frame", "--entity", &on_copy]);
    assert_eq!(one["kind"], "face");
    p.fails(&["feature", "delete", "frame", "pin"], "has_dependents", 1);
    p.ok(&["check", "frame"]);
}

#[test]
fn imports_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    // Beside both projects, named as an agent would from inside one.
    let occt = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests/data/occt");
    let beside = p.direct.parent().unwrap();
    fs::copy(occt.join("box_ap214.stp"), beside.join("box.stp")).unwrap();
    fs::write(beside.join("junk.step"), "not step").unwrap();
    let dry = p.ok(&["import", "../box.stp", "--dry-run"]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["parts"][0]["features"][0]["ok"], true, "{dry}");
    assert!(!p.direct.join("imports").exists());
    let made = p.ok(&["import", "../box.stp", "--part", "block"]);
    assert_eq!(made["file"]["path"], "imports/box.step");
    assert_eq!(made["parts"][0]["part"], "block");
    let built = p.ok(&["rebuild", "block"]);
    assert_eq!(built["parts"][0]["features"][0]["type"], "import");
    assert_eq!(built["parts"][0]["features"][0]["counts"]["faces"], 6);
    // A fillet on an imported edge, by its stable name.
    let body = p.ok(&["inspect", "block", "body"]);
    let edge = body["bodies"][0]["edges"][0]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    p.ok(&[
        "feature",
        "add",
        "block",
        "fillet",
        "--radius",
        "1",
        "--edge",
        &format!("block/body/{edge}"),
    ]);
    p.ok(&["check", "block"]);
    p.fails(&["import", "../junk.step"], "step_unreadable", 1);
    p.fails(&["feature", "add", "block", "import"], "invalid_param", 1);
}

#[test]
fn exports_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    let stamp = "2026-10-27T00:00:00";
    // The daemon writes the file, in a folder that must exist.
    p.fails(
        &["export", "p", "--out", "out/p.step", "--time-stamp", stamp],
        "not_found",
        1,
    );
    fs::create_dir(p.direct.join("out")).unwrap();
    fs::create_dir(p.daemon.join("out")).unwrap();
    let v = p.ok(&["export", "p", "--out", "out/p.step", "--time-stamp", stamp]);
    assert_eq!(
        Path::new(v["out"].as_str().unwrap()),
        Path::new("<root>").join("out").join("p.step")
    );
    assert_eq!(
        (
            &v["format"],
            &v["schema"],
            &v["unit"],
            &v["file_name"],
            &v["time_stamp"]
        ),
        (
            &json!("step"),
            &json!("ap242"),
            &json!("mm"),
            &json!("p.step"),
            &json!(stamp)
        )
    );
    assert_eq!(v["products"][0]["product"], "p");
    // The same bytes in process and through the daemon, written whole.
    let direct = fs::read(p.direct.join("out/p.step")).unwrap();
    assert_eq!(direct, fs::read(p.daemon.join("out/p.step")).unwrap());
    assert!(direct.starts_with(b"ISO-10303-21;"));
    assert!(!p.direct.join("out/.p.step.tmp").exists());
    assert_eq!(v["bytes"], json!(direct.len()));
    assert_eq!(
        v["blake3"],
        json!(blake3::hash(&direct).to_hex().to_string())
    );
    // The format by name, whatever the extension; an extension export
    // doesn't know is refused.
    let v = p.ok(&[
        "export",
        "p",
        "--out",
        "out/p.txt",
        "--format",
        "step",
        "--time-stamp",
        stamp,
    ]);
    assert_eq!(v["format"], "step");
    p.fails(&["export", "p", "--out", "out/p.txt"], "invalid_param", 1);
    let v = p.ok(&[
        "export",
        "p",
        "--out",
        "out/p214.step",
        "--schema",
        "ap214",
        "--unit",
        "m",
        "--time-stamp",
        stamp,
    ]);
    assert_eq!((&v["schema"], &v["unit"]), (&json!("ap214"), &json!("m")));
    // What it wrote imports again: the box's six faces.
    p.ok(&["import", "out/p.step", "--part", "q"]);
    let built = p.ok(&["rebuild", "q"]);
    assert_eq!(built["parts"][0]["features"][0]["counts"]["faces"], 6);
    p.fails(&["export", "nope", "--out", "out/x.step"], "not_found", 1);
    p.fails(
        &["export", "p", "--out", "out/x.step", "--schema", "ap203"],
        "invalid_param",
        1,
    );
    p.fails(
        &["export", "p", "--out", "out/x.step", "--rev", "HEAD"],
        "not_a_git_repo",
        1,
    );
    assert!(!p.direct.join("out/x.step").exists());
}

#[test]
fn mesh_exports_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    p.ok(&["part", "create", "p"]);
    p.ok(&[
        "feature", "add", "p", "box", "--id", "b", "--param", "x=10", "--param", "y=20", "--param",
        "z=5",
    ]);
    // STL by extension, binary: twelve triangles in millimetres.
    let v = p.ok(&["export", "p", "--out", "p.stl"]);
    assert_eq!(
        (&v["format"], &v["unit"], &v["ascii"]),
        (&json!("stl"), &json!("mm"), &json!(false))
    );
    assert_eq!(v["mesh"]["chord"]["value"], "0.01 mm");
    assert_eq!(v["products"][0]["triangles"], 12);
    let bytes = fs::read(p.direct.join("p.stl")).unwrap();
    assert_eq!(bytes, fs::read(p.daemon.join("p.stl")).unwrap());
    assert_eq!(bytes.len(), 84 + 50 * 12);
    assert!(bytes.starts_with(b"ryucad "));
    // ASCII in centimetres, by name, with the mesh options.
    let v = p.ok(&[
        "export",
        "p",
        "--out",
        "p.txt",
        "--format",
        "stl",
        "--ascii",
        "--unit",
        "cm",
        "--chord",
        "0.1 mm",
        "--angle",
        "30 deg",
        "--max-edge",
        "5 mm",
    ]);
    assert_eq!(v["mesh"]["max_edge"]["value"], "5 mm");
    let text = fs::read_to_string(p.direct.join("p.txt")).unwrap();
    assert!(text.starts_with("solid p/b\n"), "{text}");
    assert!(text.contains("vertex 1 2 0.5\n"), "{text}");
    // Options another format takes are refused before anything is written.
    for args in [
        &["export", "p", "--out", "x.step", "--chord", "0.1 mm"][..],
        &["export", "p", "--out", "x.stl", "--schema", "ap214"],
        &["export", "p", "--out", "x.stl", "--chord", "2 * 1 mm"],
        &["export", "p", "--out", "x.stl", "--angle", "120 deg"],
    ] {
        p.fails(args, "invalid_param", 1);
    }
    assert!(!p.direct.join("x.stl").exists() && !p.direct.join("x.step").exists());
    // 3MF by extension, in metres, its creation date the time stamp.
    let v = p.ok(&[
        "export",
        "p",
        "--out",
        "p.3mf",
        "--unit",
        "m",
        "--time-stamp",
        "2026-10-28T00:00:00",
    ]);
    assert_eq!((&v["format"], &v["unit"]), (&json!("3mf"), &json!("m")));
    let bytes = fs::read(p.direct.join("p.3mf")).unwrap();
    assert_eq!(bytes, fs::read(p.daemon.join("p.3mf")).unwrap());
    assert!(bytes.starts_with(b"PK\x03\x04"));
    p.fails(
        &["export", "p", "--out", "x.3mf", "--ascii"],
        "invalid_param",
        1,
    );
}

#[test]
fn imported_assemblies_and_updates_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    let occt = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryustep/tests/data/occt");
    let beside = p.direct.parent().unwrap();
    let text = fs::read_to_string(occt.join("assembly.stp")).unwrap();
    fs::write(beside.join("asm.stp"), &text).unwrap();
    // XDE's assembly: a block placed twice and a pin.
    let made = p.ok(&["import", "../asm.stp"]);
    let parts: Vec<(&str, &str)> = made["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["part"].as_str().unwrap(), x["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(
        parts,
        [("assembly", "assembly"), ("block", "part"), ("pin", "part")]
    );
    let tree = p.ok(&["assembly", "tree", "assembly"]);
    assert_eq!(tree["instances"].as_array().unwrap().len(), 3, "{tree}");
    // The pin moved 10 mm in a new version: a dry run says so and writes
    // nothing; the update moves it.
    let moved = text.replace(
        "#24 = CARTESIAN_POINT('',(0.,50.,0.));",
        "#24 = CARTESIAN_POINT('',(0.,60.,0.));",
    );
    assert_ne!(moved, text);
    fs::write(beside.join("asm2.stp"), &moved).unwrap();
    let dry = p.ok(&[
        "import",
        "../asm2.stp",
        "--update",
        "imports/asm.step",
        "--dry-run",
    ]);
    assert_eq!(
        (&dry["update"], &dry["dry_run"]),
        (&json!(true), &json!(true))
    );
    let pose = |p: &Pair| {
        let v = p.ok(&["feature", "show", "assembly", "3"]);
        v["pose"]["origin_m"].clone()
    };
    assert_eq!(pose(&p), json!([0, 0.05, 0]));
    p.ok(&["import", "../asm2.stp", "--update", "imports/asm.step"]);
    assert_eq!(pose(&p), json!([0, 0.06, 0]));
    // Undone through the daemon (undo needs one), in one step: the bytes,
    // the hashes and the pose.
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!(code, 0, "{v}");
    let (_, v) = p.run_in(&p.daemon, "", &["feature", "show", "assembly", "3"]);
    assert_eq!(v["result"]["pose"]["origin_m"], json!([0, 0.05, 0]));
    assert_eq!(
        fs::read_to_string(p.daemon.join("imports/asm.step")).unwrap(),
        text
    );
    p.fails(
        &["import", "../asm2.stp", "--update", "imports/nothing.step"],
        "not_found",
        1,
    );
}

#[test]
fn boms_from_the_cli() {
    let p = Pair::new();
    p.ok(&["init", "--unit", "mm"]);
    // One project ID both ways, so properties' writes answer alike.
    p.write(
        "ryucad.json",
        "{\n  \"default_unit\": \"mm\",\n  \"format\": 2,\n  \"kind\": \"ryucad.project\",\n  \"project\": \"0123456789abcdef0123456789abcdef\"\n}\n",
    );
    for part in ["bolt", "tube"] {
        p.ok(&["part", "create", part]);
        p.ok(&[
            "feature", "add", part, "box", "--id", "b", "--param", "x=1", "--param", "y=1",
            "--param", "z=1",
        ]);
    }
    p.ok(&["props", "set", "bolt", "number=100", "name=Bolt, M4"]);
    p.ok(&[
        "props",
        "set",
        "tube",
        "number=300",
        "uom=m",
        "amount=600 mm",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    for (id, of) in [("b1", "bolt"), ("b2", "bolt"), ("t1", "tube")] {
        p.ok(&[
            "feature", "add", "frame", "instance", "--id", id, "--of", of,
        ]);
    }
    let v = p.ok(&["bom"]);
    assert_eq!((&v["schema"], &v["root"]), (&json!(1), &json!("frame")));
    assert_eq!(v["tree"]["children"][0]["quantity"]["count"], 2);
    let v = p.ok(&["bom", "frame", "--flat"]);
    assert_eq!(v["flat"][1]["quantity"]["text"], "0.6 m");
    let v = p.ok(&["bom", "--out", "out/bom.csv"]);
    assert_eq!(v["out"], "out/bom.csv");
    assert!(v.get("csv").is_none());
    let csv = fs::read_to_string(p.direct.join("out/bom.csv")).unwrap();
    assert_eq!(
        csv,
        fs::read_to_string(p.daemon.join("out/bom.csv")).unwrap()
    );
    assert_eq!(
        csv,
        "level,item,number,name,part,revision,quantity,uom,make_buy,material,description\n\
         1,1,100,\"Bolt, M4\",bolt,working,2,each,make,,\n\
         2,2,300,,tube,working,0.6,m,make,,\n"
    );
    // Item numbers: set together, refused when they would clash, compacted,
    // and each change undone whole.
    let v = p.ok(&["bom", "renumber", "frame", "bolt=5", "tube=1"]);
    assert_eq!(v["items"], json!({"bolt": 5, "tube": 1}));
    let v = p.ok(&["bom", "--format", "csv"]);
    assert!(v["csv"].as_str().unwrap().contains("\n5,5,100,"), "{v}");
    p.fails(
        &["bom", "renumber", "frame", "bolt=1"],
        "bom_item_duplicate",
        1,
    );
    p.fails(&["bom", "renumber", "frame", "nut=3"], "not_found", 1);
    p.fails(&["bom", "renumber", "frame", "bolt=x"], "invalid_param", 1);
    p.fails(&["bom", "renumber", "bolt", "bolt=1"], "wrong_part_kind", 1);
    p.fails(
        &["bom", "renumber", "frame", "--compact", "bolt=1"],
        "usage",
        2,
    );
    let v = p.ok(&["bom", "renumber", "frame", "--compact"]);
    assert_eq!(v["items"], json!({"tube": 1, "bolt": 2}));
    let (code, v) = p.run_in(&p.daemon, "", &["undo"]);
    assert_eq!(code, 0, "{v}");
    let list: Value =
        serde_json::from_str(&fs::read_to_string(p.daemon.join("parts/frame/part.json")).unwrap())
            .unwrap();
    assert_eq!(list["items"], json!({"bolt": 5, "tube": 1}));
    let v = p.ok(&["where-used", "bolt", "--direct"]);
    assert_eq!(v["working"][0]["count"], 2);
    p.fails(&["bom", "diff"], "not_a_git_repo", 1);
    p.fails(&["bom", "--format", "xml"], "invalid_param", 1);
    p.fails(
        &["bom", "--out", "out/x.csv", "--format", "json"],
        "invalid_param",
        1,
    );
    p.fails(&["where-used", "nope!"], "invalid_id", 1);
}
