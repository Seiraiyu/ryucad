//! Rings from bytes combined every way, grown and shrunk: every result
//! is simple polygons in order (`poly::faults`), points clear of every
//! input edge are classified as the exact windings say, and one thread
//! and eight give the same polygons. Then a two-layer layout of the same
//! rings: connectivity never panics, one thread and eight agree, and
//! inserting and removing items equals building again. And a fill of
//! the first operand less the second, grown by a byte's clearance, with
//! a thermal pad at a byte's point: simple polygons in order, inside its
//! area, one thread and eight alike.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryu_board::geom::{Point, Region};
use ryu_board::model::{Polarity, Side};
use ryu_board::shape::{Shape, polygon};
use ryu_copper::connect::Copper;
use ryu_copper::fill::{self, Ask, ThermalPad};
use ryu_copper::layout::{Hole, HoleShape, Item, Layer, Layout};
use ryu_copper::measure::{BodyOptions, Verdict};
use ryu_copper::poly::{self, FillRule, FlattenOptions, Op, Rings};

/// Rings from bytes: the first byte picks a scale (1 nm, crowding every
/// coordinate onto a small grid, or 100 µm) and where the second operand
/// starts; then each pair of bytes is a vertex, a byte of 255 ending a
/// ring.
fn operands(data: &[u8]) -> Option<(Rings, Rings, i64)> {
    let (&head, rest) = data.split_first()?;
    let scale = if head & 1 == 0 { 1 } else { 100_000 };
    let split = (rest.len() * usize::from(head >> 1) / 128).min(rest.len());
    let rings = |bytes: &[u8]| {
        let mut out: Vec<Vec<Point>> = vec![Vec::new()];
        let mut k = 0;
        while k + 1 < bytes.len() {
            if bytes[k] == 255 {
                out.push(Vec::new());
                k += 1;
                continue;
            }
            let p = Point::new(i64::from(bytes[k] % 64) * scale, i64::from(bytes[k + 1] % 64) * scale);
            out.last_mut().unwrap().push(p);
            k += 2;
        }
        out.retain(|r| r.len() >= 3 && r.len() <= 64);
        out.truncate(16);
        Rings::new(out, 0)
    };
    Some((rings(&rest[..split]), rings(&rest[split..]), scale))
}

/// Whether `x` is more than 2 nm from every edge.
fn clear(r: &Rings, x: Point) -> bool {
    r.rings.iter().all(|r| {
        (0..r.len()).all(|k| {
            let (a, b) = (r[k], r[(k + 1) % r.len()]);
            let (u, v) = (b.minus(a), x.minus(a));
            let (t, l) = (u.dot(v), u.len2());
            if t <= 0 || l == 0 {
                v.len2() > 4
            } else if t >= l {
                x.minus(b).len2() > 4
            } else {
                let c = u.cross(v);
                c * c > 4 * l
            }
        })
    })
}

/// A two-layer layout of the operands: `a`'s rings as copper regions,
/// alternating layers; `b`'s as negative regions drawn after them on the
/// first layer, and a hole at each one's first vertex through both,
/// plated every other one.
fn layout_of(a: &Rings, b: &Rings, scale: i64) -> Layout<u32> {
    let mut l = Layout::default();
    for (name, side) in [("top", Side::Top), ("bottom", Side::Bottom)] {
        l.layers.push(Layer {
            name: name.into(),
            side,
            copper: true,
        });
    }
    let region = |r: &Vec<Point>, polarity| {
        polygon(r)
            .ok()
            .map(|c| Shape::regions(polarity, [Region::solid(c)]))
    };
    for (k, r) in a.rings.iter().enumerate() {
        if let Some(shape) = region(r, Polarity::Positive) {
            let it = Item {
                layer: k % 2,
                shape,
                net: None,
            };
            l.items.insert(k as u32, it);
        }
    }
    for (k, r) in b.rings.iter().enumerate() {
        if let Some(shape) = region(r, Polarity::Negative) {
            let it = Item {
                layer: 0,
                shape,
                net: None,
            };
            l.items.insert(100 + k as u32, it);
        }
        let hole = Hole {
            shape: HoleShape::Round {
                at: r[0],
                d: 3 * scale,
            },
            span: (0, 1),
            plated: k % 2 == 0,
            backdrilled: vec![],
        };
        l.holes.insert(k as u32, hole);
    }
    l
}

