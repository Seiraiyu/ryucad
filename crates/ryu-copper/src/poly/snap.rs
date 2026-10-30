//! Snap rounding (Hobby's hot pixels): every edge is routed through the
//! centres of the hot pixels it passes, a hot pixel being the unit square
//! about an edge's end or about a crossing of two edges rounded to the
//! nanometre. The routed edges, cut into fragments between consecutive
//! centres, meet only at their ends: no two cross, and a fragment lies
//! within half a pixel's diagonal (under a nanometre) of its edge.
//!
//! A pixel is half-open, `[x - 1/2, x + 1/2) × [y - 1/2, y + 1/2)`, so
//! pixels tile the plane and a crossing's pixel is its point rounded half
//! up. Tests run on doubled coordinates, all integers.

use std::cmp::Ordering;

use rayon::prelude::*;
use ryu_board::geom::{Point, Rect, orient};

use crate::measure::core::{Core, SPLIT_SIZE, SegP};
use crate::tree::Packed;
use crate::wide::Int;

/// An input edge, its ends in lexicographic order, with the operand it
/// belongs to and `+1` if it ran from `a` to `b`, `-1` if from `b` to `a`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct In {
    pub a: Point,
    pub b: Point,
    pub op: u8,
    pub w: i32,
}

/// A snapped fragment, `a < b` lexicographically, with how many times
/// each operand's edges run along it from `a` to `b` (less those from `b`
/// to `a`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Frag {
    pub a: Point,
    pub b: Point,
    pub m: [i32; 2],
}

/// The edges of a closed ring, each a step from one vertex to the next.
pub(crate) fn ring_edges(r: &[Point], op: u8, out: &mut Vec<In>) {
    let n = r.len();
    for k in 0..n {
        let (p, q) = (r[k], r[(k + 1) % n]);
        match p.cmp(&q) {
            Ordering::Less => out.push(In {
                a: p,
                b: q,
                op,
                w: 1,
            }),
            Ordering::Greater => out.push(In {
                a: q,
                b: p,
                op,
                w: -1,
            }),
            Ordering::Equal => {}
        }
    }
}

/// Boxes that between them hold the segment grown by `grow`: its own
/// box, or for a long slanted segment a box per stretch (as 1.A's trees
/// hold pieces).
fn boxes(a: Point, b: Point, grow: i64, out: &mut Vec<Rect>) {
    out.clear();
    let d = b.minus(a);
    if d.0.abs().min(d.1.abs()) < SPLIT_SIZE {
        let r = Rect::new(a, b);
        out.push(Rect {
            min: r.min.offset(-grow, -grow),
            max: r.max.offset(grow, grow),
        });
        return;
    }
    Core::Seg(SegP::new(a, b)).boxes(grow, out);
}

/// Edges handled together, each group with buffers of its own.
const CHUNK: usize = 1024;

/// The point where `a b` and `c d` cross, rounded half up to the
/// nanometre, when they cross at one point inside both (not at an end of
/// either, which is a hot pixel already).
pub(crate) fn crossing(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let d1 = orient(a, b, c);
    let d2 = orient(a, b, d);
    let d3 = orient(c, d, a);
    let d4 = orient(c, d, b);
    if d1 == 0 || d2 == 0 || d3 == 0 || d4 == 0 {
        return None;
    }
    if (d1 > 0) == (d2 > 0) || (d3 > 0) == (d4 > 0) {
        return None;
    }
    // a + (b - a) t with t = d3 / (d3 - d4), each coordinate rounded half
    // up: floor((2 u d3 + den) / (2 den)) with den = d3 - d4 > 0.
    let (num, den) = if d3 > d4 {
        (d3, d3 - d4)
    } else {
        (-d3, d4 - d3)
    };
    let u = b.minus(a);
    let at = |base: i64, du: i128| -> i64 {
        let n = Int::from(2 * du) * Int::from(num) + Int::from(den);
        base + floor_div(n, 2 * den)
    };
    Some(Point::new(at(a.x, u.0), at(a.y, u.1)))
}

