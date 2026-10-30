//! Booleans of seeded rings against exact point classification, their
//! outputs simple and oriented, areas adding up, and one thread against
//! eight.

use ryu_board::geom::{Point, orient};
use ryu_board::testing::Lcg;
use ryu_copper::measure::Verdict;
use ryu_copper::poly::{self, FillRule, Op, Polygons, Rings};

/// Debug builds run a quarter of the cases: brute force over every pair
/// of edges is slow without optimization.
const CASES: usize = if cfg!(debug_assertions) { 1 } else { 4 };

/// A ring of `n` vertices in a box `reach` across: a star round a centre
/// (simple), or points anywhere (crossing itself).
fn seeded_ring(g: &mut Lcg, reach: i64) -> Vec<Point> {
    let n = g.range(3, 9) as usize;
    if g.coin() {
        let c = Point::new(g.range(0, reach), g.range(0, reach));
        let r = g.range(1, reach / 2 + 1) as f64;
        let mut angles: Vec<i64> = (0..n).map(|_| g.range(0, 6283)).collect();
        angles.sort_unstable();
        angles.dedup();
        let mut ring: Vec<Point> = angles
            .iter()
            .map(|&a| {
                let (s, co) = ryu_math::sin_cos(a as f64 / 1000.0);
                let k = g.range(30, 100) as f64 / 100.0;
                Point::new(
                    c.x + ryu_board::geom::round_nm(r * k * co),
                    c.y + ryu_board::geom::round_nm(r * k * s),
                )
            })
            .collect();
        if g.coin() {
            ring.reverse();
        }
        ring
    } else {
        (0..n)
            .map(|_| Point::new(g.range(0, reach), g.range(0, reach)))
            .collect()
    }
}

/// Rings as a shape's parts are flattened to: stars (simple), wound
/// counterclockwise, so each adds 0 or 1 to the winding.
fn wound_rings(g: &mut Lcg, reach: i64) -> Rings {
    let k = g.range(1, 4) as usize;
    let rings = (0..k)
        .map(|_| {
            loop {
                let mut r = seeded_ring(g, reach);
                let a = poly::area2(&r);
                // Only stars are sure to be simple: retry until one comes.
                if is_simple(&r) && a != 0 {
                    if a < 0 {
                        r.reverse();
                    }
                    break r;
                }
            }
        })
        .collect();
    Rings::new(rings, 0)
}

/// Whether a ring's edges meet only where consecutive ones join.
fn is_simple(r: &[Point]) -> bool {
    let n = r.len();
    let mut pts = r.to_vec();
    pts.sort();
    pts.dedup();
    if pts.len() != n {
        return false;
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (r[i], r[(i + 1) % n]);
            let (c, d) = (r[j], r[(j + 1) % n]);
            if j == i + 1 || (i == 0 && j == n - 1) {
                // Neighbours share one end: they mustn't fold back.
                let (p, q, s) = if j == i + 1 { (a, b, d) } else { (c, a, b) };
                if orient(p, q, s) == 0 && s.minus(q).dot(p.minus(q)) > 0 {
                    return false;
                }
                continue;
            }
            if bad_meeting(a, b, c, d) || a == c || a == d || b == c || b == d {
                return false;
            }
        }
    }
    true
}

fn seeded_rings(g: &mut Lcg, reach: i64) -> Rings {
    let k = g.range(1, 4) as usize;
    Rings::new((0..k).map(|_| seeded_ring(g, reach)).collect(), 0)
}

