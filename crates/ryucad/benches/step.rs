//! STEP in projects (design 120's Performance): a corpus file
//! (`nist_ctc_01`, one healed body) imported into a new project, the same
//! file imported again over itself (`import --update` with unchanged
//! bytes: planned, and nothing changes), and the part exported. Opened
//! files are kept for the process, so after the first sample an import
//! heals but doesn't parse. The machine is printed first.

#[path = "../tests/bar_project/mod.rs"]
mod bar_project;

use bar_project::{machine, must};
use divan::Bencher;
use ryucad::session::Session;
use serde_json::json;
use std::path::PathBuf;

fn main() {
    println!("STEP benchmarks on {}", machine());
    divan::main();
}

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ryustep/tests/corpus/nist_ctc_01_asme1_rd.stp")
}

/// A new project in `dir`.
fn project(dir: &std::path::Path) -> Session {
    ryucad::server::init(dir, None).unwrap();
    Session::open(dir).unwrap()
}

#[divan::bench(sample_count = 10)]
fn import_into_a_new_project(bencher: Bencher) {
    bencher
        .with_inputs(|| {
            let dir = tempfile::tempdir().unwrap();
            let s = project(dir.path());
            (dir, s)
        })
        .bench_local_values(|(dir, mut s)| {
            let v = must(&mut s, "import", json!({"path": corpus()}));
            assert_eq!(v["parts"][0]["features"][0]["ok"], true, "{v}");
            (dir, s)
        });
}

#[divan::bench(sample_count = 10)]
fn reimport_unchanged(bencher: Bencher) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = project(dir.path());
    let v = must(&mut s, "import", json!({"path": corpus()}));
    let stored = v["file"]["path"].as_str().unwrap().to_owned();
    bencher.bench_local(|| {
        let v = must(
            &mut s,
            "import",
            json!({"path": corpus(), "update": stored}),
        );
        assert!(v.get("failing").is_none(), "{v}");
    });
}

#[divan::bench(sample_count = 10)]
fn export_the_part(bencher: Bencher) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = project(dir.path());
    let v = must(&mut s, "import", json!({"path": corpus()}));
    let part = v["parts"][0]["part"].as_str().unwrap().to_owned();
    bencher.bench_local(|| {
        let params = json!({"part": part, "time_stamp": "2026-10-27T00:00:00"});
        let req = ryucad::proto::Request::from_rpc("export", Some(params)).unwrap();
        let a = ryucad::server::answer_live(&mut s, None, None, req).unwrap();
        assert!(a.binary.unwrap().len() > 100_000);
    });
}
