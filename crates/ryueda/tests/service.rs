//! What the hosted review service reads from ryueda (design 667, plan
//! 8.A): every answer's schema, `version`'s probe, `diff`'s sheets,
//! framed renders, `git attributes` and `resolve`, both ways where a
//! request (in process and through the daemon).

#[path = "support/breakout.rs"]
mod breakout;
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

/// The four numbers of an SVG's viewBox.
fn view_box(svg: &str) -> [f64; 4] {
    let at = svg.find("viewBox=\"").unwrap() + 9;
    let end = at + svg[at..].find('"').unwrap();
    let n: Vec<f64> = svg[at..end]
        .split(' ')
        .map(|x| x.parse().unwrap())
        .collect();
    [n[0], n[1], n[2], n[3]]
}

/// A fitted before and after framed by each other crop alike, to a box
/// holding both unframed ones; framed by itself alone, a render is as
/// unframed.
#[test]
fn framed_renders_line_up() {
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
            &["place", "breakout", "--move", "shift J1 right 8"],
        );
        let svg = |args: &[&str]| {
            let mut all = vec![
                "render", "--block", "breakout", "--sheet", "s1", "--fit", "--out", "x.svg",
            ];
            all.extend_from_slice(args);
            ok(&board, &run, daemon, &all);
            std::fs::read_to_string(board.join("x.svg")).unwrap()
        };
        let (head, base) = (svg(&[]), svg(&["--rev", "HEAD"]));
        let head_f = svg(&["--frame", "HEAD"]);
        let base_f = svg(&["--rev", "HEAD", "--frame", "working"]);
        let (h, b, hf, bf) = (
            view_box(&head),
            view_box(&base),
            view_box(&head_f),
            view_box(&base_f),
        );
        assert_ne!(h, b, "the move changes what is drawn");
        assert_eq!(hf, bf, "framed together, they crop alike");
        let right = |v: [f64; 4]| v[0] + v[2];
        let bottom = |v: [f64; 4]| v[1] + v[3];
        assert!(
            hf[0] <= h[0].min(b[0]) && hf[1] <= h[1].min(b[1]),
            "{hf:?} {h:?} {b:?}"
        );
        assert!(right(hf) >= right(h).max(right(b)) && bottom(hf) >= bottom(h).max(bottom(b)));
        assert_eq!(svg(&["--frame", "working"]), head, "framed by itself alone");
        // --compare framed by the base: the same view as the plain framed head.
        let cmp = svg(&["--compare", "HEAD", "--frame", "HEAD"]);
        assert_eq!(view_box(&cmp), hf);
        // --frame without --fit is a usage error.
        let (code, v) = ryueda(
            &board,
            &run,
            daemon,
            &[
                "render", "--sheet", "s1", "--frame", "HEAD", "--out", "x.svg",
            ],
        );
        assert_eq!((code, &v["error"]["code"]), (2, &json!("usage")), "{v}");
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

/// `git attributes` answers the lines ryueda's driver needs: every line
/// `init` writes into a project's .gitattributes, and nothing else.
#[test]
fn git_attributes_are_the_lines_init_writes() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(top.join("board")).unwrap();
    git(&top, &["init", "-q"]);
    ok(&top.join("board"), &run, false, &["init"]);
    // Outside any project, as the service asks it.
    let v = ok(dir.path(), &run, false, &["git", "attributes"]);
    assert_eq!(v["schema"], 1);
    let lines: Vec<&str> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines, ryueda::drivers::LINES.to_vec());
    let written = std::fs::read_to_string(top.join("board/.gitattributes")).unwrap();
    for l in &lines {
        assert!(written.lines().any(|w| w == *l), "{l} in {written}");
    }
}

/// `resolve` at the working files and at a revision: what a delete took
/// away is gone at working and stands at HEAD; a block the design no
/// longer places, a design that's gone; a malformed anchor refuses.
#[test]
fn resolve_says_what_stands() {
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
            &["component", "delete", "breakout/c3"],
        );
        let anchors = [
            "board:breakout/r1",
            "board:breakout/c3",
            "board:sensor/u1",
            "board:breakout/sheet:s1",
            "board:gone/x",
            "nodesign:breakout/r1",
        ];
        let mut args = vec!["resolve"];
        args.extend(anchors);
        let v = ok(&board, &run, daemon, &args);
        assert_eq!(v["schema"], 1);
        let r = v["refs"].as_array().unwrap();
        assert_eq!(r.len(), anchors.len());
        assert_eq!(
            (&r[0]["stands"], &r[0]["sheet"]),
            (&json!(true), &json!("s1")),
            "{v}"
        );
        assert_eq!(r[1]["gone"], true, "{v}");
        assert_eq!(r[2]["stands"], true, "{v}");
        assert_eq!(
            (&r[3]["stands"], &r[3]["kind"]),
            (&json!(true), &json!("sheet")),
            "{v}"
        );
        assert_eq!(r[4]["removed_in"], "block", "{v}");
        assert_eq!(r[5]["removed_in"], "design", "{v}");
        // The box is scene's box for the same owner.
        let scene = ok(
            &board,
            &run,
            daemon,
            &["scene", "--block", "breakout", "--sheet", "s1"],
        );
        let owner = scene["owners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["ref"] == "breakout/r1")
            .unwrap();
        assert_eq!(r[0]["box"], owner["box"]);
        // At HEAD the deleted component stands.
        let at = ok(
            &board,
            &run,
            daemon,
            &["resolve", "board:breakout/c3", "--rev", "HEAD"],
        );
        assert_eq!(
            (
                &at["refs"][0]["stands"],
                at["commit"].as_str().map(str::len)
            ),
            (&json!(true), Some(40)),
            "{at}"
        );
        // A path with no design: refused.
        let (code, e) = ryueda(&board, &run, daemon, &["resolve", "breakout/r1"]);
        assert_eq!(
            (code, &e["error"]["code"], &e["error"]["details"]["param"]),
            (1, &json!("invalid_param"), &json!("ref")),
            "{e}"
        );
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}
