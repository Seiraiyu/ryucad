//! Reading each committed board of the corpus from nothing, folder and
//! archive, and drawing it: what an import, a cold `inspect` and a
//! `render` cost.

use std::path::{Path, PathBuf};

use ryudfm::odb::{ReadOptions, read_path};
use ryudfm::render::{Args, Format, render};
use ryudfm::units::Unit;

fn main() {
    divan::main();
}

const BOARDS: [&str; 6] = [
    "minimal-clean",
    "minimal-clean.tgz",
    "gb-cart256k-a",
    "gb-cart256k-a.zip",
    "vna-se-probe",
    "vna-se-probe.zip",
];

fn job(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

#[divan::bench(args = BOARDS)]
fn read(name: &str) -> usize {
    let b = read_path(&job(name), &ReadOptions::default()).unwrap();
    b.layers.len()
}

#[divan::bench(args = ["minimal-clean.tgz", "gb-cart256k-a.zip", "vna-se-probe.zip"])]
fn read_and_build_every_shape(name: &str) -> usize {
    let b = read_path(&job(name), &ReadOptions::default()).unwrap();
    let layers: Vec<usize> = (0..b.layers.len()).collect();
    b.build_shapes(b.board_step, &layers);
    layers.len()
}

#[divan::bench(args = ["minimal-clean.tgz", "gb-cart256k-a.zip", "vna-se-probe.zip"])]
fn render_png_of_a_read_board(bencher: divan::Bencher, name: &str) {
    let b = read_path(&job(name), &ReadOptions::default()).unwrap();
    bencher.bench(|| {
        render(&b, None, &Args::default(), Unit::Mm)
            .unwrap()
            .binary
            .unwrap()
            .len()
    });
}

#[divan::bench(args = ["minimal-clean.tgz", "gb-cart256k-a.zip", "vna-se-probe.zip"])]
fn render_svg_of_a_read_board(bencher: divan::Bencher, name: &str) {
    let b = read_path(&job(name), &ReadOptions::default()).unwrap();
    let args = Args {
        format: Some(Format::Svg),
        ..Args::default()
    };
    bencher.bench(|| {
        render(&b, None, &args, Unit::Mm)
            .unwrap()
            .binary
            .unwrap()
            .len()
    });
}
