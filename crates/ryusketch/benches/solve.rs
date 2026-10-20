//! Solving polygons of 10, 100 and 1,000 entities (half points, half
//! sides), all one connected component, from positions jostled by about a
//! fiftieth of a side and from solved ones (which still diagnoses).

#[path = "../tests/common/mod.rs"]
mod common;

use common::{jostle, polygon};
use divan::Bencher;
use ryukei_testkit::Lcg;
use ryusketch::{Sketch, solve};

fn main() {
    divan::main();
}

fn sketch(entities: usize) -> Sketch {
    let mut g = Lcg::new(7);
    let n = entities / 2;
    let mut s = polygon(&mut g, n, 1.0);
    // A side is about 2π / n long.
    jostle(&mut s, &mut g, 0.12 / n as f64);
    s
}

#[divan::bench(args = [10, 100, 1000], sample_count = 10)]
fn jostled(b: Bencher, entities: usize) {
    let s = sketch(entities);
    b.bench(|| solve(&s).unwrap());
}

#[divan::bench(args = [10, 100, 1000], sample_count = 10)]
fn solved(b: Bencher, entities: usize) {
    let s = solve(&sketch(entities)).unwrap().sketch;
    b.bench(|| solve(&s).unwrap());
}
