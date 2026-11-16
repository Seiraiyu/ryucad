//! What the window tests share: one folder per test process (runtime,
//! config and cache inside it), the `ryueda` binary beside the tests
//! (built if it is missing or older than its sources), the sensor
//! breakout entered by its script, an adapter or a skip, and the agent's
//! requests through the daemon.

#![allow(dead_code)]

#[path = "../../../ryueda/tests/support/breakout.rs"]
pub mod breakout;
#[path = "../../../ryueda/tests/support/draw.rs"]
pub mod draw;

use ryueda::proto::Request;
use ryueda_app::core::link::Daemon;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::SystemTime;

/// The tests' own folder; sockets in a short one beside it (107 bytes).
pub fn shared() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_string_lossy().into_owned();
        let dir =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{stem}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = std::env::temp_dir().join(format!("ryueda-app-{}", std::process::id()));
        std::fs::create_dir_all(&run).unwrap();
        // SAFETY: before any thread of the tests' own reads them; every
        // test calls this first, under `one_at_a_time`.
        unsafe {
            std::env::set_var("RYUEDA_RUNTIME_DIR", &run);
            std::env::set_var("RYUEDA_CONFIG_DIR", dir.join("config"));
            std::env::set_var("RYUEDA_CACHE_DIR", dir.join("cache"));
            std::env::set_var("RYUEDA_IDLE_TIMEOUT", "120");
            std::env::set_var("RYUEDA_ID_SEED", "app");
        }
        // The folder is a repository of its own (#920): `ryueda init`
        // sets up the repository a project is made in, and this folder is
        // under the target directory, inside the clone the tests build
        // from. Without it, every run wrote that clone's shared hooks and
        // drivers, naming this build's binary.
        if !dir.join(".git").exists() {
            git_in(&dir, &dir, &["init", "-q"]);
        }
        dir
    })
}

/// `git` in `dir` without anyone's config (`base` holds the empty one), a
/// fixed identity and dates.
fn git_cmd(base: &Path, dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.env("GIT_CONFIG_GLOBAL", base.join("no-gitconfig"))
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
    c
}

fn git_in(base: &Path, dir: &Path, args: &[&str]) {
    let out = git_cmd(base, dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// [`git`] that may fail (a merge that stops, a commit a hook refuses):
/// whether it succeeded.
pub fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git_cmd(shared(), dir)
        .args(args)
        .status()
        .unwrap()
        .success()
}

/// The repository `dir` is in.
pub fn top_of(dir: &Path) -> PathBuf {
    let out = git_cmd(shared(), dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::canonicalize(String::from_utf8_lossy(&out.stdout).trim()).unwrap()
}

/// `ryueda` in `dir`, in process; it must succeed: its result.
pub fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new(ryueda_exe())
        .current_dir(dir)
        .env("RYUEDA_NO_DAEMON", "1")
        .args(args)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

/// The breakout entered by its script and laid out in `board`.
fn breakout_files(board: &Path) {
    cli(board, &["init"]);
    for (path, text) in breakout::footprints() {
        let at = board.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        cli(board, &args.iter().map(String::as_str).collect::<Vec<_>>());
    }
    cli(board, &["layout"]);
}

/// `git` in `dir` without anyone's config, a fixed identity and dates
/// (no per-worker setup: the tests make their repositories).
pub fn git(dir: &Path, args: &[&str]) {
    git_in(shared(), dir, args);
}

fn newest(dir: &Path) -> SystemTime {
    let mut t = SystemTime::UNIX_EPOCH;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let m = if p.is_dir() {
            newest(&p)
        } else {
            e.metadata().and_then(|m| m.modified()).unwrap_or(t)
        };
        t = t.max(m);
    }
    t
}

/// The `ryueda` beside this test's profile (`target/<profile>/ryueda`),
/// built when it is missing or older than ryueda's sources, unless
/// `RYUEDA_TEST_PREBUILT` says it is there.
pub fn ryueda_exe() -> PathBuf {
    let profile = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let exe = profile.join(format!("ryueda{}", std::env::consts::EXE_SUFFIX));
    if std::env::var("RYUEDA_TEST_PREBUILT").is_ok_and(|v| !v.is_empty()) {
        return exe;
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryueda/src");
    let fresh = exe
        .metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|t| t >= newest(&src));
    if !fresh {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["build", "-p", "ryueda", "--bin", "ryueda"]);
        match profile.file_name().and_then(|n| n.to_str()) {
            Some("debug") => {}
            Some("release") => {
                cmd.arg("--release");
            }
            Some(other) => {
                cmd.args(["--profile", other]);
            }
            None => {}
        }
        for (k, _) in std::env::vars() {
            if k.starts_with("CARGO_PKG_")
                || k.starts_with("CARGO_MANIFEST_")
                || k.starts_with("CARGO_BIN_EXE_")
                || [
                    "CARGO_CRATE_NAME",
                    "CARGO_PRIMARY_PACKAGE",
                    "CARGO_TARGET_TMPDIR",
                ]
                .contains(&k.as_str())
            {
                cmd.env_remove(k);
            }
        }
        assert!(cmd.status().unwrap().success(), "cargo build -p ryueda");
    }
    exe
}

pub fn daemon() -> &'static Daemon {
    static D: OnceLock<Daemon> = OnceLock::new();
    D.get_or_init(|| {
        shared();
        Daemon::at(&ryueda_exe()).unwrap()
    })
}

