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
