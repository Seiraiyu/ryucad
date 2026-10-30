//! Offsets: a polygon set grown or shrunk by a distance with round joins.
//!
//! Each ring, run with the set on its left, is offset to one side by a
//! disc of radius `d`: every edge moved by the disc's vertex furthest that
//! way, joined at a corner that opens on that side by the disc's vertices
//! between the two edges' (an arc, round the way the ring turns), and at
//! a corner that closes by a path through the ring's own vertex, as
//! Clipper's offsets join. Growing offsets every ring to its right (out of
//! the set), shrinking to its left (into it); the result is where the
//! offset rings together wind positively: one boolean. Every vertex is a
//! ring's vertex plus a disc's, so integer. The disc is flattened within
//! the options' bound, so the result is within the input's bound, the
//! disc's deviation and a nanometre of the exact offset.

use ryu_board::geom::{Nm, Point, V};

use super::{FillRule, FlattenOptions, Op, Polygons, Rings, boolean, disc};

/// The disc's vertex furthest along `n`, the last counterclockwise of
/// those tied, walking from `from` (the disc is convex, so how far its
/// vertices lie along `n` rises and falls once round it). Taking the last
/// of a tie whichever way the walk came makes the support turn with the
/// normal, so an arc between two edges' supports never wraps round.
fn support(d: &[Point], n: V, from: usize) -> usize {
    let len = d.len();
    let along = |j: usize| i128::from(d[j].x) * n.0 + i128::from(d[j].y) * n.1;
    let mut j = from;
    for _ in 0..2 * len {
        let (next, prev) = ((j + 1) % len, (j + len - 1) % len);
        if along(next) > along(j) || (along(next) == along(j) && along(prev) < along(j)) {
            j = next;
        } else if along(prev) > along(j) {
            j = prev;
        } else {
            return j;
        }
    }
    j
}

/// A ring offset by the disc `d` (counterclockwise) to its right when
/// `right`, else to its left.
fn offset_ring(r: &[Point], d: &[Point], right: bool, out: &mut Vec<Vec<Point>>) {
    let (n, m) = (r.len(), d.len());
    let mut s = Vec::with_capacity(n);
    let mut j = 0;
    for k in 0..n {
        let e = r[(k + 1) % n].minus(r[k]);
        let normal = if right { V(e.1, -e.0) } else { V(-e.1, e.0) };
        j = support(d, normal, j);
        s.push(j);
    }
    let mut ring = Vec::new();
    for k in 0..n {
        // Vertex `k + 1`, between edge `k` and edge `k + 1`.
        let v = r[(k + 1) % n];
        let (e_in, e_out) = (v.minus(r[k]), r[(k + 2) % n].minus(v));
        let (from, to) = (s[k], s[(k + 1) % n]);
        let turn = e_in.cross(e_out);
        // A left turn opens on the right, a right turn on the left.
        let opens = if right { turn > 0 } else { turn < 0 };
        let mut j = from;
        ring.push(v.offset(d[j].x, d[j].y));
        if opens {
            while j != to {
                j = if turn > 0 {
                    (j + 1) % m
                } else {
                    (j + m - 1) % m
                };
                ring.push(v.offset(d[j].x, d[j].y));
            }
        } else if from != to {
            ring.push(v);
            ring.push(v.offset(d[to].x, d[to].y));
        }
    }
    out.push(ring);
}

/// Every ring of `p` offset to its right (`grow`) or left, and the disc's
/// deviation added to the bound.
fn offset_rings(p: &Polygons, d: Nm, grow: bool, opts: &FlattenOptions) -> Rings {
    let (pen, dev) = disc(2 * d, opts.bound() as f64 / 2.0 - 1.0);
    let mut rings = Vec::new();
    for poly in &p.polygons {
        for r in poly.rings() {
            offset_ring(r, &pen, grow, &mut rings);
        }
    }
    Rings::new(rings, p.bound + dev.ceil() as Nm)
}

/// `p` grown by `d` (shrunk by `-d` when negative), round joins.
/// Coordinates must stay within ±2^41 nm.
pub fn grow(p: &Polygons, d: Nm, opts: &FlattenOptions) -> Polygons {
    if d < 0 {
        return shrink(p, -d, opts);
    }
    if d == 0 || p.is_empty() {
        return p.clone();
    }
    let c = offset_rings(p, d, true, opts);
    boolean(
        &c,
        FillRule::Positive,
        &Rings::default(),
        FillRule::NonZero,
        Op::Union,
    )
}

/// `p` shrunk by `d` (grown by `-d` when negative): what lies farther
/// than `d` inside its outline.
pub fn shrink(p: &Polygons, d: Nm, opts: &FlattenOptions) -> Polygons {
    if d < 0 {
        return grow(p, -d, opts);
    }
    if d == 0 || p.is_empty() {
        return p.clone();
    }
    let c = offset_rings(p, d, false, opts);
    boolean(
        &c,
        FillRule::Positive,
        &Rings::default(),
        FillRule::NonZero,
        Op::Union,
    )
}

