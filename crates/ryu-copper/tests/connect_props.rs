//! Connectivity against brute force, against counts and against itself:
//! islands of seeded features with no cutters against every pair of
//! pieces judged exactly; traces cut by holes and planes cut by strips
//! against the pieces they must fall into; random edits of seeded
//! layouts with negatives and holes against a rebuild; one thread against
//! eight; clearance against every pair of pieces judged; quiet edits of
//! positive layouts against a rebuild. Debug builds run a quarter of the
//! counted cases and skip the seeded edits, threads and clearance, which
//! full.yml runs in release.

use std::collections::BTreeMap;

use ryu_board::geom::{Edge, Nm, Pen, Point, Region, Stroke};
use ryu_board::model::{Library, Polarity, Side};
use ryu_board::shape::{Part, Piece, Shape, feature_shape, polygon};
use ryu_board::testing::{Lcg, seeded_features};
use ryu_copper::connect::{Copper, Node};
use ryu_copper::layout::{Hole, HoleShape, Item, Layer, Layout};
use ryu_copper::measure::{self, Body, BodyOptions, Verdict};
use ryu_copper::poly::{self, FlattenOptions};

/// Debug builds run a quarter of the cases.
const CASES: u64 = if cfg!(debug_assertions) { 1 } else { 4 };

fn layers(n: usize) -> Vec<Layer> {
    (0..n)
        .map(|k| Layer {
            name: format!("l{k}"),
            side: if k == 0 { Side::Top } else { Side::Bottom },
            copper: true,
        })
        .collect()
}

/// Seeded features crowded on two layers (10 mm across, so most touch
/// some neighbours), keyed by number; those flattening past 4,000
/// vertices left out, as the determinism test's are; with `positive`,
/// only shapes with no negative part.
fn seeded(seed: u64, count: usize, positive: bool) -> Layout<u32> {
    let mut g = Lcg::new(seed);
    let f = seeded_features(&mut g, 6, count, 5_000_000);
    let lib = Library::default();
    let opts = FlattenOptions::default();
    let mut l = Layout {
        layers: layers(2),
        ..Layout::default()
    };
    for k in 0..f.features().len() {
        let Ok(shape) = feature_shape(&f, k, &lib) else {
            continue;
        };
        if positive && shape.parts.iter().any(|p| p.polarity == Polarity::Negative) {
            continue;
        }
        let size: usize = poly::flatten_shape(shape, &opts)
            .iter()
            .flat_map(|p| &p.1.rings)
            .map(Vec::len)
            .sum();
        if size > 4_000 {
            continue;
        }
        l.items.insert(
            k as u32,
            Item {
                layer: k % 2,
                shape: shape.clone(),
                net: None,
            },
        );
    }
    l
}

/// Round holes and slots at seeded places, half of them on an item's
/// first solid (vias in pads), through one layer or both, mostly plated,
/// some back-drilled on the bottom; keyed from 10,000.
fn add_holes(l: &mut Layout<u32>, seed: u64, count: usize) {
    let mut g = Lcg::new(seed);
    let opts = BodyOptions::default();
    let probes: Vec<Point> = l
        .items
        .values()
        .filter_map(|it| {
            Body::of_shape(&it.shape, &opts)
                .solids
                .first()
                .map(|s| s.probe())
        })
        .collect();
    for k in 0..count {
        let at = if g.coin() && !probes.is_empty() {
            probes[g.range(0, probes.len() as i64 - 1) as usize]
        } else {
            Point::new(
                g.range(-5_000_000, 5_000_000),
                g.range(-5_000_000, 5_000_000),
            )
        };
        let d = g.range(100_000, 600_000);
        let shape = if g.range(0, 3) == 0 {
            let b = at.offset(g.range(-500_000, 500_000), g.range(-500_000, 500_000));
            HoleShape::Slot { a: at, b, d }
        } else {
            HoleShape::Round { at, d }
        };
        let span = [(0, 0), (1, 1), (0, 1)][g.range(0, 2) as usize];
        let backdrilled = if span == (0, 1) && g.range(0, 3) == 0 {
            vec![(1, 1)]
        } else {
            Vec::new()
        };
        l.holes.insert(
            10_000 + k as u32,
            Hole {
                shape,
                span,
                plated: g.range(0, 2) > 0,
                backdrilled,
            },
        );
    }
}

