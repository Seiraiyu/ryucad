//! The index against brute force and against itself: every pair within a
//! distance and every nearest item against trying them all, random edits
//! against an index built from the result, and one thread against eight.
//! Release only (minutes in a debug build): its own step in full.yml.

use ryu_board::geom::{Point, Rect};
use ryu_board::model::{Library, Side};
use ryu_board::shape::feature_shape;
use ryu_board::testing::{Lcg, seeded_features};
use ryu_copper::index::{Index, Pair};
use ryu_copper::layout::{Item, Layer, Layout};
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};

fn point(g: &mut Lcg, reach: i64) -> Point {
    Point::new(g.range(-reach, reach), g.range(-reach, reach))
}

/// A layout of seeded features on two layers, keyed by number.
fn seeded_layout(seed: u64, count: usize) -> Layout<u32> {
    let mut g = Lcg::new(seed);
    // Spread over 40 mm, so shapes up to 5 mm meet some neighbours, not
    // all of them.
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

/// Every pair on a layer within `d`, by trying them all.
fn brute_pairs(ix: &Index<u32>, keys: &[u32], layer: usize, d: i64) -> Vec<Pair<u32>> {
    let on: Vec<u32> = keys
        .iter()
        .copied()
        .filter(|k| ix.layer(k) == Some(layer))
        .collect();
    let mut out = Vec::new();
    for (i, a) in on.iter().enumerate() {
        for b in &on[i + 1..] {
            let v = measure::within(ix.body(a).unwrap(), ix.body(b).unwrap(), d);
            if v != Verdict::No {
                out.push(Pair {
                    a: *a.min(b),
                    b: *a.max(b),
                    verdict: v,
                });
            }
        }
    }
    out.sort_by_key(|p| (p.a, p.b));
    out
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only: minutes in a debug build")]
fn pairs_and_nearest_against_brute_force() {
    let l = seeded_layout(0x7061_6972, 200);
    let ix = Index::of_layout(&l, BodyOptions::default());
    let keys: Vec<u32> = l.items.keys().copied().collect();
    for d in [0, 50_000, 300_000] {
        for layer in 0..2 {
            assert_eq!(
                ix.pairs(layer, d),
                brute_pairs(&ix, &keys, layer, d),
                "d {d}"
            );
        }
    }
    let mut g = Lcg::new(5);
    for _ in 0..40 {
        let p = point(&mut g, 20_000_000);
        let (k, d) = ix.nearest(0, p).unwrap();
        let at = Body::point(p);
        let best = keys
            .iter()
            .filter(|k| ix.layer(k) == Some(0))
            // Shapes of only negative parts have no body to be near.
            .filter_map(|k| Some((measure::distance(ix.body(k).unwrap(), &at)?.nm, *k)))
            .min()
            .unwrap();
        assert_eq!((d.nm, k), best);
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only: minutes in a debug build")]
fn edits_answer_as_a_rebuild_does() {
    let full = seeded_layout(0x6564_6974, 200);
    let mut g = Lcg::new(11);
    let mut live = Layout {
        layers: full.layers.clone(),
        ..Layout::default()
    };
    // Bodies made once: the index is under test, not body making.
    let made = Index::of_layout(&full, BodyOptions::default());
    let body = |k: &u32| made.body(k).unwrap().clone();
    let mut ix: Index<u32> = Index::new(2, BodyOptions::default());
    let keys: Vec<u32> = full.items.keys().copied().collect();
    for step in 0..900 {
        let k = keys[g.range(0, keys.len() as i64 - 1) as usize];
        if g.range(0, 2) > 0 {
            let it = full.items[&k].clone();
            // Sometimes on the other layer.
            let layer = if g.coin() { it.layer } else { 1 - it.layer };
            ix.insert(k, layer, body(&k));
            live.items.insert(k, Item { layer, ..it });
        } else {
            ix.remove(&k);
            live.items.remove(&k);
        }
        if step % 150 == 149 {
            let fresh = Index::from_bodies(
                2,
                live.items
                    .iter()
                    .map(|(k, it)| (*k, it.layer, body(k)))
                    .collect(),
                BodyOptions::default(),
            );
            assert_eq!(ix.len(), fresh.len());
            for layer in 0..2 {
                assert_eq!(
                    ix.pairs(layer, 50_000),
                    fresh.pairs(layer, 50_000),
                    "step {step}"
                );
                let q = Rect::new(
                    Point::new(-8_000_000, -5_000_000),
                    Point::new(6_000_000, 9_000_000),
                );
                assert_eq!(ix.in_box(layer, &q), fresh.in_box(layer, &q));
            }
            for k in live.items.keys().take(20) {
                let near: Vec<_> = ix.near(k, 100_000);
                assert_eq!(near, fresh.near(k, 100_000));
            }
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only: minutes in a debug build")]
fn one_thread_and_eight_agree() {
    let l = seeded_layout(0x7468_7264, 300);
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let ix = Index::of_layout(&l, BodyOptions::default());
                (ix.pairs(0, 100_000), ix.pairs(1, 0))
            })
    };
    assert_eq!(run(1), run(8));
}
