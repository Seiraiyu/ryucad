//! Certified separation of trimmed plane curves.
//!
//! Two curves are apart when they are farther apart than
//! [`LINEAR_RESOLUTION`] everywhere except at the ends where they are
//! joined. This is decided on their Bézier pieces ([`super::chain2`]) by
//! explicit rules: control hulls are bounds, so a test that passes is a
//! proof, and a test that can't pass after enough subdivision counts as
//! touching, never as a guess.

use super::Vector2;
use super::chain2::{Chain2, Piece2};
use crate::tol::LINEAR_RESOLUTION;

/// Rounding slack added around every bound. Converting a segment to
/// pieces and subdividing them rounds each control point by a few ulps of
/// the ±500 m size box per level (about 1e-13 m); at most about 60 levels
/// stay far below this, and it is 1% of [`LINEAR_RESOLUTION`].
pub(crate) const SLACK_2D: f64 = 1e-10;

/// Two bounds farther apart than this keep their curves farther apart than
/// [`LINEAR_RESOLUTION`], whatever the rounding.
const GAP: f64 = LINEAR_RESOLUTION + 2.0 * SLACK_2D;

/// Pieces whose hulls are both smaller than this and not certified apart
/// count as touching: their curves are then within about
/// `1.4 × LINEAR_RESOLUTION` of each other.
const SMALL: f64 = LINEAR_RESOLUTION / 4.0;

/// The relative margin of the cone and monotonicity rules: a cross or dot
/// product counts as signed only beyond this times the lengths' product.
const CONE_EPS: f64 = 1e-12;

/// How many times one pair may be subdivided before it counts as touching.
const MAX_SPLITS: u32 = 200;

/// How many times a joint or a self-crossing test may halve its pieces.
const MAX_JOINT_DEPTH: u32 = 60;

/// Which ends of two pieces (or chains) `a` and `b` meet: `a`'s end at
/// `b`'s start, and `a`'s start at `b`'s end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Joints {
    pub(crate) end_start: bool,
    pub(crate) start_end: bool,
}

pub(crate) const NO_JOINT: Joints = Joints {
    end_start: false,
    start_end: false,
};

/// Every pair `(i, j)`, `i < j`, of boxes that are not certified farther
/// apart than [`GAP`] along `x` or `y`: a sweep over the boxes sorted by
/// their least `x`.
pub(crate) fn overlapping_pairs(boxes: &[(Vector2, Vector2)]) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by(|&a, &b| boxes[a].0.x.total_cmp(&boxes[b].0.x).then(a.cmp(&b)));
    let mut active: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    for &i in &order {
        let (lo, hi) = boxes[i];
        active.retain(|&k| lo.x - boxes[k].1.x <= GAP);
        for &k in &active {
            let (klo, khi) = boxes[k];
            if lo.y - khi.y <= GAP && klo.y - hi.y <= GAP {
                out.push((k.min(i), k.max(i)));
            }
        }
        active.push(i);
    }
    out
}

/// True when two different curves meet only where `joints` says.
pub(crate) fn chains_apart(a: &Chain2, b: &Chain2, joints: Joints) -> bool {
    let (la, lb) = (a.pieces.len() - 1, b.pieces.len() - 1);
    for (i, pa) in a.pieces.iter().enumerate() {
        for (j, pb) in b.pieces.iter().enumerate() {
            let here = Joints {
                end_start: joints.end_start && i == la && j == 0,
                start_end: joints.start_end && i == 0 && j == lb,
            };
            if !pieces_apart(pa, pb, here) {
                return false;
            }
        }
    }
    true
}

