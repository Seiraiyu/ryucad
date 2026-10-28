//! Shapes of every standard symbol, of text, and of a layer of seeded
//! features built from nothing.

use std::collections::BTreeMap;

use ryu_board::geom::{Pen, Place, Point};
use ryu_board::model::{Font, FontStroke, Library, Text};
use ryu_board::shape::{feature_shape, symbol_shape, text_shape};
use ryu_board::testing::{Lcg, seeded_features, seeded_symbol};

fn main() {
    divan::main();
}

#[divan::bench]
fn symbols_1000(bencher: divan::Bencher) {
    let mut g = Lcg::new(1);
    let symbols: Vec<_> = (0..1000).map(|_| seeded_symbol(&mut g)).collect();
    bencher.bench(|| {
        symbols
            .iter()
            .map(|s| symbol_shape(s).is_ok() as usize)
            .sum::<usize>()
    });
}

#[divan::bench]
fn text_100_chars(bencher: divan::Bencher) {
    // A font of 36 characters of five strokes each.
    let mut chars = BTreeMap::new();
    for (i, c) in ('A'..='Z').chain('0'..='9').enumerate() {
        let k = i as i64;
        let s = |a: (i64, i64), b: (i64, i64)| FontStroke {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            pen: Pen::Round(1),
        };
        chars.insert(
            c,
            vec![
                s((0, 0), (0, 2000)),
                s((0, 2000), (1000, 2000)),
                s((1000, 2000), (1000, k * 50)),
                s((0, 1000), (1000, 1000)),
                s((0, 0), (1000, 0)),
            ],
        );
    }
    let font = Font {
        name: "standard".into(),
        width: 1000,
        height: 2000,
        offset: 200,
        chars,
    };
    let text = Text {
        font: "standard".into(),
        place: Place::at(Point::new(1_000_000, 0)),
        width: 800_000,
        height: 1_200_000,
        line_width: 150_000,
        string: "REV A 2026-10-28 RYUDFM 0123456789 ".repeat(3)[..100].to_string(),
    };
    bencher.bench(|| text_shape(&text, &font).unwrap().parts.len());
}

#[divan::bench(args = [1, 0])]
fn layer_10000_features(bencher: divan::Bencher, threads: usize) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    bencher
        .with_inputs(|| {
            let mut g = Lcg::new(2);
            seeded_features(&mut g, 64, 10_000, 100_000_000)
        })
        .bench_values(|f| {
            let lib = Library::default();
            pool.install(|| {
                use rayon::prelude::*;
                (0..f.features().len())
                    .into_par_iter()
                    .filter(|&i| feature_shape(&f, i, &lib).is_ok())
                    .count()
            })
        });
}