/// Shrunk then grown: what is narrower than `2 d` goes.
pub fn open(p: &Polygons, d: Nm, opts: &FlattenOptions) -> Polygons {
    grow(&shrink(p, d, opts), d, opts)
}

/// Grown then shrunk: gaps narrower than `2 d` close.
pub fn close(p: &Polygons, d: Nm, opts: &FlattenOptions) -> Polygons {
    shrink(&grow(p, d, opts), d, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::Verdict;
    use crate::poly::simplify;
    use ryu_board::geom::Point;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn square(x0: i64, y0: i64, x1: i64, y1: i64) -> Polygons {
        simplify(
            &Rings::new(vec![vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]], 0),
            FillRule::NonZero,
        )
    }

    #[test]
    fn a_square_grows_round_corners() {
        let opts = FlattenOptions::default();
        let s = square(0, 0, 1_000_000, 1_000_000);
        let g = grow(&s, 100_000, &opts);
        assert_eq!(g.polygons.len(), 1);
        // 1e12 + 4e11 + π 1e10, within the disc's deviation all round.
        let exact = 1e12 + 4e11 + std::f64::consts::PI * 1e10;
        let got = g.area2() as f64 / 2.0;
        assert!(
            (got - exact).abs() < 4.4e6 * g.bound as f64,
            "{got} vs {exact}"
        );
        assert!(g.bound <= 1 + 50, "{}", g.bound);
        assert_eq!(g.contains(p(-70_000, -70_000)), Verdict::Yes);
        assert_eq!(g.contains(p(-71_000, -71_000)), Verdict::No);
        // Shrinking it back gives the square within the bounds.
        let back = shrink(&g, 100_000, &opts);
        assert_eq!(back.contains(p(500, 500)), Verdict::Yes);
        assert_eq!(back.contains(p(-200, 500_000)), Verdict::No);
        assert_eq!(grow(&s, 0, &opts), s);
        assert_eq!(grow(&s, -100, &opts), shrink(&s, 100, &opts));
    }

    #[test]
    fn rings_shrink_and_narrow_parts_open_away() {
        let opts = FlattenOptions::default();
        let ring = simplify(
            &Rings::new(
                vec![
                    vec![p(0, 0), p(10_000, 0), p(10_000, 10_000), p(0, 10_000)],
                    vec![
                        p(4_000, 4_000),
                        p(4_000, 6_000),
                        p(6_000, 6_000),
                        p(6_000, 4_000),
                    ],
                ],
                0,
            ),
            FillRule::NonZero,
        );
        let s = shrink(&ring, 1_000, &opts);
        assert_eq!(s.polygons.len(), 1);
        assert_eq!(s.contains(p(3_500, 5_000)), Verdict::No);
        assert_eq!(s.contains(p(2_000, 5_000)), Verdict::Yes);
        // A strip 500 wide on a square: opening by 300 takes the strip.
        let strip = crate::poly::union(
            &square(0, 0, 5_000, 5_000),
            &square(5_000, 2_000, 20_000, 2_500),
        );
        let o = open(&strip, 300, &opts);
        assert_eq!(o.polygons.len(), 1);
        assert_eq!(o.contains(p(12_000, 2_250)), Verdict::No);
        assert_eq!(o.contains(p(2_500, 2_500)), Verdict::Yes);
        // Closing by 300 fills a gap 500 wide.
        let gap = crate::poly::union(
            &square(0, 0, 5_000, 5_000),
            &square(5_500, 0, 10_000, 5_000),
        );
        let c = close(&gap, 300, &opts);
        assert_eq!(c.polygons.len(), 1);
        assert_eq!(c.contains(p(5_250, 2_500)), Verdict::Yes);
    }

    #[test]
    fn small_rings_shrink_away_and_small_holes_fill() {
        let opts = FlattenOptions::default();
        // A ring 1 µm across shrunk by 1 µm leaves nothing.
        assert!(shrink(&square(0, 0, 1_000, 1_000), 1_000, &opts).is_empty());
        // A hole 1 µm across in a square grown by 1 µm fills.
        let holed = simplify(
            &Rings::new(
                vec![
                    vec![p(0, 0), p(20_000, 0), p(20_000, 20_000), p(0, 20_000)],
                    vec![
                        p(9_500, 9_500),
                        p(9_500, 10_500),
                        p(10_500, 10_500),
                        p(10_500, 9_500),
                    ],
                ],
                0,
            ),
            FillRule::NonZero,
        );
        assert_eq!(holed.polygons[0].holes.len(), 1);
        let g = grow(&holed, 1_000, &opts);
        assert_eq!(g.polygons.len(), 1);
        assert!(g.polygons[0].holes.is_empty());
        assert_eq!(g.contains(p(10_000, 10_000)), Verdict::Yes);
    }
}
