//! The broad phase: a bounding-box tree, and the pairs of boxes that
//! overlap.
//!
//! The tree is built top down, each node's items split at the median of
//! their centres along the node box's longest side, so it has depth
//! `O(log n)`, and pairs are found by descending two trees (or one tree
//! against itself) together. Results are sorted, so they don't depend on
//! the tree's shape.

use crate::geom::Box3;

/// Items per leaf at most.
const LEAF: usize = 4;

/// A tree of boxes, each with a caller's index.
#[derive(Clone, Debug)]
pub(crate) struct BoxTree {
    nodes: Vec<Node>,
    items: Vec<(Box3, usize)>,
}

#[derive(Clone, Debug)]
struct Node {
    bounds: Box3,
    /// A leaf's items, `items[first..first + count]`, or an inner node's
    /// two children.
    kind: Kind,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Leaf { first: usize, count: usize },
    Inner { left: usize, right: usize },
}

impl BoxTree {
    pub(crate) fn new(mut items: Vec<(Box3, usize)>) -> Self {
        let mut nodes = Vec::new();
        if !items.is_empty() {
            let n = items.len();
            build(&mut nodes, &mut items, 0, n);
        }
        Self { nodes, items }
    }

    /// Every pair `(i, j)` of indices, `i` from this tree and `j` from
    /// `other`, whose boxes overlap, sorted.
    pub(crate) fn pairs_with(&self, other: &Self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if !self.nodes.is_empty() && !other.nodes.is_empty() {
            self.descend(0, other, 0, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Every pair `(i, j)`, `i < j`, of indices in this tree whose boxes
    /// overlap, sorted.
    pub(crate) fn self_pairs(&self) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = self
            .pairs_with(self)
            .into_iter()
            .filter(|&(i, j)| i < j)
            .collect();
        out.dedup();
        out
    }

    fn descend(&self, a: usize, other: &Self, b: usize, out: &mut Vec<(usize, usize)>) {
        let (na, nb) = (&self.nodes[a], &other.nodes[b]);
        if !na.bounds.intersects(&nb.bounds) {
            return;
        }
        match (na.kind, nb.kind) {
            (
                Kind::Leaf {
                    first: fa,
                    count: ca,
                },
                Kind::Leaf {
                    first: fb,
                    count: cb,
                },
            ) => {
                for (ba, ia) in &self.items[fa..fa + ca] {
                    for (bb, ib) in &other.items[fb..fb + cb] {
                        if ba.intersects(bb) {
                            out.push((*ia, *ib));
                        }
                    }
                }
            }
            (Kind::Inner { left, right }, Kind::Leaf { .. }) => {
                self.descend(left, other, b, out);
                self.descend(right, other, b, out);
            }
            (Kind::Leaf { .. }, Kind::Inner { left, right }) => {
                self.descend(a, other, left, out);
                self.descend(a, other, right, out);
            }
            (
                Kind::Inner { left, right },
                Kind::Inner {
                    left: l2,
                    right: r2,
                },
            ) => {
                for x in [left, right] {
                    for y in [l2, r2] {
                        self.descend(x, other, y, out);
                    }
                }
            }
        }
    }
}

/// Builds the node for `items[first..end]`, returning its index.
fn build(nodes: &mut Vec<Node>, items: &mut [(Box3, usize)], first: usize, end: usize) -> usize {
    let bounds = items[first..end]
        .iter()
        .map(|(b, _)| *b)
        .reduce(|x, y| x.union(&y))
        .expect("a node has items");
    let index = nodes.len();
    nodes.push(Node {
        bounds,
        kind: Kind::Leaf {
            first,
            count: end - first,
        },
    });
    if end - first <= LEAF {
        return index;
    }
    let size = bounds.max() - bounds.min();
    let axis = if size.x >= size.y && size.x >= size.z {
        0
    } else if size.y >= size.z {
        1
    } else {
        2
    };
    let centre = |b: &Box3| (b.min().to_array()[axis] + b.max().to_array()[axis]) / 2.0;
    items[first..end].sort_by(|x, y| centre(&x.0).total_cmp(&centre(&y.0)).then(x.1.cmp(&y.1)));
    let mid = first + (end - first) / 2;
    let left = build(nodes, items, first, mid);
    let right = build(nodes, items, mid, end);
    nodes[index].kind = Kind::Inner { left, right };
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point3;

    fn cube(x: f64, y: f64, z: f64, s: f64) -> Box3 {
        Box3::new(Point3::new(x, y, z), Point3::new(x + s, y + s, z + s))
    }

    #[test]
    fn pairs_match_brute_force() {
        let mut seed = 5u64;
        let mut r = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let a: Vec<(Box3, usize)> = (0..300)
            .map(|i| (cube(r() * 10.0, r() * 10.0, r() * 10.0, r()), i))
            .collect();
        let b: Vec<(Box3, usize)> = (0..200)
            .map(|i| (cube(r() * 10.0, r() * 10.0, r() * 10.0, r() * 0.5), i))
            .collect();
        let (ta, tb) = (BoxTree::new(a.clone()), BoxTree::new(b.clone()));
        let mut want = Vec::new();
        for (x, i) in &a {
            for (y, j) in &b {
                if x.intersects(y) {
                    want.push((*i, *j));
                }
            }
        }
        want.sort_unstable();
        assert_eq!(ta.pairs_with(&tb), want);
        let mut own = Vec::new();
        for (x, i) in &a {
            for (y, j) in &a {
                if i < j && x.intersects(y) {
                    own.push((*i, *j));
                }
            }
        }
        own.sort_unstable();
        assert_eq!(ta.self_pairs(), own);
        assert!(BoxTree::new(Vec::new()).self_pairs().is_empty());
    }
}
