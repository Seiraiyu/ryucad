//! The app's command line: --version in the CLI's envelope, --help, and
//! anything else refused as usage, never taken for a folder.

mod common;

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

/// `--smoke` opens the seeded catalogue through a real daemon and draws
/// (skipped without an adapter unless RYUPLM_REQUIRE_GPU is set).
#[test]
fn smoke_draws_the_seeded_catalogue() {
    let _one = common::one_at_a_time();
    if !common::gpu() {
        return;
    }
    let root = common::catalogue("smoke", 300);
    let out = Command::new(env!("CARGO_BIN_EXE_ryuplm-app"))
        .args(["--smoke", root.to_str().unwrap()])
        .env("RYUPLM_NO_DIALOG", "1")
        .env("PATH", common::path_with_ryuplm())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["result"]["items"], 300);
    assert!(
        v["result"]["frame"]["drawn_pixels"].as_u64().unwrap() > 10_000,
        "{v}"
    );
}
