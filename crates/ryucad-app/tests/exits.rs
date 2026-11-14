//! #799: every way the app can end leaves a line in its trace, and on
//! Windows a fault leaves a minidump. Each case runs the binary with
//! `RYUCAD_APP_FAIL` (debug builds only), which fails just after the
//! trace starts, before any window: no display or adapter is needed.

#![cfg(debug_assertions)]

use std::path::Path;
use std::process::{Command, Output};

/// Runs the app failing as `how`, in a runtime folder of its own; its
/// output and trace.
fn fail(how: &str, run: &Path) -> (Output, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ryucad-app"))
        .env("RYUCAD_RUNTIME_DIR", run)
        .env("RYUCAD_APP_FAIL", how)
        .env("RUST_BACKTRACE", "0")
        .arg("--smoke")
        .arg(run)
        .output()
        .unwrap();
    let trace = std::fs::read_to_string(run.join("ryucad-app.log")).unwrap_or_default();
    (out, trace)
}

fn dumps(run: &Path) -> usize {
    std::fs::read_dir(run)
        .unwrap()
        .filter(|e| {
            let n = e.as_ref().unwrap().file_name();
            let n = n.to_string_lossy();
            n.starts_with("ryucad-app-") && n.ends_with(".dmp")
        })
        .count()
}

/// Asserts the trace has `want`, and on Windows that a minidump was
/// written (`dump`).
fn check(how: &str, want: &[&str], dump: bool) {
    let run = tempfile::tempdir().unwrap();
    let (out, trace) = fail(how, run.path());
    assert!(!out.status.success(), "{how}: {:?}", out.status);
    assert!(trace.contains("started"), "{how}: {trace}");
    for w in want {
        assert!(trace.contains(w), "{how}: no {w:?} in\n{trace}");
    }
    if cfg!(windows) && dump {
        assert!(trace.contains("minidump: "), "{how}: {trace}");
        assert_eq!(dumps(run.path()), 1, "{how}: {trace}");
    }
}

#[test]
fn a_panic() {
    check(
        "panic",
        &[
            "panicked on thread 'main'",
            "a panic for the tests",
            "exited: after the main thread's panic",
        ],
        true,
    );
}

#[test]
fn an_allocation_that_fails() {
    check(
        "alloc",
        &["allocation of 4611686018427387904 bytes failed; frames 0"],
        true,
    );
}

#[test]
fn exit_called_elsewhere() {
    check("exit", &["exited: exit() called, not by main"], false);
}

#[test]
fn a_fault() {
    let want: &[&str] = if cfg!(windows) {
        &["fault: unhandled exception 0xc0000005"]
    } else {
        &["fault: signal SIGSEGV"]
    };
    check("fault", want, true);
}

#[test]
fn a_stack_overflow() {
    let want: &[&str] = if cfg!(windows) {
        &["fault: stack overflow 0xc00000fd"]
    } else {
        &["fault: signal SIGSEGV"]
    };
    check("overflow", want, true);
}
