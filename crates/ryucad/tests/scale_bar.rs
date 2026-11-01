//! Design 192's measurements at scale, over the benchmark project
//! (`ryucad::bench::scale`): 10,000 parts placing 100,000 occurrences,
//! generated afresh in a temporary folder (the race bodies' STEP file is
//! kept in the target folder after the first run). Through a real daemon,
//! as an agent or the app would ask: a cold rebuild of the root over an
//! empty disk cache, a daemon over the warm cache, a request that builds
//! nothing, one feature edited until `model.changed` reaches a
//! subscriber, the window's open (the root's report and every body's
//! display mesh), interference from
//! nothing and after one move, and the daemon's peak memory. Each prints
//! its number and the machine.
//!
//! Design 192's bars, as plan 7.D recorded them: a daemon over the warm
//! cache answers the root's `rebuild` within 10 s; a request that builds
//! nothing (`feature.list` of one part, on a connection already open)
//! within 20 ms; one feature edited reaches a subscriber within 300 ms
//! (the median of three edits); interference from nothing within 120 s,
//! after one move within 1 s; and the daemon's peak memory
//! stays within `session.max_memory` (4 GiB; measured on Linux only). The
//! cold rebuild is recorded, not barred. With `RYUCAD_BARS=1` they are
//! asserted as they are; otherwise ten times looser, which still catches
//! a cost that grows out of hand. Every bar is judged after the run, so
//! one that fails doesn't hide the others.
//!
//! Ignored, since it takes many minutes:
//!
//! ```text
//! RYUCAD_BARS=1 cargo test -p ryucad --features bench --release --test scale_bar -- --ignored --nocapture
//! ```
//!
//! `RYUCAD_SCALE_PARTS` makes a smaller project of the same shape (CI's is
//! 1,000 parts).

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

/// What the window's open costs as it asks since plan 7.A: `part.list`,
/// the report of the root it shows (every occurrence), then each body's
/// coarse display mesh (the view draws more than the app's
/// `COARSE_FIRST`), compact, 256 bodies to a request, one request after
/// another on one connection. Returns the seconds until the report, until
/// every mesh, how many bodies and how many bytes.
fn window_open(root: &Path) -> (f64, f64, usize, usize) {
    use ryucad::proto::{Request, Transfer};
    let mut conn = ryucad::client::Daemon::at(&ryucad_exe())
        .unwrap()
        .connect(root)
        .unwrap();
    let t = Instant::now();
    conn.call(&Request::PartList {}).unwrap().unwrap();
    let req = Request::Rebuild {
        part: Some("root".into()),
        rev: None,
    };
    let report = conn.call(&req).unwrap().unwrap();
    let shown = t.elapsed().as_secs_f64();
    // One occurrence of each body, as the window asks.
    let mut bodies = std::collections::BTreeMap::new();
    for o in report["parts"][0]["occurrences"].as_array().unwrap() {
        for s in o["solids"].as_array().unwrap() {
            let path = o["path"].as_str().unwrap().to_owned();
            bodies
                .entry(s["hash"].as_str().unwrap().to_owned())
                .or_insert(path);
        }
    }
    let paths: Vec<String> = bodies.values().cloned().collect();
    let mut bytes = 0;
    for batch in paths.chunks(256) {
        let req = Request::MeshGet {
            part: "root".into(),
            feature: None,
            features: batch.to_vec(),
            quality: Some("coarse".into()),
            transfer: Some(Transfer::Compact),
            rev: None,
        };
        bytes += conn
            .call_answer(&req)
            .unwrap()
            .unwrap()
            .binary
            .unwrap()
            .len();
    }
    (shown, t.elapsed().as_secs_f64(), bodies.len(), bytes)
}

/// How much slack the bars get: none where they are being recorded
/// (`RYUCAD_BARS` set), else ten times.
fn slack() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.0
    } else {
        10.0
    }
}

/// Design 192's bars as they were measured: each a name, the value, the
/// bar and its unit, judged together at the end.
#[derive(Default)]
struct Bars(Vec<(&'static str, f64, f64, &'static str)>);

impl Bars {
    fn judge(&mut self, what: &'static str, value: f64, bar: f64, unit: &'static str) {
        self.0.push((what, value, bar * slack(), unit));
    }

    fn assert(&self) {
        let failed: Vec<String> = self
            .0
            .iter()
            .filter(|(_, v, bar, _)| v > bar)
            .map(|(what, v, bar, unit)| format!("{what}: {v:.1} {unit}, bar {bar:.1} {unit}"))
            .collect();
        for (what, v, bar, unit) in &self.0 {
            let mark = if v > bar { "MISSED" } else { "met" };
            println!("  bar {mark}: {what} {v:.1} {unit} (bar {bar:.1} {unit})");
        }
        assert!(failed.is_empty(), "bars missed: {failed:?}");
    }
}

