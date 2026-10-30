//! Packed R-trees: boxes bulk-loaded in Hilbert order, 16 to a node.
//! The index keeps one per layer; a region keeps one of its pieces.

use ryu_board::geom::{Nm, Point, Rect};

/// Children per node.
const FANOUT: usize = 16;

/// The Hilbert index of a point on a grid of 2^32 cells a side, each 512
/// nm, over the board's ±2^40 nm.
fn hilbert(p: Point) -> u64 {
    let cell = |v: i64| ((v.clamp(-(1 << 40), (1 << 40) - 1) + (1 << 40)) >> 9) as u64;
    let (mut x, mut y) = (cell(p.x), cell(p.y));
    let mut d = 0u64;
    let mut s = 1u64 << 31;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        // Rotate the quadrant.
        if ry == 0 {
            if rx == 1 {
                x = ((1 << 32) - 1) - x;
                y = ((1 << 32) - 1) - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s >>= 1;
    }
    d
}

fn centre(r: &Rect) -> Point {
    Point::new(r.min.x / 2 + r.max.x / 2, r.min.y / 2 + r.max.y / 2)
}

pub fn meets(a: &Rect, b: &Rect) -> bool {
    a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.y <= b.max.y && b.min.y <= a.max.y
}

pub fn grown(r: &Rect, d: Nm) -> Rect {
    Rect {
        min: r.min.offset(-d, -d),
        max: r.max.offset(d, d),
    }
}

/// A packed R-tree: boxes of entries, then of groups of 16, up to a root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Packed {
    /// Each entry's box and slot, in Hilbert order.
    pub entries: Vec<(Rect, u32)>,
    /// `levels[0]` holds the boxes of groups of 16 entries, each next
    /// level the boxes of groups of 16 of the one below.
    levels: Vec<Vec<Rect>>,
}

