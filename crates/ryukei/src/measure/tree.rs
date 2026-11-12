//! A box tree over an entity's mesh pieces, and the nearest pair of
//! pieces between two trees.

use super::prims::{Prim, between, box_gap};
use crate::geom::Point3;

/// A piece and what it came from: an index the caller gives meaning to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    pub prim: Prim,
    pub tag: u32,
}

struct Node {
    lo: Point3,
    hi: Point3,
    /// Children, or the range of `pieces` a leaf holds.
    kids: Option<[usize; 2]>,
    range: [usize; 2],
}

pub(crate) struct Tree {
    pieces: Vec<Piece>,
    nodes: Vec<Node>,
}

/// The nearest pair found between two trees.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Nearest {
    pub distance: f64,
    pub a: (Point3, u32),
    pub b: (Point3, u32),
}

const LEAF: usize = 4;

fn bounds(pieces: &[Piece]) -> (Point3, Point3) {
    let mut lo = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pieces {
        for q in p.prim.points() {
            lo = lo.min(*q);
            hi = hi.max(*q);
        }
    }
    (lo, hi)
}

fn centre(p: &Piece, axis: usize) -> f64 {
    let pts = p.prim.points();
    let sum: f64 = pts
        .iter()
        .map(|q| match axis {
            0 => q.x,
            1 => q.y,
            _ => q.z,
        })
        .sum();
    sum / pts.len() as f64
}