/// `floor(n / d)` for `d > 0`, the quotient fitting an `i64`: estimated in
/// floating point and corrected exactly.
fn floor_div(n: Int, d: i128) -> i64 {
    let dd = Int::from(d);
    let mut q = (n.to_f64() / d as f64).floor() as i64;
    loop {
        let lo = Int::from(i128::from(q)) * dd;
        if lo > n {
            q -= 1;
            continue;
        }
        let hi = Int::from(i128::from(q) + 1) * dd;
        if hi <= n {
            q += 1;
            continue;
        }
        return q;
    }
}

/// A bound on the segment's parameter `t`, `n / d` with `d > 0`, open
/// where `t` may not equal it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bound {
    n: i128,
    d: i128,
    open: bool,
}

impl Bound {
    fn new(n: i128, d: i128, open: bool) -> Bound {
        if d < 0 {
            Bound { n: -n, d: -d, open }
        } else {
            Bound { n, d, open }
        }
    }

    fn cmp_value(&self, o: &Bound) -> Ordering {
        (self.n * o.d).cmp(&(o.n * self.d))
    }
}

/// Where the segment `a b` enters the pixel about `c`, if it meets it: the
/// least `t` in `[0, 1]` with `a + (b - a) t` in the pixel, and whether
/// that least value is only approached (the entry lies on an open side).
fn entry(a: Point, b: Point, c: Point) -> Option<Bound> {
    let mut lo = Bound::new(0, 1, false);
    let mut hi = Bound::new(1, 1, false);
    for (p, q, m) in [(a.x, b.x, c.x), (a.y, b.y, c.y)] {
        let (p, d) = (2 * i128::from(p), 2 * (i128::from(q) - i128::from(p)));
        let (l, h) = (2 * i128::from(m) - 1, 2 * i128::from(m) + 1);
        if d == 0 {
            if p < l || p >= h {
                return None;
            }
            continue;
        }
        // p + d t >= l and p + d t < h.
        let (low, high) = if d > 0 {
            (Bound::new(l - p, d, false), Bound::new(h - p, d, true))
        } else {
            (Bound::new(h - p, d, true), Bound::new(l - p, d, false))
        };
        match low.cmp_value(&lo) {
            Ordering::Greater => lo = low,
            Ordering::Equal => lo.open |= low.open,
            Ordering::Less => {}
        }
        match high.cmp_value(&hi) {
            Ordering::Less => hi = high,
            Ordering::Equal => hi.open |= high.open,
            Ordering::Greater => {}
        }
    }
    match lo.cmp_value(&hi) {
        Ordering::Less => Some(lo),
        Ordering::Equal if !lo.open && !hi.open => Some(lo),
        _ => None,
    }
}

/// Orders entries along a segment: by value, a closed entry before an
/// open one at the same value.
fn by_entry(x: &(Bound, Point), y: &(Bound, Point)) -> Ordering {
    x.0.cmp_value(&y.0).then(x.0.open.cmp(&y.0.open))
}

/// Every hot pixel's centre: each edge's ends and each crossing rounded.
fn hot_pixels(edges: &[In]) -> Vec<Point> {
    let mut entries = Vec::new();
    let mut bx = Vec::new();
    for (k, e) in edges.iter().enumerate() {
        boxes(e.a, e.b, 0, &mut bx);
        entries.extend(bx.iter().map(|r| (*r, k as u32)));
    }
    let tree = Packed::new(entries);
    let mut hot: Vec<Point> = edges
        .par_chunks(CHUNK)
        .enumerate()
        .flat_map_iter(|(c, chunk)| {
            let (mut bx, mut near, mut out) = (Vec::new(), Vec::new(), Vec::new());
            for (k, e) in chunk.iter().enumerate() {
                let i = (c * CHUNK + k) as u32;
                out.push(e.a);
                out.push(e.b);
                boxes(e.a, e.b, 0, &mut bx);
                near.clear();
                for r in &bx {
                    tree.query(r, &mut near);
                }
                near.sort_unstable();
                near.dedup();
                for &j in &near {
                    if j <= i {
                        continue;
                    }
                    let f = edges[j as usize];
                    if let Some(p) = crossing(e.a, e.b, f.a, f.b) {
                        out.push(p);
                    }
                }
            }
            out
        })
        .collect();
    hot.par_sort_unstable();
    hot.dedup();
    hot
}

