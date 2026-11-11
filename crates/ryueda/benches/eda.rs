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

/// The symbol lint of a 64-pin IC drawn from its template, with its fixes
/// tried: what lib check spends a symbol.
#[divan::bench]
fn symbol_lint_64(b: divan::Bencher) {
    use ryueda::standards::{Class, Standards};
    use ryueda::template::{PinSpec, template};
    let st = Standards::default();
    let pins: Vec<PinSpec> = (0..64)
        .map(|i| PinSpec {
            number: (i + 1).to_string(),
            name: format!("GPIO{i}"),
            ty: ryueda::symbol::PinType::Bidirectional,
        })
        .collect();
    let mut s = template("U", Class::Ic, &pins, &st).unwrap();
    // Two pins swapped, so a group finding and its fix are tried.
    let (a, c) = (s.pins["3"].at, s.pins["9"].at);
    s.pins.get_mut("3").unwrap().at = c;
    s.pins.get_mut("9").unwrap().at = a;
    let prefs = ryusch::lint::Prefs::default();
    let cx = ryueda::symlint::Context { standards: &st, prefs: &prefs, from: "self" };
    b.bench(|| ryueda::symlint::lint(divan::black_box(&s), Some(Class::Ic), &cx));
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
