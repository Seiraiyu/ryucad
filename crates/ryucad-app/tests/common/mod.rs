//! Shared by the app's tests: a `ryucad` binary of this build, and
//! projects with daemons of their own.

#![allow(dead_code)]

use ryucad::client::Daemon;
use serde_json::{Value, json};
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
/// ryucad-app` doesn't build other packages' binaries. With
/// `RYUCAD_TEST_PREBUILT` set, the one beside the tests is taken as it is.
/// A window's workspace in Direct edit, as every window was before
/// ryucad 19.B: for tests of what the window writes.
pub fn open_editing(
    root: &std::path::Path,
    daemon: &Daemon,
    name: &str,
    wake: ryucad_app::core::link::Wake,
) -> ryucad::error::Result<ryucad_app::core::workspace::Workspace> {
    let mut ws = ryucad_app::core::workspace::Workspace::open(root, daemon, name, wake)?;
    ws.set_mode(ryucad_app::core::modes::Mode::Edit);
    Ok(ws)
}

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
        let path = bin.join(format!("ryucad{}", std::env::consts::EXE_SUFFIX));
        // A run of the workspace's tests (CI's partitions) has built it
        // already, with the dev-dependencies' features; built again here
        // without them, every workspace crate would build a second time.
        let prebuilt = std::env::var("RYUCAD_TEST_PREBUILT").is_ok_and(|v| !v.is_empty());
        if prebuilt {
            assert!(path.exists(), "RYUCAD_TEST_PREBUILT, but no {path:?}");
        } else {
            assert!(cmd.status().unwrap().success(), "building ryucad");
        }
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
    /// A part `p`: a 10 mm square on xy (`s1`, its corners `p1` to `p4`
    /// counterclockwise from the bottom left), a 6 mm square on a datum
    /// plane `d` 10 mm above it (`s2`, likewise), and a loft `l` between
    /// them with its start tangent along the normal, weighed 1.5; and a
    /// 2 mm circle about (20, 0) on xy (`s3`) swept 20 mm up z along line
    /// `l1` of sketch `s4` on zx (`w`).
    pub fn with_loft(self) -> Project {
        let square = |h: f64| {
            format!(
                r#"[{{"op": "add", "id": "p1", "type": "point", "at": [-{h}, -{h}]}},
                    {{"op": "add", "id": "p2", "type": "point", "at": [{h}, -{h}]}},
                    {{"op": "add", "id": "p3", "type": "point", "at": [{h}, {h}]}},
                    {{"op": "add", "id": "p4", "type": "point", "at": [-{h}, {h}]}},
                    {{"op": "add", "id": "l1", "type": "line", "start": "p1", "end": "p2"}},
                    {{"op": "add", "id": "l2", "type": "line", "start": "p2", "end": "p3"}},
                    {{"op": "add", "id": "l3", "type": "line", "start": "p3", "end": "p4"}},
                    {{"op": "add", "id": "l4", "type": "line", "start": "p4", "end": "p1"}}]"#
            )
        };
        self.ok(&["part", "create", "p"]);
        self.ok(&[
            "feature", "add", "p", "sketch", "--id", "s1", "--plane", "xy",
        ]);
        self.ok(&["sketch", "edit", "p", "s1", &square(5.0)]);
        self.ok(&[
            "feature",
            "add",
            "p",
            "plane",
            "--id",
            "d",
            "--by",
            "offset",
            "--from",
            "xy",
            "--distance",
            "10",
        ]);
        self.ok(&[
            "feature", "add", "p", "sketch", "--id", "s2", "--plane", "p/d",
        ]);
        self.ok(&["sketch", "edit", "p", "s2", &square(3.0)]);
        self.ok(&[
            "feature",
            "add",
            "p",
            "loft",
            "--id",
            "l",
            "--section",
            "s1",
            "--section",
            "s2",
            "--start-tangent",
            "normal",
            "--start-weight",
            "1.5",
        ]);
        self.ok(&[
            "feature", "add", "p", "sketch", "--id", "s3", "--plane", "xy",
        ]);
        self.ok(&[
            "sketch",
            "edit",
            "p",
            "s3",
            r#"[{"op": "add", "id": "c", "type": "point", "at": [20, 0]},
                {"op": "add", "id": "ci", "type": "circle", "center": "c", "r": 2}]"#,
        ]);
        // On zx, x runs along z: from (20, 0, 0) to (20, 0, 20).
        self.ok(&[
            "feature", "add", "p", "sketch", "--id", "s4", "--plane", "zx",
        ]);
        self.ok(&[
            "sketch",
            "edit",
            "p",
            "s4",
            r#"[{"op": "add", "id": "q1", "type": "point", "at": [0, 20]},
                {"op": "add", "id": "q2", "type": "point", "at": [20, 20]},
                {"op": "add", "id": "l1", "type": "line", "start": "q1", "end": "q2"}]"#,
        ]);
        self.ok(&[
            "feature", "add", "p", "sweep", "--id", "w", "--sketch", "s3", "--path", "p/s4/l1",
        ]);
        self
    }

    /// A part `p`: a 20 × 10 × 5 mm box `base` hollowed inwards 1 mm into
    /// a cavity (`sh`), then split by the yz plane (`sp`), which founds
    /// solid `sp` from the side in front.
    pub fn with_shell_and_split(self) -> Project {
        self.ok(&["part", "create", "p"]);
        self.ok(&[
            "feature",
            "add",
            "p",
            "box",
            "--id",
            "base",
            "--param",
            "x=20",
            "--param",
            "y=10",
            "--param",
            "z=5",
            "--origin=-10,-5,0",
        ]);
        self.ok(&[
            "feature",
            "add",
            "p",
            "shell",
            "--id",
            "sh",
            "--target",
            "base",
            "--thickness",
            "1 mm",
        ]);
        self.ok(&[
            "feature", "add", "p", "split", "--id", "sp", "--target", "base", "--plane", "yz",
        ]);
        self
    }
}

