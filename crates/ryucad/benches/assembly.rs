//! Assemblies at scale (design 72's Performance): `tests/bar_project`'s
//! 1,001 occurrences of 20 parts held by 1,000 mates. The assembly's
//! rebuild over parts already built, with its poses holding and with every
//! pin jostled off (solved again each time), and the interference of all
//! of it from an empty cache (each pin stands on the plate: 1,000 pairs
//! touching, every one intersected). The machine is printed first, so the
//! numbers carry their hardware.

#[path = "../tests/bar_project/mod.rs"]
mod bar_project;

use bar_project::{id, jostle, machine, project};
use divan::Bencher;

fn main() {
    println!("assembly benchmarks on {}", machine());
    divan::main();
}

/// Times the assembly's rebuild in `root` over its parts as built.
fn rebuild(bencher: Bencher, jostled: bool) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = project(dir.path());
    if jostled {
        jostle(dir.path());
        s.refresh().unwrap();
    }
    s.rebuilt(&id("asm")).unwrap();
    let part = s.project().part(&id("asm")).unwrap().clone();
    bencher.bench_local(|| {
        let r = s.rebuilt_as(part.clone()).unwrap();
        assert!(r.solve.as_ref().unwrap().report.converged);
    });
}

#[divan::bench(sample_count = 10)]
fn assembly_rebuild_held(bencher: Bencher) {
    rebuild(bencher, false);
}

#[divan::bench(sample_count = 10)]
fn assembly_rebuild_jostled(bencher: Bencher) {
    rebuild(bencher, true);
}

#[divan::bench(sample_count = 3)]
fn interference_from_nothing(bencher: Bencher) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = project(dir.path());
    let model = s
        .model(&id("asm"), &ryucad::doc::assembly::Rev::Working)
        .unwrap();
    bencher.bench_local(|| {
        let mut cache = ryucad::interference::Cache::default();
        let v = ryucad::interference::find(&id("asm"), &model, &[], &mut cache).unwrap();
        assert_eq!(v["interferences"], serde_json::json!([]));
    });
}