fn stroke(a: Point, b: Point, w: Nm) -> Shape {
    Shape {
        parts: vec![Part {
            polarity: Polarity::Positive,
            piece: Piece::Stroke(Stroke {
                path: vec![Edge::line(a, b)],
                pen: Pen::Round(w),
            }),
        }],
        ..Shape::default()
    }
}

fn block(x0: i64, y0: i64, x1: i64, y1: i64, polarity: Polarity) -> Shape {
    let ring = [
        Point::new(x0, y0),
        Point::new(x1, y0),
        Point::new(x1, y1),
        Point::new(x0, y1),
    ];
    Shape::regions(polarity, [Region::solid(polygon(&ring).unwrap())])
}

fn find(root: &mut [usize], mut i: usize) -> usize {
    while root[i] != i {
        root[i] = root[root[i]];
        i = root[i];
    }
    i
}

type PieceId = (u32, u32);

#[test]
fn islands_without_cutters_match_every_pair_judged() {
    for case in 0..CASES {
        let l = seeded(0x6e65_7473 + case, 150, true);
        let c = Copper::new(&l, BodyOptions::default());
        // Every piece: an item's solids in groups that touch.
        let opts = BodyOptions::default();
        let mut pieces: Vec<(PieceId, usize, Body)> = Vec::new();
        for (&k, it) in &l.items {
            let b = Body::of_shape(&it.shape, &opts);
            let groups = b.groups();
            if groups.len() == 1 {
                pieces.push(((k, 0), it.layer, b));
            } else {
                for (i, g) in groups.iter().enumerate() {
                    pieces.push(((k, i as u32), it.layer, b.subset(g)));
                }
            }
        }
        let mut root: Vec<usize> = (0..pieces.len()).collect();
        let mut doubts: Vec<(PieceId, PieceId)> = Vec::new();
        for i in 0..pieces.len() {
            for j in i + 1..pieces.len() {
                if pieces[i].1 != pieces[j].1 || pieces[i].0.0 == pieces[j].0.0 {
                    continue;
                }
                match measure::touches(&pieces[i].2, &pieces[j].2) {
                    Verdict::Yes => {
                        let (a, b) = (find(&mut root, i), find(&mut root, j));
                        root[a.max(b)] = a.min(b);
                    }
                    Verdict::Uncertain => doubts.push((pieces[i].0, pieces[j].0)),
                    Verdict::No => {}
                }
            }
        }
        let mut by: BTreeMap<usize, Vec<PieceId>> = BTreeMap::new();
        for (i, piece) in pieces.iter().enumerate() {
            let r = find(&mut root, i);
            by.entry(r).or_default().push(piece.0);
        }
        let mut want: Vec<Vec<PieceId>> = by.into_values().collect();
        want.sort();
        let mut got: Vec<Vec<PieceId>> = c.islands().into_iter().map(|i| i.pieces).collect();
        got.sort();
        assert_eq!(got, want, "case {case}");
        let got: Vec<(PieceId, PieceId)> = c
            .uncertain()
            .into_iter()
            .map(|u| match (u.a, u.b) {
                (Node::Piece(a, i), Node::Piece(b, j)) => ((a, i), (b, j)),
                other => panic!("a wall in doubt: {other:?}"),
            })
            .collect();
        doubts.sort();
        assert_eq!(got, doubts, "case {case}");
        // With no cutters nothing resolves.
        assert!(l.items.keys().all(|k| !c.is_resolved(k)));
    }
}

#[test]
fn traces_cut_by_holes_fall_into_their_count() {
    let mut g = Lcg::new(0x6375_7473);
    for case in 0..20 * CASES {
        let w = g.range(50_000, 300_000);
        let k = g.range(0, 6) as usize;
        let len = 2_000_000 * (k as i64 + 1);
        let mut l = Layout {
            layers: layers(1),
            ..Layout::default()
        };
        let trace = stroke(Point::new(0, 0), Point::new(len, 0), w);
        l.items.insert(
            0,
            Item {
                layer: 0,
                shape: trace,
                net: None,
            },
        );
        for h in 0..k {
            // Each hole in its own 2 mm stretch, wider than the trace and
            // off its axis by less than the excess, a micrometre to spare.
            let extra = g.range(20_000, 300_000);
            let x = 2_000_000 * (h as i64 + 1) + g.range(-500_000, 500_000);
            let y = g.range(-(extra / 2 - 1_000), extra / 2 - 1_000);
            l.holes.insert(
                h as u32,
                Hole {
                    shape: HoleShape::Round {
                        at: Point::new(x, y),
                        d: w + extra,
                    },
                    span: (0, 0),
                    plated: false,
                    backdrilled: vec![],
                },
            );
        }
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(c.pieces(&0), k as u32 + 1, "case {case}");
        assert_eq!(c.islands().len(), k + 1, "case {case}");
        // Plated, every wall joins the pieces either side of it.
        for h in l.holes.values_mut() {
            h.plated = true;
        }
        let c = Copper::new(&l, BodyOptions::default());
        assert_eq!(c.islands().len(), 1, "case {case}");
        assert!(c.uncertain().is_empty(), "case {case}");
    }
}

