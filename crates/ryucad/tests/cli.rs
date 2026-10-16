//! The CLI as an agent sees it: run the binary, parse stdout as JSON.

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn ryucad_in(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (out.status.code().unwrap(), json)
}

fn ryucad(args: &[&str]) -> (i32, Value) {
    ryucad_in(&std::env::temp_dir(), args)
}

/// Runs a command that must succeed and returns its result.
fn ok(dir: &Path, args: &[&str]) -> Value {
    let (code, v) = ryucad_in(dir, args);
    assert_eq!((code, &v["ok"]), (0, &Value::Bool(true)), "{args:?}: {v}");
    v["result"].clone()
}

/// Runs a command that must fail with `error` and exit code `exit`.
fn fails(dir: &Path, args: &[&str], error: &str, exit: i32) -> Value {
    let (code, v) = ryucad_in(dir, args);
    assert_eq!(v["ok"], false, "{args:?}: {v}");
    assert_eq!(
        (code, v["error"]["code"].as_str().unwrap()),
        (exit, error),
        "{args:?}: {v}"
    );
    v["error"].clone()
}

#[test]
fn version_reports_every_component() {
    let (code, v) = ryucad(&["version"]);
    assert_eq!(code, 0);
    assert_eq!(v["ok"], true);
    let r = &v["result"];
    assert_eq!(r["ryucad"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["ryukei"], env!("CARGO_PKG_VERSION"));
    assert_eq!(r["protocol"], 1);
    assert!(r["target"].as_str().unwrap().contains('-'));
    assert!(!r["commit"].as_str().unwrap().is_empty());
}

#[test]
fn an_agent_session() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    ok(d, &["init", "--unit", "mm"]);
    ok(d, &["part", "create", "bracket"]);
    let r = ok(
        d,
        &[
            "feature", "add", "bracket", "box", "--id", "plate", "--param", "x=100", "--param",
            "y=60", "--param", "z=5",
        ],
    );
    let rev = r["revision"].as_str().unwrap().to_owned();
    ok(
        d,
        &[
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
        ],
    );
    // From a subfolder, the project is found above.
    let sub = d.join("parts/bracket");
    let list = ok(&sub, &["feature", "list", "bracket"]);
    assert_eq!(list["features"][1]["id"], "cylinder-1");
    let r = ok(d, &["rebuild"]);
    assert_eq!(r["parts"][0]["failed"], 0);
    let inspect = ok(d, &["inspect", "bracket", "plate"]);
    let face = inspect["bodies"][0]["faces"][0]["name"]
        .as_str()
        .unwrap()
        .to_owned();
    // Editing keeps names.
    ok(
        d,
        &["feature", "edit", "bracket", "plate", "--param", "x=120"],
    );
    let again = ok(d, &["inspect", "bracket", "plate", "--entity", &face]);
    assert_eq!(again["kind"], "face");
    ok(d, &["feature", "move", "bracket", "cylinder-1", "--first"]);
    ok(d, &["feature", "delete", "bracket", "plate"]);
    assert_eq!(ok(d, &["check-format"])["clean"], true);
    let shown = ok(d, &["feature", "show", "bracket", "cylinder-1"]);
    assert_eq!(
        shown["params"]["height"],
        serde_json::json!({"value": "2 in", "m": 0.0508})
    );
    assert_eq!(shown["placement"]["origin"][2]["value"], "5 mm");
}

#[test]
fn every_error_code_has_its_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    fails(d, &["part", "list"], "project_not_found", 3);
    ok(d, &["init"]);
    fails(d, &["init"], "project_exists", 1);
    fails(d, &["part", "create", "Bad"], "invalid_id", 1);
    ok(d, &["part", "create", "p"]);
    fails(d, &["part", "create", "p"], "duplicate_id", 1);
    fails(
        d,
        &["feature", "add", "p", "prism"],
        "unknown_feature_type",
        1,
    );
    fails(d, &["feature", "add", "p", "sphere"], "invalid_param", 1);
    fails(
        d,
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
    fails(
        d,
        &[
            "feature", "add", "p", "sphere", "--param", "radius=1", "--z", "0,0,0",
        ],
        "invalid_placement",
        1,
    );
    fails(d, &["feature", "show", "p", "ghost"], "not_found", 1);
    let r = ok(
        d,
        &[
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
        ],
    );
    fails(d, &["rebuild", "p"], "rebuild_failed", 1);
    ok(
        d,
        &["feature", "edit", "p", "t", "--param", "major_radius=3"],
    );
    let stale = r["revision"].as_str().unwrap();
    fails(
        d,
        &["feature", "delete", "p", "t", "--expect", stale],
        "conflict",
        1,
    );
    fails(d, &["feature", "bogus"], "usage", 2);
    fails(
        d,
        &["feature", "add", "p", "sphere", "--after", "t", "--first"],
        "usage",
        2,
    );
    let t = d.join("parts/p/features/t.json");
    fs::write(&t, "{").unwrap();
    fails(d, &["feature", "show", "p", "t"], "invalid_json", 1);
    fs::write(&t, "{}").unwrap();
    fails(d, &["feature", "show", "p", "t"], "invalid_document", 1);
    fs::write(
        d.join("ryucad.json"),
        r#"{"default_unit":"mm","format":2,"kind":"ryucad.project"}"#,
    )
    .unwrap();
    fails(d, &["part", "list"], "format_too_new", 3);
}

#[test]
fn pretty_output_is_the_same_document() {
    let (_, compact) = ryucad(&["version"]);
    let out = Command::cargo_bin("ryucad")
        .unwrap()
        .args(["version", "--pretty"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.lines().count() > 3);
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), compact);
}
