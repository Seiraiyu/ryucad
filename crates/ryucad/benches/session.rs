//! Benchmarks on a 100-feature project: the per-request freshness check, a
//! cold rebuild, a CLI command through the daemon, a watched daemon's
//! periodic check, display meshes, a render, and a broadcast to
//! subscribers. Then a solid part (a plate with rounded corners,
//! counterbored holes patterned 10 × 5 and a chamfered rim): its cold
//! rebuild, and the rebuild after its pattern's spacing changes.

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
    handle(
        &mut s,
        Request::PartCreate {
            part: "p".into(),
            kind: None,
        },
    )
    .unwrap();
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

#[divan::bench(sample_count = 100)]
fn watched_tick_100_features(bencher: Bencher) {
    let dir = project();
    let mut live = ryucad::live::Live::new();
    let _sub = live.hub.subscribe("p-1", &[]);
    let mut slot = Some(Session::open(dir.path()).unwrap());
    std::thread::sleep(Duration::from_millis(2100));
    ryucad::server::settle(&mut slot, &mut live, dir.path());
    bencher.bench_local(|| ryucad::server::tick(&mut slot, &mut live, dir.path()));
}

fn mesh_request() -> Request {
    Request::from_rpc(
        "mesh.get",
        Some(json!({"part": "p", "quality": "normal", "transfer": "binary"})),
    )
    .unwrap()
}

#[divan::bench(sample_count = 5)]
fn mesh_get_cold_100_features(bencher: Bencher) {
    let dir = project();
    bencher.bench_local(|| {
        let mut slot = None;
        ryucad::server::handle_safely(&mut slot, None, None, dir.path(), mesh_request()).unwrap()
    });
}

#[divan::bench(sample_count = 50)]
fn mesh_get_cached_100_features(bencher: Bencher) {
    let dir = project();
    let mut slot = None;
    ryucad::server::handle_safely(&mut slot, None, None, dir.path(), mesh_request()).unwrap();
    bencher.bench_local(|| {
        ryucad::server::handle_safely(&mut slot, None, None, dir.path(), mesh_request()).unwrap()
    });
}

#[divan::bench(sample_count = 10)]
fn render_1024x768_100_features(bencher: Bencher) {
    let dir = project();
    let req = || {
        Request::from_rpc(
            "render",
            Some(json!({"part": "p", "width": 1024, "height": 768})),
        )
        .unwrap()
    };
    let mut slot = None;
    ryucad::server::handle_safely(&mut slot, None, None, dir.path(), req()).unwrap();
    bencher.bench_local(|| {
        ryucad::server::handle_safely(&mut slot, None, None, dir.path(), req()).unwrap()
    });
}

#[divan::bench(sample_count = 200)]
fn broadcast_to_8_subscribers(bencher: Bencher) {
    let mut hub = ryucad::notify::Hub::default();
    let subs: Vec<_> = (0..8)
        .map(|i| hub.subscribe(&format!("p-{i}"), &[]))
        .collect();
    let drains: Vec<_> = subs
        .iter()
        .map(|s| {
            let s = std::sync::Arc::clone(s);
            std::thread::spawn(move || while s.next().is_some() {})
        })
        .collect();
    let params = json!({"participant": {"id": "p-1", "selection": ["a/b/0123456789abcdef0123456789abcdef"]}});
    bencher.bench_local(|| {
        hub.send(
            ryucad::notify::Topic::Participants,
            "participant.changed",
            &params,
        )
    });
    subs.iter().for_each(|s| s.close());
    drains.into_iter().for_each(|d| d.join().unwrap());
}

fn call(s: &mut Session, method: &str, params: serde_json::Value) -> serde_json::Value {
    handle(s, Request::from_rpc(method, Some(params)).unwrap()).unwrap()
}

/// The name of the edge of an `inspect` body between the vertices at `a`
/// and `b` (meters).
fn edge_at(body: &serde_json::Value, a: [f64; 3], b: [f64; 3]) -> String {
    let at = |p: [f64; 3]| {
        body["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| (0..3).all(|k| (v["point"][k].as_f64().unwrap() - p[k]).abs() < 1e-9))
            .map(|v| v["name"].clone())
            .unwrap()
    };
    let (a, b) = (at(a), at(b));
    body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| (e["start"] == a && e["end"] == b) || (e["start"] == b && e["end"] == a))
        .map(|e| format!("p/b/{}", e["name"].as_str().unwrap()))
        .unwrap()
}

/// A plate 120 × 60 × 10 mm with its four vertical edges rounded, a sketch
/// on its top with one point, a counterbored hole there patterned 10 × 5,
/// and its top rim chamfered.
fn solid_part() -> (tempfile::TempDir, Session) {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path(), None).unwrap();
    let mut s = Session::open(dir.path()).unwrap();
    handle(
        &mut s,
        Request::PartCreate {
            part: "p".into(),
            kind: None,
        },
    )
    .unwrap();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "box", "id": "b", "params": {"x": 120, "y": 60, "z": 10}}),
    );
    let b = call(&mut s, "inspect", json!({"part": "p", "feature": "b"}))["bodies"][0].clone();
    let corners = [[0.0, 0.0], [0.12, 0.0], [0.12, 0.06], [0.0, 0.06]];
    let edges: Vec<String> = corners
        .iter()
        .map(|[x, y]| edge_at(&b, [*x, *y, 0.0], [*x, *y, 0.01]))
        .collect();
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "fillet", "id": "round", "radius": 5, "edges": edges}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "sketch", "id": "t", "placement": {"origin": [0, 0, 10]}}),
    );
    call(
        &mut s,
        "sketch.edit",
        json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [15, 10]},
            {"op": "add", "type": "fix", "a": "p1"}
        ]}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "hole", "id": "h", "sketch": "t", "form": "counterbore",
               "diameter": 3.4, "counterbore": {"diameter": 6.5, "depth": 3.4},
               "depth": "through_all"}),
    );
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "pattern", "id": "grid", "features": ["h"],
               "direction": "x", "spacing": 10, "count": 10,
               "direction2": "y", "spacing2": 10, "count2": 5}),
    );
    let b = call(&mut s, "inspect", json!({"part": "p", "feature": "grid"}))["bodies"][0].clone();
    let rim = edge_at(&b, [0.005, 0.0, 0.01], [0.115, 0.0, 0.01]);
    call(
        &mut s,
        "feature.add",
        json!({"part": "p", "type": "chamfer", "id": "rim", "form": "equal", "d": 1,
               "edges": [rim]}),
    );
    let part = Id::parse("p", "part").unwrap();
    assert_eq!(s.rebuilt(&part).unwrap().failed(), 0);
    (dir, s)
}

#[divan::bench(sample_count = 10)]
fn cold_rebuild_solid_part(bencher: Bencher) {
    let (dir, _) = solid_part();
    let part = Id::parse("p", "part").unwrap();
    bencher.bench_local(|| {
        let mut s = Session::open(dir.path()).unwrap();
        s.rebuilt(&part).unwrap().hash.clone()
    });
}

#[divan::bench(sample_count = 10)]
fn pattern_spacing_edit_solid_part(bencher: Bencher) {
    let (_dir, mut s) = solid_part();
    let part = Id::parse("p", "part").unwrap();
    let mut spacing = 10.0;
    bencher.bench_local(|| {
        spacing = if spacing == 10.0 { 10.5 } else { 10.0 };
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "grid", "spacing": spacing}),
        );
        s.rebuilt(&part).unwrap().hash.clone()
    });
}
