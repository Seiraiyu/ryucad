//! What the window tests share: one folder per test process (runtime and
//! config inside it, a repository of its own), the `ryuplm` binary
//! beside the tests (built if it is missing or older than its sources),
//! seeded catalogues, an adapter or a skip, and the agent's requests
//! through the daemon.

#![allow(dead_code)]

use ryuplm::proto::Request;
use ryuplm_app::core::link::Daemon;
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
        let run = std::env::temp_dir().join(format!("ryuplm-app-{}", std::process::id()));
        std::fs::create_dir_all(&run).unwrap();
        // SAFETY: before any thread of the tests' own reads them; every
        // test calls this first, under `one_at_a_time`.
        unsafe {
            std::env::set_var("RYUPLM_RUNTIME_DIR", &run);
            std::env::set_var("RYUPLM_CONFIG_DIR", dir.join("config"));
            std::env::set_var("RYUPLM_IDLE_TIMEOUT", "120");
            std::env::set_var("RYUPLM_ID_SEED", "app");
            std::env::set_var("RYUPLM_NO_DIALOG", "1");
        }
        // A repository of its own (ryueda-app's #920): this folder is
        // under the target directory, inside the clone the tests build
        // from, and `ryuplm init` writes the repository it is made in.
        if !dir.join(".git").exists() {
            git(&dir, &["init", "-q"]);
        }
        dir
    })
}

/// `git` in `dir` without anyone's config, a fixed identity and dates.
pub fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", dir.join("no-gitconfig"))
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
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `ryuplm` in `dir`, in process; it must succeed: its result.
pub fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new(ryuplm_exe())
        .current_dir(dir)
        .env("RYUPLM_NO_DAEMON", "1")
        .args(args)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(v["ok"], true, "{args:?}: {v}");
    v["result"].clone()
}

/// `files` (a seeded catalogue's, `ryuplm::testing`'s) written as a
/// catalogue in a repository of its own under the tests' folder,
/// committed, with `ryuplm init`'s manifest kept.
pub fn write(name: &str, files: std::collections::BTreeMap<String, Vec<u8>>) -> PathBuf {
    let root = shared().join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    cli(&root, &["init"]);
    // The seeded manifest (its classes), with `ryuplm init`'s project ID.
    let ours: Value =
        serde_json::from_slice(&std::fs::read(root.join(ryuplm::catalogue::MANIFEST)).unwrap())
            .unwrap();
    for (p, b) in files {
        let b = if p == ryuplm::catalogue::MANIFEST {
            let mut m: Value = serde_json::from_slice(&b).unwrap();
            m["project"] = ours["project"].clone();
            ryuplm::canonical::to_string(&m).into_bytes()
        } else {
            b
        };
        let at = root.join(&p);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, b).unwrap();
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seeded"]);
    std::fs::canonicalize(root).unwrap()
}

/// `ryuplm::testing::catalogue`'s `n` items as a committed catalogue.
pub fn catalogue(name: &str, n: usize) -> PathBuf {
    write(name, ryuplm::testing::catalogue("app", n))
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

/// The `ryuplm` beside this test's profile, built when it is missing or
/// older than ryuplm's sources, unless `RYUPLM_TEST_PREBUILT` says it is
/// there.
pub fn ryuplm_exe() -> PathBuf {
    let profile = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let exe = profile.join(format!("ryuplm{}", std::env::consts::EXE_SUFFIX));
    if std::env::var("RYUPLM_TEST_PREBUILT").is_ok_and(|v| !v.is_empty()) {
        return exe;
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ryuplm/src");
    // Newer than its sources, and of this build's commit: the window
    // refuses a daemon of another build.
    let fresh = exe
        .metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|t| t >= newest(&src))
        && Command::new(&exe)
            .arg("version")
            .output()
            .ok()
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .is_some_and(|v| v["result"]["commit"] == ryuplm::proto::VersionInfo::current().commit);
    if !fresh {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["build", "-p", "ryuplm", "--bin", "ryuplm"]);
        match profile.file_name().and_then(|n| n.to_str()) {
            Some("debug") | None => {}
            Some("release") => {
                cmd.arg("--release");
            }
            Some(other) => {
                cmd.args(["--profile", other]);
            }
        }
        // As ryueda-app's: the test's own cargo variables would make the
        // build another package's.
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
        assert!(cmd.status().unwrap().success(), "cargo build -p ryuplm");
    }
    exe
}

/// `PATH` with `ryuplm_exe()`'s folder first: what `ryuplm-app` finds
/// its daemon's binary on.
pub fn path_with_ryuplm() -> std::ffi::OsString {
    let dir = ryuplm_exe().parent().unwrap().to_owned();
    let mut paths = vec![dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).unwrap()
}

pub fn daemon() -> &'static Daemon {
    static D: OnceLock<Daemon> = OnceLock::new();
    D.get_or_init(|| {
        shared();
        Daemon::at(&ryuplm_exe()).unwrap()
    })
}

/// The agent's request through the catalogue's daemon (the tests' real
/// `ryuplm`, never the test binary).
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
/// when `RYUPLM_REQUIRE_GPU` (or `RYUCAD_REQUIRE_GPU`) is set.
pub fn gpu() -> bool {
    let ok = ryuplm_app::ui::smoke::headless().is_some();
    if !ok {
        let required = ["RYUPLM_REQUIRE_GPU", "RYUCAD_REQUIRE_GPU"]
            .iter()
            .any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
        assert!(!required, "no GPU adapter, and RYUPLM_REQUIRE_GPU is set");
        eprintln!("no GPU adapter: skipped");
    }
    ok
}

/// Lavapipe crashes with draws in parallel (#230): one at a time.
pub fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ONE.lock().unwrap_or_else(|e| e.into_inner())
}
