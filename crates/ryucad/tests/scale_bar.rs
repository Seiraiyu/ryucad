//! Design 192's measurements at scale, over the benchmark project
//! (`ryucad::bench::scale`): 10,000 parts placing 100,000 occurrences,
//! generated afresh in a temporary folder (the race bodies' STEP file is
//! kept in the target folder after the first run). Through a real daemon,
//! as an agent or the app would ask: a cold rebuild of the root over an
//! empty disk cache, a daemon over the warm cache, a request that builds
//! nothing, one feature edited until `model.changed` reaches a
//! subscriber, what the window's open asks for, interference from
//! nothing and after one move, and the daemon's peak memory. Each prints
//! its number and the machine. Plan 7.D records the bars; until then
//! nothing here is asserted but that the answers are right.
//!
//! Ignored, since it takes many minutes:
//!
//! ```text
//! cargo test -p ryucad --features bench --release --test scale_bar -- --ignored --nocapture
//! ```
//!
//! `RYUCAD_SCALE_PARTS` makes a smaller project of the same shape.

mod bar_project;

use bar_project::machine;
use ryucad::bench::scale::{Spec, race_file, write};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn spec() -> Spec {
    match std::env::var("RYUCAD_SCALE_PARTS") {
        Ok(n) => Spec::parts(n.parse().expect("a number of parts")),
        Err(_) => Spec::full(),
    }
}

/// The race file, kept in the target folder after the first run.
fn race(n: usize) -> Vec<u8> {
    let kept = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("scale-race-{n}.step"));
    if let Ok(b) = std::fs::read(&kept) {
        return b;
    }
    let b = race_file(n);
    std::fs::write(&kept, &b).unwrap();
    b
}

struct Run {
    dir: PathBuf,
    root: PathBuf,
}

impl Run {
    fn cmd(&self, args: &[&str]) -> std::process::Command {
        let mut c = std::process::Command::new(ryucad_exe());
        c.current_dir(&self.root)
            .args(args)
            .env("RYUCAD_RUNTIME_DIR", self.dir.join("run"))
            .env("RYUCAD_CONFIG_DIR", self.dir.join("config"))
            .env("RYUCAD_CACHE_DIR", self.dir.join("cache"))
            .env("RYUCAD_IDLE_TIMEOUT", "3600");
        c
    }

    /// Runs the CLI: how long it took, and its answer.
    fn ryucad(&self, args: &[&str]) -> (f64, Value) {
        let t = Instant::now();
        let out = self.cmd(args).output().unwrap();
        let v: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("{args:?}: {}", String::from_utf8_lossy(&out.stderr)));
        (t.elapsed().as_secs_f64(), v)
    }

    fn ok(&self, args: &[&str]) -> (f64, Value) {
        let (t, v) = self.ryucad(args);
        assert_eq!(v["ok"], true, "{args:?}: {v}");
        (t, v)
    }

    /// The daemon's peak resident memory, in MiB (Linux; 0 elsewhere).
    fn peak_mib(&self) -> f64 {
        let (_, v) = self.ok(&["daemon", "status"]);
        let pid = v["result"]["pid"].as_u64().unwrap();
        std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|s| {
                let l = s.lines().find(|l| l.starts_with("VmHWM:"))?;
                l.split_whitespace().nth(1)?.parse::<f64>().ok()
            })
            .map_or(0.0, |kb| kb / 1024.0)
    }

    /// The best of five of `args`, in milliseconds.
    fn best_ms(&self, args: &[&str]) -> f64 {
        (0..5)
            .map(|_| self.ok(args).0 * 1e3)
            .fold(f64::INFINITY, f64::min)
    }
}

