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
        r#"{"conflicts":{"/params/x":{"kind":"value"}},"format":1,"kind":"ryucad.feature"}"#,
    );
    p.fails(&["feature", "show", "p", "t"], "merge_conflict", 1);
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
        ("sketch.edit", &["sketch", "edit"]),
        ("sketch.solve", &["sketch", "solve"]),
        ("sketch.get", &["sketch", "get"]),
        ("rebuild", &["rebuild"]),
        ("inspect", &["inspect"]),
        ("check_format", &["check-format"]),
        ("diff", &["diff"]),
        ("check", &["check"]),
        ("merge.status", &["merge", "status"]),
        ("merge.resolve", &["merge", "resolve"]),
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
            .env("RYUCAD_NO_DAEMON", "1")
            .env("RYUCAD_CONFIG_DIR", dir.path().join("config"))
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
    let (_, v) = run(&["doctor"]);
    assert_eq!(codes(&v), ["git_hook_missing", "git_hook_missing"]);
    // In the repository's own config: the drivers and the hook.
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