impl Tree {
    pub(crate) fn new(mut pieces: Vec<Piece>) -> Self {
        let mut nodes = Vec::new();
        if !pieces.is_empty() {
            let n = pieces.len();
            build(&mut pieces, 0, n, &mut nodes);
        }
        Self { pieces, nodes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// The nearest pair of pieces of `self` and `other`, searched depth
    /// first nearer boxes first, pruned by the best so far.
    pub(crate) fn nearest(&self, other: &Tree) -> Option<Nearest> {
        if self.is_empty() || other.is_empty() {
            return None;
        }
        let mut best: Option<Nearest> = None;
        let mut stack = vec![(0usize, 0usize)];
        while let Some((i, j)) = stack.pop() {
            let (a, b) = (&self.nodes[i], &other.nodes[j]);
            let gap = box_gap([a.lo, a.hi], [b.lo, b.hi]);
            if best.is_some_and(|n| gap >= n.distance) {
                continue;
            }
            match (a.kids, b.kids) {
                (None, None) => {
                    for p in &self.pieces[a.range[0]..a.range[1]] {
                        for q in &other.pieces[b.range[0]..b.range[1]] {
                            let (d, x, y) = between(&p.prim, &q.prim);
                            if best.is_none_or(|n| d < n.distance) {
                                best = Some(Nearest {
                                    distance: d,
                                    a: (x, p.tag),
                                    b: (y, q.tag),
                                });
                            }
                        }
                    }
                }
                _ => {
                    // Split the larger node (or the one that can be).
                    let size = |n: &Node| (n.hi - n.lo).length();
                    let split_a = b.kids.is_none() || (a.kids.is_some() && size(a) >= size(b));
                    let mut next: Vec<(usize, usize)> = if split_a {
                        let [k0, k1] = a.kids.expect("an inner node");
                        vec![(k0, j), (k1, j)]
                    } else {
                        let [k0, k1] = b.kids.expect("an inner node");
                        vec![(i, k0), (i, k1)]
                    };
                    let gap_of = |&(x, y): &(usize, usize)| {
                        let (p, q) = (&self.nodes[x], &other.nodes[y]);
                        box_gap([p.lo, p.hi], [q.lo, q.hi])
                    };
                    // Farther first on the stack, so the nearer pops first.
                    next.sort_by(|u, v| gap_of(v).total_cmp(&gap_of(u)));
                    stack.extend(next);
                }
            }
        }
        best
    }

    /// The piece nearest `p`: the distance, its nearest point and its tag.
    /// The same answer, ties included, as [`Tree::nearest`] of a tree
    /// holding `p` alone against this one, which descends in this order,
    /// without building that tree or allocating: a node pops, its children
    /// are pushed farther first (the nearer on a tie of their gaps is the
    /// second), and a tree of `n` pieces is under `log2(n) + 1` deep, so the
    /// stack never holds more than 64.
    pub(crate) fn nearest_point(&self, p: Point3) -> Option<(f64, Point3, u32)> {
        if self.is_empty() {
            return None;
        }
        let gap = |n: &Node| box_gap([p, p], [n.lo, n.hi]);
        let mut best: Option<(f64, Point3, u32)> = None;
        let mut stack = [0usize; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let n = &self.nodes[stack[top]];
            if best.is_some_and(|b| gap(n) >= b.0) {
                continue;
            }
            match n.kids {
                None => {
                    for q in &self.pieces[n.range[0]..n.range[1]] {
                        let (d, _, y) = between(&Prim::Point(p), &q.prim);
                        if best.is_none_or(|b| d < b.0) {
                            best = Some((d, y, q.tag));
                        }
                    }
                }
                Some([k0, k1]) => {
                    assert!(top + 2 <= stack.len(), "a box tree deeper than 63");
                    // `nearest` sorts [k0, k1] farther first, stably, then
                    // pushes in that order.
                    let (g0, g1) = (gap(&self.nodes[k0]), gap(&self.nodes[k1]));
                    let (first, second) = if g1.total_cmp(&g0) == std::cmp::Ordering::Greater {
                        (k1, k0)
                    } else {
                        (k0, k1)
                    };
                    stack[top] = first;
                    stack[top + 1] = second;
                    top += 2;
                }
            }
        }
        best
    }

    /// The root's box, `None` for an empty tree.
    pub(crate) fn bounds(&self) -> Option<(Point3, Point3)> {
        self.nodes.first().map(|n| (n.lo, n.hi))
    }

    /// Every pair of pieces of `self` and `other` within `d` of each
    /// other, by tag, sorted.
    pub(crate) fn within(&self, other: &Tree, d: f64) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        if self.is_empty() || other.is_empty() {
            return out;
        }
        let mut stack = vec![(0usize, 0usize)];
        while let Some((i, j)) = stack.pop() {
            let (a, b) = (&self.nodes[i], &other.nodes[j]);
            if box_gap([a.lo, a.hi], [b.lo, b.hi]) > d {
                continue;
            }
            match (a.kids, b.kids) {
                (None, None) => {
                    for p in &self.pieces[a.range[0]..a.range[1]] {
                        for q in &other.pieces[b.range[0]..b.range[1]] {
                            if between(&p.prim, &q.prim).0 <= d {
                                out.push((p.tag, q.tag));
                            }
                        }
                    }
                }
                (Some([k0, k1]), None) => stack.extend([(k0, j), (k1, j)]),
                (None, Some([k0, k1])) => stack.extend([(i, k0), (i, k1)]),
                (Some([a0, a1]), Some([b0, b1])) => {
                    stack.extend([(a0, b0), (a0, b1), (a1, b0), (a1, b1)]);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The tags of the pieces in leaves whose boxes the segment `p`–`q`
    /// meets (each box grown by `pad`), sorted.
    pub(crate) fn along(&self, p: Point3, q: Point3, pad: f64) -> Vec<u32> {
        let mut out = Vec::new();
        if self.is_empty() {
            return out;
        }
        let d = q - p;
        let meets = |lo: Point3, hi: Point3| {
            let (mut t0, mut t1) = (0.0f64, 1.0f64);
            for (o, dv, l, h) in [
                (p.x, d.x, lo.x - pad, hi.x + pad),
                (p.y, d.y, lo.y - pad, hi.y + pad),
                (p.z, d.z, lo.z - pad, hi.z + pad),
            ] {
                if dv == 0.0 {
                    if o < l || o > h {
                        return false;
                    }
                    continue;
                }
                let (a, b) = ((l - o) / dv, (h - o) / dv);
                let (a, b) = if a <= b { (a, b) } else { (b, a) };
                t0 = t0.max(a);
                t1 = t1.min(b);
                if t0 > t1 {
                    return false;
                }
            }
            true
        };
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let n = &self.nodes[i];
            if !meets(n.lo, n.hi) {
                continue;
            }
            match n.kids {
                Some([a, b]) => stack.extend([a, b]),
                None => out.extend(self.pieces[n.range[0]..n.range[1]].iter().map(|p| p.tag)),
            }
        }
        out.sort_unstable();
        out
    }

    /// The tags of the pieces whose own boxes meet the box `lo`–`hi`
    /// (touching counts), sorted, into `out`, which is cleared first. A
    /// caller asking many times keeps `out`, so once it has grown nothing
    /// is allocated: the walk's stack is 64 nodes (see `nearest_point`).
    pub(crate) fn in_box(&self, lo: Point3, hi: Point3, out: &mut Vec<u32>) {
        out.clear();
        if self.is_empty() {
            return;
        }
        let meets = |a: Point3, b: Point3| {
            a.x <= hi.x && lo.x <= b.x && a.y <= hi.y && lo.y <= b.y && a.z <= hi.z && lo.z <= b.z
        };
        let mut stack = [0usize; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let n = &self.nodes[stack[top]];
            if !meets(n.lo, n.hi) {
                continue;
            }
            match n.kids {
                Some([a, b]) => {
                    assert!(top + 2 <= stack.len(), "a box tree deeper than 63");
                    stack[top] = a;
                    stack[top + 1] = b;
                    top += 2;
                }
                None => {
                    for p in &self.pieces[n.range[0]..n.range[1]] {
                        let pts = p.prim.points();
                        let (a, b) = pts[1..]
                            .iter()
                            .fold((pts[0], pts[0]), |(a, b), q| (a.min(*q), b.max(*q)));
                        if meets(a, b) {
                            out.push(p.tag);
                        }
                    }
                }
            }
        }
        out.sort_unstable();
    }
}

fn build(pieces: &mut [Piece], start: usize, end: usize, nodes: &mut Vec<Node>) -> usize {
    let (lo, hi) = bounds(&pieces[start..end]);
    let me = nodes.len();
    nodes.push(Node {
        lo,
        hi,
        kids: None,
        range: [start, end],
    });
    if end - start <= LEAF {
        return me;
    }
    let d = hi - lo;
    let axis = if d.x >= d.y && d.x >= d.z {
        0
    } else if d.y >= d.z {
        1
    } else {
        2
    };
    pieces[start..end].sort_by(|p, q| {
        centre(p, axis)
            .total_cmp(&centre(q, axis))
            .then(p.tag.cmp(&q.tag))
    });
    let mid = (start + end) / 2;
    let k0 = build(pieces, start, mid, nodes);
    let k1 = build(pieces, mid, end, nodes);
    nodes[me].kids = Some([k0, k1]);
    me
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_matches_brute_force() {
        let mut g = 0x1234_5678_u64;
        let mut next = || {
            g = g
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (g >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut tri = |dx: f64| {
            let mut p = || Point3::new(next() + dx, next(), next());
            Prim::Triangle([p(), p(), p()])
        };
        let a: Vec<Piece> = (0..60)
            .map(|k| Piece {
                prim: tri(0.0),
                tag: k,
            })
            .collect();
        let b: Vec<Piece> = (0..70)
            .map(|k| Piece {
                prim: tri(1.5),
                tag: k,
            })
            .collect();
        let brute = a
            .iter()
            .flat_map(|p| b.iter().map(move |q| between(&p.prim, &q.prim).0))
            .fold(f64::INFINITY, f64::min);
        let got = Tree::new(a).nearest(&Tree::new(b)).unwrap();
        assert_eq!(got.distance, brute);
    }
}