#[test]
fn planes_cut_by_strips_fall_into_their_count() {
    let mut g = Lcg::new(0x7374_7270);
    for case in 0..20 * CASES {
        let k = g.range(0, 5) as usize;
        let plane = block(
            0,
            0,
            2_000_000 * (k as i64 + 1),
            10_000_000,
            Polarity::Positive,
        );
        let strips: Vec<Shape> = (0..k)
            .map(|h| {
                let x = 2_000_000 * (h as i64 + 1) + g.range(-500_000, 500_000);
                let half = g.range(5_000, 250_000);
                block(
                    x - half,
                    -1_000_000,
                    x + half,
                    11_000_000,
                    Polarity::Negative,
                )
            })
            .collect();
        // Drawn after the plane the strips cut it; drawn before, nothing.
        for (after, want) in [(true, k + 1), (false, 1)] {
            let mut l = Layout {
                layers: layers(1),
                ..Layout::default()
            };
            let key = if after { 0 } else { 100 };
            l.items.insert(
                key,
                Item {
                    layer: 0,
                    shape: plane.clone(),
                    net: None,
                },
            );
            for (h, s) in strips.iter().enumerate() {
                let it = Item {
                    layer: 0,
                    shape: s.clone(),
                    net: None,
                };
                l.items.insert(1 + h as u32, it);
            }
            let c = Copper::new(&l, BodyOptions::default());
            assert_eq!(c.islands().len(), want, "case {case}, after {after}");
            assert_eq!(c.pieces(&key), want as u32, "case {case}, after {after}");
        }
    }
}

