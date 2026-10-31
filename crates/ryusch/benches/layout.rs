//! `cargo bench -p ryusch`: a seeded block laid out from nothing, and
//! again over its own layout after one value changed (what a write does).

use ryusch::engine::lay_out;
use ryusch::testing::seeded;

fn main() {
    divan::main();
}

#[divan::bench(args = [4, 12, 24])]
fn from_nothing(bencher: divan::Bencher, ics: usize) {
    let b = seeded(11, ics);
    bencher.bench(|| lay_out(divan::black_box(&b)).unwrap());
}

#[divan::bench(args = [4, 12, 24])]
fn after_one_edit(bencher: divan::Bencher, ics: usize) {
    let mut b = seeded(11, ics);
    b.previous = Some(lay_out(&b).unwrap().layout);
    let p = b.part("u1").unwrap();
    b.parts[p].value = "LOGIC-HC".into();
    bencher.bench(|| lay_out(divan::black_box(&b)).unwrap());
}
