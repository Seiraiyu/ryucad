//! A committed board's copper: built from a read board (connectivity and
//! the netlist compare), every gap under 0.2 mm on every copper layer,
//! and one trace removed and put back with its queries.

use std::path::Path;
use std::sync::Arc;

use ryu_board::model::Board;
use ryu_copper::connect::Node;
use ryudfm::copper::{CopperBoard, Key};
use ryudfm::odb::{ReadOptions, read_path};

fn main() {
    divan::main();
}

const BOARDS: [&str; 3] = ["minimal-clean.tgz", "gb-cart256k-a.zip", "vna-se-probe.zip"];

fn board(name: &str) -> Arc<Board> {
    let b = read_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus")
            .join(name),
        &ReadOptions::default(),
    )
    .unwrap();
    let all: Vec<usize> = (0..b.layers.len()).collect();
    b.build_shapes(b.board_step, &all);
    Arc::new(b)
}

#[divan::bench(args = BOARDS, sample_count = 5, sample_size = 1)]
fn copper_built(bencher: divan::Bencher, name: &str) {
    let b = board(name);
    bencher.bench(|| CopperBoard::new(Arc::clone(&b)).report.opens.len());
}

#[divan::bench(args = BOARDS, sample_count = 5, sample_size = 1)]
fn gaps_under_200_um(bencher: divan::Bencher, name: &str) {
    let cb = CopperBoard::new(board(name));
    bencher.bench(|| {
        (0..cb.built.copper.len())
            .map(|l| cb.copper.clearance(l, 200_000).len())
            .sum::<usize>()
    });
}

/// The first trace on the first copper layer.
fn trace(cb: &CopperBoard) -> Key {
    *cb.copper
        .items()
        .find(|(k, it)| it.layer == 0 && cb.is_trace(k))
        .expect("a trace")
        .0
}

#[divan::bench(args = BOARDS)]
fn a_trace_out_and_back(bencher: divan::Bencher, name: &str) {
    let mut cb = CopperBoard::new(board(name));
    let k = trace(&cb);
    bencher.bench_local(|| {
        let it = cb.copper.remove_item(&k).unwrap();
        cb.copper.insert_item(k, it);
        let near = cb.copper.bodies().near(&k, 200_000).len();
        (near, cb.copper.island_of(&Node::Piece(k, 0)))
    });
}
