//! Design 192's benchmark project at 1,000 parts (`ryucad::bench::scale`,
//! without its STEP imports, which take a minute to make), in process: a
//! request that builds nothing, one feature edited (its part rebuilt and
//! the assemblies over it kept current), and interference of the whole
//! root from an empty cache. The full-size measurements, through a real
//! daemon, are `tests/scale_bar.rs`'s. The machine is printed first, so
//! the numbers carry their hardware.
//!
//! ```text
//! cargo bench -p ryucad --features bench --bench scale
//! ```

#[path = "../tests/bar_project/mod.rs"]
mod bar_project;

use bar_project::machine;
use divan::Bencher;
use ryucad::bench::scale::{Spec, part_id, write};
use ryucad::doc::assembly::Rev;
use ryucad::doc::id::Id;
use ryucad::proto::Request;
use ryucad::server::handle;
use ryucad::session::Session;
use serde_json::json;
use std::time::Instant;

const PARTS: usize = 1000;

fn main() {
    println!("scale benchmarks on {}", machine());
    divan::main();
}

fn spec() -> Spec {
    Spec {
        races: false,
        ..Spec::parts(PARTS)
    }
}

/// The project written and its root rebuilt once, with the folder its
/// session's watch keeps its markers in (outside the project).
///
/// The session watches its folder for file events, as a daemon's does:
/// without a watch, every request checks every file first (about 70 ms at
/// 1,000 parts), which is the scan, not the request.
fn project(dir: &tempfile::TempDir) -> (Session, tempfile::TempDir) {
    let markers = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let summary = write(dir, &spec(), &[]);
    let mut s = Session::open(dir).unwrap().with_watch(markers.path());
    let t = Instant::now();
    s.rebuilt(&root()).unwrap();
    println!(
        "{} parts, {} occurrences: the root built in {:.1} s",
        summary.parts,
        summary.occurrences,
        t.elapsed().as_secs_f64()
    );
    (s, markers)
}

fn root() -> Id {
    Id::parse("root", "part").unwrap()
}

/// Design 192's request that builds nothing: one part's feature list.
#[divan::bench(sample_count = 20)]
fn a_request_that_builds_nothing(bencher: Bencher) {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _markers) = project(&dir);
    let part = part_id(&spec(), 0, 1);
    bencher.bench_local(|| {
        handle(&mut s, Request::FeatureList { part: part.clone() }).unwrap();
    });
}

/// Every part listed: an answer the size of the project, recorded beside
/// the bar, not barred.
#[divan::bench(sample_count = 20)]
fn every_part_listed(bencher: Bencher) {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _markers) = project(&dir);
    bencher.bench_local(|| {
        handle(&mut s, Request::PartList {}).unwrap();
    });
}

#[divan::bench(sample_count = 10)]
fn one_feature_edited(bencher: Bencher) {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _markers) = project(&dir);
    let part = part_id(&spec(), 0, 1);
    let mut z = 7.5;
    bencher.bench_local(|| {
        z = if z == 7.5 { 8.0 } else { 7.5 };
        let req = Request::from_rpc(
            "feature.edit",
            Some(json!({"part": part, "id": "slab", "params": {"z": format!("{z} mm")}})),
        )
        .unwrap();
        handle(&mut s, req).unwrap();
    });
}

#[divan::bench(sample_count = 3)]
fn interference_from_nothing(bencher: Bencher) {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _markers) = project(&dir);
    let model = s.model(&root(), &Rev::Working).unwrap();
    bencher.bench_local(|| {
        let mut cache = ryucad::interference::Cache::default();
        ryucad::interference::find(&root(), &model, &[], &mut cache).unwrap();
    });
}
