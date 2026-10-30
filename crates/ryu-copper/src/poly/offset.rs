//! Offsets: a polygon set grown or shrunk by a distance with round joins.
//!
//! Growing by `d` is the Minkowski sum with a disc of radius `d`: the set
//! itself and, for every edge, the hull of the disc at both its ends (a
//! stadium, which holds the disc at each vertex: the round joins). So it
//! is one union. Shrinking is the set less the same stadiums along its
//! edges: what lies within `d` of its outline goes. The disc is flattened
//! within the options' bound, so the result is within the input's bound,
//! the disc's deviation and a nanometre of the exact offset.

use ryu_board::geom::Nm;

use super::flatten::convex_sweep;
use super::{FillRule, FlattenOptions, Op, Polygons, Rings, boolean, disc};

/// The stadiums of every edge of every ring, and the disc's deviation.
fn stadiums(p: &Polygons, d: Nm, opts: &FlattenOptions) -> Rings {
    let (pen, dev) = disc(2 * d, opts.bound() as f64 / 2.0 - 2.0);
    let mut rings = Vec::new();
    for poly in &p.polygons {
        for r in poly.rings() {
            for k in 0..r.len() {
                convex_sweep(&pen, &[r[k], r[(k + 1) % r.len()]], &mut rings);
            }
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
    let s = stadiums(p, d, opts);
    boolean(
        &p.rings(),
        FillRule::NonZero,
        &s,
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
    let s = stadiums(p, d, opts);
    boolean(
        &p.rings(),
        FillRule::NonZero,
        &s,
        FillRule::NonZero,
        Op::Difference,
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
}
