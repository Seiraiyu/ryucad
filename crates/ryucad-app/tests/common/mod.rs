//! Shared by the app's tests: a `ryucad` binary of this build, and
//! projects with daemons of their own.

#![allow(dead_code)]

use ryucad::client::Daemon;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The folder every test of this binary shares, emptied when the binary
/// starts: daemons find their sockets, logs and preferences there, and
/// each project has its own endpoint in it.
fn shared() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_str().unwrap();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(stem);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Sockets go in a short folder of their own: under a deep checkout
        // the target folder's path leaves no room in a Unix socket address
        // (107 bytes). It is named for `dir`, so checkouts stay apart.
        let mut h = std::hash::DefaultHasher::new();
        std::hash::Hash::hash(&dir, &mut h);
        let run = std::env::temp_dir().join(format!(
            "ryucad-{:08x}",
            std::hash::Hasher::finish(&h) as u32
        ));
        let _ = std::fs::remove_dir_all(&run);
        // SAFETY: set once, before any test starts a daemon or reads them.
        unsafe {
            std::env::set_var("RYUCAD_RUNTIME_DIR", &run);
            std::env::set_var("RYUCAD_CONFIG_DIR", dir.join("config"));
            std::env::set_var("RYUCAD_IDLE_TIMEOUT", "120");
        }
        dir
    })
}

/// The `ryucad` binary of this build, built if needed: `cargo test -p
/// ryucad-app` doesn't build other packages' binaries.
pub fn ryucad() -> &'static Daemon {
    static DAEMON: OnceLock<Daemon> = OnceLock::new();
    DAEMON.get_or_init(|| {
        shared();
        // target/<triple>?/<profile>/deps/<test>: the binary goes in <profile>.
        let exe = std::env::current_exe().unwrap();
        let bin = exe.parent().unwrap().parent().unwrap();
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["build", "-p", "ryucad", "--bin", "ryucad"]);
        match bin.file_name().unwrap().to_str().unwrap() {
            "debug" => {}
            "release" => {
                cmd.arg("--release");
            }
            other => {
                cmd.args(["--profile", other]);
            }
        }
        // A target folder is tagged; a target triple's folder inside it isn't.
        let above = bin.parent().unwrap();
        if !above.join("CACHEDIR.TAG").exists() {
            cmd.arg("--target").arg(above.file_name().unwrap());
        }
        assert!(cmd.status().unwrap().success(), "building ryucad");
        let path = bin.join(format!("ryucad{}", std::env::consts::EXE_SUFFIX));
        Daemon::at(&path).unwrap()
    })
}

/// A graphics adapter to draw with, software ones included. Without one
/// the tests that draw pass with a note, unless `RYUCAD_REQUIRE_GPU` is
/// set (CI sets it where lavapipe or WARP is installed).
pub fn gpu() -> Option<(
    eframe::wgpu::Adapter,
    eframe::wgpu::Device,
    eframe::wgpu::Queue,
)> {
    let g = ryucad_app::ui::scene::headless();
    if g.is_none() {
        let required = std::env::var("RYUCAD_REQUIRE_GPU").is_ok_and(|v| !v.is_empty());
        assert!(!required, "no GPU adapter, and RYUCAD_REQUIRE_GPU is set");
        eprintln!("no GPU adapter: skipped");
    }
    g
}

/// A project in a folder of its own.
pub struct Project {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
}

impl Project {
    pub fn new() -> Project {
        Project::named("project")
    }

    /// A project in a folder called `name`.
    pub fn named(name: &str) -> Project {
        let dir = tempfile::tempdir_in(shared()).unwrap();
        // The plain form, as ryucad reports it (not `\\?\C:\…` on Windows).
        let root = ryucad::doc::project::plain(dir.path().canonicalize().unwrap()).join(name);
        std::fs::create_dir(&root).unwrap();
        let p = Project { _dir: dir, root };
        p.ok(&["init", "--unit", "mm"]);
        p
    }

    /// Runs the CLI in the project as the agent `agent`; returns its JSON.
    pub fn cli(&self, args: &[&str]) -> Value {
        let out = Command::new(&ryucad().exe)
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// [`Project::cli`], which must succeed; returns its result.
    pub fn ok(&self, args: &[&str]) -> Value {
        let v = self.cli(args);
        assert_eq!(v["ok"], true, "ryucad {args:?}: {v}");
        v["result"].clone()
    }

    /// A part with a box and a cylinder on it.
    pub fn with_part(self) -> Project {
        self.ok(&["part", "create", "p"]);
        self.ok(&[
            "feature", "add", "p", "box", "--id", "base", "--param", "x=20", "--param", "y=10",
            "--param", "z=5",
        ]);
        self.ok(&[
            "feature", "add", "p", "cylinder", "--id", "boss", "--param", "radius=3", "--param",
            "height=8", "--origin", "0,0,5",
        ]);
        self
    }
}

impl Project {
    /// A part with a sketch on xy: a 20 x 10 rectangle from the origin,
    /// fully defined (its width a parameter), with a construction diagonal,
    /// and a 5 mm extrude of it.
    pub fn with_sketch(self) -> Project {
        self.ok(&["part", "create", "p"]);
        self.ok(&["param", "set", "p", "width=20 mm"]);
        self.ok(&["feature", "add", "p", "sketch", "--id", "s"]);
        let ops = serde_json::json!([
            {"op": "add", "type": "point", "at": [0, 0]},
            {"op": "add", "type": "point", "at": [19, 1]},
            {"op": "add", "type": "point", "at": [21, 9]},
            {"op": "add", "type": "point", "at": [1, 11]},
            {"op": "add", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "type": "line", "start": "p2", "end": "p3"},
            {"op": "add", "type": "line", "start": "p3", "end": "p4"},
            {"op": "add", "type": "line", "start": "p4", "end": "p1"},
            {"op": "add", "type": "line", "construction": true, "start": "p1", "end": "p3"},
            {"op": "add", "type": "horizontal", "a": "l1"},
            {"op": "add", "type": "horizontal", "a": "l3"},
            {"op": "add", "type": "vertical", "a": "l2"},
            {"op": "add", "type": "vertical", "a": "l4"},
            {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "type": "length", "a": "l1", "value": "width"},
            {"op": "add", "type": "length", "a": "l2", "value": "10 mm"}
        ]);
        self.ok(&["sketch", "edit", "p", "s", &ops.to_string()]);
        self.ok(&[
            "feature", "add", "p", "extrude", "--id", "e", "--sketch", "s", "--length", "5",
        ]);
        self
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = Command::new(&ryucad().exe)
            .args(["daemon", "stop"])
            .current_dir(&self.root)
            .output();
    }
}
