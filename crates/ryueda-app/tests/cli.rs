//! The app's command line: --version in the CLI's envelope, --help, and
//! anything else refused as usage, never taken for a folder.

use std::process::Command;

fn app(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ryueda-app"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn version_is_the_cli_s_envelope() {
    let out = app(&["--version"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["ryueda"], env!("CARGO_PKG_VERSION"));
    assert!(v["result"]["commit"].is_string());
}

#[test]
fn help_and_usage() {
    let out = app(&["--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: ryueda-app [DIR]"));
    let out = app(&["--frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
    assert_eq!(app(&["a", "b"]).status.code(), Some(2));
}
