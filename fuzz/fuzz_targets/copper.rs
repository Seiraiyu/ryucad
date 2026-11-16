//! Rings from bytes combined every way, grown and shrunk: every result
//! is simple polygons in order (`poly::faults`), points clear of every
//! input edge are classified as the exact windings say, and one thread
//! and eight give the same polygons. Plan 1.C adds connectivity.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryu_board::geom::Point;
use ryu_copper::measure::Verdict;
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
});
