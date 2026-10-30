//! Measurement and the index against brute force: exact distances between
//! seeded segments, arcs (their true ends irrational) and shapes against
//! dense sampling, the exact comparisons consistent with the distance,
//! and every index query against a scan of every item.

use std::cmp::Ordering;

use ryu_board::geom::{Arc, Contour, Dir, Point};
use ryu_board::model::Library;
use ryu_board::shape::{Piece, feature_shape};
use ryu_board::testing::{Lcg, seeded_features};
use ryu_copper::measure::core::{ArcP, Core, Pt, SegP, edge_cores};
use ryu_copper::measure::{self, Body, Solid};

fn point(g: &mut Lcg, reach: i64) -> Point {
    Point::new(g.range(-reach, reach), g.range(-reach, reach))
}

/// An arc whose written end lies up to `off` nanometres off its circle.
fn seeded_arc(g: &mut Lcg, reach: i64, off: i64) -> Arc {
    let c = point(g, reach);
    let r = g.range(10, reach) as f64;
    let at = |t: f64, r: f64| {
        let (s, co) = ryu_math::sin_cos(t);
        Point::new(c.x + (r * co).round() as i64, c.y + (r * s).round() as i64)
    };
    let a0 = g.range(0, 6283) as f64 / 1000.0;
    let a1 = g.range(0, 6283) as f64 / 1000.0;
    let start = at(a0, r);
    let end = if g.range(0, 9) == 0 {
        start
    } else {
        at(a1, r + g.range(-off, off) as f64)
    };
    Arc {
        start,
        end,
        centre: c,
        dir: if g.coin() { Dir::Ccw } else { Dir::Cw },
    }
}

/// One to three pieces: a segment, a point, or an arc with its join.
fn seeded_cores(g: &mut Lcg, reach: i64) -> Vec<Core> {
    match g.range(0, 4) {
        0 | 1 => vec![Core::Seg(SegP::new(point(g, reach), point(g, reach)))],
        2 => {
            let p = point(g, reach);
            vec![Core::Seg(SegP::new(p, p))]
        }
        _ => {
            let a = seeded_arc(g, reach, 3);
            if a.is_degenerate() {
                return vec![Core::Seg(SegP::new(a.start, a.end))];
            }
            let mut out = Vec::new();
            edge_cores(&ryu_board::geom::Edge::Arc(a), &mut out);
            out
        }
    }
}

/// Points along a piece, at most `step` apart.
fn samples(c: &Core, step: f64) -> Vec<(f64, f64)> {
    match c {
        Core::Seg(s) => {
            let [a, b] = s.ends();
            let (a, b) = (a.approx(), b.approx());
            let len = dist(a, b);
            let n = (len / step).ceil().max(1.0) as usize;
            (0..=n)
                .map(|k| {
                    let t = k as f64 / n as f64;
                    (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1))
                })
                .collect()
        }
        Core::Arc(a) => {
            let r = a.arc.radius();
            let sweep = a.arc.sweep();
            let n = (r * sweep / step).ceil().max(1.0) as usize;
            let s = a.arc.start.minus(a.arc.centre);
            let a0 = ryu_math::atan2(s.1 as f64, s.0 as f64);
            let sign = if a.arc.dir == Dir::Ccw { 1.0 } else { -1.0 };
            let (cx, cy) = (a.arc.centre.x as f64, a.arc.centre.y as f64);
            let mut out: Vec<_> = (0..n)
                .map(|k| {
                    let t = a0 + sign * sweep * k as f64 / n as f64;
                    {
                        let (sin, cos) = ryu_math::sin_cos(t);
                        (cx + r * cos, cy + r * sin)
                    }
                })
                .collect();
            out.push(a.q.approx());
            out
        }
    }
}

fn body(cores: &[Core]) -> Body {
    Body::new(
        cores
            .iter()
            .map(|&core| Solid::Thick { core, w: 0 })
            .collect(),
        0,
    )
}

fn dist(p: (f64, f64), q: (f64, f64)) -> f64 {
    let (dx, dy) = (p.0 - q.0, p.1 - q.1);
    (dx * dx + dy * dy).sqrt()
}

/// The least distance between dense samples of both pieces: samples 4 nm
/// apart find where it may be (within 10 nm of their least), then
/// samples `step` apart within 5 nm of those settle it.
fn sampled(a: &[Core], b: &[Core], step: f64) -> f64 {
    let ca: Vec<_> = a.iter().flat_map(|c| samples(c, 4.0)).collect();
    let cb: Vec<_> = b.iter().flat_map(|c| samples(c, 4.0)).collect();
    let mut coarse = f64::MAX;
    for p in &ca {
        for q in &cb {
            coarse = coarse.min(dist(*p, *q));
        }
    }
    let near_a: Vec<_> = ca
        .iter()
        .copied()
        .filter(|p| cb.iter().any(|q| dist(*p, *q) <= coarse + 10.0))
        .collect();
    let near_b: Vec<_> = cb
        .iter()
        .copied()
        .filter(|q| ca.iter().any(|p| dist(*p, *q) <= coarse + 10.0))
        .collect();
    let close = |p: &(f64, f64), to: &[(f64, f64)]| to.iter().any(|q| dist(*p, *q) <= 5.0);
    let fa: Vec<_> = a
        .iter()
        .flat_map(|c| samples(c, step))
        .filter(|p| close(p, &near_a))
        .collect();
    let fb: Vec<_> = b
        .iter()
        .flat_map(|c| samples(c, step))
        .filter(|q| close(q, &near_b))
        .collect();
    let mut best = f64::MAX;
    for p in &fa {
        for q in &fb {
            best = best.min(dist(*p, *q));
        }
    }
    best
}

