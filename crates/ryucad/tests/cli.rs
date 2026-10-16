//! The CLI as an agent sees it: run the binary, parse stdout as JSON.

use assert_cmd::Command;
use serde_json::Value;

fn ryucad(args: &[&str]) -> (i32, Value) {
    let out = Command::cargo_bin("ryucad")
        .unwrap()
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
