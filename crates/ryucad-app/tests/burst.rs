//! #799's repro, with no person and no customer file: the real window
//! (`ryucad-app`, the binary) open on an empty project while a seeded
//! assembly the shape of the PCBA's import lands in it at once -- design
//! 192's generator at 450 parts, 11 assemblies and 4,500 occurrences,
//! against the import's 440 parts and about 4,300 bodies -- and stays up
//! while the daemon rebuilds and meshes it and the window draws it. The
//! window must still be running at the end, and its trace (`ryucad-app.log`,
//! with a heartbeat a second) must hold no abnormal line: no panic, fault,
//! failed allocation, wgpu error or lost device, and no exit.
//!
//! It needs a graphics adapter (`common::gpu`: lavapipe, WARP) and a
//! display: on Linux `DISPLAY` or `WAYLAND_DISPLAY` (CI runs it under
//! `xvfb-run`), else it passes with a note unless `RYUCAD_REQUIRE_DISPLAY`
//! is set. `RYUCAD_BURST_PARTS` sizes the burst (default 440, rounded up
//! to whole subassemblies of 50), `RYUCAD_BURST_SOAK` the seconds it is
//! watched once loaded (default 20).

#![cfg(any(windows, target_os = "linux"))]

mod common;

use common::{Project, ryucad, ryucad_beside_app};
use ryucad::bench::scale::{self, Spec};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long the burst may take to be listed, rebuilt and meshed.
const WAIT: Duration = Duration::from_secs(1800);

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn display() -> bool {
    if cfg!(windows) {
        return true;
    }
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    set("DISPLAY") || set("WAYLAND_DISPLAY")
}

/// The window, killed when the test ends however it ends, and its
/// daemon stopped.
struct Window {
    child: Child,
    root: std::path::PathBuf,
    run: tempfile::TempDir,
}

