//! #849's bar (design 698, plan 33.A): a script's single writes (#810's
//! project, `ryucad::bench::scripted`: 56 parts, 209 instances, about
//! 1,000 calls) with nobody watching, with a bare `ryucad watch`
//! subscriber, and with a window's `Workspace` attached headless and
//! polled as a window's frames poll it. Each configuration runs the calls
//! through one `ryucad session --stdio` (the daemon's own cost), and the
//! first 200 as a process each (start, hello and one request, as #849
//! ran them), each run on a project, daemon and disk cache of its own.
//! Every configuration must build the same model and log the same
//! activity. It prints each run's time and the daemon's `--timings`, with
//! the machine, for `ryucad-session.md`. Release builds only: run it on a
//! quiet box (load below a quarter of the cores). The bar is the median of
//! three rounds.

mod common;

use common::{Project, ryucad, ryucad_cmd};
use ryucad::bench::scripted::Script;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// The calls run one process each.
const SLICE: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Config {
    Alone,
    Watched,
    Window,
}

const CONFIGS: [Config; 3] = [Config::Alone, Config::Watched, Config::Window];

impl Config {
    fn name(self) -> &'static str {
        match self {
            Config::Alone => "no subscriber",
            Config::Watched => "a watch subscriber",
            Config::Window => "a window's workspace",
        }
    }
}

/// What one run left: its time, the daemon's timings, what it built and
/// what it logged.
struct Run {
    secs: f64,
    timings: Value,
    model: BTreeMap<String, Value>,
    activity: Vec<Value>,
}

fn cpu() -> String {
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string("/proc/cpuinfo")
        && let Some(l) = s.lines().find(|l| l.starts_with("model name"))
    {
        return l.split(':').nth(1).unwrap_or_default().trim().to_owned();
    }
    #[cfg(target_os = "macos")]
    if let Ok(o) = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
    {
        return String::from_utf8_lossy(&o.stdout).trim().to_owned();
    }
    #[cfg(windows)]
    if let Ok(v) = std::env::var("PROCESSOR_IDENTIFIER") {
        return v;
    }
    "an unknown CPU".to_owned()
}

fn machine() -> String {
    format!(
        "{} {}, {}, {} cores",
        std::env::consts::OS,
        std::env::consts::ARCH,
        cpu(),
        std::thread::available_parallelism().map_or(0, |n| n.get())
    )
}

/// `ryucad` in `root`, with the run's own disk cache.
fn cmd(root: &Path, cache: &Path) -> Command {
    let mut c = ryucad_cmd(&ryucad().exe);
    c.current_dir(root).env("RYUCAD_CACHE_DIR", cache);
    c
}

fn call_line(
    stdin: &mut impl Write,
    out: &mut impl BufRead,
    id: usize,
    method: &str,
    params: &Value,
) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    writeln!(stdin, "{line}").unwrap();
    stdin.flush().unwrap();
    let mut answer = String::new();
    out.read_line(&mut answer).unwrap();
    let v: Value = serde_json::from_str(&answer).unwrap();
    v.get("result")
        .cloned()
        .unwrap_or_else(|| panic!("{method}: {v}"))
}

