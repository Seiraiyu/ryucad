//! Phase 0 benchmarks: the per-request freshness check, a cold rebuild,
//! and a CLI command through the daemon, on a 100-feature project.

use divan::Bencher;
use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::{handle, init};
use ryucad::session::Session;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn main() {
    divan::main();
}

/// A project with one part of 100 features, all five types.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    handle(&mut s, Request::PartCreate { part: "p".into() }).unwrap();
    let kinds = [
        ("box", json!({"x": 10, "y": 20, "z": 30})),
        ("cylinder", json!({"radius": 5, "height": 10})),
        ("cone", json!({"radius": 5, "height": 10, "top_radius": 2})),
        ("sphere", json!({"radius": 5})),
        ("torus", json!({"major_radius": 10, "minor_radius": 2})),
    ];
    for i in 0..100 {
        let (ty, params) = &kinds[i % 5];
        let req = Request::from_rpc(
            "feature.add",
            Some(json!({"part": "p", "type": ty, "params": params,
                        "placement": {"origin": [i, 0, 0], "z": [0, 1, 1]}})),
        )
        .unwrap();
        handle(&mut s, req).unwrap();
    }
    // Let modification times age past the window in which they aren't
    // trusted, so the benchmark measures the steady state.
    std::thread::sleep(Duration::from_millis(2100));
    dir
}

#[divan::bench(sample_count = 200)]
fn freshness_check_100_features(bencher: Bencher) {
    let dir = project();
    let mut s = Session::open(dir.path()).unwrap();
    std::thread::sleep(Duration::from_millis(2100));
    s.refresh().unwrap();
    bencher.bench_local(|| s.refresh().unwrap());
}

#[divan::bench(sample_count = 20)]
fn cold_rebuild_100_features(bencher: Bencher) {
    let dir = project();
    let part = Id::parse("p", "part").unwrap();
    bencher.bench_local(|| {
        let mut s = Session::open(dir.path()).unwrap();
        s.rebuilt(&part).unwrap().hash.clone()
    });
}

fn cli(root: &Path, run: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_ryucad"))
        .args(args)
        .current_dir(root)
        .env("RYUCAD_RUNTIME_DIR", run)
        .env("RYUCAD_IDLE_TIMEOUT", "60")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[divan::bench(sample_count = 50)]
fn cli_feature_list_through_the_daemon(bencher: Bencher) {
    let dir = project();
    let run = tempfile::tempdir().unwrap();
    let run = run.path().join("run");
    cli(dir.path(), &run, &["feature", "list", "p"]);
    bencher.bench_local(|| cli(dir.path(), &run, &["feature", "list", "p"]));
    cli(dir.path(), &run, &["daemon", "stop"]);
}