/// The hot pixels' tree: each centre's box.
fn pixel_tree(hot: &[Point]) -> Packed {
    Packed::new(
        hot.iter()
            .enumerate()
            .map(|(k, p)| (Rect::of_point(*p), k as u32))
            .collect(),
    )
}

/// Buffers a chunk of edges reuses.
#[derive(Default)]
struct Scratch {
    boxes: Vec<Rect>,
    near: Vec<u32>,
    met: Vec<(Bound, Point)>,
    on: Vec<(i128, Point)>,
    chain: Vec<Point>,
    pts: Vec<Point>,
}

impl Scratch {
    /// The hot pixels whose boxes meet `a b`'s grown by `grow`, once each.
    fn near(&mut self, a: Point, b: Point, grow: i64, tree: &Packed) {
        boxes(a, b, grow, &mut self.boxes);
        self.near.clear();
        for r in &self.boxes {
            tree.query(r, &mut self.near);
        }
        self.near.sort_unstable();
        self.near.dedup();
    }

    /// The centres of the hot pixels `a b` passes, in order from `a` to
    /// `b`, into `chain`.
    fn route(&mut self, a: Point, b: Point, hot: &[Point], tree: &Packed) {
        self.near(a, b, 1, tree);
        self.met.clear();
        for &k in &self.near {
            let c = hot[k as usize];
            if let Some(t) = entry(a, b, c) {
                self.met.push((t, c));
            }
        }
        self.met.sort_by(by_entry);
        self.chain.clear();
        self.chain.extend(self.met.iter().map(|m| m.1));
    }

    /// `a`, the hot pixel centres lying exactly inside `a b` in order,
    /// and `b`, into `pts`: `a b` is a step of the last route, which lies
    /// in its edge's box grown by a nanometre, so the route's candidates
    /// hold every such centre.
    fn split(&mut self, a: Point, b: Point, hot: &[Point]) {
        let u = b.minus(a);
        let l = u.len2();
        self.on.clear();
        for &k in &self.near {
            let c = hot[k as usize];
            if c != a && c != b && orient(a, b, c) == 0 {
                let t = c.minus(a).dot(u);
                if t > 0 && t < l {
                    self.on.push((t, c));
                }
            }
        }
        self.on.sort_unstable();
        self.pts.clear();
        self.pts.push(a);
        self.pts.extend(self.on.iter().map(|o| o.1));
        self.pts.push(b);
    }
}