/// `calls` through one `ryucad session --stdio`, each of which must
/// succeed; their results.
fn through_stdio(root: &Path, cache: &Path, calls: &[(String, Value)]) -> Vec<Value> {
    let mut child = cmd(root, cache)
        .args(["session", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let results = calls
        .iter()
        .enumerate()
        .map(|(i, (m, p))| call_line(&mut stdin, &mut out, i, m, p))
        .collect();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    results
}

/// One call's result, through a session of its own.
fn one(root: &Path, cache: &Path, method: &str, params: Value) -> Value {
    through_stdio(root, cache, &[(method.to_owned(), params)]).remove(0)
}

/// Each feature of each part, with its hash: what the run built.
fn built(root: &Path, cache: &Path) -> BTreeMap<String, Value> {
    let r = one(root, cache, "rebuild", json!({}));
    r["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let features: Vec<Value> = p["features"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| json!([f["id"], f["hash"]]))
                .collect();
            (p["part"].as_str().unwrap().to_owned(), json!(features))
        })
        .collect()
}

/// Each activity entry's kind, method and params: what the run logged.
fn logged(root: &Path, cache: &Path) -> Vec<Value> {
    let r = one(root, cache, "activity.log", json!({"limit": 5000}));
    r["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| json!([e["kind"], e["method"], e["params"]]))
        .collect()
}

/// Waits until the daemon lists at least one participant.
fn attached(root: &Path, cache: &Path) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(60) {
        let out = cmd(root, cache)
            .args(["daemon", "status"])
            .output()
            .unwrap();
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        if v["result"]["participants"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("nobody attached");
}

/// A window's workspace on `root`, polled every 16 ms on a thread of its
/// own until `stop`. It diagnoses once as a window does when it opens,
/// says so on `ready` once idle, and shows `top` once the daemon lists
/// it. Returns how many frames it polled.
fn window(
    root: PathBuf,
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<()>,
) -> std::thread::JoinHandle<u64> {
    std::thread::spawn(move || {
        let mut ws = common::open_editing(&root, ryucad(), "Dana", Arc::new(|| {})).unwrap();
        ws.diagnose();
        assert!(ws.wait_until(Duration::from_secs(60), |w| w.idle()));
        ready.send(()).unwrap();
        let (mut frames, mut focused) = (0u64, false);
        while !stop.load(Ordering::SeqCst) {
            ws.pump();
            ws.tick(Instant::now());
            ws.load_wanted();
            if !focused && ws.model.listed.contains_key("top") {
                ws.set_focus(Some("top".to_owned()));
                focused = true;
            }
            frames += 1;
            std::thread::sleep(Duration::from_millis(16));
        }
        frames
    })
}

/// `calls` in `config`: through one stdio session, or with `each`, one
/// process a call.
fn run(config: Config, calls: &[(String, Value)], each: bool) -> Run {
    let p = Project::new();
    let cache = tempfile::tempdir().unwrap();
    let (root, c) = (p.root.clone(), cache.path());
    // The daemon starts here, with this run's own disk cache.
    let _ = cmd(&root, c).args(["daemon", "stop"]).output();
    assert!(
        cmd(&root, c)
            .args(["part", "list"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let stop = Arc::new(AtomicBool::new(false));
    let mut watch: Option<Child> = None;
    let mut win = None;
    match config {
        Config::Alone => {}
        Config::Watched => {
            let child = cmd(&root, c)
                .args(["watch", "--count", "1000000", "--timeout", "7200"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            watch = Some(child);
            attached(&root, c);
        }
        Config::Window => {
            let (tx, rx) = mpsc::channel();
            win = Some(window(root.clone(), Arc::clone(&stop), tx));
            rx.recv_timeout(Duration::from_secs(120)).unwrap();
        }
    }
    let t = Instant::now();
    if each {
        for call in calls {
            through_stdio(&root, c, std::slice::from_ref(call));
        }
    } else {
        through_stdio(&root, c, calls);
    }
    let secs = t.elapsed().as_secs_f64();
    // The window stops polling first: `daemon status` reads the timings
    // only when nobody holds or waits for the lock.
    stop.store(true, Ordering::SeqCst);
    if let Some(w) = win {
        let frames = w.join().unwrap();
        assert!(frames > 0);
    }
    let timings = timings(&root, c);
    if let Some(mut w) = watch {
        let _ = w.kill();
        let _ = w.wait();
    }
    let model = built(&root, c);
    let activity = logged(&root, c);
    let _ = cmd(&root, c).args(["daemon", "stop"]).output();
    Run {
        secs,
        timings,
        model,
        activity,
    }
}

/// The daemon's timings, asked until it isn't busy.
fn timings(root: &Path, cache: &Path) -> Value {
    let t = Instant::now();
    loop {
        let out = cmd(root, cache)
            .args(["daemon", "status", "--timings"])
            .output()
            .unwrap();
        let status: Value = serde_json::from_slice(&out.stdout).unwrap();
        let timings = &status["result"]["timings"];
        if timings.is_object() {
            return timings.clone();
        }
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "always busy: {status}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One line per method, the busiest first.
fn methods(t: &Value) -> String {
    let mut rows: Vec<(&String, &Value)> = t["methods"]
        .as_object()
        .map(|m| m.iter().collect())
        .unwrap_or_default();
    rows.sort_by(|a, b| {
        let held = |v: &Value| {
            v["held_ms"].as_f64().unwrap_or(0.0) + v["publish_ms"].as_f64().unwrap_or(0.0)
        };
        held(b.1).total_cmp(&held(a.1))
    });
    rows.iter()
        .map(|(m, v)| {
            format!(
                "    {m}: {} requests, held {:.0} ms, waited {:.0} ms, {} publishes {:.0} ms",
                v["count"],
                v["held_ms"].as_f64().unwrap_or(0.0),
                v["wait_ms"].as_f64().unwrap_or(0.0),
                v["publishes"],
                v["publish_ms"].as_f64().unwrap_or(0.0)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn report(how: &str, n: usize, runs: &[Run; 3]) {
    let alone = runs[0].secs;
    println!("#849 on {}: {how} ({n} calls)", machine());
    for (c, r) in CONFIGS.iter().zip(runs) {
        println!("  {}: {:.2} s ({:.2}x)", c.name(), r.secs, r.secs / alone);
        println!("{}", methods(&r.timings));
    }
}

fn same(how: &str, runs: &[Run; 3]) {
    for (c, r) in CONFIGS.iter().zip(runs).skip(1) {
        assert!(
            r.model == runs[0].model,
            "{how}: {} built another model",
            c.name()
        );
        assert!(
            r.activity == runs[0].activity,
            "{how}: {} logged other activity",
            c.name()
        );
    }
}

/// The window's run against the unwatched one: 1.25 with
/// `RYUCAD_BARS=1` on a quiet box (design 698), else 2.0, which still
/// catches #849's 3x on a noisy box (question 1016).
fn ratio_bar() -> f64 {
    if std::env::var_os("RYUCAD_BARS").is_some() {
        1.25
    } else {
        2.0
    }
}

/// `daemon.publish_max_ms`, as the run's daemon read it.
fn publish_max_ms() -> f64 {
    ryucad::config::user("daemon.publish_max_ms")
        .as_f64()
        .unwrap_or(1000.0)
}

fn publishes(t: &Value) -> u64 {
    t["methods"]["daemon.publish"]["publishes"]
        .as_u64()
        .unwrap_or(0)
}

/// Rounds of the three configurations: the bar is judged on the median
/// round, since a single unwatched run varies by a quarter on a shared
/// box.
const ROUNDS: usize = 3;

/// The median of `xs`.
fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}

#[test]
#[cfg_attr(debug_assertions, ignore = "a release build's bar")]
fn a_window_costs_a_script_little() {
    let calls = Script::new(56, 209).singles();
    let slice = &calls[..SLICE.min(calls.len())];
    let mut ratios: Vec<(&str, Vec<f64>)> =
        vec![("stdio", Vec::new()), ("one process a call", Vec::new())];
    let max = publish_max_ms();
    for round in 1..=ROUNDS {
        println!("round {round} of {ROUNDS}");
        let stdio = CONFIGS.map(|c| run(c, &calls, false));
        report("through one stdio session", calls.len(), &stdio);
        let each = CONFIGS.map(|c| run(c, slice, true));
        report("one process a call", slice.len(), &each);
        same("stdio", &stdio);
        same("one process a call", &each);
        ratios[0].1.push(stdio[2].secs / stdio[0].secs);
        ratios[1].1.push(each[2].secs / each[0].secs);
        // At most one publish per `publish_max_ms` of the stdio run, plus
        // one.
        for (c, r) in CONFIGS.iter().zip(&stdio).skip(1) {
            let allowed = (r.secs * 1000.0 / max).floor() as u64 + 1;
            assert!(
                publishes(&r.timings) <= allowed,
                "{}: {} publishes in {:.1} s",
                c.name(),
                publishes(&r.timings),
                r.secs
            );
        }
    }
    let bar = ratio_bar();
    for (how, rs) in &ratios {
        let m = median(rs.clone());
        let all: Vec<String> = rs.iter().map(|r| format!("{r:.2}x")).collect();
        println!(
            "#849 {how}: a window's runs took {} the unwatched ones; median {m:.2}x (bar {bar})",
            all.join(", ")
        );
    }
    for (how, rs) in &ratios {
        let m = median(rs.clone());
        assert!(
            m <= bar,
            "{how}: a window's median run took {m:.2}x the unwatched one (bar {bar})"
        );
    }
}