impl Packed {
    pub fn new(mut entries: Vec<(Rect, u32)>) -> Packed {
        entries.sort_by_cached_key(|(r, s)| (hilbert(centre(r)), *s, r.min, r.max));
        let mut levels = Vec::new();
        let mut below: Vec<Rect> = entries.iter().map(|e| e.0).collect();
        while below.len() > 1 {
            let up: Vec<Rect> = below
                .chunks(FANOUT)
                .map(|c| c.iter().copied().reduce(Rect::union).expect("a chunk"))
                .collect();
            levels.push(up.clone());
            below = up;
        }
        Packed { entries, levels }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The box holding every entry, if any.
    pub fn bounds(&self) -> Option<Rect> {
        match self.levels.last() {
            Some(top) => Some(top[0]),
            None => self.entries.first().map(|e| e.0),
        }
    }

    /// Every entry whose box meets `q`.
    pub fn query(&self, q: &Rect, out: &mut Vec<u32>) {
        match self.levels.len() {
            0 => out.extend(self.entries.iter().filter(|e| meets(&e.0, q)).map(|e| e.1)),
            n => self.descend(n, 0, q, out),
        }
    }

    /// Visits node `i` of `levels[level - 1]`.
    fn descend(&self, level: usize, i: usize, q: &Rect, out: &mut Vec<u32>) {
        if !meets(&self.levels[level - 1][i], q) {
            return;
        }
        let first = i * FANOUT;
        if level == 1 {
            let last = (first + FANOUT).min(self.entries.len());
            out.extend(
                self.entries[first..last]
                    .iter()
                    .filter(|e| meets(&e.0, q))
                    .map(|e| e.1),
            );
        } else {
            let last = (first + FANOUT).min(self.levels[level - 2].len());
            for c in first..last {
                self.descend(level - 1, c, q, out);
            }
        }
    }

    /// A node's box: `level` 0 is the entries.
    fn node(&self, level: usize, i: usize) -> Rect {
        if level == 0 {
            self.entries[i].0
        } else {
            self.levels[level - 1][i]
        }
    }

    /// The nodes under one: the root's are listed by [`Self::roots`].
    fn children(&self, level: usize, i: usize) -> impl Iterator<Item = (usize, usize)> {
        let below = if level == 1 {
            self.entries.len()
        } else {
            self.levels[level - 2].len()
        };
        let first = i * FANOUT;
        (first..(first + FANOUT).min(below)).map(move |c| (level - 1, c))
    }

    fn roots(&self) -> Vec<(usize, usize)> {
        match self.levels.len() {
            0 => (0..self.entries.len()).map(|i| (0, i)).collect(),
            n => vec![(n, 0)],
        }
    }

    /// Pairs of entries of two trees, nearest first, while they may lie
    /// no farther apart than the limit: `visit` gets each pair's slots and
    /// returns the limit from then on (a nearest search lowers it as it
    /// finds nearer pairs), or `None` to stop.
    ///
    /// How far apart two nodes may lie is the gap between their boxes, or
    /// where one is an entry, what `near_a` or `near_b` says of that
    /// entry's slot and the other node's box when that is more (a long
    /// diagonal segment's box says little). Both must be lower bounds, or
    /// anything over the limit they are given when the truth is too.
    /// Pairs at an equal bound come in a fixed order.
    pub fn nearest_pairs(
        &self,
        other: &Packed,
        limit: f64,
        near_a: impl Fn(u32, &Rect, f64) -> f64,
        near_b: impl Fn(u32, &Rect, f64) -> f64,
        mut visit: impl FnMut(u32, u32) -> Option<f64>,
    ) {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        type Node = (usize, usize);
        let bound = |a: Node, b: Node, limit: f64| {
            let (ra, rb) = (self.node(a.0, a.1), other.node(b.0, b.1));
            let mut g = ryu_math::sqrt(gap2(&ra, &rb) as f64);
            if a.0 == 0 && g <= limit {
                g = g.max(near_a(self.entries[a.1].1, &rb, limit));
            }
            if b.0 == 0 && g <= limit {
                g = g.max(near_b(other.entries[b.1].1, &ra, limit));
            }
            g
        };
        // Bounds are never negative, so their bits order as they do.
        let mut heap: BinaryHeap<Reverse<(u64, Node, Node)>> = BinaryHeap::new();
        let push = |heap: &mut BinaryHeap<_>, a: Node, b: Node, limit: f64| {
            let g = bound(a, b, limit);
            if g <= limit {
                heap.push(Reverse((g.max(0.0).to_bits(), a, b)));
            }
        };
        for a in self.roots() {
            for b in other.roots() {
                push(&mut heap, a, b, limit);
            }
        }
        let mut limit = limit;
        while let Some(Reverse((g, a, b))) = heap.pop() {
            if f64::from_bits(g) > limit {
                break;
            }
            match (a.0, b.0) {
                (0, 0) => match visit(self.entries[a.1].1, other.entries[b.1].1) {
                    Some(l) => limit = l,
                    None => return,
                },
                // Open the higher node (the first, on a tie).
                (la, lb) if la >= lb => {
                    for c in self.children(a.0, a.1) {
                        push(&mut heap, c, b, limit);
                    }
                }
                _ => {
                    for c in other.children(b.0, b.1) {
                        push(&mut heap, a, c, limit);
                    }
                }
            }
        }
    }
}

/// The squared gap between two boxes, each way: zero where they overlap.
pub fn gap2(a: &Rect, b: &Rect) -> i128 {
    let gx = (b.min.x - a.max.x).max(a.min.x - b.max.x).max(0);
    let gy = (b.min.y - a.max.y).max(a.min.y - b.max.y).max(0);
    i128::from(gx) * i128::from(gx) + i128::from(gy) * i128::from(gy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn hilbert_order_is_fixed() {
        // The four quadrants of the first split, in curve order.
        let q = 1 << 39;
        let v = [
            hilbert(p(-q, -q)),
            hilbert(p(-q, q)),
            hilbert(p(q, q)),
            hilbert(p(q, -q)),
        ];
        assert!(v.windows(2).all(|w| w[0] < w[1]), "{v:?}");
        // Neighbouring cells are neighbours along the curve.
        assert_eq!(hilbert(p(-(1 << 40), -(1 << 40))), 0);
        let l = -(1 << 40);
        assert!(hilbert(p(l, l + 512)) == 1 || hilbert(p(l + 512, l)) == 1);
    }

    #[test]
    fn packed_queries_find_every_meeting_box() {
        let entries: Vec<(Rect, u32)> = (0..1000)
            .map(|k| {
                let (x, y) = ((k % 40) * 1000, (k / 40) * 1000);
                (Rect::new(p(x, y), p(x + 500, y + 500)), k as u32)
            })
            .collect();
        let t = Packed::new(entries.clone());
        let q = Rect::new(p(4_200, 3_200), p(9_100, 5_000));
        let mut got = Vec::new();
        t.query(&q, &mut got);
        got.sort();
        let want: Vec<u32> = entries
            .iter()
            .filter(|(r, _)| meets(r, &q))
            .map(|e| e.1)
            .collect();
        assert_eq!(got, want);
        // Six columns by three rows.
        assert_eq!(want.len(), 18);
    }
}

#[cfg(test)]
mod nearest_tests {
    use super::*;

    #[test]
    fn nearest_pairs_find_the_nearest_first() {
        let p = |x: i64, y: i64| Point::new(x, y);
        let a = Packed::new(
            (0..300)
                .map(|k| (Rect::of_point(p(k * 1000, 0)), k as u32))
                .collect(),
        );
        let b = Packed::new(
            (0..300)
                .map(|k| (Rect::of_point(p(k * 1000 + 50_000, 7_000 + k)), k as u32))
                .collect(),
        );
        // A nearest search: each pair's true distance lowers the limit.
        let (mut best, mut seen) = (f64::MAX, 0);
        let mut at = (0, 0);
        a.nearest_pairs(
            &b,
            f64::MAX,
            |_, _, _| 0.0,
            |_, _, _| 0.0,
            |i, j| {
                seen += 1;
                let dx = (i as f64 * 1000.0) - (j as f64 * 1000.0 + 50_000.0);
                let dy = 7_000.0 + j as f64;
                let d = (dx * dx + dy * dy).sqrt();
                if d < best {
                    best = d;
                    at = (i, j);
                }
                Some(best)
            },
        );
        assert_eq!(at, (50, 0));
        assert_eq!(best, 7_000.0);
        // Far fewer than the 90,000 pairs.
        assert!(seen < 2_000, "{seen}");
    }
}