fn same(c: &Copper<u32>, l: &Layout<u32>, what: &str) {
    let fresh = Copper::new(l, BodyOptions::default());
    assert_eq!(c.islands(), fresh.islands(), "{what}");
    assert_eq!(c.uncertain(), fresh.uncertain(), "{what}");
    for k in l.items.keys() {
        assert_eq!(c.is_resolved(k), fresh.is_resolved(k), "{what}: item {k}");
    }
    for layer in 0..l.layers.len() {
        assert_eq!(
            c.hard_holes(layer),
            fresh.hard_holes(layer),
            "{what}: layer {layer}"
        );
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: booleans of seeded shapes take minutes in a debug build"
)]
fn edits_match_a_rebuild() {
    for case in 0..CASES {
        let mut full = seeded(0x6564_6974 + case, 120, false);
        add_holes(&mut full, case, 30);
        let mut g = Lcg::new(case);
        // Start from about half the items and holes.
        let mut l = Layout {
            layers: full.layers.clone(),
            ..Layout::default()
        };
        for (k, it) in &full.items {
            if g.coin() {
                l.items.insert(*k, it.clone());
            }
        }
        for (k, h) in &full.holes {
            if g.coin() {
                l.holes.insert(*k, h.clone());
            }
        }
        let mut c = Copper::new(&l, BodyOptions::default());
        let items: Vec<u32> = full.items.keys().copied().collect();
        let holes: Vec<u32> = full.holes.keys().copied().collect();
        for step in 0..40 {
            if g.range(0, 3) < 2 {
                let k = items[g.range(0, items.len() as i64 - 1) as usize];
                if l.items.remove(&k).is_some() {
                    c.remove_item(&k);
                } else {
                    let mut it = full.items[&k].clone();
                    if g.coin() {
                        it.layer = 1 - it.layer;
                    }
                    l.items.insert(k, it.clone());
                    c.insert_item(k, it);
                }
            } else {
                let k = holes[g.range(0, holes.len() as i64 - 1) as usize];
                if l.holes.remove(&k).is_some() {
                    c.remove_hole(&k);
                } else {
                    l.holes.insert(k, full.holes[&k].clone());
                    c.insert_hole(k, full.holes[&k].clone());
                }
            }
            if step % 8 == 7 {
                same(&c, &l, &format!("case {case}, step {step}"));
            }
        }
        same(&c, &l, &format!("case {case}, at the end"));
        // The seeds must exercise what they test.
        let fresh = Copper::new(&full, BodyOptions::default());
        assert!(
            full.items.keys().any(|k| fresh.is_resolved(k)),
            "case {case}"
        );
        assert!(
            fresh.islands().iter().any(|i| i.layers.len() == 2),
            "case {case}"
        );
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: booleans of seeded shapes take minutes in a debug build"
)]
fn one_thread_and_eight_agree() {
    let mut l = seeded(0x7468_7264, 150, false);
    add_holes(&mut l, 9, 40);
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let c = Copper::new(&l, BodyOptions::default());
                (c.islands(), c.uncertain())
            })
    };
    assert_eq!(run(1), run(8));
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: booleans of seeded shapes take minutes in a debug build"
)]
fn clearance_matches_every_pair_judged() {
    for case in 0..CASES {
        let mut l = seeded(0x6761_7073 + case, 120, false);
        add_holes(&mut l, case + 40, 30);
        let c = Copper::new(&l, BodyOptions::default());
        // A millimetre: at 0.3 mm the seeds' small islands lie too far from
        // the large one on some layers to leave any gap to judge.
        let d = 1_000_000;
        for layer in 0..2 {
            let keys: Vec<u32> = l
                .items
                .iter()
                .filter(|(_, it)| it.layer == layer)
                .map(|(k, _)| *k)
                .collect();
            let mut want = Vec::new();
            for (x, &a) in keys.iter().enumerate() {
                for &b in &keys[x + 1..] {
                    for (i, pa) in c.piece_bodies(&a) {
                        for (j, pb) in c.piece_bodies(&b) {
                            let ia = c.island_of(&Node::Piece(a, i));
                            let ib = c.island_of(&Node::Piece(b, j));
                            if ia == ib || measure::closer(&pa, &pb, d) == Verdict::No {
                                continue;
                            }
                            want.push(((a, i), (b, j), measure::distance(&pa, &pb).unwrap().nm));
                        }
                    }
                }
            }
            want.sort_by_key(|x| (x.2, x.0, x.1));
            let got: Vec<_> = c
                .clearance(layer, d)
                .iter()
                .map(|g| (g.a, g.b, g.distance.nm))
                .collect();
            assert_eq!(got, want, "case {case}, layer {layer}");
            assert!(!got.is_empty(), "case {case}: the seeds must have gaps");
        }
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: booleans of seeded shapes take minutes in a debug build"
)]
fn hole_clearance_matches_every_wall_judged() {
    for case in 0..CASES {
        let mut l = seeded(0x686f_6c67 + case, 120, false);
        add_holes(&mut l, case + 60, 30);
        let d = 1_000_000;
        let run = |threads: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| {
                    let c = Copper::new(&l, BodyOptions::default());
                    let got: Vec<Vec<_>> = (0..2)
                        .map(|layer| {
                            c.hole_clearance(layer, d)
                                .iter()
                                .map(|g| (g.hole, g.piece, g.distance.nm, g.joined))
                                .collect()
                        })
                        .collect();
                    (c, got)
                })
        };
        let (c, got) = run(1);
        assert_eq!(got, run(8).1, "case {case}: one thread and eight");
        for (layer, got) in got.iter().enumerate() {
            let mut want = Vec::new();
            for (hk, h) in &l.holes {
                if !(h.span.0 <= layer && layer <= h.span.1) {
                    continue;
                }
                let wall = h.shape.body();
                let plated_here =
                    h.plated && !h.backdrilled.iter().any(|s| s.0 <= layer && layer <= s.1);
                let joined = if plated_here {
                    c.island_of(&Node::Hole(*hk))
                } else {
                    None
                };
                for (k, it) in &l.items {
                    if it.layer != layer {
                        continue;
                    }
                    for (i, body) in c.piece_bodies(k) {
                        let Some(island) = c.island_of(&Node::Piece(*k, i)) else {
                            continue;
                        };
                        if joined == Some(island) || measure::closer(&body, &wall, d) == Verdict::No
                        {
                            continue;
                        }
                        // As the engine does, a pair with no distance is skipped.
                        let Some(dist) = measure::distance(&body, &wall) else {
                            continue;
                        };
                        want.push((*hk, (*k, i), dist.nm, joined));
                    }
                }
            }
            want.sort_by_key(|x| (x.2, x.0, x.1));
            assert_eq!(*got, want, "case {case}, layer {layer}");
        }
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release only: booleans of seeded shapes take minutes in a debug build"
)]
fn quiet_edits_match_a_rebuild() {
    for case in 0..CASES {
        // Positive shapes only, so most inserts and removals are quiet;
        // the holes make some of them not.
        let mut full = seeded(0x7175_6965 + case, 100, true);
        add_holes(&mut full, case + 80, 20);
        let mut l = Layout {
            layers: full.layers.clone(),
            holes: full.holes.clone(),
            ..Layout::default()
        };
        let mut g = Lcg::new(case + 7);
        for (k, it) in &full.items {
            if g.coin() {
                l.items.insert(*k, it.clone());
            }
        }
        let mut c = Copper::new(&l, BodyOptions::default());
        let items: Vec<u32> = full.items.keys().copied().collect();
        let mut quiet = 0;
        for step in 0..40 {
            let k = items[g.range(0, items.len() as i64 - 1) as usize];
            let before = c.resolves();
            if l.items.remove(&k).is_some() {
                c.remove_item(&k);
            } else {
                l.items.insert(k, full.items[&k].clone());
                c.insert_item(k, full.items[&k].clone());
            }
            if c.resolves() == before {
                quiet += 1;
            }
            same(&c, &l, &format!("case {case}, step {step}"));
        }
        assert!(
            quiet >= 10,
            "case {case}: only {quiet} edits resolved nothing"
        );
    }
}

