//! What the hosted review service reads from ryueda (design 667, plan
//! 8.A): every answer's schema, `version`'s probe, `diff`'s sheets,
//! framed renders, `git attributes` and `resolve`, both ways where a
//! request (in process and through the daemon).

#[path = "support/breakout.rs"]
mod breakout;
#[allow(dead_code)]
#[path = "support/draw.rs"]
mod draw;
#[path = "support/gitrepo.rs"]
mod gitrepo;
use gitrepo::*;

use serde_json::{Value, json};
use std::path::Path;

/// `ryueda` that must succeed: its result.
fn ok(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> Value {
    let (code, v) = ryueda(dir, run, daemon, args);
    assert_eq!(code, 0, "{args:?}: {v}");
    v["result"].clone()
}

#[test]
fn version_says_the_schema_of_its_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir_all(dir.path().join("p")).unwrap();
    let v = ok(&dir.path().join("p"), &run, false, &["version"]);
    assert_eq!(v["schema_outputs"], 1, "{v}");
    assert_eq!(v["schema_outputs"], json!(ryueda::OUTPUT_SCHEMA));
}

/// `check`'s report says its schema whether it passed or failed (the
/// report in `check_failed`'s details), at the working files, at a
/// revision and with `--against`.
#[test]
fn check_reports_say_their_schema() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = breakout_in_git(&top, &run);
        for args in [
            vec!["check", "--offline"],
            vec!["check", "--offline", "--rev", "HEAD"],
            vec!["check", "--offline", "--against", "HEAD"],
        ] {
            let (code, v) = ryueda(&board, &run, daemon, &args);
            let report = if code == 0 {
                &v["result"]
            } else {
                assert_eq!(v["error"]["code"], "check_failed", "{args:?}: {v}");
                &v["error"]["details"]["report"]
            };
            assert_eq!(report["schema"], 1, "{args:?}: {v}");
        }
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

/// The answers the service reads through the server say their schema:
/// diff (plain and --netlist), release list, render (plain and
/// --compare), at the working files and at a revision. (`merge status`
/// refuses outside a merge: `tests/merge.rs` asserts its schema.)
#[test]
fn server_answers_say_their_schema() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = breakout_in_git(&top, &run);
        ok(
            &board,
            &run,
            daemon,
            &["component", "edit", "breakout/r1", "--value", "10k"],
        );
        for args in [
            vec!["diff"],
            vec!["diff", "--netlist", "--from", "HEAD", "--to", "working"],
            vec!["release", "list"],
            vec!["render", "--out", "x.svg"],
            vec!["render", "--rev", "HEAD", "--out", "x.svg"],
            vec!["render", "--compare", "HEAD", "--out", "x.svg"],
        ] {
            let v = ok(&board, &run, daemon, &args);
            assert_eq!(v["schema"], 1, "{args:?}: {v}");
        }
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn scene_says_its_schema() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    for daemon in [false, true] {
        let v = ok(&board, &run, daemon, &["scene", "--block", "breakout"]);
        assert_eq!(v["schema"], 1, "{}", v["schema"]);
    }
    ryueda(&board, &run, true, &["daemon", "stop"]);
}

/// `diff --netlist` lists the sheets a change touched, counted as
/// `render --compare` marks them; an untouched block's sheets aren't
/// listed.
#[test]
fn diff_lists_the_sheets_a_change_touched() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = breakout_in_git(&top, &run);
        ok(
            &board,
            &run,
            daemon,
            &["component", "edit", "breakout/r1", "--value", "10k"],
        );
        ok(
            &board,
            &run,
            daemon,
            &["place", "breakout", "--move", "shift J1 right 4"],
        );
        let d = ok(
            &board,
            &run,
            daemon,
            &["diff", "--netlist", "--from", "HEAD"],
        );
        let sheets = d["sheets"].as_array().unwrap_or_else(|| panic!("{d}"));
        assert!(
            sheets.iter().all(|s| s["block"] != "sensor"),
            "the sensor didn't change: {d}"
        );
        let s = sheets
            .iter()
            .find(|s| s["block"] == "breakout")
            .unwrap_or_else(|| panic!("{d}"));
        assert_eq!(
            (&s["design"], &s["status"]),
            (&json!("board"), &json!("changed")),
            "{s}"
        );
        assert!(
            s["changed"].as_u64().unwrap() >= 1 && s["moved"].as_u64().unwrap() >= 1,
            "{s}"
        );
        // The same counts as the compare's strip.
        let sheet = s["sheet"].as_str().unwrap();
        let r = ok(
            &board,
            &run,
            daemon,
            &[
                "render",
                "--block",
                "breakout",
                "--compare",
                "HEAD",
                "--out",
                "c.svg",
            ],
        );
        assert_eq!(
            r["changed"][sheet],
            json!(format!(
                "Against HEAD: {} added, {} changed, {} moved, {} removed",
                s["added"], s["changed"], s["moved"], s["removed"]
            )),
            "{r} {s}"
        );
        // Without --netlist: no sheets.
        assert!(ok(&board, &run, daemon, &["diff"]).get("sheets").is_none());
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}
