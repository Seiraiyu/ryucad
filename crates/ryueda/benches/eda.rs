//! Loading, flattening and ERC on a generated design of 10,000 component
//! occurrences and about 20,000 nets (`tests/support/generated.rs`), and
//! KiCad import of our KiCad-format test files: reading them, and a dry
//! run into a project. The whole KiCad library's import is timed by the
//! corpus test (`tests/kicad_corpus.rs`), which prints it.

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

fn kicad_data() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/kicad")
}

#[divan::bench]
fn kicad_read(b: divan::Bencher) {
    let d = kicad_data();
    let sym = std::fs::read_to_string(d.join("symbols/Ryu_Test.kicad_sym")).unwrap();
    let fp =
        std::fs::read_to_string(d.join("footprints/Ryu_Test.pretty/Every_Pad.kicad_mod")).unwrap();
    b.bench(|| {
        let mut notes = ryueda::kicad::Notes::default();
        (
            ryueda::kicad::symbol::read(&sym).unwrap(),
            ryueda::kicad::footprint::read(&fp, &mut notes).unwrap(),
        )
    });
}

#[divan::bench]
fn kicad_import_dry_run(b: divan::Bencher) {
    let dir = tempfile::tempdir().unwrap();
    ryueda::server::init(dir.path(), None, false).unwrap();
    let mut s = ryueda::session::Session::open(dir.path()).unwrap();
    let d = kicad_data();
    let opts = ryueda::kicad::import::Options {
        paths: vec![
            d.join("symbols/Ryu_Test.kicad_sym"),
            d.join("footprints/Ryu_Test.pretty"),
        ],
        models: vec![d.join("3dmodels")],
        dry_run: true,
        ..Default::default()
    };
    b.bench_local(|| ryueda::kicad::import::import(&mut s, &opts, None).unwrap());
}