/// A layout edited into another by `apply_layout` is that layout built afresh,
/// for seeded pairs of layouts sharing most keys (ryueda 7.E).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: seeded layouts built twice")]
fn apply_matches_a_rebuild() {
    for seed in 0..32u64 {
        let mut a = seeded(seed, 60, seed % 2 == 0);
        add_holes(&mut a, seed, 6);
        let mut b = a.clone();
        let mut g = Lcg::new(seed ^ 0xa991);
        let keys: Vec<u32> = b.items.keys().copied().collect();
        for k in keys.iter().step_by(5) {
            b.items.remove(k);
        }
        let donor = seeded(seed + 1000, 12, true);
        for (i, (_, it)) in donor.items.into_iter().enumerate() {
            b.items
                .insert(10_000 + i as u32 + g.range(0, 3) as u32 * 100, it);
        }
        if let Some(k) = b.holes.keys().next().copied() {
            b.holes.remove(&k);
        }
        let mut c = Copper::new(&a, BodyOptions::default());
        let edits = c.apply_layout(&a, &b);
        assert!(edits.items + edits.holes > 0);
        same(&c, &b, &format!("seed {seed}"));
    }
}

/// `clearance_of` and `hole_clearance_of` are the whole queries filtered
/// to the keys (ryueda 7.E), over seeded layouts with holes.
#[test]
#[cfg_attr(debug_assertions, ignore = "release only: seeded layouts")]
fn clearance_of_keys_is_the_whole_query_filtered() {
    for seed in 0..16u64 {
        let mut l = seeded(seed, 80, true);
        add_holes(&mut l, seed, 8);
        let c = Copper::new(&l, BodyOptions::default());
        let keys: std::collections::BTreeSet<u32> = l
            .items
            .keys()
            .chain(l.holes.keys())
            .copied()
            .filter(|k| k % 3 == seed as u32 % 3)
            .collect();
        for layer in 0..l.layers.len() {
            let whole: Vec<_> = c
                .clearance(layer, 400_000)
                .into_iter()
                .filter(|g| keys.contains(&g.a.0) || keys.contains(&g.b.0))
                .collect();
            assert_eq!(
                c.clearance_of(layer, 400_000, &keys),
                whole,
                "seed {seed} layer {layer}"
            );
            let holes: Vec<_> = c
                .hole_clearance(layer, 400_000)
                .into_iter()
                .filter(|g| keys.contains(&g.hole) || keys.contains(&g.piece.0))
                .collect();
            assert_eq!(
                c.hole_clearance_of(layer, 400_000, &keys),
                holes,
                "seed {seed} layer {layer}"
            );
        }
    }
}
