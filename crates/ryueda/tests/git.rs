//! ryueda in Git, from the CLI: reads at any revision, both ways (in
//! process and through the daemon), `git install`, `init` in a
//! repository, textconv through `git diff`, and doctor.

#[path = "support/breakout.rs"]
mod breakout;
#[path = "support/draw.rs"]
mod draw;

use assert_cmd::Command;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `git`, reading no user's or system's config, with a fixed identity
/// and dates.
fn git_command(dir: &Path) -> std::process::Command {
    let mut git = std::process::Command::new("git");
    git.env(
        "GIT_CONFIG_GLOBAL",
        std::env::temp_dir().join("ryueda-no-gitconfig"),
    )
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_AUTHOR_NAME", "Test")
    .env("GIT_AUTHOR_EMAIL", "t@example.com")
    .env("GIT_COMMITTER_NAME", "Test")
    .env("GIT_COMMITTER_EMAIL", "t@example.com")
    .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
    .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
    .args([
        "-c",
        "commit.gpgsign=false",
        "-c",
        "init.defaultBranch=main",
        "-c",
        "core.autocrlf=false",
    ])
    .current_dir(dir);
    git
}

/// Runs git in `dir`; panics on failure.
fn git(dir: &Path, args: &[&str]) {
    let out = git_command(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `ryueda` in `dir`, in process or through a daemon run from `run`.
fn ryueda_cmd(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("ryueda").unwrap();
    cmd.env("RYUEDA_RUNTIME_DIR", run)
        .env("RYUEDA_IDLE_TIMEOUT", "60")
        .env("RYUEDA_CONFIG_DIR", run.with_file_name("config"))
        .env("RYUEDA_CACHE_DIR", run.with_file_name("cache"))
        .env("RYUEDA_ID_SEED", "git")
        .env("GIT_CONFIG_GLOBAL", run.with_file_name("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("RYUEDA_PARTICIPANT")
        .current_dir(dir)
        .args(args);
    if daemon {
        cmd.env_remove("RYUEDA_NO_DAEMON");
    } else {
        cmd.env("RYUEDA_NO_DAEMON", "1");
    }
    cmd
}

/// [`ryueda_cmd`] run: its exit code and its JSON.
fn ryueda(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> (i32, Value) {
    let out = ryueda_cmd(dir, run, daemon, args).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    (out.status.code().unwrap(), v)
}

/// The sensor breakout in `top/board`, committed.
fn breakout_in_git(top: &Path, run: &Path) -> PathBuf {
    git(top, &["init", "-q"]);
    let board = top.join("board");
    std::fs::create_dir_all(&board).unwrap();
    assert_eq!(ryueda(&board, run, false, &["init"]).0, 0);
    for (path, text) in breakout::footprints() {
        let at = board.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(ryueda(&board, run, false, &args).0, 0, "{args:?}");
    }
    git(top, &["add", "."]);
    git(top, &["commit", "-q", "-m", "breakout"]);
    board
}

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
        !config.contains("merge-driver"),
        "no merge driver before plan 4.C"
    );
    assert!(
        !top.join(".git/hooks/pre-merge-commit").exists(),
        "no hook line before plan 4.C"
    );
    let (code, v) = ryueda(&top.join("board"), &run, false, &["git", "install"]);
    assert_eq!((code, &v["result"]["hooks"]), (0, &json!({})), "{v}");
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