/// The sensor breakout entered by its script and laid out, in a folder
/// of its own.
pub fn breakout(name: &str) -> PathBuf {
    let board = shared().join(name);
    let _ = std::fs::remove_dir_all(&board);
    std::fs::create_dir_all(&board).unwrap();
    breakout_files(&board);
    std::fs::canonicalize(board).unwrap()
}

/// The breakout committed, then edited as `ryueda/tests/git.rs`'s
/// `edited_breakout` edits it: r1's value, r3 added and connected, c3
/// deleted, scl renamed, J1 moved.
pub fn breakout_in_git(name: &str) -> PathBuf {
    let board = breakout(name);
    git(&board, &["init", "-q"]);
    git(&board, &["add", "."]);
    git(&board, &["commit", "-q", "-m", "laid out"]);
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
        let out = Command::new(ryueda_exe())
            .current_dir(&board)
            .env("RYUEDA_NO_DAEMON", "1")
            .args(&args)
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
    }
    board
}

/// The agent's request through the project's daemon.
///
/// Through the tests' [`daemon`] (the real `ryueda`), never
/// `ryueda::client::request`, which starts daemons from the current
/// binary: here, the test's own.
pub fn agent(root: &Path, method: &str, params: Value) -> Value {
    let mut conn = daemon().connect(root).unwrap();
    let hello = Request::SessionHello {
        kind: ryu_session::live::Kind::Agent,
        name: Some("agent".into()),
        participant: None,
    };
    conn.call(&hello).unwrap().unwrap();
    let req = Request::from_rpc(method, Some(params)).unwrap();
    conn.call(&req).unwrap().unwrap()
}

/// Whether drawing tests run: an adapter, or a skip; a failure instead
/// when `RYUEDA_REQUIRE_GPU` (or ryucad's `RYUCAD_REQUIRE_GPU`) is set.
pub fn gpu() -> bool {
    let ok = ryueda_app::ui::smoke::headless().is_some();
    if !ok {
        let required = ["RYUEDA_REQUIRE_GPU", "RYUCAD_REQUIRE_GPU"]
            .iter()
            .any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
        assert!(!required, "no GPU adapter, and RYUEDA_REQUIRE_GPU is set");
        eprintln!("no GPU adapter: skipped");
    }
    ok
}

/// Lavapipe segfaults with draws in parallel (#230): one at a time.
pub fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ours: R5 (r8) on the rails and C98 on SCL.
pub const OURS: &[&[&str]] = &[
    &[
        "component",
        "add",
        "breakout",
        "--of",
        "r_4k7",
        "--id",
        "r8",
        "--refdes",
        "R5",
    ],
    &["connect", "breakout/r8/1", "rail:3V3"],
    &["connect", "breakout/r8/2", "rail:GND"],
    &[
        "component",
        "add",
        "breakout",
        "--of",
        "c_100n",
        "--id",
        "c98",
        "--refdes",
        "C98",
    ],
    &["connect", "breakout/c98/1", "SCL"],
];

/// Theirs: another R5 (r9), C97 on SCL, and R1 47k: a designator clash,
/// and both sides' new parts laid out in the same free space.
pub const THEIRS: &[&[&str]] = &[
    &[
        "component",
        "add",
        "breakout",
        "--of",
        "r_4k7",
        "--id",
        "r9",
        "--refdes",
        "R5",
    ],
    &["connect", "breakout/r9/1", "rail:3V3"],
    &["connect", "breakout/r9/2", "rail:GND"],
    &[
        "component",
        "add",
        "breakout",
        "--of",
        "c_100n",
        "--id",
        "c97",
        "--refdes",
        "C97",
    ],
    &["connect", "breakout/c97/1", "SCL"],
    &["component", "edit", "breakout/r1", "--value", "47k"],
];

/// A merge of the breakout begun in a repository of its own,
/// `shared()/<name>`, with the board in `board/`: base, `theirs` and
/// `main` (ours) committed, then `git merge --no-commit theirs`. Answers
/// the repository and the board.
///
/// #920: `ryueda init` installs ryueda's merge driver and hook into the
/// repository its folder is in. It runs here only after asserting that
/// this is the test's own; no test runs `ryueda git install`.
pub fn merging(name: &str, ours: &[&[&str]], theirs: &[&[&str]]) -> (PathBuf, PathBuf) {
    let top = shared().join(name);
    let _ = std::fs::remove_dir_all(&top);
    std::fs::create_dir_all(&top).unwrap();
    git(&top, &["init", "-q"]);
    let top = std::fs::canonicalize(top).unwrap();
    let board = top.join("board");
    std::fs::create_dir_all(&board).unwrap();
    assert_eq!(
        top_of(&board),
        top,
        "#920: the board's repository must be the test's own"
    );
    breakout_files(&board);
    let commit = |m: &str| {
        git(&top, &["add", "-A"]);
        git(&top, &["commit", "-q", "-m", m]);
    };
    commit("base");
    git(&top, &["checkout", "-q", "-b", "theirs"]);
    for args in theirs {
        cli(&board, args);
    }
    commit("theirs");
    git(&top, &["checkout", "-q", "main"]);
    for args in ours {
        cli(&board, args);
    }
    commit("ours");
    // `--no-commit`: the merge stops whether or not the driver did.
    let _ = git_ok(&top, &["merge", "--no-commit", "-q", "theirs"]);
    (top, std::fs::canonicalize(board).unwrap())
}

/// A merge whose one record is in `ryueda.json`: each side sets the
/// project's property `rev` differently.
pub fn manifest_merging(name: &str) -> (PathBuf, PathBuf) {
    merging(
        name,
        &[&["project", "edit", "--property", "rev=ours"]],
        &[&["project", "edit", "--property", "rev=theirs"]],
    )
}
