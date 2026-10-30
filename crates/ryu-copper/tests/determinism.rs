//! Bit-exact measurement on every platform and CPU level:
//! `tests/golden/copper.txt` holds the hashes of seeded pieces' exact
//! comparisons and distances, seeded shapes' distances and containment,
//! and an index's pairs and nearest items, built on one thread and on
//! eight. After a deliberate change, regenerate with `UPDATE_GOLDEN=1
//! cargo test -p ryu-copper --test determinism` and review the diff like
//! any other change.

use std::cmp::Ordering;

use ryu_board::geom::{Arc, Dir, Edge, Point};
use ryu_board::model::{Library, Side};
use ryu_board::shape::feature_shape;
use ryu_board::testing::{Golden, Lcg, Section, seeded_features};
use ryu_copper::index::Index;
use ryu_copper::layout::{Item, Layer, Layout};
use ryu_copper::measure::core::{Core, SegP, edge_cores};
use ryu_copper::measure::{self, Body, BodyOptions, Distance, Solid};
use ryu_copper::poly::{self, FillRule, FlattenOptions, Op, Polygons, Rings};

fn point(g: &mut Lcg, reach: i64) -> Point {
    Point::new(g.range(-reach, reach), g.range(-reach, reach))
}

/// A segment, a point, or an arc (its end up to 3 nm off its circle)
/// with its join, as a body of pieces of no width.
fn seeded_piece(g: &mut Lcg, reach: i64) -> Body {
    let mut cores = Vec::new();
    match g.range(0, 3) {
        0 => cores.push(Core::Seg(SegP::new(point(g, reach), point(g, reach)))),
        1 => {
            let p = point(g, reach);
            cores.push(Core::Seg(SegP::new(p, p)));
        }
        _ => {
            let c = point(g, reach);
            let r = g.range(10, reach) as f64;
            let at = |t: f64, r: f64| {
                let (s, co) = ryu_math::sin_cos(t);
                Point::new(
                    c.x + ryu_board::geom::round_nm(r * co),
                    c.y + ryu_board::geom::round_nm(r * s),
                )
            };
            let a0 = g.range(0, 6283) as f64 / 1000.0;
            let a1 = g.range(0, 6283) as f64 / 1000.0;
            let arc = Arc {
                start: at(a0, r),
                end: at(a1, r + g.range(-3, 3) as f64),
                centre: c,
                dir: if g.coin() { Dir::Ccw } else { Dir::Cw },
            };
            edge_cores(&Edge::Arc(arc), &mut cores);
        }
    }
    Body::new(
        cores
            .into_iter()
            .map(|core| Solid::Thick { core, w: 0 })
            .collect(),
        0,
    )
}

fn ordering(o: Ordering) -> i64 {
    o as i64
}

fn distance(s: &mut Section, d: Option<Distance>) {
    let Some(d) = d else {
        s.str("none");
        return;
    };
    for v in [
        d.nm,
        i64::from(d.exact),
        d.bound,
        d.a.x,
        d.a.y,
        d.b.x,
        d.b.y,
        d.a_solid as i64,
        d.b_solid as i64,
    ] {
        s.i64(v);
    }
}

fn layout(seed: u64) -> Layout<u32> {
    layout_of(seed, 150)
}

fn layout_of(seed: u64, count: usize) -> Layout<u32> {
    let mut g = Lcg::new(seed);
    let f = seeded_features(&mut g, 6, count, 20_000_000);
    let lib = Library::default();
    let mut l = Layout {
        layers: vec![
            Layer {
                name: "top".into(),
                side: Side::Top,
                copper: true,
            },
            Layer {
                name: "bottom".into(),
                side: Side::Bottom,
                copper: true,
            },
        ],
        ..Layout::default()
    };
    for k in 0..f.features().len() {
        if let Ok(shape) = feature_shape(&f, k, &lib) {
            l.items.insert(
                k as u32,
                Item {
                    layer: k % 2,
                    shape: shape.clone(),
                    net: None,
                },
            );
        }
    }
    l
}

fn index_sections(golden: &mut Golden, threads: usize) {
    let l = layout(0x676f_6c64);
    let (pairs, nearest) = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            let ix = Index::of_layout(&l, BodyOptions::default());
            let pairs: Vec<_> = (0..2).flat_map(|layer| ix.pairs(layer, 100_000)).collect();
            let mut g = Lcg::new(9);
            let nearest: Vec<_> = (0..50)
                .map(|_| ix.nearest(0, point(&mut g, 20_000_000)))
                .collect();
            (pairs, nearest)
        });
    let mut s = golden.section(&format!("index.pairs.{threads}"));
    for p in &pairs {
        s.i64(p.a.into());
        s.i64(p.b.into());
        s.str(&format!("{:?}", p.verdict));
    }
    let mut s = golden.section(&format!("index.nearest.{threads}"));
    for n in nearest {
        match n {
            Some((k, d)) => {
                s.i64(k.into());
                distance(&mut s, Some(d));
            }
            None => s.str("none"),
        }
    }
}

fn polygons(s: &mut Section, p: &Polygons) {
    s.i64(p.bound);
    for poly in &p.polygons {
        for r in poly.rings() {
            s.i64(r.len() as i64);
            for q in r {
                s.i64(q.x);
                s.i64(q.y);
            }
        }
        s.str("|");
    }
}