/// The distance is consistent with the comparisons either side of it.
fn consistent(a: &Body, b: &Body, d: &measure::Distance) {
    let n = i128::from(d.nm);
    if n > 0 {
        assert_ne!(measure::compare(a, b, 2 * n - 1), Ordering::Less);
    }
    assert_ne!(measure::compare(a, b, 2 * n + 1), Ordering::Greater);
    assert_eq!(d.exact, measure::compare(a, b, 2 * n) == Ordering::Equal);
}

#[test]
fn piece_distances_against_sampling() {
    let mut g = Lcg::new(0x6d65_6173);
    // Samples a quarter nanometre apart lie within an eighth of one of
    // every point.
    let step = 0.25;
    for case in 0..400 {
        let a = seeded_cores(&mut g, 1_000);
        let b = seeded_cores(&mut g, 1_000);
        let (ba, bb) = (body(&a), body(&b));
        let d = measure::distance(&ba, &bb).unwrap();
        consistent(&ba, &bb, &d);
        let s = sampled(&a, &b, step);
        // The true distance is at most the sampled one and at least it
        // less the sampling's reach; rounding adds half a nanometre.
        assert!(
            (d.nm as f64) <= s + 0.5 + 1e-6 && (d.nm as f64) >= s - step - 0.5 - 1e-6,
            "case {case}: {} against {s}\n{a:?}\n{b:?}",
            d.nm
        );
        let back = measure::distance(&bb, &ba).unwrap();
        assert_eq!((back.nm, back.exact), (d.nm, d.exact), "case {case}");
    }
}

#[test]
fn true_ends_are_where_the_ray_meets_the_circle() {
    let mut g = Lcg::new(0x656e_6473);
    for _ in 0..200 {
        let a = seeded_arc(&mut g, 100_000, 50);
        if a.is_degenerate() || a.is_full() {
            continue;
        }
        let p = ArcP::new(a);
        let (x, y) = p.q.approx();
        let r = a.radius();
        let (dx, dy) = (x - a.centre.x as f64, y - a.centre.y as f64);
        assert!(((dx * dx + dy * dy).sqrt() - r).abs() < 1e-6);
        if let Pt::End { .. } = p.q {
            let e = a.end.minus(a.centre);
            assert!((dx * e.1 as f64 - dy * e.0 as f64).abs() < 1e-3 * r);
        }
    }
}

/// A contour as points with chords within 0.05 nm of its arcs.
fn polygon(c: &Contour) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for e in c.edges() {
        let mut cores = Vec::new();
        edge_cores(e, &mut cores);
        for core in &cores {
            let mut s = match core {
                Core::Seg(g) => g.ends().iter().map(Pt::approx).collect(),
                // A chord L long on radius r sags L² / 8r: 0.05 nm at most.
                Core::Arc(a) => samples(core, (0.4 * a.arc.radius()).sqrt().max(0.05)),
            };
            s.pop();
            out.extend(s);
        }
    }
    out
}

fn winding(polys: &[Vec<(f64, f64)>], p: (f64, f64)) -> i32 {
    let mut w = 0;
    for poly in polys {
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            let side = (b.0 - a.0) * (p.1 - a.1) - (p.0 - a.0) * (b.1 - a.1);
            if a.1 <= p.1 && b.1 > p.1 && side > 0.0 {
                w += 1;
            } else if a.1 > p.1 && b.1 <= p.1 && side < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

#[test]
fn regions_contain_what_their_outlines_enclose() {
    let mut g = Lcg::new(0x7265_6769);
    let f = seeded_features(&mut g, 24, 150, 200_000);
    let lib = Library::default();
    let mut checked = 0;
    for k in 0..f.features().len() {
        let Ok(shape) = feature_shape(&f, k, &lib) else {
            continue;
        };
        for part in &shape.parts {
            let Piece::Region(r) = &part.piece else {
                continue;
            };
            let shell = ryu_copper::measure::shell_of(r);
            let polys: Vec<_> = std::iter::once(&r.outer)
                .chain(&r.holes)
                .map(polygon)
                .collect();
            let b = r.bounds();
            let outline = Body::new(
                shell
                    .cores
                    .iter()
                    .map(|&core| Solid::Thick { core, w: 0 })
                    .collect(),
                0,
            );
            for _ in 0..40 {
                let p = Point::new(
                    g.range(b.min.x - 50, b.max.x + 50),
                    g.range(b.min.y - 50, b.max.y + 50),
                );
                // Far enough from the outline for the chords to agree.
                if measure::within(&outline, &Body::point(p), 2).is_yes() {
                    continue;
                }
                let want = winding(&polys, (p.x as f64, p.y as f64)) != 0;
                assert_eq!(shell.contains(p), want, "feature {k} at {p:?}");
                checked += 1;
            }
        }
    }
    assert!(checked > 2000, "{checked}");
}
