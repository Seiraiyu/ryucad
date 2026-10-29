//! Loading, flattening and ERC on a generated design of 10,000 component
//! occurrences and about 20,000 nets (`tests/support/generated.rs`).
//! KiCad import joins these in plan 0.D.

#[path = "../tests/support/generated.rs"]
mod generated;

use ryueda::id::Id;
use ryueda::project::Project;
use std::path::PathBuf;
use std::sync::OnceLock;

fn main() {
    divan::main();
}

/// The design's folder, written once per run.
fn board() -> &'static PathBuf {
    static DIR: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        generated::write(&board, 100, 100);
        (dir, board)
    })
    .1
}

fn design() -> Id {
    Id::parse("d", "design").unwrap()
}

#[divan::bench]
fn load() -> Project {
    Project::load(board()).unwrap()
}

#[divan::bench]
fn flatten(b: divan::Bencher) {
    let p = Project::load(board()).unwrap();
    b.bench(|| ryueda::netlist::flatten(&p, &design()).unwrap());
}

#[divan::bench]
fn erc(b: divan::Bencher) {
    let p = Project::load(board()).unwrap();
    let n = ryueda::netlist::flatten(&p, &design()).unwrap();
    b.bench(|| ryueda::erc::check_netlist(&p, &n));
}