/// True when a NURBS curve doesn't touch itself: each piece is free of
/// self-crossing, consecutive pieces meet only at their shared end (and
/// the last meets the first for a whole period), and others are apart.
pub(crate) fn chain_simple(c: &Chain2) -> bool {
    let n = c.pieces.len();
    if n == 1 {
        return piece_simple(&c.pieces[0], c.closed, MAX_JOINT_DEPTH);
    }
    if !c
        .pieces
        .iter()
        .all(|p| piece_simple(p, false, MAX_JOINT_DEPTH))
    {
        return false;
    }
    let boxes: Vec<_> = c.pieces.iter().map(Piece2::hull).collect();
    let mut pairs = overlapping_pairs(&boxes);
    pairs.extend((0..n - 1).map(|i| (i, i + 1)));
    if c.closed {
        pairs.push((0, n - 1));
    }
    pairs.sort_unstable();
    pairs.dedup();
    pairs.iter().all(|&(i, j)| {
        let joints = Joints {
            end_start: j == i + 1,
            start_end: c.closed && i == 0 && j == n - 1,
        };
        pieces_apart(&c.pieces[i], &c.pieces[j], joints)
    })
}

/// True when two pieces meet only at the joints given.
fn pieces_apart(a: &Piece2, b: &Piece2, joints: Joints) -> bool {
    match (joints.end_start, joints.start_end) {
        (false, false) => apart(a, b, MAX_SPLITS),
        (true, false) => joined(a, b, MAX_JOINT_DEPTH),
        (false, true) => joined(b, a, MAX_JOINT_DEPTH),
        (true, true) => {
            let [a0, a1] = a.split(0.5);
            joined(b, &a0, MAX_JOINT_DEPTH) && joined(&a1, b, MAX_JOINT_DEPTH)
        }
    }
}

/// True when `a` and `b` are certified farther apart than
/// [`LINEAR_RESOLUTION`]: their bounds are, or, after subdividing the
/// larger, every pair of parts is. Parts both smaller than [`SMALL`] that
/// are not certified apart, or running out of splits, count as touching.
fn apart(a: &Piece2, b: &Piece2, splits: u32) -> bool {
    if separated(a, b) {
        return true;
    }
    let (ea, eb) = (extent(a), extent(b));
    if (ea < SMALL && eb < SMALL) || splits == 0 {
        return false;
    }
    if ea >= eb {
        let [a0, a1] = a.split(0.5);
        apart(&a0, b, splits - 1) && apart(&a1, b, splits - 1)
    } else {
        let [b0, b1] = b.split(0.5);
        apart(a, &b0, splits - 1) && apart(a, &b1, splits - 1)
    }
}

