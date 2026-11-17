//! ryueda in Git, from the CLI: reads at any revision, both ways (in
//! process and through the daemon), `git install`, `init` in a
//! repository, textconv through `git diff`, and doctor.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;
#[path = "support/gitrepo.rs"]
mod gitrepo;
use gitrepo::*;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[test]
fn reads_at_any_revision_both_ways() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = breakout_in_git(&top, &run);
        let (_, before) = ryueda(&board, &run, daemon, &["netlist"]);
        // A design file broken in the work tree: HEAD still reads.
        let design = std::fs::read_dir(board.join("designs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&design, "{").unwrap();
        let (code, at) = ryueda(&board, &run, daemon, &["netlist", "--rev", "HEAD"]);
        assert_eq!(code, 0, "{at}");
        assert_eq!(at["result"]["commit"].as_str().unwrap().len(), 40);
        let mut without = at["result"].clone();
        without.as_object_mut().unwrap().remove("commit");
        assert_eq!(without, before["result"]);
        for args in [
            vec!["erc", "--rev", "HEAD"],
            vec!["check", "--rev", "HEAD"],
            vec!["describe", "--rev", "HEAD"],
            vec!["lib", "check", "--rev", "HEAD"],
            vec!["block", "show", "breakout", "--rev", "HEAD"],
        ] {
            let (code, v) = ryueda(&board, &run, daemon, &args);
            assert!(
                code == 0 || v["error"]["code"] == "check_failed",
                "{args:?}: {v}"
            );
            if code == 0 {
                assert_eq!(v["result"]["commit"], at["result"]["commit"], "{args:?}");
            }
        }
        let (code, v) = ryueda(
            &board,
            &run,
            daemon,
            &["render", "--rev", "HEAD", "--out", "x.svg"],
        );
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["commit"], at["result"]["commit"]);
        assert!(board.join("x.svg").is_file());
        assert_eq!(
            ryueda(&board, &run, daemon, &["netlist", "--rev", "nope"]).1["error"]["code"],
            "rev_not_found"
        );
        assert_eq!(
            ryueda(&board, &run, daemon, &["netlist", "--rev", "working"]).0,
            1,
            "working is the broken files"
        );
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn outside_a_repository_a_revision_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let board = dir.path().join("board");
    std::fs::create_dir_all(&board).unwrap();
    ryueda(&board, &run, false, &["init"]);
    let (code, v) = ryueda(&board, &run, false, &["netlist", "--rev", "HEAD"]);
    assert_eq!((code, &v["error"]["code"]), (1, &json!("not_a_git_repo")));
}

#[test]
fn init_in_a_repository_sets_the_clone_up_and_install_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(top.join("board")).unwrap();
    git(&top, &["init", "-q"]);
    let (code, v) = ryueda(&top.join("board"), &run, false, &["init"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["git"]["scope"], "local");
    let attrs = std::fs::read_to_string(top.join("board/.gitattributes")).unwrap();
    assert!(
        attrs.contains("designs/*.json merge=ryueda diff=ryueda text eol=lf"),
        "{attrs}"
    );
    let config = std::fs::read_to_string(top.join(".git/config")).unwrap();
    assert!(
        config.contains("' git textconv") && config.contains("renames = false"),
        "{config}"
    );
    assert!(
        config.contains("' git merge-driver %O %A %B %P") && config.contains("[merge \"ryueda\"]"),
        "{config}"
    );
    let hook = std::fs::read_to_string(top.join(".git/hooks/pre-merge-commit")).unwrap();
    assert!(
        hook.contains("# ryueda") && hook.contains("' merge status --hook"),
        "{hook}"
    );
    // Installed again: nothing to write.
    let (code, v) = ryueda(&top.join("board"), &run, false, &["git", "install"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        std::fs::read_to_string(top.join(".git/hooks/pre-merge-commit")).unwrap(),
        hook
    );
    // Outside a merge the hook's check passes at once.
    let out = ryueda_cmd(
        &top.join("board"),
        &run,
        false,
        &["merge", "status", "--hook"],
    )
    .output()
    .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (code, v) = ryueda(
        &top.join("board"),
        &run,
        false,
        &["git", "install", "--global"],
    );
    assert_eq!((code, &v["result"]["scope"]), (0, &json!("global")), "{v}");
    let global = std::fs::read_to_string(run.with_file_name("gitconfig")).unwrap();
    assert!(
        global.contains("textconv") && !global.contains("renames"),
        "{global}"
    );
    assert!(global.contains("git merge-driver"), "{global}");
}

#[test]
fn git_diff_reads_values() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    // The installed textconv runs this test's ryueda.
    let exe = assert_cmd::cargo::cargo_bin("ryueda");
    let exe = exe.to_string_lossy().replace('\\', "/");
    git(
        &top,
        &[
            "config",
            "diff.ryueda.textconv",
            &format!("'{exe}' git textconv"),
        ],
    );
    let comp = std::fs::read_dir(board.join("blocks"))
        .unwrap()
        .flat_map(|b| {
            std::fs::read_dir(b.unwrap().path().join("components"))
                .into_iter()
                .flatten()
        })
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(&comp).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let old = v["value"].as_str().unwrap_or_default().to_owned();
    let mut edited = v.clone();
    edited["value"] = json!("47k");
    std::fs::write(&comp, serde_json::to_string_pretty(&edited).unwrap() + "\n").unwrap();
    let out = git_command(&top)
        .args(["diff", "--no-color", "--", comp.to_str().unwrap()])
        .output()
        .unwrap();
    let diff = String::from_utf8(out.stdout).unwrap();
    assert!(diff.contains("+value = 47k"), "{diff}");
    if !old.is_empty() {
        assert!(diff.contains(&format!("-value = {old}")), "{diff}");
    }
    assert!(!diff.contains('{'), "values, not JSON: {diff}");
    // Printed as Git reads it: plain text, never the JSON envelope.
    let printed = std::process::Command::new(assert_cmd::cargo::cargo_bin("ryueda"))
        .args(["git", "textconv", comp.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(printed.status.success());
    assert!(
        String::from_utf8(printed.stdout)
            .unwrap()
            .starts_with("ryueda.component format ")
    );
}

#[test]
fn doctor_repairs_are_logged_and_undone() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    // A model the library holds and no footprint names, and lost lines.
    std::fs::create_dir_all(board.join("library/models")).unwrap();
    std::fs::write(board.join("library/models/old.step"), "ISO-10303-21;\n").unwrap();
    std::fs::write(board.join(".gitattributes"), "*.png binary\n").unwrap();
    for daemon in [false, true] {
        let (code, v) = ryueda(&board, &run, daemon, &["doctor"]);
        assert_eq!(code, 0, "{v}");
        let codes: Vec<&str> = v["result"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["code"].as_str().unwrap())
            .collect();
        assert!(
            codes.contains(&"gitattributes_missing") && codes.contains(&"model_orphaned"),
            "{v}"
        );
    }
    let (_, v) = ryueda(&board, &run, true, &["doctor"]);
    let ids: Vec<String> = v["result"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "gitattributes_missing" || i["code"] == "model_orphaned")
        .map(|i| i["repairs"][0]["id"].as_str().unwrap().to_owned())
        .collect();
    let mut args = vec!["doctor"];
    for id in &ids {
        args.extend(["--repair", id.as_str()]);
    }
    let (code, v) = ryueda(&board, &run, true, &args);
    assert_eq!(code, 0, "{v}");
    let left: Vec<&Value> = v["result"]["report"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "gitattributes_missing" || i["code"] == "model_orphaned")
        .collect();
    assert!(left.is_empty(), "{v}");
    assert!(!board.join("library/models/old.step").exists());
    // One logged change, undone whole.
    let (code, u) = ryueda(&board, &run, true, &["undo"]);
    assert_eq!(code, 0, "{u}");
    assert!(board.join("library/models/old.step").exists());
    assert_eq!(
        std::fs::read_to_string(board.join(".gitattributes")).unwrap(),
        "*.png binary\n"
    );
    // A stale repair is refused.
    let (code, v) = ryueda(
        &board,
        &run,
        true,
        &["doctor", "--repair", "model_orphaned:remove:x@00000000"],
    );
    assert_eq!((code, &v["error"]["code"]), (1, &json!("repair_conflict")));
    ryueda(&board, &run, true, &["daemon", "stop"]);
}

#[test]
fn check_reports_doctor_and_a_revision_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    std::fs::create_dir_all(board.join("library/models")).unwrap();
    std::fs::write(board.join("library/models/old.step"), "ISO-10303-21;\n").unwrap();
    let (_, v) = ryueda(&board, &run, false, &["check"]);
    let report = if v["ok"] == true {
        &v["result"]
    } else {
        &v["error"]["details"]["report"]
    };
    let doctor: Vec<&str> = report["doctor"]
        .as_array()
        .unwrap_or_else(|| panic!("{v}"))
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(doctor.contains(&"model_orphaned"), "{report}");
    assert!(
        !doctor.contains(&"unknown_file"),
        "the loader's faults report those"
    );
    // At a revision there is no folder to doctor.
    let (_, v) = ryueda(&board, &run, false, &["check", "--rev", "HEAD"]);
    let report = if v["ok"] == true {
        &v["result"]
    } else {
        &v["error"]["details"]["report"]
    };
    assert_eq!(report["doctor"], json!([]), "{report}");
}

/// The breakout, laid out and committed, then edited: R1's value, a new
/// R3 on SDA, C3 deleted, SCL renamed and J1 moved.
fn edited_breakout(top: &Path, run: &Path) -> PathBuf {
    let board = breakout_in_git(top, run);
    assert_eq!(ryueda(&board, run, false, &["layout"]).0, 0);
    git(top, &["add", "."]);
    git(top, &["commit", "-q", "-m", "laid out"]);
    for args in [
        vec!["component", "edit", "breakout/r1", "--value", "10k"],
        vec![
            "component",
            "add",
            "breakout",
            "--of",
            "r_4k7",
            "--id",
            "r3",
        ],
        vec!["connect", "breakout/r3/1", "SDA"],
        vec!["connect", "breakout/r3/2", "rail:3V3"],
        vec!["component", "delete", "breakout/c3"],
        vec!["net", "rename", "breakout/scl", "I2C_SCL"],
        vec!["place", "breakout", "--move", "shift J1 right 4"],
    ] {
        let (code, v) = ryueda(&board, run, false, &args);
        assert_eq!(code, 0, "{args:?}: {v}");
    }
    board
}

#[test]
fn diff_says_what_changed_both_ways() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = edited_breakout(&top, &run);
        let (code, v) = ryueda(&board, &run, daemon, &["diff"]);
        assert_eq!(code, 0, "{v}");
        let r = &v["result"];
        assert_eq!(r["from"]["rev"], "HEAD");
        assert_eq!(r["from"]["commit"].as_str().unwrap().len(), 40);
        assert_eq!(r["to"], json!({"rev": "working", "commit": null}));
        let b = r["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["block"] == "breakout")
            .unwrap();
        let comp = |id: &str| {
            b["components"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["id"] == id)
                .cloned()
                .unwrap()
        };
        assert_eq!(comp("r1")["status"], "changed");
        assert_eq!(
            (comp("r3")["status"].clone(), comp("r3")["ref"].clone()),
            (json!("added"), json!("breakout/r3"))
        );
        assert_eq!(comp("c3")["status"], "removed");
        let scl = b["nets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == "scl")
            .unwrap();
        assert_eq!(scl["name"], "I2C_SCL");
        assert_eq!(b["layout"]["status"], "changed");
        assert!(
            b["layout"]["counts"]["moved"].as_u64().unwrap() >= 1,
            "{}",
            b["layout"]
        );
        assert!(
            b["layout"].get("changes").is_none(),
            "counts only without --layout"
        );
        let (_, l) = ryueda(
            &board,
            &run,
            daemon,
            &["diff", "--layout", "--block", "breakout"],
        );
        let changes = l["result"]["blocks"][0]["layout"]["changes"]
            .as_array()
            .unwrap();
        assert!(
            changes
                .iter()
                .any(|c| c["change"] == "moved" && c["ref"] == "breakout/j1"),
            "{changes:?}"
        );
        let (_, n) = ryueda(&board, &run, daemon, &["diff", "--netlist"]);
        let nets = n["result"]["netlist"][0]["nets"].as_array().unwrap();
        assert!(
            nets.iter().any(|x| x["renamed"]["to"]
                .as_str()
                .is_some_and(|t| t.ends_with("I2C_SCL"))),
            "{nets:?}"
        );
        // Nothing between a revision and itself; a revision it lacks.
        let (_, same) = ryueda(
            &board,
            &run,
            daemon,
            &["diff", "--from", "HEAD", "--to", "HEAD"],
        );
        assert_eq!(same["result"]["blocks"], json!([]));
        assert_eq!(
            ryueda(&board, &run, daemon, &["diff", "--from", "nope"]).1["error"]["code"],
            "rev_not_found"
        );
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn check_against_a_revision_says_what_changed_electrically() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = edited_breakout(&top, &run);
        let (code, v) = ryueda(&board, &run, daemon, &["check", "--against", "HEAD"]);
        let report = if code == 0 {
            v["result"].clone()
        } else {
            v["error"]["details"]["report"].clone()
        };
        assert!(code == 0 || v["error"]["code"] == "check_failed", "{v}");
        let a = &report["against"];
        assert_eq!(a["from"]["rev"], "HEAD");
        assert!(
            !a["netlist"][0]["occurrences"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{a}"
        );
        assert_eq!(
            ryueda(
                &board,
                &run,
                daemon,
                &["check", "--rev", "HEAD", "--against", "HEAD~1"]
            )
            .1["error"]["code"],
            "invalid_param"
        );
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

#[test]
fn an_unfinished_merge_fails_check() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    // A conflict record, as 4.C's driver will leave one.
    let r1 = board.join("blocks/breakout/components/r1.json");
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(&r1).unwrap()).unwrap();
    c["conflicts"] =
        json!({"/value": {"kind": "value", "base": "4k7", "ours": "10k", "theirs": "2k2"}});
    std::fs::write(&r1, serde_json::to_string_pretty(&c).unwrap() + "\n").unwrap();
    let (code, v) = ryueda(&board, &run, false, &["check"]);
    assert_eq!(
        (code, &v["error"]["code"]),
        (1, &json!("check_failed")),
        "{v}"
    );
    let report = &v["error"]["details"]["report"];
    assert!(
        report["faults"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "merge_conflict"),
        "{report}"
    );
    let (_, d) = ryueda(&board, &run, false, &["doctor"]);
    assert!(
        d["result"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == "merge_conflict"),
        "{d}"
    );
    git(&top, &["checkout", "--", "."]);
    // A path Git left unmerged (a text merge, no driver yet).
    git(&top, &["checkout", "-q", "-b", "theirs"]);
    let (_, _) = ryueda(
        &board,
        &run,
        false,
        &["component", "edit", "breakout/r1", "--value", "2k2"],
    );
    git(&top, &["commit", "-qam", "theirs"]);
    git(&top, &["checkout", "-q", "main"]);
    let (_, _) = ryueda(
        &board,
        &run,
        false,
        &["component", "edit", "breakout/r1", "--value", "10k"],
    );
    git(&top, &["commit", "-qam", "ours"]);
    let out = git_command(&top)
        .args(["merge", "-q", "theirs"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge stops");
    let (code, v) = ryueda(&board, &run, false, &["check"]);
    assert_eq!(code, 1, "{v}");
    let doctor = v["error"]["details"]["report"]["doctor"]
        .as_array()
        .unwrap();
    assert!(doctor.iter().any(|f| f["code"] == "unmerged_path"), "{v}");
}

#[test]
fn render_compare_paints_what_changed() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = edited_breakout(&top, &run);
        let (code, v) = ryueda(
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
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["result"]["compare"]["rev"], "HEAD");
        assert_eq!(v["result"]["compare"]["commit"].as_str().unwrap().len(), 40);
        let svg = std::fs::read_to_string(board.join("c.svg")).unwrap();
        for want in [
            "data-ref=\"breakout/r3\"",
            "data-ref=\"breakout/c3\"",
            "#1a7f37",
            "#9a6700",
            "#cf222e",
            "stroke-dasharray",
            "Against HEAD:",
        ] {
            assert!(svg.contains(want), "{want}");
        }
        let (code, _) = ryueda(
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
                "c.png",
            ],
        );
        assert_eq!(code, 0);
        assert!(
            std::fs::read(board.join("c.png"))
                .unwrap()
                .starts_with(b"\x89PNG")
        );
        // A plain render has no references.
        ryueda(
            &board,
            &run,
            daemon,
            &["render", "--block", "breakout", "--out", "p.svg"],
        );
        assert!(
            !std::fs::read_to_string(board.join("p.svg"))
                .unwrap()
                .contains("data-ref")
        );
        assert_eq!(
            ryueda(
                &board,
                &run,
                daemon,
                &[
                    "render",
                    "--rev",
                    "HEAD",
                    "--compare",
                    "HEAD",
                    "--out",
                    "x.svg"
                ]
            )
            .1["error"]["code"],
            "invalid_param"
        );
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

/// `scene --rev` (plan 13.A): the sheet as the revision draws it, read
/// only, with the commit; the working sheet hasn't what was deleted.
#[test]
fn a_scene_reads_a_revision_both_ways() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = edited_breakout(&top, &run);
        let refs = |v: &Value| -> Vec<String> {
            v["result"]["owners"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["ref"].as_str().unwrap().to_owned())
                .collect()
        };
        let (code, then) = ryueda(
            &board,
            &run,
            daemon,
            &["scene", "--block", "breakout", "--rev", "HEAD"],
        );
        assert_eq!(code, 0, "{then}");
        assert_eq!(then["result"]["commit"].as_str().unwrap().len(), 40);
        let (code, now) = ryueda(&board, &run, daemon, &["scene", "--block", "breakout"]);
        assert_eq!(code, 0, "{now}");
        assert!(now["result"].get("commit").is_none());
        assert!(
            refs(&then).contains(&"breakout/c3".to_owned()),
            "deleted since HEAD"
        );
        assert!(!refs(&now).contains(&"breakout/c3".to_owned()));
        assert!(
            refs(&now).contains(&"breakout/r3".to_owned()),
            "added since HEAD"
        );
        assert!(!refs(&then).contains(&"breakout/r3".to_owned()));
        // `--rev working` is the working sheet.
        let (_, w) = ryueda(
            &board,
            &run,
            daemon,
            &["scene", "--block", "breakout", "--rev", "working"],
        );
        assert_eq!(w["result"]["owners"], now["result"]["owners"]);
        let (_, bad) = ryueda(&board, &run, daemon, &["scene", "--rev", "nope"]);
        assert_eq!(bad["error"]["code"], "rev_not_found", "{bad}");
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

/// `scene --compare` (plan 13.A) over the edited breakout: r1 changed,
/// r3 added, c3 ghosted, J1 moved with a leader, the counts, both ways.
#[test]
fn a_compare_scene_marks_ghosts_and_counts_both_ways() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = edited_breakout(&top, &run);
        let (code, v) = ryueda(
            &board,
            &run,
            daemon,
            &["scene", "--block", "breakout", "--compare", "HEAD"],
        );
        assert_eq!(code, 0, "{v}");
        let r = &v["result"];
        let mark = |reference: &str| {
            r["owners"]
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["ref"] == reference)
                .and_then(|o| o["mark"].as_str().map(str::to_owned))
        };
        assert_eq!(mark("breakout/r1").as_deref(), Some("changed"));
        assert_eq!(mark("breakout/r3").as_deref(), Some("added"));
        assert_eq!(mark("breakout/j1").as_deref(), Some("moved"));
        let ghosts: Vec<&str> = r["ghosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["ref"].as_str().unwrap())
            .collect();
        assert!(ghosts.contains(&"breakout/c3"), "{ghosts:?}");
        assert!(
            r["leaders"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["ref"] == "breakout/j1")
        );
        assert!(r["counts"]["removed"].as_u64().unwrap() >= 1);
        assert!(r["sheets"][0]["counts"].is_object());
        assert_eq!(r["against"]["rev"], "HEAD");
        for bad in [
            vec!["scene", "--compare", "HEAD", "--rev", "HEAD"],
            vec!["scene", "--compare", "working"],
        ] {
            let (_, e) = ryueda(&board, &run, daemon, &bad);
            assert_eq!(e["error"]["code"], "invalid_param", "{bad:?}: {e}");
        }
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
}

/// `history.log` (plan 13.A): the project's commits newest first and what
/// is uncommitted, both ways; outside a repository, `not_a_git_repo`.
#[test]
fn history_lists_commits_and_what_is_uncommitted_both_ways() {
    for daemon in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        let board = edited_breakout(&top, &run);
        let (code, v) = ryueda(&board, &run, daemon, &["git", "log"]);
        assert_eq!(code, 0, "{v}");
        let commits = v["result"]["commits"].as_array().unwrap();
        let subjects: Vec<&str> = commits
            .iter()
            .map(|c| c["subject"].as_str().unwrap())
            .collect();
        assert_eq!(subjects[0], "laid out", "newest first: {subjects:?}");
        for c in commits {
            assert_eq!(c["id"].as_str().unwrap().len(), 40);
            assert_eq!(c["author"], "Test");
            assert!(c["time"].as_i64().is_some());
        }
        let uncommitted = v["result"]["uncommitted"].as_array().unwrap();
        assert!(
            uncommitted.iter().any(|f| f["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("blocks/breakout/components/r3.json"))
                && f["change"] == "created"),
            "{uncommitted:?}"
        );
        let (_, one) = ryueda(&board, &run, daemon, &["git", "log", "--limit", "1"]);
        assert_eq!(one["result"]["commits"].as_array().unwrap().len(), 1);
        if daemon {
            ryueda(&board, &run, true, &["daemon", "stop"]);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let alone = dir.path().join("alone");
    std::fs::create_dir_all(&alone).unwrap();
    assert_eq!(ryueda(&alone, &run, false, &["init"]).0, 0);
    let (code, v) = ryueda(&alone, &run, false, &["git", "log"]);
    assert_ne!(code, 0);
    assert_eq!(v["error"]["code"], "not_a_git_repo", "{v}");
}

#[test]
fn output_compare_pages_only_changed_sheets() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = edited_breakout(&top, &run);
    let (_, all) = ryueda(&board, &run, false, &["output", "--out", "all.pdf"]);
    let (code, v) = ryueda(
        &board,
        &run,
        false,
        &["output", "--compare", "HEAD", "--out", "c.pdf"],
    );
    assert_eq!(code, 0, "{v}");
    let pages = v["result"]["pages"].as_array().unwrap();
    assert!(!pages.is_empty());
    assert!(
        pages.len() < all["result"]["pages"].as_array().unwrap().len(),
        "the sensor's sheet is the same"
    );
    assert!(pages.iter().all(|p| p["block"] == "breakout"), "{pages:?}");
    assert!(
        std::fs::read(board.join("c.pdf"))
            .unwrap()
            .starts_with(b"%PDF")
    );
    // Nothing changed since the working files' own commit: no file.
    git(&top, &["add", "."]);
    git(&top, &["commit", "-q", "-m", "edits"]);
    let (code, v) = ryueda(
        &board,
        &run,
        false,
        &["output", "--compare", "HEAD", "--out", "none.pdf"],
    );
    assert_eq!((code, &v["result"]["unchanged"]), (0, &json!(true)), "{v}");
    assert!(!board.join("none.pdf").exists());
}

/// Three levels, breakout over sensor over a leaf, where only the leaf
/// changed: its sheets are the only pages, and the outline still reaches
/// them through the unchanged blocks above (which open the leaf's first
/// page).
#[test]
fn output_compare_bookmarks_a_leaf_under_unchanged_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    for args in [
        vec!["block", "create", "Decoupling", "--id", "leaf"],
        vec!["component", "add", "leaf", "--of", "c_100n", "--id", "c9"],
        vec!["connect", "leaf/c9/1", "rail:3V3"],
        vec!["connect", "leaf/c9/2", "rail:GND"],
        vec!["instance", "add", "sensor", "--of", "leaf", "--id", "d1"],
        vec!["layout"],
    ] {
        let (code, v) = ryueda(&board, &run, false, &args);
        assert_eq!(code, 0, "{args:?}: {v}");
    }
    git(&top, &["add", "."]);
    git(&top, &["commit", "-q", "-m", "three levels"]);
    let (code, v) = ryueda(
        &board,
        &run,
        false,
        &["component", "edit", "leaf/c9", "--value", "1u"],
    );
    assert_eq!(code, 0, "{v}");
    let (code, v) = ryueda(
        &board,
        &run,
        false,
        &["output", "--compare", "HEAD", "--out", "c.pdf"],
    );
    assert_eq!(code, 0, "{v}");
    let pages = v["result"]["pages"].as_array().unwrap();
    assert!(!pages.is_empty());
    assert!(pages.iter().all(|p| p["block"] == "leaf"), "{pages:?}");
    let pdf = String::from_utf8_lossy(&std::fs::read(board.join("c.pdf")).unwrap()).into_owned();
    for want in [
        "/Title (Breakout)",
        "/Title (Sensor (s1))",
        "/Title (Decoupling (d1))",
    ] {
        assert!(pdf.contains(want), "{want} in the outline");
    }
    for p in pages {
        let want = format!("/Title ({})", p["name"].as_str().unwrap());
        assert!(pdf.contains(&want), "{want} in the outline");
    }
}

#[test]
fn a_compare_is_set_on_a_participant() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = edited_breakout(&top, &run);
    // No daemon, nobody to show.
    assert_eq!(
        ryueda(&board, &run, false, &["compare", "HEAD"]).1["error"]["code"],
        "no_live_session"
    );
    let (code, v) = ryueda(&board, &run, true, &["compare", "HEAD"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"]["compare"]["rev"], "HEAD");
    let who = v["result"]["participant"].as_str().unwrap().to_owned();
    let (_, got) = ryueda(&board, &run, true, &["compare"]);
    assert_eq!(got["result"]["participant"], json!(who));
    assert_eq!(
        got["result"]["compare"]["commit"].as_str().unwrap().len(),
        40
    );
    let (_, list) = ryueda(&board, &run, true, &["participants"]);
    let me = list["result"]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(who))
        .cloned()
        .unwrap();
    assert_eq!(me["compare"], "HEAD");
    for (args, code) in [
        (vec!["compare", "working"], "invalid_param"),
        (vec!["compare", "nope"], "rev_not_found"),
        (vec!["compare", "HEAD", "--off"], "usage"),
    ] {
        assert_eq!(
            ryueda(&board, &run, true, &args).1["error"]["code"],
            code,
            "{args:?}"
        );
    }
    let (_, off) = ryueda(&board, &run, true, &["compare", "--off"]);
    assert_eq!(off["result"]["compare"], Value::Null);
    ryueda(&board, &run, true, &["daemon", "stop"]);
}

#[test]
fn the_merge_driver_writes_over_ours_and_exits_by_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let comp = |refdes: &str, value: &str| {
        format!(
            "{{\n  \"format\": 1,\n  \"kind\": \"ryueda.component\",\n  \"of\": {{\n    \"id\": \"r\"\n  }},\n  \"refdes\": \"{refdes}\",\n  \"value\": \"{value}\"\n}}\n"
        )
    };
    let write = |name: &str, text: &str| {
        let p = dir.path().join(name);
        std::fs::write(&p, text).unwrap();
        p
    };
    let (b, o, t) = (
        write("b", &comp("R1", "1k")),
        write("o", &comp("R2", "1k")),
        write("t", &comp("R1", "2k")),
    );
    let out = ryueda_cmd(dir.path(), &run, false, &["git", "merge-driver"])
        .args([&b, &o, &t])
        .arg("blocks/x/components/r1.json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "prints nothing");
    assert_eq!(std::fs::read_to_string(&o).unwrap(), comp("R2", "2k"));
    // Both sides changed one value: exit 1, the record in ours' file.
    let o = write("o", &comp("R1", "3k"));
    let out = ryueda_cmd(dir.path(), &run, false, &["git", "merge-driver"])
        .args([&b, &o, &t])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&std::fs::read(&o).unwrap()).unwrap();
    assert_eq!(v["conflicts"]["/value"]["theirs"], "2k");
}

#[test]
#[ignore = "needs board place (task 11)"]
fn board_files_merge_by_value_and_two_moves_of_one_part_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    let top = dir.path().join("repo");
    std::fs::create_dir_all(&top).unwrap();
    let board = breakout_in_git(&top, &run);
    let ok = |args: &[&str]| {
        let (c, v) = ryueda(&board, &run, false, args);
        assert_eq!(c, 0, "{args:?}: {v}");
    };
    ok(&[
        "board", "create", "Main", "--design", "board", "--rect", "30,20", "--id", "main",
    ]);
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-qm", "board"]);
    git(&top, &["branch", "theirs"]);
    // Ours places U1 and moves J1; theirs places C1 and moves J1 elsewhere.
    ok(&["board", "place", "main", "U1", "--at", "10,10"]);
    ok(&["board", "place", "main", "J1", "--at", "2,10"]);
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-qm", "ours"]);
    git(&top, &["checkout", "-q", "theirs"]);
    ok(&["board", "place", "main", "C1", "--at", "12,10"]);
    ok(&["board", "place", "main", "J1", "--at", "3,10"]);
    git(&top, &["add", "-A"]);
    git(&top, &["commit", "-qm", "theirs"]);
    git(&top, &["checkout", "-q", "main"]);
    let (merged, _) = git_try(&top, &[], &["merge", "-q", "theirs"]);
    assert!(!merged, "J1 moved both ways conflicts");
    // U1 and C1 merged with no conflict; J1's file holds the record.
    let placements = board.join("boards/main/placements");
    assert!(placements.join("s1.u1.json").is_file());
    assert!(placements.join("s1.c1.json").is_file());
    let j1: Value =
        serde_json::from_str(&std::fs::read_to_string(placements.join("j1.json")).unwrap())
            .unwrap();
    assert!(j1["conflicts"]["/at"].is_object(), "{j1}");
}