/// A ring round a centre: a star, or points anywhere.
fn seeded_ring(g: &mut Lcg, reach: i64) -> Vec<Point> {
    let n = g.range(3, 9) as usize;
    let c = point(g, reach);
    (0..n)
        .map(|_| {
            if g.coin() {
                point(g, reach)
            } else {
                Point::new(
                    c.x + g.range(-reach / 8, reach / 8),
                    c.y + g.range(-reach / 8, reach / 8),
                )
            }
        })
        .collect()
}

fn seeded_rings(g: &mut Lcg, reach: i64) -> Rings {
    let k = g.range(1, 4) as usize;
    Rings::new((0..k).map(|_| seeded_ring(g, reach)).collect(), 0)
}

/// Seeded features whose flattening stays small: a concave pen swept
/// along an arc can flatten to millions of edges, which real boards
/// hardly draw.
fn small_layout(seed: u64, count: usize) -> Layout<u32> {
    let mut l = layout_of(seed, count);
    let opts = FlattenOptions::default();
    l.items.retain(|_, it| {
        poly::flatten_shape(&it.shape, &opts)
            .iter()
            .flat_map(|p| &p.1.rings)
            .map(Vec::len)
            .sum::<usize>()
            <= 4_000
    });
    l
}

fn poly_sections(golden: &mut Golden) {
    let mut g = Lcg::new(0x706f_6c79);
    let mut s = golden.section("poly.booleans");
    let mut kept = Vec::new();
    for case in 0..200 {
        let reach = if case % 2 == 0 { 50 } else { 3_000_000 };
        let (a, b) = (seeded_rings(&mut g, reach), seeded_rings(&mut g, reach));
        for op in [Op::Union, Op::Intersection, Op::Difference, Op::Xor] {
            let out = poly::boolean(&a, FillRule::NonZero, &b, FillRule::EvenOdd, op);
            polygons(&mut s, &out);
        }
        if case % 2 == 1 {
            kept.push(poly::simplify(&a, FillRule::NonZero));
        }
    }
    let mut s = golden.section("poly.offsets");
    let opts = FlattenOptions::default();
    for (k, p) in kept.iter().enumerate().take(40) {
        let d = 10_000 + 7_919 * k as i64;
        polygons(&mut s, &poly::grow(p, d, &opts));
        polygons(&mut s, &poly::shrink(p, d, &opts));
    }
    let l = small_layout(0x666c_6174, 150);
    let mut s = golden.section("poly.flatten");
    for it in l.items.values() {
        for (pol, r) in poly::flatten_shape(&it.shape, &opts) {
            s.str(&format!("{pol:?}"));
            s.i64(r.bound);
            for ring in &r.rings {
                s.i64(ring.len() as i64);
                for q in ring {
                    s.i64(q.x);
                    s.i64(q.y);
                }
            }
        }
    }
}

fn layer_sections(golden: &mut Golden, threads: usize) {
    let l = small_layout(0x6c61_7972, 400);
    let merged = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| poly::merge_layers(&l, &FlattenOptions::default()));
    let mut s = golden.section(&format!("poly.layers.{threads}"));
    for m in &merged {
        polygons(&mut s, m);
    }
}

#[test]
fn measurement_is_bit_exact() {
    let mut golden = Golden::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/copper.txt"
    ));
    {
        let mut g = Lcg::new(0x7072_6564);
        let mut s = golden.section("pieces.compare");
        let mut pieces = Vec::new();
        for _ in 0..500 {
            let a = seeded_piece(&mut g, 100_000);
            let b = seeded_piece(&mut g, 100_000);
            for dd in [0, 1, 2 * g.range(0, 150_000) as i128] {
                s.i64(ordering(measure::compare(&a, &b, dd)));
            }
            pieces.push((a, b));
        }
        let mut s = golden.section("pieces.distance");
        for (a, b) in &pieces {
            distance(&mut s, measure::distance(a, b));
        }
    }
    {
        let mut g = Lcg::new(0x7368_6170);
        let f = seeded_features(&mut g, 6, 150, 20_000_000);
        let lib = Library::default();
        let bodies: Vec<Body> = (0..f.features().len())
            .filter_map(|k| feature_shape(&f, k, &lib).as_ref().ok())
            .map(|s| Body::of_shape(s, &BodyOptions::default()))
            .collect();
        let mut s = golden.section("shapes.distance");
        for w in bodies.windows(2) {
            distance(&mut s, measure::distance(&w[0], &w[1]));
        }
        let mut s = golden.section("shapes.contains");
        for b in &bodies {
            for _ in 0..10 {
                s.str(&format!(
                    "{:?}",
                    measure::contains(b, point(&mut g, 5_000_000))
                ));
            }
        }
    }
    index_sections(&mut golden, 1);
    index_sections(&mut golden, 8);
    poly_sections(&mut golden);
    layer_sections(&mut golden, 1);
    layer_sections(&mut golden, 8);
    let sections = golden.sections_for_test();
    assert_eq!(
        sections["poly.layers.1"], sections["poly.layers.8"],
        "layers differ between one thread and eight"
    );
    for kind in ["pairs", "nearest"] {
        assert_eq!(
            sections[&format!("index.{kind}.1")],
            sections[&format!("index.{kind}.8")],
            "{kind} differ between one thread and eight"
        );
    }
    golden.finish();
}
