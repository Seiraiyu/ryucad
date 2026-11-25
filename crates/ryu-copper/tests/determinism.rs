//! Bit-exact measurement on every platform and CPU level:
//! `tests/golden/copper.txt` holds the hashes of seeded pieces' exact
//! comparisons and distances, seeded shapes' distances and containment,
//! an index's pairs and nearest items, booleans, offsets, flattening and
//! merged layers, eight seeded fills, and a seeded layout's islands,
//! uncertain contacts, netlist compare, gaps under 0.4 mm and point
//! contacts, the index,
//! layers and connectivity built on one thread and on eight. After a
//! deliberate change, regenerate with `UPDATE_GOLDEN=1 cargo test -p
//! ryu-copper --test determinism` and review the diff like any other
//! change.

mod seeded_fill;

use std::cmp::Ordering;

use ryu_board::geom::{Arc, Dir, Edge, Point};
use ryu_board::model::{Library, Side};
use ryu_board::shape::feature_shape;
use ryu_board::testing::{Golden, Lcg, Section, seeded_features};
use ryu_copper::connect::Copper;
use ryu_copper::index::Index;
use ryu_copper::layout::{Hole, HoleShape, Item, Layer, Layout, NetPoint, PointSide};
use ryu_copper::measure::core::{Core, SegP, edge_cores};
use ryu_copper::measure::{self, Body, BodyOptions, Distance, Solid, Verdict};
use ryu_copper::netlist;
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

/// A sparse seeded layout (so a debug build stays quick) with claimed
/// nets, holes (half on items: vias in pads, some back-drilled) and
/// netlist points on items, from both sides and none.
fn connect_layout() -> Layout<u32> {
    let mut l = small_layout(0x636f_6e6e, 160);
    let mut g = Lcg::new(0x686f_6c65);
    let opts = BodyOptions::default();
    let probes: Vec<Point> = l
        .items
        .values()
        .filter_map(|it| {
            Body::of_shape(&it.shape, &opts)
                .solids
                .first()
                .map(Solid::probe)
        })
        .collect();
    for (k, it) in l.items.iter_mut() {
        it.net = Some(format!("N{}", k % 5));
    }
    for k in 0..50u32 {
        let at = if g.coin() && !probes.is_empty() {
            probes[g.range(0, probes.len() as i64 - 1) as usize]
        } else {
            point(&mut g, 20_000_000)
        };
        let d = g.range(100_000, 600_000);
        let span = [(0, 0), (1, 1), (0, 1)][g.range(0, 2) as usize];
        let backdrilled = if span == (0, 1) && g.coin() {
            vec![(1, 1)]
        } else {
            vec![]
        };
        l.holes.insert(
            10_000 + k,
            Hole {
                shape: HoleShape::Round { at, d },
                span,
                plated: g.range(0, 2) > 0,
                backdrilled,
            },
        );
    }
    for (k, at) in probes.iter().enumerate().step_by(2) {
        let side = match k % 3 {
            0 => None,
            1 => Some(PointSide::Top),
            _ => Some(PointSide::Bottom),
        };
        l.points.push(NetPoint {
            net: format!("N{}", k % 7),
            at: *at,
            side,
            key: 20_000 + k as u32,
        });
    }
    l
}

