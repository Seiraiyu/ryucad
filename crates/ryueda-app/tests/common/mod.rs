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
        dir
    })
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
    let run = |args: &[&str]| {
        let out = Command::new(ryueda_exe())
            .current_dir(&board)
            .env("RYUEDA_NO_DAEMON", "1")
            .args(args)
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["ok"], true, "{args:?}: {v}");
    };
    run(&["init"]);
    for (path, text) in breakout::footprints() {
        let at = board.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    for args in breakout::script() {
        run(&args.iter().map(String::as_str).collect::<Vec<_>>());
    }
    run(&["layout"]);
    std::fs::canonicalize(board).unwrap()
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