impl Window {
    fn trace(&self) -> String {
        std::fs::read_to_string(self.run.path().join("ryucad-app.log")).unwrap_or_default()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = Command::new(&ryucad().exe)
            .args(["daemon", "stop"])
            .current_dir(&self.root)
            .env("RYUCAD_RUNTIME_DIR", self.run.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// A heartbeat's counts: frames, parts loaded and listed, meshes, asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Beat {
    frames: u64,
    loaded: u64,
    listed: u64,
    meshes: u64,
    asked: u64,
}

fn number_after(line: &str, key: &str) -> Option<u64> {
    let rest = &line[line.find(key)? + key.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The last heartbeat in the trace.
fn last_beat(trace: &str) -> Option<Beat> {
    let line = trace.lines().rev().find(|l| l.contains(" alive: "))?;
    let parts = &line[line.find("parts ")? + 6..];
    let (loaded, rest) = parts.split_once('/')?;
    Some(Beat {
        frames: number_after(line, "frames ")?,
        loaded: loaded.parse().ok()?,
        listed: number_after(rest, "")?,
        meshes: number_after(line, "meshes ")?,
        asked: number_after(line, "asked ")?,
    })
}

/// The trace's lines that say the window is failing or has gone.
fn abnormal(trace: &str) -> Vec<&str> {
    const SIGNS: [&str; 8] = [
        "panicked",
        "fault:",
        "allocation of",
        "minidump",
        "wgpu error",
        "wgpu device lost",
        "exited:",
        " ERROR ",
    ];
    trace
        .lines()
        .filter(|l| SIGNS.iter().any(|s| l.contains(s)))
        .collect()
}

/// Copies `from`'s files into `to`, but its manifest and attributes: the
/// project's own stay, as an import leaves them.
fn copy_project(from: &Path, to: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let e = e.unwrap();
            let path = e.path();
            let rel = path.strip_prefix(from).unwrap();
            if e.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(to.join(rel)).unwrap();
                stack.push(path);
            } else if rel != Path::new(ryucad::doc::project::MANIFEST)
                && rel != Path::new(".gitattributes")
            {
                std::fs::copy(&path, to.join(rel)).unwrap();
                n += 1;
            }
        }
    }
    n
}

/// Waits for the trace to satisfy `done`, failing early if the window
/// exits or the trace turns abnormal.
fn wait_for(w: &mut Window, what: &str, limit: Duration, mut done: impl FnMut(&str) -> bool) {
    let start = Instant::now();
    loop {
        let trace = w.trace();
        if let Some(status) = w.child.try_wait().unwrap() {
            panic!("the window exited ({status}) waiting for {what}; its trace:\n{trace}");
        }
        let bad = abnormal(&trace);
        assert!(bad.is_empty(), "waiting for {what}: {bad:#?}\n{trace}");
        if done(&trace) {
            return;
        }
        assert!(
            start.elapsed() < limit,
            "{what}: not in {limit:?}; the trace:\n{trace}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
fn a_burst_of_parts_leaves_the_window_up() {
    if common::gpu().is_none() {
        return;
    }
    if !display() {
        let required = std::env::var("RYUCAD_REQUIRE_DISPLAY").is_ok_and(|v| !v.is_empty());
        assert!(!required, "no display, and RYUCAD_REQUIRE_DISPLAY is set");
        eprintln!("no display: skipped");
        return;
    }
    let spec = Spec {
        races: false,
        ..Spec::parts(env_num("RYUCAD_BURST_PARTS", 440) as usize)
    };
    let p = Project::named("burst");
    ryucad_beside_app();
    // The window's own runtime folder: its daemon, and its trace.
    let run = tempfile::tempdir().unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_ryucad-app"))
        .arg(&p.root)
        .env("RYUCAD_RUNTIME_DIR", run.path())
        .env("RYUCAD_APP_HEARTBEAT", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut w = Window {
        child,
        root: p.root.clone(),
        run,
    };
    wait_for(
        &mut w,
        "the window's first frames",
        Duration::from_secs(300),
        |t| last_beat(t).is_some_and(|b| b.frames > 0),
    );
    // The burst: every file at once, as an import's one commit writes them.
    let stage = tempfile::tempdir().unwrap();
    let summary = scale::write(stage.path(), &spec, &[]);
    let started = Instant::now();
    let files = copy_project(stage.path(), &p.root);
    eprintln!(
        "burst: {files} files, {} parts, {} assemblies, {} occurrences",
        summary.parts, summary.assemblies, summary.occurrences
    );
    // Loaded: every part listed, meshes held and none asked for, the same
    // for five heartbeats.
    let want = summary.parts as u64;
    let mut same: Option<(Beat, u32)> = None;
    wait_for(&mut w, "the burst to be drawn", WAIT, |t| {
        let Some(b) = last_beat(t) else { return false };
        let key = Beat { frames: 0, ..b };
        same = match same {
            Some((k, n)) if k == key => Some((k, n + 1)),
            _ => Some((key, 0)),
        };
        b.listed >= want && b.meshes > 0 && b.asked == 0 && same.is_some_and(|(_, n)| n >= 10)
    });
    eprintln!("burst: drawn in {:?}", started.elapsed());
    // Then watched a while longer, drawing what it has.
    let soak = Duration::from_secs(env_num("RYUCAD_BURST_SOAK", 20));
    let until = Instant::now() + soak;
    wait_for(&mut w, "the soak", soak + Duration::from_secs(60), |_| {
        Instant::now() >= until
    });
    let trace = w.trace();
    eprintln!("{trace}");
    assert!(w.child.try_wait().unwrap().is_none(), "{trace}");
    let b = last_beat(&trace).unwrap();
    assert!(b.listed >= want, "{b:?}");
}

#[test]
fn heartbeats_are_read() {
    let t = "1 2 started: x\n3 4 alive: frames 12 (the last began 0 s ago), parts 7/461, \
             meshes 5, asked 2, memory 300 MiB (peak 310 MiB)\n";
    assert_eq!(
        last_beat(t),
        Some(Beat {
            frames: 12,
            loaded: 7,
            listed: 461,
            meshes: 5,
            asked: 2
        })
    );
    assert!(abnormal(t).is_empty());
    assert_eq!(abnormal("1 2 panicked on thread 'main': x").len(), 1);
}