fuzz_target!(|data: &[u8]| {
    let Some((a, b, scale)) = operands(data) else {
        return;
    };
    let wind = |r: &Rings, x: Point| -> i32 { r.rings.iter().map(|r| poly::winding(r, x)).sum() };
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                [Op::Union, Op::Intersection, Op::Difference, Op::Xor]
                    .map(|op| poly::boolean(&a, FillRule::NonZero, &b, FillRule::EvenOdd, op))
            })
    };
    let one = run(1);
    for (op, out) in [Op::Union, Op::Intersection, Op::Difference, Op::Xor].iter().zip(&one) {
        let faults = poly::faults(out);
        assert!(faults.is_empty(), "{op:?}: {faults:?}");
        for i in 0..64 {
            for j in 0..64 {
                // Points between the grid's, where no vertex lies.
                let x = Point::new(i * scale + scale / 2, j * scale + scale / 3);
                if !clear(&a, x) || !clear(&b, x) {
                    continue;
                }
                let want = op.apply(
                    FillRule::NonZero.inside(wind(&a, x)),
                    FillRule::EvenOdd.inside(wind(&b, x)),
                );
                let expect = if want { Verdict::Yes } else { Verdict::No };
                assert_eq!(out.contains(x), expect, "{op:?} at {x:?}");
            }
        }
    }
    assert_eq!(one, run(8));
    let opts = FlattenOptions::default();
    let d = 3 * scale;
    for p in [poly::grow(&one[0], d, &opts), poly::shrink(&one[0], d, &opts)] {
        let faults = poly::faults(&p);
        assert!(faults.is_empty(), "{faults:?}");
    }
    // A fill of `a` less `b`, grown by a byte's clearance.
    let area = poly::simplify(&a, FillRule::NonZero);
    let byte = |k: usize| i64::from(data[k % data.len()]);
    let grown = byte(1) * scale / 8 + 1;
    let obstacle = poly::grow(&poly::simplify(&b, FillRule::EvenOdd), grown, &fill::opts());
    let ask = Ask {
        area: area.clone(),
        obstacles: vec![obstacle.clone().into()],
        spoke_obstacles: vec![obstacle.into()],
        thermals: vec![ThermalPad {
            at: Point::new((byte(2) % 64) * scale, (byte(3) % 64) * scale),
            rotation_mdeg: byte(4) * 1_411,
            half: [scale, 2 * scale],
            gap: scale,
            spoke: scale,
        }],
        min_width: byte(5) % 4 * scale / 2,
        own: vec![area.body()],
        keep_islands: byte(6) & 1 == 1,
    };
    let fill_on = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| fill::fill(&ask))
    };
    let f = fill_on(1);
    let faults = poly::faults(&f.polygons);
    assert!(faults.is_empty(), "fill: {faults:?}");
    assert!(poly::difference(&f.polygons, &area).is_empty(), "fill outside its area");
    assert_eq!(f, fill_on(8));
    // Connectivity of the same rings.
    let l = layout_of(&a, &b, scale);
    let build = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let c = Copper::new(&l, BodyOptions::default());
                (c.islands(), c.uncertain())
            })
    };
    let whole = build(1);
    assert_eq!(whole, build(8));
    let empty = Layout {
        layers: l.layers.clone(),
        ..Layout::default()
    };
    let mut c = Copper::new(&empty, BodyOptions::default());
    for (k, h) in &l.holes {
        c.insert_hole(*k, h.clone());
    }
    for (k, it) in &l.items {
        c.insert_item(*k, it.clone());
    }
    assert_eq!((c.islands(), c.uncertain()), whole);
    let mut half = l.clone();
    for k in l.items.keys().step_by(2) {
        half.items.remove(k);
        c.remove_item(k);
    }
    let fresh = Copper::new(&half, BodyOptions::default());
    assert_eq!((c.islands(), c.uncertain()), (fresh.islands(), fresh.uncertain()));
});
