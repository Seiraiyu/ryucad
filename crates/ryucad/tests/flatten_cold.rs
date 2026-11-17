//! A cold flattened import, timed (#955's review of plan 33.C): the
//! committed OCCT assembly and the seeded board of 40 components placed
//! 1,000 times, each imported flattened and rebuilt in a fresh process
//! with no disk cache. Ignored: run one at a time, release, with
//! `--ignored --nocapture`, to compare builds.

mod bar_project;
mod common;

use common::must;
use ryucad::doc::id::Id;
use serde_json::json;
use std::time::Instant;

fn cold(path: &std::path::Path, what: &str) {
    let (_dir, mut s) = common::project();
    let t = Instant::now();
    must(
        &mut s,
        "import",
        json!({"path": path, "flatten": true, "part": "board"}),
    );
    let r = s
        .rebuilt(&Id::parse("board", "part").unwrap())
        .unwrap()
        .clone();
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(r.failed(), 0);
    println!(
        "cold flattened {what} on {}: {} features, {secs:.3} s",
        bar_project::machine(),
        r.features.len()
    );
}

#[test]
#[ignore = "a timing, one per process"]
fn cold_assembly() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ryustep/tests/data/occt/assembly.stp");
    cold(&p, "assembly.stp");
}

#[test]
#[ignore = "a timing, one per process"]
fn cold_board() {
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("board.step");
    std::fs::write(&file, common::board::board(40, 1000)).unwrap();
    cold(&file, "board (40 x 1000)");
}