/// Snap-rounds the edges: fragments between hot pixel centres, each once,
/// with what every operand's edges along it add up to, and none whose
/// edges cancel.
pub(crate) fn fragments(mut edges: Vec<In>) -> Vec<Frag> {
    edges.par_sort_unstable();
    let hot = hot_pixels(&edges);
    let tree = pixel_tree(&hot);
    let mut frags: Vec<Frag> = edges
        .par_chunks(CHUNK)
        .flat_map_iter(|chunk| {
            let mut s = Scratch::default();
            let mut out = Vec::new();
            for e in chunk {
                let mut m = [0; 2];
                m[e.op as usize] = e.w;
                s.route(e.a, e.b, &hot, &tree);
                let chain = std::mem::take(&mut s.chain);
                for w in chain.windows(2) {
                    s.split(w[0], w[1], &hot);
                    for p in s.pts.windows(2) {
                        let (p, q) = (p[0], p[1]);
                        out.push(if p < q {
                            Frag { a: p, b: q, m }
                        } else {
                            Frag {
                                a: q,
                                b: p,
                                m: [-m[0], -m[1]],
                            }
                        });
                    }
                }
                s.chain = chain;
            }
            out
        })
        .collect();
        eprintln!(
            "  hot {t1:?} tree {:?} route {:?}",
            t2 - t1,
            t.elapsed() - t2
        );
    }
    frags.par_sort_unstable();
    let mut merged: Vec<Frag> = Vec::with_capacity(frags.len());
    for f in frags {
        match merged.last_mut() {
            Some(last) if (last.a, last.b) == (f.a, f.b) => {
                last.m[0] += f.m[0];
                last.m[1] += f.m[1];
            }
            _ => merged.push(f),
        }
    }
    merged.retain(|f| f.m != [0, 0]);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn crossings_round_half_up() {
        assert_eq!(
            crossing(p(0, 0), p(10, 10), p(0, 10), p(10, 0)),
            Some(p(5, 5))
        );
        // (0,0)-(3,1) and (0,1)-(3,0) cross at (1.5, 0.5): rounded up.
        assert_eq!(crossing(p(0, 0), p(3, 1), p(0, 1), p(3, 0)), Some(p(2, 1)));
        // Touching at an end, or parallel: no crossing.
        assert_eq!(crossing(p(0, 0), p(10, 0), p(5, 0), p(5, 5)), None);
        assert_eq!(crossing(p(0, 0), p(10, 0), p(0, 1), p(10, 1)), None);
        // (-0.5, -0.5) rounds to (0, 0); (-1.5) to -1.
        assert_eq!(
            crossing(p(-1, -1), p(0, 0), p(-1, 0), p(0, -1)),
            Some(p(0, 0))
        );
        assert_eq!(
            crossing(p(-2, -1), p(-1, 0), p(-2, 0), p(-1, -1)),
            Some(p(-1, 0))
        );
        // At the limits: still exact.
        let m = (1i64 << 41) - 1;
        assert_eq!(
            crossing(p(-m, -m), p(m, m), p(-m, m), p(m, -m)),
            Some(p(0, 0))
        );
    }

    #[test]
    fn pixels_are_half_open() {
        // A horizontal segment along y = 0.5 belongs to the pixels above.
        // In doubled terms a pixel about (0, 0) spans [-1, 1).
        assert!(entry(p(-5, 0), p(5, 0), p(0, 0)).is_some());
        assert!(entry(p(-5, 1), p(5, 1), p(0, 0)).is_none());
        // A segment from (0, 0) to (2, 1) passes (1, 0) and (1, 1)? Its
        // point at x = 1 is y = 0.5, in the pixel about (1, 1); it enters
        // (1, 0) at x = 0.5, y = 0.25.
        assert!(entry(p(0, 0), p(2, 1), p(1, 0)).is_some());
        assert!(entry(p(0, 0), p(2, 1), p(1, 1)).is_some());
        // Through a corner: the corner's own pixel is met at one point.
        let t = entry(p(0, 0), p(2, 2), p(1, 1)).unwrap();
        assert_eq!((t.n * 4, t.d), (t.d, t.d));
        // Through a corner the other way: (0.5, 1.5) belongs to the pixel
        // about (1, 2), so the segment misses the one about (0, 1).
        assert!(entry(p(0, 2), p(2, 0), p(0, 1)).is_none());
        assert!(entry(p(0, 2), p(2, 0), p(1, 2)).is_some());
    }

    #[test]
    fn edges_bend_through_the_hot_pixels_they_pass() {
        // A long edge passing within half a nanometre of another's end is
        // routed through it: (0, 0) to (100, 1) enters the pixel about
        // (50, 0) at x = 49.5, y = 0.495; at x = 59.5 it is 0.595 up,
        // clear of (60, 0)'s.
        let mut e = Vec::new();
        ring_edges(&[p(0, 0), p(100, 1), p(100, 2)], 0, &mut e);
        ring_edges(&[p(50, 0), p(60, 0), p(60, -5)], 1, &mut e);
        let f = fragments(e);
        let find = |a, b| f.iter().find(|f| (f.a, f.b) == (a, b)).map(|f| f.m);
        assert_eq!(find(p(0, 0), p(50, 0)), Some([1, 0]), "{f:?}");
        assert_eq!(find(p(50, 0), p(100, 1)), Some([1, 0]));
        assert_eq!(find(p(50, 0), p(60, 0)), Some([0, 1]));
    }

    #[test]
    fn coincident_edges_merge_and_cancel() {
        let mut e = Vec::new();
        ring_edges(&[p(0, 0), p(10, 0), p(10, 10), p(0, 10)], 0, &mut e);
        ring_edges(&[p(10, 0), p(20, 0), p(20, 10), p(10, 10)], 0, &mut e);
        let f = fragments(e);
        // The shared side runs both ways and goes.
        assert!(!f.iter().any(|f| f.a.x == 10 && f.b.x == 10));
        assert_eq!(f.len(), 6);
    }
}
