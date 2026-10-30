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
