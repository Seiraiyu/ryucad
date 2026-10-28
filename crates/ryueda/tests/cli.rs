//! The CLI as an agent sees it: run the binary, parse stdout as JSON.

use assert_cmd::Command;
use serde_json::{Value, json};

fn run(args: &[&str]) -> (i32, Value) {
    let out = Command::cargo_bin("ryueda")
        .unwrap()
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let v = serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"));
    (out.status.code().unwrap(), v)
}

#[test]
fn version_names_ryueda() {
    let (code, v) = run(&["version"]);
    assert_eq!(
        (code, &v["ok"], &v["warnings"]),
        (0, &json!(true), &json!([]))
    );
    let r = &v["result"];
    for k in ["ryueda", "commit", "target", "protocol"] {
        assert!(r.get(k).is_some(), "{k}: {r}");
    }
    assert!(r.get("ryucad").is_none());
    assert_eq!(run(&["--version"]), (0, v.clone()));
    assert_eq!(run(&["-V", "--pretty"]).1, v);
}

#[test]
fn bad_arguments_are_a_usage_error_in_json() {
    let (code, v) = run(&["no-such-command"]);
    assert_eq!((code, &v["error"]["code"]), (2, &json!("usage")));
}