fn connect_sections(golden: &mut Golden, threads: usize) {
    let l = connect_layout();
    let (islands, doubts, report, gaps, contacts, walls) = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            let c = Copper::new(&l, BodyOptions::default());
            let report = netlist::compare(&c, &l.points);
            let gaps: Vec<_> = (0..l.layers.len())
                .flat_map(|layer| c.clearance(layer, 400_000))
                .collect();
            let islands = c.islands();
            let contacts: Vec<_> = islands.iter().flat_map(|i| c.point_contacts(i)).collect();
            let walls: Vec<_> = (0..l.layers.len())
                .flat_map(|layer| c.hole_clearance(layer, 400_000))
                .collect();
            (islands, c.uncertain(), report, gaps, contacts, walls)
        });
    let mut s = golden.section(&format!("connect.islands.{threads}"));
    for i in &islands {
        for (k, p) in &i.pieces {
            s.i64((*k).into());
            s.i64((*p).into());
        }
        s.str("|");
        for h in &i.holes {
            s.i64((*h).into());
        }
        s.str("|");
        for c in &i.claims {
            s.str(c);
        }
        s.str(";");
    }
    let mut s = golden.section(&format!("connect.uncertain.{threads}"));
    for u in &doubts {
        s.i64(u.layer as i64);
        s.str(&format!("{:?} {:?}", u.a, u.b));
        s.i64(u.bound);
    }
    // By its fields: derived Debug text isn't promised across toolchains
    // (#932's review), so this section moved once, in plan 1.D.
    let mut s = golden.section(&format!("netlist.compare.{threads}"));
    s.str(if report.judged { "judged" } else { "unjudged" });
    for o in &report.opens {
        s.str("open");
        s.str(&o.net);
        for (id, keys) in &o.islands {
            s.i64(id.0.into());
            s.i64(id.1.into());
            for k in keys {
                s.i64((*k).into());
            }
            s.str(",");
        }
    }
    for x in &report.shorts {
        s.str("short");
        s.i64(x.island.0.into());
        s.i64(x.island.1.into());
        for (net, keys) in &x.nets {
            s.str(net);
            for k in keys {
                s.i64((*k).into());
            }
            s.str(",");
        }
        for (a, b) in &x.contacts {
            s.i64((*a).into());
            s.i64((*b).into());
        }
        s.str("|");
        for h in &x.holes {
            s.i64((*h).into());
        }
    }
    for o in &report.off_copper {
        s.str("off");
        s.i64(o.key.into());
        s.str(&o.net);
        s.i64(o.at.x);
        s.i64(o.at.y);
        s.str(if o.uncertain { "?" } else { "." });
    }
    for m in &report.mismatches {
        s.str("mismatch");
        s.i64(m.island.0.into());
        s.i64(m.island.1.into());
        s.str(&m.net);
        for (k, n) in &m.items {
            s.i64((*k).into());
            s.str(n);
        }
    }
    let mut s = golden.section(&format!("connect.clearance.{threads}"));
    for g in &gaps {
        for v in [
            g.layer as i64,
            g.a.0.into(),
            g.a.1.into(),
            g.b.0.into(),
            g.b.1.into(),
        ] {
            s.i64(v);
        }
        s.i64(g.distance.nm);
        s.i64(g.distance.bound);
        s.str(if g.distance.exact { "=" } else { "~" });
        s.str(match g.verdict {
            Verdict::Yes => "y",
            Verdict::Uncertain => "?",
            Verdict::No => "n",
        });
    }
    let mut s = golden.section(&format!("connect.contacts.{threads}"));
    for pc in &contacts {
        s.i64(pc.layer as i64);
        s.i64(pc.a.into());
        s.i64(pc.b.into());
    }
    let mut s = golden.section(&format!("connect.hole_clearance.{threads}"));
    for g in &walls {
        for v in [
            g.layer as i64,
            g.hole.into(),
            g.piece.0.into(),
            g.piece.1.into(),
            g.joined.map_or(-1, |j| j.0.into()),
        ] {
            s.i64(v);
        }
        s.i64(g.distance.nm);
        s.i64(g.distance.bound);
        s.str(if g.distance.exact { "=" } else { "~" });
        s.str(match g.verdict {
            Verdict::Yes => "y",
            Verdict::Uncertain => "?",
            Verdict::No => "n",
        });
    }
}

/// Eight seeded fill asks (spokes included): the polygons, islands
/// removed, uncertain contacts and spokes kept.
fn fill_sections(golden: &mut Golden) {
    for seed in 0..8 {
        let f = ryu_copper::fill::fill(&seeded_fill::seeded(seed).ask);
        let mut s = golden.section(&format!("fill.{seed}"));
        polygons(&mut s, &f.polygons);
        s.i64(f.islands_removed as i64);
        for k in &f.uncertain {
            s.i64(*k as i64);
        }
        s.str("|");
        for n in &f.spokes {
            s.i64(i64::from(*n));
        }
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
    connect_sections(&mut golden, 1);
    connect_sections(&mut golden, 8);
    fill_sections(&mut golden);
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
    for kind in [
        "connect.islands",
        "connect.uncertain",
        "netlist.compare",
        "connect.clearance",
        "connect.contacts",
    ] {
        assert_eq!(
            sections[&format!("{kind}.1")],
            sections[&format!("{kind}.8")],
            "{kind} differ between one thread and eight"
        );
    }
    golden.finish();
}