/// What the window's open costs as it asks today: `part.list`, then a
/// `rebuild` of every part, one request after another on one connection.
/// Only the first [`OPEN_SAMPLE`] parts are asked for (every one takes
/// hours at 10,000); returns the seconds per request and how many the
/// open would send.
fn window_open(root: &Path) -> (f64, usize) {
    use ryucad::proto::Request;
    let mut conn = ryucad::client::Daemon::at(&ryucad_exe())
        .unwrap()
        .connect(root)
        .unwrap();
    let list = conn.call(&Request::PartList {}).unwrap().unwrap();
    let parts: Vec<String> = list["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["part"].as_str().unwrap().to_owned())
        .collect();
    let t = Instant::now();
    for p in parts.iter().take(OPEN_SAMPLE) {
        let req = Request::Rebuild {
            part: Some(p.clone()),
            rev: None,
        };
        conn.call(&req).unwrap().unwrap();
    }
    let each = t.elapsed().as_secs_f64() / OPEN_SAMPLE.min(parts.len()) as f64;
    (each, parts.len() + 1)
}

/// How many of the window's requests are timed.
const OPEN_SAMPLE: usize = 200;

fn ryucad_exe() -> PathBuf {
    assert_cmd::cargo::cargo_bin("ryucad")
}

/// Prints a measurement as soon as it is taken: a run may be cut short.
fn say(what: &str) {
    use std::io::Write;
    println!("  {what}");
    let _ = std::io::stdout().flush();
}

#[test]
#[ignore = "many minutes: run by name"]
fn the_benchmark_project_measured() {
    let spec = spec();
    let race = race(spec.imports());
    let dir = tempfile::tempdir().unwrap();
    let run = Run {
        dir: dir.path().to_path_buf(),
        root: dir.path().join("p"),
    };
    // The window's connection below is this process's: it finds the same
    // daemon.
    // SAFETY: this is the binary's one test, so no other thread reads the
    // environment.
    unsafe {
        std::env::set_var("RYUCAD_RUNTIME_DIR", run.dir.join("run"));
        std::env::set_var("RYUCAD_CONFIG_DIR", run.dir.join("config"));
        std::env::set_var("RYUCAD_CACHE_DIR", run.dir.join("cache"));
    }
    let t = Instant::now();
    let summary = write(&run.root, &spec, &race);
    println!(
        "the benchmark project ({} parts, {} assemblies, {} occurrences, {} features, {} \
         files), on {}, {} rayon threads:",
        summary.parts,
        summary.assemblies,
        summary.occurrences,
        summary.features,
        summary.files,
        machine(),
        rayon::current_num_threads(),
    );
    say(&format!("generated in {:.1} s", t.elapsed().as_secs_f64()));
    // Design 192's bar: the daemon within a budget of 4 GiB.
    let max = (4u64 << 30).to_string();
    run.ok(&["config", "set", "session.max_memory", &max, "--no-daemon"]);
    // A daemon over an empty cache builds everything.
    let (cold, first) = run.ok(&["rebuild", "root"]);
    say(&format!(
        "a cold daemon's rebuild of the root: {cold:.1} s (peak {:.0} MiB)",
        run.peak_mib()
    ));
    run.ok(&["daemon", "stop"]);
    // One over the warm cache, twice: the first may wait on the disk
    // still writing what the cold one wrote.
    for _ in 0..2 {
        let (warm, again) = run.ok(&["rebuild", "root"]);
        assert_eq!(
            again["result"]["parts"][0]["hash"],
            first["result"]["parts"][0]["hash"]
        );
        say(&format!(
            "a daemon over the warm cache: {warm:.1} s (peak {:.0} MiB)",
            run.peak_mib()
        ));
        run.ok(&["daemon", "stop"]);
    }
    run.ok(&["rebuild", "root"]);
    say(&format!(
        "a request that builds nothing: {:.0} ms",
        run.best_ms(&["part", "list"])
    ));
    // A subscriber, then one feature edited: until it hears.
    let watch = run
        .cmd(&["watch", "--topic", "model", "--timeout", "3600"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let t = Instant::now();
    // Subscribing settles the session: once the daemon answers again,
    // that is done.
    std::thread::sleep(Duration::from_millis(500));
    run.ok(&["daemon", "status"]);
    run.ok(&["part", "list"]);
    say(&format!(
        "subscribing: {:.1} s; then a request that builds nothing: {:.0} ms",
        t.elapsed().as_secs_f64(),
        run.best_ms(&["part", "list"])
    ));
    let part = ryucad::bench::scale::part_id(&spec, 0, 1);
    let t = Instant::now();
    run.ok(&["feature", "edit", &part, "slab", "--param", "z=7.5"]);
    let edit = t.elapsed().as_secs_f64();
    let out = watch.wait_with_output().unwrap();
    let heard = t.elapsed().as_secs_f64();
    let note: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(note["ok"], true, "{note}");
    // Issue #91: what a request after an edit waits for.
    let (next, _) = run.ok(&["part", "list"]);
    say(&format!(
        "one feature edited: {:.0} ms, heard by a subscriber after {:.0} ms; the next \
         request {:.0} ms",
        edit * 1e3,
        heard * 1e3,
        next * 1e3
    ));
    // What the window asks for when it opens.
    let (each, requests) = window_open(&run.root);
    say(&format!(
        "the window's open: {requests} requests at {:.0} ms each, so {:.0} s (peak {:.0} MiB)",
        each * 1e3,
        each * requests as f64,
        run.peak_mib()
    ));
    // Interference, from nothing and after one move.
    let (interfere, v) = run.ok(&["interference", "root"]);
    say(&format!(
        "interference from nothing: {interfere:.1} s ({} pairs)",
        v["result"]["pairs"]
    ));
    let sub = ryucad::bench::scale::sub_id(0);
    run.ok(&["assembly", "move", &sub, "n34", "--translate", "0,0,1mm"]);
    let (again, _) = run.ok(&["interference", "root"]);
    say(&format!("interference after one move: {again:.1} s"));
    say(&format!("peak memory: {:.0} MiB", run.peak_mib()));
    let (_, status) = run.ok(&["daemon", "status"]);
    say(&format!(
        "the daemon's session: {}",
        status["result"]["session"]
    ));
    run.ok(&["daemon", "stop"]);
}
