//! The release check, `ryucad-app --smoke`, on the built binary.

mod common;

use common::{Project, ryucad};
use serde_json::Value;
use std::process::Command;

#[test]
fn the_smoke_run_loads_the_model_and_draws_it() {
    if common::gpu().is_none() {
        return;
    }
    let p = Project::new().with_part();
    // The app finds `ryucad` beside itself, as installed.
    ryucad();
    let out = Command::new(env!("CARGO_BIN_EXE_ryucad-app"))
        .arg("--smoke")
        .arg(&p.root)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert!(out.status.success());
    let r = &v["result"];
    assert_eq!(
        (r["parts"].as_u64(), r["bodies"].as_u64()),
        (Some(1), Some(2))
    );
    let drawn = r["frame"]["drawn_pixels"].as_u64().unwrap();
    // The model covers some of the frame, not none and not all of it.
    assert!(drawn > 1000 && drawn < 320 * 240, "{drawn}");
}

#[test]
fn version_and_a_folder_that_isnt_a_project() {
    let out = Command::new(env!("CARGO_BIN_EXE_ryucad-app"))
        .arg("--version")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ryucad"], env!("CARGO_PKG_VERSION"));
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ryucad-app"))
        .arg("--smoke")
        .arg(dir.path())
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "project_not_found", "{v}");
    assert!(!out.status.success());
}

#[test]
fn help_and_unknown_arguments_open_no_window() {
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ryucad-app"))
            .args(args)
            .output()
            .unwrap()
    };
    let out = run(&["--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: ryucad-app [DIR]"));
    for args in [&["--frobnicate"][..], &["a", "b"]] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("Usage:"));
    }
}