/// Whether two segments meet anywhere but at a shared end.
fn bad_meeting(a: Point, b: Point, c: Point, d: Point) -> bool {
    let on = |p: Point, q: Point, x: Point| {
        orient(p, q, x) == 0
            && x != p
            && x != q
            && x.minus(p).dot(q.minus(p)) > 0
            && x.minus(q).dot(p.minus(q)) > 0
    };
    if (a == c && b == d) || (a == d && b == c) {
        return true;
    }
    if on(a, b, c) || on(a, b, d) || on(c, d, a) || on(c, d, b) {
        return true;
    }
    let (d1, d2, d3, d4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    d1 != 0 && d2 != 0 && d3 != 0 && d4 != 0 && (d1 > 0) != (d2 > 0) && (d3 > 0) != (d4 > 0)
}

/// Checks a result is simple polygons in canonical order.
fn check_simple(p: &Polygons) {
    let faults = poly::faults(p);
    assert!(faults.is_empty(), "{faults:?}");
}

/// Whether `x` is more than `reach` from every edge of the rings.
fn clear_of(rings: &[&Rings], x: Point, reach: i128) -> bool {
    rings.iter().flat_map(|r| &r.rings).all(|r| {
        (0..r.len()).all(|k| {
            let (a, b) = (r[k], r[(k + 1) % r.len()]);
            let (u, v) = (b.minus(a), x.minus(a));
            let (t, l) = (u.dot(v), u.len2());
            if t <= 0 || l == 0 {
                v.len2() > reach * reach
            } else if t >= l {
                x.minus(b).len2() > reach * reach
            } else {
                let c = u.cross(v);
                c * c > reach * reach * l
            }
        })
    })
}

fn inside(r: &Rings, rule: FillRule, x: Point) -> bool {
    rule.inside(r.rings.iter().map(|r| poly::winding(r, x)).sum())
}

#[test]
fn booleans_classify_points_exactly() {
    let mut g = Lcg::new(0x626f_6f6c);
    let mut checked = 0;
    for case in 0..100 * CASES {
        let reach = if case % 2 == 0 { 40 } else { 2_000_000 };
        let (a, b) = (seeded_rings(&mut g, reach), seeded_rings(&mut g, reach));
        let rule_a = if g.coin() {
            FillRule::NonZero
        } else {
            FillRule::EvenOdd
        };
        let rule_b = if g.coin() {
            FillRule::NonZero
        } else {
            FillRule::Positive
        };
        for op in [Op::Union, Op::Intersection, Op::Difference, Op::Xor] {
            let out = poly::boolean(&a, rule_a, &b, rule_b, op);
            check_simple(&out);
            assert_eq!(out.bound, 1);
            for _ in 0..40 {
                let x = Point::new(g.range(-2, reach + 2), g.range(-2, reach + 2));
                if !clear_of(&[&a, &b], x, 2) {
                    continue;
                }
                let want = op.apply(inside(&a, rule_a, x), inside(&b, rule_b, x));
                let got = out.contains(x);
                assert_eq!(
                    got,
                    if want { Verdict::Yes } else { Verdict::No },
                    "case {case} {op:?} at {x:?}: {a:?} {b:?}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 2_500 * CASES, "{checked}");
}

/// The total length of the rings' edges, rounded up.
fn perimeter(r: &Rings) -> i128 {
    r.rings
        .iter()
        .map(|r| {
            (0..r.len())
                .map(|k| ryu_board::geom::ceil_sqrt(r[(k + 1) % r.len()].minus(r[k]).len2()))
                .sum::<i128>()
        })
        .sum()
}

#[test]
fn areas_add_up() {
    let mut g = Lcg::new(0x6172_6561);
    for case in 0..75 * CASES {
        let reach = if case % 2 == 0 { 50 } else { 5_000_000 };
        let a = poly::simplify(&seeded_rings(&mut g, reach), FillRule::NonZero);
        let b = poly::simplify(&seeded_rings(&mut g, reach), FillRule::NonZero);
        let u = poly::union(&a, &b);
        let i = poly::intersection(&a, &b);
        let d = poly::difference(&a, &b);
        let x = poly::xor(&a, &b);
        for p in [&u, &i, &d, &x] {
            check_simple(p);
        }
        // Each boolean moves an edge by under a nanometre: twice the area
        // moves by under twice the perimeter.
        let slack = 4 * (perimeter(&a.rings()) + perimeter(&b.rings())) + 8;
        let close = |l: i128, r: i128, what: &str| {
            assert!(
                (l - r).abs() <= slack,
                "case {case} {what}: {l} vs {r} (slack {slack})"
            );
        };
        close(
            u.area2() + i.area2(),
            a.area2() + b.area2(),
            "union and intersection",
        );
        close(
            d.area2() + i.area2(),
            a.area2(),
            "difference and intersection",
        );
        close(x.area2(), u.area2() - i.area2(), "exclusive or");
    }
}

#[test]
fn one_thread_and_eight_agree() {
    let mut g = Lcg::new(0x7468_7265);
    let cases: Vec<(Rings, Rings)> = (0..60)
        .map(|_| (seeded_rings(&mut g, 300), seeded_rings(&mut g, 300)))
        .collect();
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                cases
                    .iter()
                    .map(|(a, b)| {
                        poly::boolean(a, FillRule::NonZero, b, FillRule::EvenOdd, Op::Xor)
                    })
                    .collect::<Vec<_>>()
            })
    };
    assert_eq!(run(1), run(8));
}

#[test]
fn layers_take_the_last_part_drawn() {
    use ryu_board::model::Polarity;
    let mut g = Lcg::new(0x6c61_7965);
    let mut checked = 0;
    for case in 0..40 * CASES {
        let reach = if case % 2 == 0 { 60 } else { 3_000_000 };
        let parts: Vec<(Polarity, Rings)> = (0..g.range(1, 7))
            .map(|_| {
                let pol = if g.range(0, 3) == 0 {
                    Polarity::Negative
                } else {
                    Polarity::Positive
                };
                (pol, wound_rings(&mut g, reach))
            })
            .collect();
        let holes: Vec<Rings> = (0..g.range(0, 2))
            .map(|_| wound_rings(&mut g, reach))
            .collect();
        let out = poly::merge_layer(&parts, &holes);
        check_simple(&out);
        let all: Vec<&Rings> = parts.iter().map(|p| &p.1).chain(&holes).collect();
        for _ in 0..60 {
            let x = Point::new(g.range(-2, reach + 2), g.range(-2, reach + 2));
            if !clear_of(&all, x, 4) {
                continue;
            }
            let last = parts
                .iter()
                .rev()
                .find(|(_, r)| inside(r, FillRule::NonZero, x))
                .map(|p| p.0);
            let want = last == Some(Polarity::Positive)
                && !holes.iter().any(|h| inside(h, FillRule::NonZero, x));
            assert_eq!(
                out.contains(x),
                if want { Verdict::Yes } else { Verdict::No },
                "case {case} at {x:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 500 * CASES, "{checked}");
}

/// The distance from `x` to the polygons' outline, and whether they hold
/// it.
fn to_outline(p: &Polygons, x: Point) -> (f64, bool) {
    let mut d = f64::INFINITY;
    let mut inside = false;
    for poly in &p.polygons {
        if poly::winding(&poly.outer, x) != 0 && poly.holes.iter().all(|h| poly::winding(h, x) == 0)
        {
            inside = true;
        }
        for r in poly.rings() {
            for k in 0..r.len() {
                let (a, b) = (r[k], r[(k + 1) % r.len()]);
                let (ux, uy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
                let (vx, vy) = ((x.x - a.x) as f64, (x.y - a.y) as f64);
                let l = ux * ux + uy * uy;
                let t = if l == 0.0 {
                    0.0
                } else {
                    ((ux * vx + uy * vy) / l).clamp(0.0, 1.0)
                };
                let (ex, ey) = (vx - ux * t, vy - uy * t);
                d = d.min(ryu_math::sqrt(ex * ex + ey * ey));
            }
        }
    }
    (d, inside)
}

#[test]
fn offsets_lie_within_their_bounds() {
    use ryu_copper::poly::FlattenOptions;
    let mut g = Lcg::new(0x6f66_6673);
    let opts = FlattenOptions::default();
    let mut checked = 0;
    for case in 0..20 * CASES {
        let reach = 2_000_000;
        let p = poly::simplify(&seeded_rings(&mut g, reach), FillRule::NonZero);
        let d = g.range(1_000, reach / 8);
        let grown = poly::grow(&p, d, &opts);
        let shrunk = poly::shrink(&p, d, &opts);
        check_simple(&grown);
        check_simple(&shrunk);
        assert!(grown.bound <= p.bound + 52 && shrunk.bound <= p.bound + 52);
        for _ in 0..80 {
            let x = Point::new(
                g.range(-reach / 4, reach + reach / 4),
                g.range(-reach / 4, reach + reach / 4),
            );
            let (dist, inside) = to_outline(&p, x);
            let m = (grown.bound + 2) as f64;
            let outside_by = if inside { 0.0 } else { dist };
            if outside_by < d as f64 - m {
                assert_eq!(
                    grown.contains(x),
                    Verdict::Yes,
                    "case {case} grown at {x:?}"
                );
                checked += 1;
            } else if outside_by > d as f64 + m {
                assert_eq!(grown.contains(x), Verdict::No, "case {case} grown at {x:?}");
                checked += 1;
            }
            let m = (shrunk.bound + 2) as f64;
            if inside && dist > d as f64 + m {
                assert_eq!(
                    shrunk.contains(x),
                    Verdict::Yes,
                    "case {case} shrunk at {x:?}"
                );
            } else if !inside || dist < d as f64 - m {
                assert_eq!(
                    shrunk.contains(x),
                    Verdict::No,
                    "case {case} shrunk at {x:?}"
                );
            }
        }
    }
    assert!(checked > 1_000 * CASES, "{checked}");
}
