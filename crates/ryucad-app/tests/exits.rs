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

/// Runs the app failing as `how`; asserts the trace has every line of
/// `want` (each one of its alternatives), and on Windows that `dump`
/// minidumps were written. Its output and trace.
fn check(how: &str, want: &[&[&str]], dump: usize) -> (Output, String) {
    let run = tempfile::tempdir().unwrap();
    let (out, trace) = fail(how, run.path());
    assert!(!out.status.success(), "{how}: {:?}", out.status);
    assert!(trace.contains("started"), "{how}: {trace}");
    for w in want {
        assert!(
            w.iter().any(|w| trace.contains(w)),
            "{how}: none of {w:?} in\n{trace}"
        );
    }
    if cfg!(windows) {
        assert_eq!(dumps(run.path()), dump, "{how}: {trace}");
    }
    (out, trace)
}

#[test]
fn a_panic() {
    let (_, trace) = check(
        "panic",
        &[
            &["panicked on thread 'main'"],
            &["a panic for the tests"],
            &["exited: after the main thread's panic"],
        ],
        1,
    );
    if cfg!(windows) {
        assert!(trace.contains("minidump: "), "{trace}");
    }
}

/// On Unix the abort writes the line. On Windows that abort is a
/// fail-fast nothing sees: the null took a dump of its own, and Rust says
/// so on standard error (the trace's, without a console).
#[test]
fn an_allocation_that_fails() {
    let want: &[&[&str]] = if cfg!(windows) {
        &[]
    } else {
        &[&["allocation of 4611686018427387904 bytes failed; frames 0"]]
    };
    let (out, trace) = check("alloc", want, 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("memory allocation of 4611686018427387904 bytes failed"),
        "{stderr}"
    );
    // A dump that may not matter says nothing in the trace.
    assert!(!trace.contains("minidump"), "{trace}");
}

/// A null whose caller recovers (`try_reserve`) is no failure: no line,
/// and on Windows its dump is gone again.
#[test]
fn an_allocation_that_recovers() {
    let (out, trace) = check("null", &[&["exited: process::exit(5)"]], 0);
    assert_eq!(out.status.code(), Some(5), "{trace}");
    for bad in ["allocation of", "fault:", "minidump"] {
        assert!(!trace.contains(bad), "{bad:?} in\n{trace}");
    }
}

#[test]
fn exit_called_elsewhere() {
    check("exit", &[&["exited: exit() called, not by main"]], 0);
}

#[test]
fn a_fault() {
    let want: &[&[&str]] = if cfg!(windows) {
        &[&["fault: unhandled exception 0xc0000005"], &["minidump: "]]
    } else {
        &[&["fault: signal SIGSEGV", "fault: signal SIGBUS"]]
    };
    check("fault", want, 1);
}

/// macOS raises a guard page's fault as SIGBUS, Linux as SIGSEGV.
#[test]
fn a_stack_overflow() {
    let want: &[&[&str]] = if cfg!(windows) {
        &[&["fault: stack overflow 0xc00000fd"], &["minidump: "]]
    } else {
        &[&["fault: signal SIGSEGV", "fault: signal SIGBUS"]]
    };
    check("overflow", want, 1);
}

/// `--help` ends as `main` says, not as an `exit()` from elsewhere.
#[test]
fn help() {
    let run = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ryucad-app"))
        .env("RYUCAD_RUNTIME_DIR", run.path())
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out.status);
    let trace = std::fs::read_to_string(run.path().join("ryucad-app.log")).unwrap_or_default();
    assert!(trace.contains("exited: help"), "{trace}");
    assert!(!trace.contains("not by main"), "{trace}");
}
