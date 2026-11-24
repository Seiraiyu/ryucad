//! The app's command line: --version in the CLI's envelope, --help, and
//! anything else refused as usage, never taken for a folder.

use std::process::Command;

fn app(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ryuplm-app"))
        .args(args)
        .env("RYUPLM_NO_DIALOG", "1")
        .output()
        .unwrap()
}

#[test]
fn version_is_the_cli_s_envelope() {
    let out = app(&["--version"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["ryuplm"], env!("CARGO_PKG_VERSION"));
    assert!(v["result"]["commit"].is_string());
    assert_eq!(v["result"]["schema_outputs"], 1);
}

#[test]
fn help_and_usage() {
    let out = app(&["--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: ryuplm-app [DIR]"));
    let out = app(&["--frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
    assert_eq!(app(&["a", "b"]).status.code(), Some(2));
}

#[test]
fn smoke_outside_a_catalogue_says_so() {
    let d = tempfile::tempdir().unwrap();
    let out = app(&["--smoke", d.path().to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], false);
}