/// The best of five `req`s on one connection already open, in
/// milliseconds: what an agent's or the window's request costs, without a
/// CLI process starting and connecting.
fn held_ms(root: &Path, req: &ryucad::proto::Request) -> f64 {
    let mut conn = ryucad::client::Daemon::at(&ryucad_exe())
        .unwrap()
        .connect(root)
        .unwrap();
    (0..5)
        .map(|_| {
            let t = Instant::now();
            conn.call(req).unwrap().unwrap();
            t.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::INFINITY, f64::min)
}

/// Design 192's request that builds nothing: one part's feature list. Its
/// bar is the daemon's cost per request, "a request that builds nothing",
/// whatever the project's size (issue #25); `part.list` answers every one
/// of 10,222 parts (about 1.5 MB of JSON), so it measures the answer's
/// size, and is recorded beside it, not barred.
fn nothing(spec: &ryucad::bench::scale::Spec) -> ryucad::proto::Request {
    ryucad::proto::Request::FeatureList {
        part: ryucad::bench::scale::part_id(spec, 0, 1),
    }
}

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
    let mut bars = Bars::default();
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
    // still writing what the cold one wrote. The second is barred.
    let mut warm = 0.0;
    for _ in 0..2 {
        let again;
        (warm, again) = run.ok(&["rebuild", "root"]);
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
    bars.judge("a daemon over the warm cache", warm, 10.0, "s");
    run.ok(&["rebuild", "root"]);
    let held = held_ms(&run.root, &nothing(&spec));
    say(&format!(
        "a request that builds nothing (one part's features): {held:.1} ms on an open \
         connection; every part listed: {:.1} ms on an open connection, {:.0} ms through the CLI",
        held_ms(&run.root, &ryucad::proto::Request::PartList {}),
        run.best_ms(&["part", "list"])
    ));
    bars.judge("a request that builds nothing", held, 20.0, "ms");
    // A subscriber, then one feature edited: until it hears. Three edits,
    // each heard by a subscriber of its own; the bar is the median (an
    // agent edits again and again, and the first edit after subscribing
    // warms what the daemon keeps for subscribers).
    let part = ryucad::bench::scale::part_id(&spec, 0, 1);
    let mut heard_ms = Vec::new();
    for (k, z) in ["z=7.5", "z=8", "z=7.5"].into_iter().enumerate() {
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
        if k == 0 {
            say(&format!(
                "subscribing: {:.1} s; then a request that builds nothing: {:.1} ms on an open \
                 connection",
                t.elapsed().as_secs_f64(),
                held_ms(&run.root, &nothing(&spec))
            ));
        }
        let t = Instant::now();
        run.ok(&["feature", "edit", &part, "slab", "--param", z]);
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
        heard_ms.push(heard * 1e3);
    }
    heard_ms.sort_by(f64::total_cmp);
    bars.judge(
        "one feature edited until a subscriber hears (the median of three)",
        heard_ms[1],
        300.0,
        "ms",
    );
    // What the window asks for when it opens: with no meshes made yet,
    // then from a new daemon over the cache they were written to.
    for when in ["meshed now", "from the cache"] {
        let (shown, meshed, bodies, bytes) = window_open(&run.root);
        say(&format!(
            "the window's open ({when}): the root's report in {shown:.1} s, every one of its \
             {bodies} bodies' coarse meshes in {meshed:.1} s, {:.0} MB (peak {:.0} MiB)",
            bytes as f64 / 1e6,
            run.peak_mib()
        ));
        run.ok(&["daemon", "stop"]);
    }
    run.ok(&["rebuild", "root"]);
    // Interference, from nothing and after one move.
    let (interfere, v) = run.ok(&["interference", "root"]);
    say(&format!(
        "interference from nothing: {interfere:.1} s ({} pairs)",
        v["result"]["pairs"]
    ));
    bars.judge("interference from nothing", interfere, 120.0, "s");
    let sub = ryucad::bench::scale::sub_id(0);
    run.ok(&["assembly", "move", &sub, "n34", "--translate", "0,0,1mm"]);
    let (again, _) = run.ok(&["interference", "root"]);
    say(&format!("interference after one move: {again:.1} s"));
    bars.judge("interference after one move", again, 1.0, "s");
    let peak = run.peak_mib();
    say(&format!("peak memory: {peak:.0} MiB"));
    if peak > 0.0 {
        // Memory isn't a machine's speed: no slack.
        bars.0
            .push(("the daemon's peak memory", peak, 4096.0, "MiB"));
    }
    let (_, status) = run.ok(&["daemon", "status"]);
    say(&format!(
        "the daemon's session: {}",
        status["result"]["session"]
    ));
    run.ok(&["daemon", "stop"]);
    bars.assert();
}
