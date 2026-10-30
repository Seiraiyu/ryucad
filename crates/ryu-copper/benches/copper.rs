//! Exact comparisons and distances between seeded pieces and shapes, and
//! the index over 1,000 seeded features: built, every pair within
//! 100 µm, an edit with its queries; booleans, offsets and merged layers.

use ryu_board::geom::{Arc, Dir, Point};
use ryu_board::model::{Library, Side};
use ryu_board::shape::feature_shape;
use ryu_board::testing::{Lcg, seeded_features};
use ryu_copper::index::Index;
use ryu_copper::layout::{Item, Layer, Layout};
use ryu_copper::measure::core::{ArcP, Core, SegP};
use ryu_copper::measure::{self, Body, BodyOptions, Solid};

fn main() {
    divan::main();
}

fn thick(core: Core, w: i64) -> Body {
    Body::new(vec![Solid::Thick { core, w }], 0)
}

fn traces(n: usize) -> Vec<Body> {
    let mut g = Lcg::new(1);
    (0..n)
        .map(|_| {
            let a = Point::new(g.range(0, 1_000_000), g.range(0, 1_000_000));
            let b = a.offset(g.range(-200_000, 200_000), g.range(-200_000, 200_000));
            thick(Core::Seg(SegP::new(a, b)), 100_000)
        })
        .collect()
}

#[divan::bench]
fn trace_pairs_within_1000(bencher: divan::Bencher) {
    let t = traces(1000);
    bencher.bench(|| {
        t.windows(2)
            .filter(|w| measure::within(&w[0], &w[1], 150_000).is_yes())
            .count()
    });
}

#[divan::bench]
fn arc_pairs_distance_1000(bencher: divan::Bencher) {
    let mut g = Lcg::new(2);
    let arcs: Vec<Body> = (0..1000)
        .map(|_| {
            let c = Point::new(g.range(0, 1_000_000), g.range(0, 1_000_000));
            // A written end 1 nm off its circle: an irrational true end.
            let arc = Arc {
                start: c.offset(300_000, 0),
                end: c.offset(0, 300_001),
                centre: c,
                dir: Dir::Ccw,
            };
            thick(Core::Arc(ArcP::new(arc)), 100_000)
        })
        .collect();
    bencher.bench(|| {
        arcs.windows(2)
            .filter_map(|w| measure::distance(&w[0], &w[1]))
            .map(|d| d.nm)
            .sum::<i64>()
    });
}

fn layout(count: usize) -> Layout<u32> {
    let mut g = Lcg::new(3);
    // Round and square pens and pads, and one other symbol: a board's
    // mix, without the giant sweeps the properties exercise.
    let f = seeded_features(&mut g, 3, count, 50_000_000);
    let lib = Library::default();
    let mut l = Layout {
        layers: vec![Layer {
            name: "top".into(),
            side: Side::Top,
            copper: true,
        }],
        ..Layout::default()
    };
    for k in 0..f.features().len() {
        if let Ok(shape) = feature_shape(&f, k, &lib) {
            l.items.insert(
                k as u32,
                Item {
                    layer: 0,
                    shape: shape.clone(),
                    net: None,
                },
            );
        }
    }
    l
}

#[divan::bench]
fn index_build_1000(bencher: divan::Bencher) {
    let l = layout(1000);
    bencher.bench(|| Index::of_layout(&l, BodyOptions::default()).len());
}

#[divan::bench]
fn index_pairs_1000(bencher: divan::Bencher) {
    let ix = Index::of_layout(&layout(1000), BodyOptions::default());
    bencher.bench(|| ix.pairs(0, 100_000).len());
}

#[divan::bench]
fn index_edit_and_query(bencher: divan::Bencher) {
    let mut ix = Index::of_layout(&layout(1000), BodyOptions::default());
    let trace = Body::stroke(Point::new(0, 0), Point::new(2_000_000, 0), 150_000);
    bencher.bench_local(|| {
        ix.insert(u32::MAX, 0, trace.clone());
        let near = ix.near(&u32::MAX, 100_000).len();
        ix.remove(&u32::MAX);
        near
    });
}

/// 2,000 overlapping pads (60 to 120-gons) on 20 mm.
fn pads() -> ryu_copper::poly::Rings {
    let mut g = Lcg::new(3);
    let rings = (0..2_000)
        .map(|_| {
            let c = Point::new(g.range(0, 20_000_000), g.range(0, 20_000_000));
            let (pen, _) = ryu_copper::poly::disc(g.range(100_000, 600_000), 48.0);
            pen.iter().map(|p| p.offset(c.x, c.y)).collect()
        })
        .collect();
    ryu_copper::poly::Rings::new(rings, 0)
}

#[divan::bench(sample_count = 5, sample_size = 1)]
fn union_of_2000_pads(bencher: divan::Bencher) {
    use ryu_copper::poly::{self, FillRule};
    let r = pads();
    bencher.bench(|| poly::simplify(&r, FillRule::NonZero).polygons.len());
}

#[divan::bench(sample_count = 5, sample_size = 1)]
fn pads_grown_by_50_um(bencher: divan::Bencher) {
    use ryu_copper::poly::{self, FillRule, FlattenOptions};
    let p = poly::simplify(&pads(), FillRule::NonZero);
    bencher.bench(|| {
        poly::grow(&p, 50_000, &FlattenOptions::default())
            .polygons
            .len()
    });
}

#[divan::bench(sample_count = 5, sample_size = 1)]
fn merge_1000_seeded_features(bencher: divan::Bencher) {
    use ryu_copper::poly::{self, FlattenOptions};
    let mut g = Lcg::new(4);
    let f = seeded_features(&mut g, 8, 1_000, 20_000_000);
    let lib = Library::default();
    let opts = FlattenOptions::default();
    let mut l: Layout<u32> = Layout::default();
    for name in ["top", "bottom"] {
        l.layers.push(Layer {
            name: name.into(),
            side: Side::Top,
            copper: true,
        });
    }
    for k in 0..f.features().len() {
        if let Ok(s) = feature_shape(&f, k, &lib) {
            // Concave pens swept along arcs flatten to millions of edges,
            // which real boards hardly draw.
            let v: usize = poly::flatten_shape(s, &opts)
                .iter()
                .flat_map(|p| &p.1.rings)
                .map(Vec::len)
                .sum();
            if v <= 4_000 {
                l.items.insert(
                    k as u32,
                    Item {
                        layer: k % 2,
                        shape: s.clone(),
                        net: None,
                    },
                );
            }
        }
    }
    bencher.bench(|| poly::merge_layers(&l, &opts).len());
}
