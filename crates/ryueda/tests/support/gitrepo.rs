//! Git repositories for ryueda's tests: git without anyone's config,
//! ryueda in process or through a daemon, and the breakout committed.
#![allow(dead_code)]

use super::breakout;
use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// `git`, reading no user's or system's config, with a fixed identity
/// and dates.
pub fn git_command(dir: &Path) -> std::process::Command {
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
pub fn git(dir: &Path, args: &[&str]) {
    let out = git_command(dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `ryueda` in `dir`, in process or through a daemon run from `run`.
pub fn ryueda_cmd(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> Command {
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
pub fn ryueda(dir: &Path, run: &Path, daemon: bool, args: &[&str]) -> (i32, Value) {
    let out = ryueda_cmd(dir, run, daemon, args).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    (out.status.code().unwrap(), v)
}

/// The sensor breakout in `top/board`, committed.
pub fn breakout_in_git(top: &Path, run: &Path) -> PathBuf {
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

/// Runs git in `dir` with `extra` config before `args`; its success and
/// what it said on stderr.
pub fn git_try(dir: &Path, extra: &[&str], args: &[&str]) -> (bool, String) {
    let mut cmd = git_command(dir);
    for c in extra {
        cmd.args(["-c", c]);
    }
    let out = cmd.args(args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
