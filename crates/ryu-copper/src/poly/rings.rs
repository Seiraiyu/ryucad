//! Rings from the fragments a result keeps, and polygons from rings.
//!
//! A fragment is kept where the result's inside differs either side of
//! it, turned so the inside is on its left. Around a vertex the kept
//! fragments then alternate in and out, and following each in to the out
//! first clockwise from it walks the boundary of one inside sector: rings
//! that touch at a vertex come out apart, each simple. Counterclockwise
//! rings are outer, clockwise ones holes, each given to the smallest
//! outer ring holding it.

use std::collections::HashMap;

use rayon::prelude::*;
use ryu_board::geom::{Point, Rect, V, orient};

use super::snap::Frag;
use super::sweep::Sides;
use super::{Polygon, area2, ring_bounds};
use crate::tree::Packed;

/// Which half-turn counterclockwise from `r` the direction `d` lies in:
/// 0 for `[0, π)`, 1 for `[π, 2π)`.
fn half(r: V, d: V) -> u8 {
    let c = r.cross(d);
    if c > 0 || (c == 0 && r.dot(d) > 0) {
        0
    } else {
        1
    }
}

/// Whether `d1` lies before `d2` going counterclockwise from `r`.
fn ccw_before(r: V, d1: V, d2: V) -> bool {
    let (h1, h2) = (half(r, d1), half(r, d2));
    if h1 != h2 {
        return h1 < h2;
    }
    d1.cross(d2) > 0
}

/// Removes vertices between two collinear steps going on the same way,
/// where nothing else meets the ring (`busy` holds vertices other kept
/// fragments meet).
fn tidy(r: Vec<Point>, busy: &dyn Fn(Point) -> bool) -> Vec<Point> {
    let removable = |a: Point, v: Point, b: Point| {
        !busy(v) && orient(a, v, b) == 0 && v.minus(a).dot(b.minus(v)) > 0
    };
    let mut s: Vec<Point> = Vec::with_capacity(r.len());
    for v in r {
        while s.len() >= 2 && removable(s[s.len() - 2], s[s.len() - 1], v) {
            s.pop();
        }
        s.push(v);
    }
    // Where the ring closes.
    while s.len() > 3 {
        let n = s.len();
        if removable(s[n - 2], s[n - 1], s[0]) {
            s.pop();
        } else if removable(s[n - 1], s[0], s[1]) {
            s.remove(0);
        } else {
            break;
        }
    }
    s
}

/// Turns a ring to start at its least vertex.
fn rotated(mut r: Vec<Point>) -> Vec<Point> {
    let k = (0..r.len()).min_by_key(|&k| r[k]).unwrap_or(0);
    r.rotate_left(k);
    r
}

/// The kept fragments, inside on the left, as closed rings.
fn trace(kept: &[(Point, Point)]) -> Vec<Vec<Point>> {
    // Outgoing steps by their start.
    let mut out: HashMap<Point, Vec<usize>> = HashMap::new();
    for (k, &(a, _)) in kept.iter().enumerate() {
        out.entry(a).or_default().push(k);
    }
    let next = |k: usize| -> usize {
        let (a, v) = kept[k];
        let cands = &out[&v];
        if cands.len() == 1 {
            return cands[0];
        }
        // First clockwise from back along the step: last counterclockwise.
        let r = a.minus(v);
        let mut best = cands[0];
        for &c in &cands[1..] {
            let (d1, d2) = (kept[best].1.minus(v), kept[c].1.minus(v));
            if ccw_before(r, d1, d2) {
                best = c;
            }
        }
        best
    };
    let mut used = vec![false; kept.len()];
    let mut rings = Vec::new();
    for s in 0..kept.len() {
        if used[s] {
            continue;
        }
        let mut ring = Vec::new();
        let mut k = s;
        while !used[k] {
            used[k] = true;
            ring.push(kept[k].0);
            k = next(k);
        }
        rings.push(ring);
    }
    rings
}

