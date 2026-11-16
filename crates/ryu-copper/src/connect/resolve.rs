//! Resolved items: their copper as the boolean leaves it, in groups, and
//! contacts between such groups judged on polygons with their bounds.

use std::collections::HashMap;

use ryu_board::geom::Point;

use crate::measure::{self, Verdict};
use crate::poly::{self, FlattenOptions, Polygons};

/// A boolean's polygons in groups that touch: polygons sharing a vertex
/// (the only way a result's polygons meet) are one group. Groups in the
/// order of their first polygon, polygons kept in order, each group with
/// the polygons' bound.
pub fn groups(m: &Polygons) -> Vec<Polygons> {
    let n = m.polygons.len();
    let mut root: Vec<usize> = (0..n).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    let mut first: HashMap<Point, usize> = HashMap::new();
    for (i, p) in m.polygons.iter().enumerate() {
        for r in p.rings() {
            for q in r {
                match first.get(q).copied() {
                    Some(j) => {
                        let (a, b) = (find(&mut root, i), find(&mut root, j));
                        if a != b {
                            root[a.max(b)] = a.min(b);
                        }
                    }
                    None => {
                        first.insert(*q, i);
                    }
                }
            }
        }
    }
    let mut out: Vec<Polygons> = Vec::new();
    let mut slot: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        let r = find(&mut root, i);
        let g = *slot[r].get_or_insert_with(|| {
            out.push(Polygons {
                polygons: Vec::new(),
                bound: m.bound,
            });
            out.len() - 1
        });
        out[g].polygons.push(m.polygons[i].clone());
    }
    out
}

/// Whether the copper two polygon sets stand for touches. `Yes` when an
/// integer point lies inside both farther than their bounds from every
/// edge (`Polygons::contains` is `Yes` for both): the true copper of each
/// holds it. Candidates are vertices of their intersection shrunk by the
/// bounds and more; each is checked exactly, so a candidate that misses
/// only costs a `Uncertain`. `No` when the polygons lie farther apart than
/// their bounds together, exactly. Else `Uncertain`.
pub fn group_touch(a: &Polygons, b: &Polygons, fo: &FlattenOptions) -> Verdict {
    let reach = a.bound + b.bound;
    if measure::within(&a.body(), &b.body(), reach) == Verdict::No {
        return Verdict::No;
    }
    let both = poly::intersection(a, b);
    if !both.is_empty() {
        for depth in [reach + 2, 2 * reach + 4, 4 * reach + 8] {
            let core = poly::shrink(&both, depth, fo);
            for p in &core.polygons {
                let x = p.outer[0];
                if a.contains(x) == Verdict::Yes && b.contains(x) == Verdict::Yes {
                    return Verdict::Yes;
                }
            }
        }
    }
    Verdict::Uncertain
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poly::Polygon;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn rect(x0: i64, y0: i64, x1: i64, y1: i64) -> Polygon {
        Polygon {
            outer: vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)],
            holes: vec![],
        }
    }

    fn set(polygons: Vec<Polygon>, bound: i64) -> Polygons {
        Polygons { polygons, bound }
    }

    #[test]
    fn polygons_meeting_at_a_vertex_are_one_group() {
        let m = set(
            vec![
                rect(0, 0, 10, 10),
                rect(10, 10, 20, 20),
                rect(100, 0, 110, 10),
            ],
            3,
        );
        let g = groups(&m);
        assert_eq!(g.len(), 2);
        assert_eq!(
            g[0].polygons,
            vec![rect(0, 0, 10, 10), rect(10, 10, 20, 20)]
        );
        assert_eq!(g[1].polygons, vec![rect(100, 0, 110, 10)]);
        assert_eq!(g[1].bound, 3);
        assert!(groups(&Polygons::default()).is_empty());
    }

    #[test]
    fn groups_touch_deep_apart_or_uncertain() {
        let fo = FlattenOptions::default();
        let a = set(vec![rect(0, 0, 10_000, 10_000)], 52);
        let b = |x0: i64| set(vec![rect(x0, 0, x0 + 10_000, 10_000)], 52);
        // Overlapping by a micrometre: touching.
        assert_eq!(group_touch(&a, &b(9_000), &fo), Verdict::Yes);
        // A micrometre apart: not.
        assert_eq!(group_touch(&a, &b(11_000), &fo), Verdict::No);
        // Sharing an edge, or 50 nm apart: the bounds can't tell.
        assert_eq!(group_touch(&a, &b(10_000), &fo), Verdict::Uncertain);
        assert_eq!(group_touch(&a, &b(10_050), &fo), Verdict::Uncertain);
        // Exact polygons sharing an edge do touch... but only bound-0
        // polygons can say so, and a boolean's never are.
        let exact = |x0: i64| set(vec![rect(x0, 0, x0 + 10_000, 10_000)], 0);
        assert_eq!(group_touch(&exact(0), &exact(10_001), &fo), Verdict::No);
    }
}