/// True when `a`, whose end is `b`'s start, meets `b` only there. The
/// cone rule decides the pieces at the joint; when it can't, both are
/// halved, the halves at the joint are tried again and the other pairs
/// must be [`apart`]. Running out of depth is a cusp: touching.
fn joined(a: &Piece2, b: &Piece2, depth: u32) -> bool {
    let apex_a = a.pts[a.pts.len() - 1].p;
    let apex_b = b.pts[0].p;
    let ga = a.pts[..a.pts.len() - 1].iter().map(|h| h.p - apex_a);
    let gb = b.pts[1..].iter().map(|h| h.p - apex_b);
    if cones_apart(ga, gb) {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let [a0, a1] = a.split(0.5);
    let [b0, b1] = b.split(0.5);
    apart(&a0, &b0, MAX_SPLITS)
        && apart(&a1, &b1, MAX_SPLITS)
        && apart(&a0, &b1, MAX_SPLITS)
        && joined(&a1, &b0, depth - 1)
}

/// The cone rule. Each piece lies in the cone from its joint end spanned
/// by the vectors to its other control points (`ga`, `gb`; the small gap
/// between the two ends is the vertex's tolerance). The cones meet only at
/// the apex when, over all pairs, every cross product is beyond
/// `CONE_EPS |a| |b|` on one side, or every dot product is below
/// `-CONE_EPS |a| |b|`: if a direction `d` lay in both cones, `d × d` or
/// `d · d` would be a positive combination of those products.
fn cones_apart(ga: impl Iterator<Item = Vector2>, gb: impl Iterator<Item = Vector2>) -> bool {
    let nonzero = |v: &Vector2| *v != Vector2::ZERO;
    let ga: Vec<Vector2> = ga.filter(nonzero).collect();
    let gb: Vec<Vector2> = gb.filter(nonzero).collect();
    if ga.is_empty() || gb.is_empty() {
        return false;
    }
    let all = |test: &dyn Fn(Vector2, Vector2, f64) -> bool| {
        ga.iter().all(|&u| {
            gb.iter()
                .all(|&v| test(u, v, CONE_EPS * u.length() * v.length()))
        })
    };
    all(&|u, v, m| u.cross(v) > m)
        || all(&|u, v, m| u.cross(v) < -m)
        || all(&|u, v, m| u.dot(v) < -m)
}

/// True when one piece can't cross itself: it is monotone along its chord
/// (every step of its control polygon has a positive share of the chord,
/// by the cone margin; a rational Bézier with positive weights whose
/// control polygon is monotone in a direction is monotone in it), or its
/// halves are simple and meet only at their joint. `closed` marks a piece
/// whose end is its start.
fn piece_simple(p: &Piece2, closed: bool, depth: u32) -> bool {
    if !closed && monotone(p) {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let [h0, h1] = p.split(0.5);
    let joints = Joints {
        end_start: true,
        start_end: closed,
    };
    piece_simple(&h0, false, depth - 1)
        && piece_simple(&h1, false, depth - 1)
        && pieces_apart(&h0, &h1, joints)
}

/// The control polygon advances along the chord at every step.
fn monotone(p: &Piece2) -> bool {
    let chord = p.pts[p.pts.len() - 1].p - p.pts[0].p;
    if chord == Vector2::ZERO {
        return false;
    }
    let len = chord.length();
    p.pts.windows(2).all(|w| {
        let step = w[1].p - w[0].p;
        step == Vector2::ZERO || step.dot(chord) > CONE_EPS * step.length() * len
    })
}

/// The larger side of a piece's hull box.
fn extent(p: &Piece2) -> f64 {
    let (lo, hi) = p.hull();
    (hi.x - lo.x).max(hi.y - lo.y)
}

/// True when the hulls of `a` and `b` are farther apart than [`GAP`] along
/// `x`, `y`, or the chord of either piece or its normal.
fn separated(a: &Piece2, b: &Piece2) -> bool {
    let (alo, ahi) = a.hull();
    let (blo, bhi) = b.hull();
    let boxed = (blo.x - ahi.x)
        .max(alo.x - bhi.x)
        .max(blo.y - ahi.y)
        .max(alo.y - bhi.y);
    if boxed > GAP {
        return true;
    }
    [a, b].iter().any(|p| {
        let chord = p.pts[p.pts.len() - 1].p - p.pts[0].p;
        let len = chord.length();
        if len == 0.0 {
            return false;
        }
        let t = chord / len;
        let n = Vector2::new(-t.y, t.x);
        gap_along(t, a, b) > GAP || gap_along(n, a, b) > GAP
    })
}

/// The gap between the two pieces' control points projected on `axis`.
fn gap_along(axis: Vector2, a: &Piece2, b: &Piece2) -> f64 {
    let span = |p: &Piece2| {
        p.pts
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), h| {
                let x = h.p.dot(axis);
                (lo.min(x), hi.max(x))
            })
    };
    let ((alo, ahi), (blo, bhi)) = (span(a), span(b));
    (blo - ahi).max(alo - bhi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_and_prune_finds_every_close_pair() {
        let boxes = [
            (Vector2::new(0.0, 0.0), Vector2::new(1.0, 1.0)),
            (Vector2::new(2.0, 0.0), Vector2::new(3.0, 1.0)),
            (Vector2::new(1.0 + 1e-8, 0.5), Vector2::new(1.5, 2.0)),
            (Vector2::new(0.5, 1.0 + 2e-8), Vector2::new(1.2, 2.0)),
            (Vector2::new(-1.0, -1.0), Vector2::new(5.0, -0.5)),
        ];
        assert_eq!(overlapping_pairs(&boxes), [(0, 2), (2, 3)]);
    }
}