/// The winding of a ring round `q`, in doubled coordinates (`q` is twice
/// a point, so a midpoint is whole), `q` not on the ring.
fn winding2(r: &[Point], edges: Option<&Packed>, q: (i128, i128)) -> i32 {
    let n = r.len();
    let mut w = 0;
    let mut visit = |k: usize| {
        let (a, b) = (r[k], r[(k + 1) % n]);
        let (ax, ay) = (2 * i128::from(a.x), 2 * i128::from(a.y));
        let (bx, by) = (2 * i128::from(b.x), 2 * i128::from(b.y));
        let o = (bx - ax) * (q.1 - ay) - (by - ay) * (q.0 - ax);
        if ay <= q.1 {
            if by > q.1 && o > 0 {
                w += 1;
            }
        } else if by <= q.1 && o < 0 {
            w -= 1;
        }
    };
    match edges {
        None => (0..n).for_each(&mut visit),
        Some(t) => {
            // Only edges meeting the ray from `q` along +x can cross it.
            let y = (q.1.div_euclid(2)) as i64;
            let x = (q.0.div_euclid(2)) as i64;
            let ray = Rect::new(Point::new(x, y), Point::new(i64::MAX / 4, y + 1));
            let mut near = Vec::new();
            t.query(&ray, &mut near);
            near.sort_unstable();
            near.dedup();
            near.into_iter().for_each(|k| visit(k as usize));
        }
    }
    w
}

/// Rings with this many vertices or more get a tree of their edges for
/// holes' tests.
const TREE_AT: usize = 64;

/// Polygons from the fragments where `inside` differs either side.
pub(crate) fn assemble(
    frags: &[Frag],
    sides: &[Sides],
    inside: impl Fn([i32; 2]) -> bool + Sync,
) -> Vec<Polygon> {
    let kept: Vec<(Point, Point)> = frags
        .par_iter()
        .zip(sides)
        .filter_map(|(f, s)| match (inside(s.above), inside(s.below)) {
            (true, false) => Some((f.a, f.b)),
            (false, true) => Some((f.b, f.a)),
            _ => None,
        })
        .collect();
    let mut degree: HashMap<Point, u32> = HashMap::new();
    for &(a, _) in &kept {
        *degree.entry(a).or_default() += 1;
    }
    let busy = |v: Point| degree.get(&v).copied().unwrap_or(0) > 1;
    let rings: Vec<Vec<Point>> = trace(&kept)
        .into_iter()
        .map(|r| rotated(tidy(r, &busy)))
        .collect();
    let (mut outers, holes): (Vec<_>, Vec<_>) = rings
        .into_iter()
        .map(|r| (area2(&r), r))
        .partition(|(a, _)| *a > 0);
    outers.sort_by(|x, y| x.1.cmp(&y.1));
    let trees: Vec<Option<Packed>> = outers
        .par_iter()
        .map(|(_, r)| {
            (r.len() >= TREE_AT).then(|| {
                Packed::new(
                    (0..r.len())
                        .map(|k| (Rect::new(r[k], r[(k + 1) % r.len()]), k as u32))
                        .collect(),
                )
            })
        })
        .collect();
    let boxes: Vec<Rect> = outers.iter().map(|(_, r)| ring_bounds(r)).collect();
    let outer_tree = Packed::new(
        boxes
            .iter()
            .enumerate()
            .map(|(k, b)| (*b, k as u32))
            .collect(),
    );
    // Each hole's owner: the smallest outer ring round its first edge's
    // middle, which lies on no other ring.
    let owners: Vec<Option<usize>> = holes
        .par_iter()
        .map(|(_, h)| {
            let q = (
                i128::from(h[0].x) + i128::from(h[1].x),
                i128::from(h[0].y) + i128::from(h[1].y),
            );
            let hb = ring_bounds(h);
            let mut near = Vec::new();
            outer_tree.query(&hb, &mut near);
            near.sort_unstable();
            near.into_iter()
                .map(|k| k as usize)
                .filter(|&k| boxes[k].contains_rect(&hb))
                .filter(|&k| winding2(&outers[k].1, trees[k].as_ref(), q) != 0)
                .min_by_key(|&k| (outers[k].0, k))
        })
        .collect();
    let mut polygons: Vec<Polygon> = outers
        .into_iter()
        .map(|(_, outer)| Polygon {
            outer,
            holes: Vec::new(),
        })
        .collect();
    for ((_, h), owner) in holes.into_iter().zip(owners) {
        if let Some(k) = owner {
            polygons[k].holes.push(h);
        }
    }
    for p in &mut polygons {
        p.holes.sort();
    }
    polygons
}
