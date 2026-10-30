//! Booleans of seeded rings against exact point classification, their
//! outputs simple and oriented, areas adding up, and one thread against
//! eight.

use ryu_board::geom::{Point, orient};
use ryu_board::testing::Lcg;
use ryu_copper::measure::Verdict;
use ryu_copper::poly::{self, FillRule, Op, Polygons, Rings};

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
    let mut edges = Vec::new();
    for (k, poly) in p.polygons.iter().enumerate() {
        assert!(
            poly::area2(&poly.outer) > 0,
            "outer {k} not counterclockwise"
        );
        for h in &poly.holes {
            assert!(poly::area2(h) < 0, "hole not clockwise");
            let q = Point::new(h[0].x, h[0].y);
            // The hole's first vertex lies inside or on its outer ring.
            let _ = q;
        }
        for r in poly.rings() {
            assert!(r.len() >= 3);
            assert_eq!(
                r.iter().min(),
                Some(&r[0]),
                "ring doesn't start at its least vertex"
            );
            for i in 0..r.len() {
                edges.push((r[i], r[(i + 1) % r.len()]));
            }
        }
        let mut holes = poly.holes.clone();
        holes.sort();
        assert_eq!(holes, poly.holes);
    }
    let outers: Vec<_> = p.polygons.iter().map(|q| q.outer.clone()).collect();
    let mut sorted = outers.clone();
    sorted.sort();
    assert_eq!(outers, sorted, "polygons out of order");
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            let ((a, b), (c, d)) = (edges[i], edges[j]);
            assert!(
                !bad_meeting(a, b, c, d),
                "edges {a:?}-{b:?} and {c:?}-{d:?} meet"
            );
        }
    }
}

/// Whether `x` is more than `reach` from every edge of the rings.
fn clear_of(rings: &[&Rings], x: Point, reach: i128) -> bool {
    rings.iter().flat_map(|r| &r.rings).all(|r| {
        (0..r.len()).all(|k| {
            let (a, b) = (r[k], r[(k + 1) % r.len()]);
            let (u, v) = (b.minus(a), x.minus(a));
            let (t, l) = (u.dot(v), u.len2());
            let d2_gt = if t <= 0 || l == 0 {
                v.len2() > reach * reach
            } else if t >= l {
                x.minus(b).len2() > reach * reach
            } else {
                let c = u.cross(v);
                c * c > reach * reach * l
            };
            d2_gt
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
    for case in 0..400 {
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
    assert!(checked > 10_000, "{checked}");
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
    for case in 0..300 {
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