impl Project {
    /// A sheet metal part `p`: a 60 x 40 mm plate 1.5 mm thick (base
    /// flange `plate`, K-factor 0.44), a 20 mm flange `f` down its right
    /// top edge, and an unfold `u` of it, which the tip shows flat.
    pub fn with_sheet_metal(self) -> Project {
        self.ok(&["part", "create", "p"]);
        self.ok(&[
            "feature", "add", "p", "sketch", "--id", "s", "--plane", "xy",
        ]);
        let mut ops = Vec::new();
        for (i, at) in [[0, 0], [60, 0], [60, 40], [0, 40]].into_iter().enumerate() {
            ops.push(json!({"op": "add", "id": format!("p{}", i + 1), "type": "point", "at": at}));
            ops.push(json!({"op": "add", "type": "fix", "a": format!("p{}", i + 1)}));
        }
        for i in 1..=4 {
            ops.push(json!({"op": "add", "id": format!("l{i}"), "type": "line",
                            "start": format!("p{i}"), "end": format!("p{}", i % 4 + 1)}));
        }
        self.ok(&["sketch", "edit", "p", "s", &ops_text(&ops)]);
        self.ok(&[
            "feature",
            "add",
            "p",
            "base_flange",
            "--id",
            "plate",
            "--sketch",
            "s",
            "--thickness",
            "1.5mm",
            "--radius",
            "1mm",
            "--k-factor",
            "0.44",
        ]);
        let body = self.ok(&["inspect", "p", "plate"])["bodies"][0].clone();
        let at = |q: [f64; 3]| -> String {
            body["vertices"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| {
                    (0..3).all(|k| (v["point"][k].as_f64().unwrap() - q[k] * 1e-3).abs() < 1e-9)
                })
                .map(|v| v["name"].as_str().unwrap().to_owned())
                .unwrap()
        };
        let (a, b) = (at([60.0, 0.0, 1.5]), at([60.0, 40.0, 1.5]));
        let edge = body["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
            .map(|e| format!("p/plate/{}", e["name"].as_str().unwrap()))
            .unwrap();
        self.ok(&[
            "feature", "add", "p", "flange", "--id", "f", "--edge", &edge, "--angle", "90deg",
            "--length", "20mm",
        ]);
        self.ok(&[
            "feature", "add", "p", "unfold", "--id", "u", "--target", "plate",
        ]);
        self
    }

    /// A part with a sketch on xy: a 20 x 10 rectangle from the origin,
    /// fully defined (its width a parameter), with a construction diagonal,
    /// and a 5 mm extrude of it.
    /// Part `p` with surface features (ryucad 18): `w`, an open chain of
    /// two lines extruded into a wall (a sheet of one boundary); `f`, a
    /// 10 mm square filled (a sheet); `sw`, box `b`'s six faces extracted
    /// and sewn into a closed sheet; `t`, box `c`'s top extracted and
    /// thickened 2 mm into a solid.
    pub fn with_surfaces(self) -> Project {
        self.ok(&["part", "create", "p"]);
        let sketch = |id: &str, ops: &[Value]| {
            self.ok(&["feature", "add", "p", "sketch", "--id", id, "--plane", "xy"]);
            self.ok(&["sketch", "edit", "p", id, &ops_text(ops)]);
        };
        let point =
            |id: &str, at: [f64; 2]| json!({"op": "add", "id": id, "type": "point", "at": at});
        let line = |id: &str, a: &str, b: &str| json!({"op": "add", "id": id, "type": "line", "start": a, "end": b});
        sketch(
            "s1",
            &[
                point("o", [0.0, 0.0]),
                point("a", [10.0, 0.0]),
                point("b", [10.0, 10.0]),
                line("l1", "o", "a"),
                line("l2", "a", "b"),
            ],
        );
        self.ok(&[
            "feature",
            "add",
            "p",
            "extrude",
            "--id",
            "w",
            "--sketch",
            "s1",
            "--surface",
            "--length",
            "20mm",
        ]);
        let mut square = Vec::new();
        for (i, at) in [[30.0, 0.0], [40.0, 0.0], [40.0, 10.0], [30.0, 10.0]]
            .into_iter()
            .enumerate()
        {
            square.push(point(&format!("q{}", i + 1), at));
        }
        for i in 1..=4 {
            square.push(line(
                &format!("l{i}"),
                &format!("q{i}"),
                &format!("q{}", i % 4 + 1),
            ));
        }
        sketch("s2", &square);
        self.ok(&[
            "feature", "add", "p", "fill", "--id", "f", "--curve", "p/s2/l1", "--curve", "p/s2/l2",
            "--curve", "p/s2/l3", "--curve", "p/s2/l4",
        ]);
        let faces = |solid: &str| -> Vec<(String, f64)> {
            let body = self.ok(&["inspect", "p", solid])["bodies"][0].clone();
            let mut out: Vec<(String, f64)> = body["faces"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    let z = f["surface"]["frame"]["origin"][2].as_f64().unwrap_or(0.0);
                    (format!("p/{solid}/{}", f["name"].as_str().unwrap()), z)
                })
                .collect();
            out.sort_by(|a, b| a.1.total_cmp(&b.1));
            out
        };
        for (id, x) in [("b", "60"), ("c", "90")] {
            self.ok(&[
                "feature",
                "add",
                "p",
                "box",
                "--id",
                id,
                "--param",
                "x=10",
                "--param",
                "y=10",
                "--param",
                "z=10",
                "--origin",
                &format!("{x},0,0"),
            ]);
        }
        let mut sew = vec!["feature", "add", "p", "sew", "--id", "sw", "--no-solid"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for (k, (face, _)) in faces("b").iter().enumerate() {
            let id = format!("x{k}");
            self.ok(&[
                "feature", "add", "p", "extract", "--id", &id, "--face", face,
            ]);
            sew.extend(["--sheet".to_owned(), id]);
        }
        self.ok(&sew.iter().map(String::as_str).collect::<Vec<_>>());
        let top = faces("c").last().unwrap().0.clone();
        self.ok(&[
            "feature", "add", "p", "extract", "--id", "xt", "--face", &top,
        ]);
        self.ok(&[
            "feature", "add", "p", "thicken", "--id", "t", "--sheet", "xt", "--front", "2mm",
        ]);
        self
    }

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

/// A bolt `b1` fixed at the origin and `b2` free 40 mm along x, in
/// assembly `frame`.
pub fn two_bolts() -> Project {
    two_bolts_as("cylinder", &["radius=3", "height=20"])
}

/// [`two_bolts`] with each bolt's shank a `kind` primitive of `params`.
pub fn two_bolts_as(kind: &str, params: &[&str]) -> Project {
    let p = Project::new();
    p.ok(&["part", "create", "bolt"]);
    let mut add = vec!["feature", "add", "bolt", kind, "--id", "shank"];
    for x in params {
        add.extend(["--param", x]);
    }
    p.ok(&add);
    p.ok(&["part", "create", "frame", "--assembly"]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b1", "--of", "bolt", "--fixed",
    ]);
    p.ok(&[
        "feature", "add", "frame", "instance", "--id", "b2", "--of", "bolt", "--origin", "40,0,0",
    ]);
    p
}

/// A plate `base` (100 × 50 × 5 mm, fixed at the origin) and a free bolt
/// `pin` (radius 3, height 20) placed 50 mm along x, in assembly `frame`,
/// with no mate: the plate's top and the bolt's foot as references.
pub fn plate_and_bolt() -> (Project, String, String) {
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
    (p, top, foot)
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

/// A `file://` URL for a repository at `root`, as Git takes one on every
/// platform (forward slashes, three after the scheme on Windows).
pub fn file_url(root: &std::path::Path) -> String {
    let path = root.display().to_string();
    if cfg!(windows) {
        format!("file:///{}", path.replace('\\', "/"))
    } else {
        format!("file://{path}")
    }
}

/// A library project, `fasteners`, in a repository of its own: part `p`
/// (a box and a cylinder) numbered `B-6` and released as A, official on
/// `main`. Its `file://` URL.
pub fn library() -> (Project, String) {
    let lib = Project::named("fasteners").with_part();
    lib.ok(&["pdm", "init"]);
    lib.ok(&["props", "set", "p", "number=B-6"]);
    let lib = lib.committed();
    lib.ok(&["release", "propose", "p"]);
    lib.git(&["add", "."]);
    lib.git(&["commit", "-q", "-m", "p A"]);
    let url = file_url(&lib.root);
    (lib, url)
}

/// Releases `lib`'s `p` again, as B, after making its box longer.
pub fn release_again(lib: &Project) {
    lib.ok(&["feature", "edit", "p", "base", "--param", "x=30"]);
    lib.git(&["commit", "-q", "-am", "longer"]);
    lib.ok(&["release", "propose", "p"]);
    lib.git(&["add", "."]);
    lib.git(&["commit", "-q", "-m", "p B"]);
}

/// Sketch ops as the CLI takes them.
fn ops_text(ops: &[Value]) -> String {
    serde_json::to_string(ops).unwrap()
}
