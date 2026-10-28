//! Shared by the app's tests: a `ryucad` binary of this build, and
//! projects with daemons of their own.

#![allow(dead_code)]

use ryucad::client::Daemon;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The folder every test of this process shares, emptied when it starts:
/// daemons find their sockets, logs and preferences there, and each
/// project has its own endpoint in it. Under `cargo test` that is every
/// test of this binary; `cargo nextest` (PR CI) runs each test in a
/// process of its own, side by side, so each has a folder of its own.
fn shared() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().unwrap();
        let mut stem = exe.file_stem().unwrap().to_str().unwrap().to_owned();
        if std::env::var_os("NEXTEST").is_some() {
            stem = format!("{stem}-{}", std::process::id());
        }
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
            std::env::set_var("RYUCAD_CACHE_DIR", dir.join("cache"));
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
        // A target folder is tagged, or at least holds Cargo's rustc
        // cache (a build of only the ci profile may leave no tag); a target
        // triple's folder inside it has neither.
        let above = bin.parent().unwrap();
        if !above.join("CACHEDIR.TAG").exists() && !above.join(".rustc_info.json").exists() {
            cmd.arg("--target").arg(above.file_name().unwrap());
        }
        assert!(cmd.status().unwrap().success(), "building ryucad");
        let path = bin.join(format!("ryucad{}", std::env::consts::EXE_SUFFIX));
        // The shared folder is a repository of its own with the diff driver
        // in its config, so doctor finds the same Git setup whether the
        // target folder sits inside a checkout (as in CI) or not.
        let dir = shared();
        let init = Command::new("git").args(["init", "-q"]).arg(dir).status();
        assert!(init.unwrap().success(), "git init");
        let install = Command::new(&path)
            .args(["git", "install"])
            .current_dir(dir)
            .status();
        assert!(install.unwrap().success(), "ryucad git install");
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

    /// Runs Git in the project, as someone with a fixed identity (CI has
    /// none).
    pub fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// [`Project::git`] for a command that may fail (a merge that stops
    /// for conflicts): whether it succeeded.
    pub fn try_git(&self, args: &[&str]) -> bool {
        Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// After [`Project::with_part`] and [`Project::committed`], with
    /// ryucad's merge driver and hooks: the box made 15 mm long on branch
    /// `theirs` and 12 mm on `main`, which is checked out. Merging
    /// `theirs` leaves one conflict.
    pub fn diverged(self) -> Project {
        self.ok(&["git", "install"]);
        self.git(&["checkout", "-q", "-b", "theirs"]);
        self.ok(&["feature", "edit", "p", "base", "--param", "x=15"]);
        self.git(&["commit", "-q", "-am", "theirs"]);
        self.git(&["checkout", "-q", "main"]);
        self.ok(&["feature", "edit", "p", "base", "--param", "x=12"]);
        self.git(&["commit", "-q", "-am", "ours"]);
        self
    }

    /// The project in a repository of its own, everything committed on
    /// `main`.
    pub fn committed(self) -> Project {
        self.git(&["init", "-q", "-b", "main"]);
        self.git(&["add", "."]);
        self.git(&["commit", "-q", "-m", "start"]);
        self
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

impl Project {
    /// After [`Project::with_part`]: a sketch `top` on the box's top face
    /// (placed, not referenced), under-defined: a line, a circle of
    /// radius 1.5 mm (dimension `d1`) and a construction line.
    pub fn with_top_sketch(self) -> Project {
        self.ok(&[
            "feature", "add", "p", "sketch", "--id", "top", "--origin", "0,0,5",
        ]);
        let ops = serde_json::json!([
            {"op": "add", "type": "point", "at": [5, 2]},
            {"op": "add", "type": "point", "at": [17, 2]},
            {"op": "add", "type": "point", "at": [15, 7]},
            {"op": "add", "type": "point", "at": [6, 8]},
            {"op": "add", "type": "line", "start": "p1", "end": "p2"},
            {"op": "add", "type": "circle", "center": "p3", "r": 1.5},
            {"op": "add", "type": "line", "construction": true, "start": "p4", "end": "p3"},
            {"op": "add", "type": "radius", "a": "ci1", "value": "1.5 mm"}
        ]);
        self.ok(&["sketch", "edit", "p", "top", &ops.to_string()]);
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

/// The planar face of `part`'s solid `solid` whose outward normal is `z`
/// along z (`1` up, `-1` down).
pub fn face_up(p: &Project, part: &str, solid: &str, z: f64) -> String {
    let v = p.ok(&["inspect", part, solid]);
    v["bodies"][0]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            let sign = if f["reversed"] == true { -1.0 } else { 1.0 };
            f["surface"]["type"] == "plane"
                && (f["surface"]["frame"]["z"][2].as_f64().unwrap() * sign - z).abs() < 1e-12
        })
        .map(|f| f["name"].as_str().unwrap().to_owned())
        .unwrap()
}

/// A plate fixed in `frame` and a bolt stood on its top by revolute `m1`,
/// with three copies of the bolt round the plate's z (`ring`).
pub fn mated() -> Project {
    let p = Project::new();
    p.ok(&["part", "create", "plate"]);
    p.ok(&[
        "feature", "add", "plate", "box", "--id", "slab", "--param", "x=100", "--param", "y=50",
        "--param", "z=5",
    ]);
    p.ok(&["part", "create", "bolt"]);
    p.ok(&[
        "feature",
        "add",
        "bolt",
        "cylinder",
        "--id",
        "shank",
        "--param",
        "radius=3",
        "--param",
        "height=20",
    ]);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "base", "--of", "plate", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "pin", "--of", "bolt", "--origin", "50,0,0",
    ]);
    let top = format!("frame/base/slab/{}", face_up(&p, "plate", "slab", 1.0));
    let foot = format!("frame/pin/shank/{}", face_up(&p, "bolt", "shank", -1.0));
    p.ok(&[
        "feature",
        "add",
        "frame",
        "mate",
        "--id",
        "m1",
        "--form",
        "revolute",
        "--a",
        &top,
        "--a-offset",
        r#"{"origin": ["20 mm", "10 mm", "0 mm"]}"#,
        "--b",
        &foot,
        "--b-offset",
        r#"{"flip": true}"#,
    ]);
    p.ok(&[
        "feature",
        "add",
        "frame",
        "instance_pattern",
        "--id",
        "ring",
        "--instance",
        "pin",
        "--axis",
        "z",
        "--count",
        "4",
        "--angle",
        "360deg",
    ]);
    p
}
