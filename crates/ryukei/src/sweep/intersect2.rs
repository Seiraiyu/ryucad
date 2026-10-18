//! Certified separation of profile segments.
//!
//! Every pair of segments that are not neighbours in a loop must be farther
//! apart than [`LINEAR_RESOLUTION`]; neighbours may meet only at their
//! shared end. Both are decided on the segments' Bézier pieces
//! ([`super::chain`]) by explicit rules: control hulls are bounds, so a
//! test that passes is a proof, and a test that can't pass after enough
//! subdivision counts as touching, never as a guess.

use super::chain::{Chain, Piece};
use crate::geom::Vector2;
use crate::tol::LINEAR_RESOLUTION;
use rayon::prelude::*;

/// Rounding slack added around every bound. Converting a segment to
/// pieces and subdividing them rounds each control point by a few ulps of
/// the ±500 m size box per level (about 1e-13 m); at most about 60 levels
/// stay far below this, and it is 1% of [`LINEAR_RESOLUTION`].
pub(super) const SLACK_2D: f64 = 1e-10;

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
struct Joints {
    end_start: bool,
    start_end: bool,
}

const NO_JOINT: Joints = Joints {
    end_start: false,
    start_end: false,
};

/// The first pair of segments, in input order, that touch: `(i, i)` for a
/// segment that touches itself, else `(i, j)` with `i < j`. `loops` gives
/// each loop's first segment and length in `chains`, which holds every
/// segment in input order.
pub(super) fn first_touching(chains: &[Chain], loops: &[(usize, usize)]) -> Option<(usize, usize)> {
    let mut owner = vec![(0, 0); chains.len()];
    for &(first, len) in loops {
        for o in &mut owner[first..first + len] {
            *o = (first, len);
        }
    }
    let boxes: Vec<_> = chains.iter().map(|c| (c.lo, c.hi)).collect();
    let mut pairs = overlapping_pairs(&boxes);
    pairs.extend(
        (0..chains.len())
            .filter(|&i| chains[i].free_form)
            .map(|i| (i, i)),
    );
    pairs.sort_unstable();
    pairs
        .par_iter()
        .find_first(|&&(i, j)| {
            if i == j {
                return !chain_simple(&chains[i]);
            }
            let joints = neighbours(owner[i], i, j);
            !chains_apart(&chains[i], &chains[j], joints)
        })
        .copied()
}

/// How segments `i < j` meet, given `i`'s loop as its first segment and
/// length. Segments of different loops don't meet.
fn neighbours((first, len): (usize, usize), i: usize, j: usize) -> Joints {
    if j >= first + len {
        return NO_JOINT;
    }
    let (a, b) = (i - first, j - first);
    Joints {
        end_start: (a + 1) % len == b,
        start_end: (b + 1) % len == a,
    }
}

/// Every pair `(i, j)`, `i < j`, of boxes that are not certified farther
/// apart than [`GAP`] along `x` or `y`: a sweep over the boxes sorted by
/// their least `x`.
fn overlapping_pairs(boxes: &[(Vector2, Vector2)]) -> Vec<(usize, usize)> {
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

/// True when two different segments meet only where `joints` says.
fn chains_apart(a: &Chain, b: &Chain, joints: Joints) -> bool {
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

/// True when a NURBS segment doesn't touch itself: each piece is free of
/// self-crossing, consecutive pieces meet only at their shared end (and
/// the last meets the first for a whole period), and others are apart.
fn chain_simple(c: &Chain) -> bool {
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
    let boxes: Vec<_> = c.pieces.iter().map(Piece::hull).collect();
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
fn pieces_apart(a: &Piece, b: &Piece, joints: Joints) -> bool {
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
fn apart(a: &Piece, b: &Piece, splits: u32) -> bool {
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
fn joined(a: &Piece, b: &Piece, depth: u32) -> bool {
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
fn piece_simple(p: &Piece, closed: bool, depth: u32) -> bool {
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
fn monotone(p: &Piece) -> bool {
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
fn extent(p: &Piece) -> f64 {
    let (lo, hi) = p.hull();
    (hi.x - lo.x).max(hi.y - lo.y)
}

/// True when the hulls of `a` and `b` are farther apart than [`GAP`] along
/// `x`, `y`, or the chord of either piece or its normal.
fn separated(a: &Piece, b: &Piece) -> bool {
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
fn gap_along(axis: Vector2, a: &Piece, b: &Piece) -> f64 {
    let span = |p: &Piece| {
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
    use crate::geom::{Circle2, Curve2, Frame2, KnotVector, NurbsCurve2, Point2};
    use crate::math;
    use crate::sweep::profile::tests::{arc, circle, fault, line, polygon, profile, square};
    use crate::sweep::{Loop, LoopKey, SegmentKey, ValidProfile};
    use crate::{Error, ProfileError};
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};
    use std::sync::Arc;

    fn touching(a: u128, b: u128) -> ProfileError {
        ProfileError::Touching {
            a: SegmentKey(a),
            b: SegmentKey(b),
        }
    }

    fn valid(loops: Vec<Loop>) -> ValidProfile {
        ValidProfile::new(profile(loops)).unwrap()
    }

    fn is_valid(loops: Vec<Loop>) -> bool {
        match ValidProfile::new(profile(loops)) {
            Ok(_) => true,
            Err(Error::InvalidProfile(ProfileError::Touching { .. })) => false,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }

    /// A loop of one closed segment, keyed `key` like its loop.
    fn lone(key: u128, curve: Curve2, t0: f64, t1: f64) -> Loop {
        Loop {
            key: LoopKey(key),
            segments: vec![arc(key, curve, t0, t1)],
        }
    }

    /// A periodic cubic with uniform knots `0, 1, ..., n` and these
    /// control points.
    fn periodic(pts: &[(f64, f64)]) -> Curve2 {
        let n = pts.len();
        let knots: Vec<f64> = (0..=n).map(|i| i as f64).collect();
        let k = KnotVector::periodic(3, &knots, &vec![1; n + 1]).unwrap();
        let pts = pts.iter().map(|&(x, y)| Point2::new(x, y)).collect();
        Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap()))
    }

    #[test]
    fn crossing_lines() {
        let bowtie = polygon(1, 10, &[(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)]);
        assert_eq!(fault(vec![bowtie]), touching(10, 12));
        let a = square(1, 10, (0.0, 0.0), 1.0);
        let b = square(2, 20, (0.5, 0.5), 1.0);
        assert_eq!(fault(vec![a, b]), touching(11, 20));
    }

    #[test]
    fn touching_at_an_end() {
        let a = square(1, 10, (0.0, 0.0), 1.0);
        let b = polygon(2, 20, &[(1.0, 0.5), (2.0, 0.0), (2.0, 1.0)]);
        assert_eq!(fault(vec![a, b]), touching(11, 20));
        // A loop that comes back through one of its own vertices.
        let pinched = polygon(
            1,
            10,
            &[
                (0.0, 0.0),
                (2.0, 0.0),
                (1.0, 1.0),
                (2.0, 2.0),
                (0.0, 2.0),
                (1.0, 1.0),
            ],
        );
        assert_eq!(fault(vec![pinched]), touching(11, 14));
    }

    #[test]
    fn parallel_lines_either_side_of_resolution() {
        for (gap, ok) in [(0.5e-8, false), (2e-8, true)] {
            let a = square(1, 10, (0.0, 0.0), 1.0);
            let b = square(2, 20, (1.0 + gap, 0.25), 1.0);
            assert_eq!(is_valid(vec![a.clone(), b.clone()]), ok, "{gap}");
            if !ok {
                assert_eq!(fault(vec![a, b]), touching(11, 20));
            }
        }
    }

    #[test]
    fn a_circle_tangent_to_a_line() {
        let a = square(1, 10, (0.0, 0.0), 1.0);
        let c = lone(2, circle((0.5, 1.5), 0.5), 0.0, TAU);
        assert_eq!(fault(vec![a.clone(), c]), touching(12, 2));
        let c = lone(2, circle((0.5, 1.5 + 0.5e-8), 0.5), 0.0, TAU);
        assert_eq!(fault(vec![a.clone(), c]), touching(12, 2));
        let c = lone(2, circle((0.5, 1.5 + 2e-8), 0.5), 0.0, TAU);
        valid(vec![a, c]);
    }

    #[test]
    fn arcs_nearly_touching() {
        for (gap, ok) in [(2e-8, true), (0.5e-8, false)] {
            // Two circles nearly touching on a diagonal.
            let d = (2.0 + gap) / 2f64.sqrt();
            let a = lone(1, circle((0.0, 0.0), 1.0), 0.0, TAU);
            let b = lone(2, circle((d, d), 1.0), 1.0, 1.0 - TAU);
            assert_eq!(is_valid(vec![a, b]), ok, "{gap}");
            // A circle nearly touching a ring around it from inside.
            let outer = lone(3, circle((0.0, 0.0), 2.0), 0.0, TAU);
            let inner = lone(4, circle((1.0 - gap, 0.0), 1.0), 0.0, TAU);
            assert_eq!(is_valid(vec![outer, inner]), ok, "{gap}");
        }
    }

    /// A slot: two lines joined by semicircles, tangent at every joint.
    fn slot(key: u128, first: u128, (x, y): (f64, f64), len: f64, r: f64) -> Loop {
        Loop {
            key: LoopKey(key),
            segments: vec![
                line(first, (x, y - r), (x + len, y - r)),
                arc(first + 1, circle((x + len, y), r), -FRAC_PI_2, FRAC_PI_2),
                line(first + 2, (x + len, y + r), (x, y + r)),
                arc(first + 3, circle((x, y), r), FRAC_PI_2, 3.0 * FRAC_PI_2),
            ],
        }
    }

    #[test]
    fn slots_with_tangent_joints() {
        let p = valid(vec![slot(1, 10, (0.0, 0.0), 3.0, 1.0)]);
        let area = p.lumps()[0].outer.area;
        assert!((area - (6.0 + PI)).abs() <= 1e-12, "{area}");
        // Two slots side by side, close but apart.
        valid(vec![
            slot(1, 10, (0.0, 0.0), 3.0, 1.0),
            slot(2, 20, (0.0, 2.0 + 2e-8), 3.0, 1.0),
        ]);
        // A thin slot: its straight sides 4e-8 apart.
        valid(vec![slot(1, 10, (0.0, 0.0), 3.0, 2e-8)]);
    }

    #[test]
    fn sharp_valid_corners() {
        // A wedge with a one-degree tip.
        let (s, c) = math::sin_cos(PI / 360.0);
        valid(vec![polygon(1, 10, &[(0.0, 0.0), (c, s), (c, -s)])]);
        // A line and an arc of two degrees over it: one-degree corners at
        // both ends.
        let one = PI / 180.0;
        let r = 0.5 / math::sin(one);
        let over = circle((0.5, -r * math::cos(one)), r);
        let horn = Loop {
            key: LoopKey(2),
            segments: vec![
                line(20, (0.0, 0.0), (1.0, 0.0)),
                arc(21, over, FRAC_PI_2 - one, FRAC_PI_2 + one),
            ],
        };
        valid(vec![horn]);
    }

    #[test]
    fn a_cusp_is_rejected() {
        // A line, then an arc leaving back along it.
        let f = Frame2::new(Point2::new(1.0, 0.5), Vector2::new(0.0, -1.0), false).unwrap();
        let back = Curve2::Circle2(Circle2::new(f, 0.5).unwrap());
        let cusp = Loop {
            key: LoopKey(1),
            segments: vec![
                line(10, (0.0, 0.0), (1.0, 0.0)),
                arc(11, back, 0.0, PI),
                line(12, (1.0, 1.0), (0.0, 1.0)),
                line(13, (0.0, 1.0), (0.0, 0.0)),
            ],
        };
        assert_eq!(fault(vec![cusp]), touching(10, 11));
    }

    #[test]
    fn a_nurbs_figure_eight_touches_itself() {
        let eight = periodic(&[
            (1.0, 1.0),
            (2.0, 0.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (-2.0, 0.0),
            (-1.0, -1.0),
        ]);
        assert_eq!(fault(vec![lone(5, eight, 0.0, 6.0)]), touching(5, 5));
    }

    #[test]
    fn a_nurbs_segment_with_a_loop_in_it() {
        let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(3.0, 2.0),
            Point2::new(-1.0, 2.0),
            Point2::new(2.0, 0.0),
        ];
        let n = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap()));
        let l = Loop {
            key: LoopKey(1),
            segments: vec![arc(10, n, 0.0, 1.0), line(11, (2.0, 0.0), (0.0, 0.0))],
        };
        assert_eq!(fault(vec![l]), touching(10, 10));
    }

    /// A blob: a periodic cubic on control points around a circle.
    pub(crate) fn blob((x, y): (f64, f64), r: f64) -> Curve2 {
        let pts: Vec<_> = [1.0, 0.8, 1.1, 0.9, 1.0, 0.7, 1.2, 0.95]
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let (s, c) = math::sin_cos(TAU * i as f64 / 8.0);
                (x + r * k * c, y + r * k * s)
            })
            .collect();
        periodic(&pts)
    }

    #[test]
    fn nurbs_blobs_are_accepted() {
        let p = valid(vec![
            lone(1, blob((0.0, 0.0), 1.0), 0.0, 8.0),
            lone(2, blob((3.0, 0.0), 1.0), 8.0, 0.0),
        ]);
        assert_eq!(p.lumps().len(), 2);
        // The same blob as two segments.
        let b = blob((0.0, 0.0), 1.0);
        let halves = Loop {
            key: LoopKey(3),
            segments: vec![arc(30, b.clone(), 2.5, 6.0), arc(31, b, 6.0, 10.5)],
        };
        valid(vec![halves]);
    }

    #[test]
    fn two_segment_loops() {
        let half = Loop {
            key: LoopKey(1),
            segments: vec![
                arc(10, circle((0.0, 0.0), 1.0), 0.0, PI),
                line(11, (-1.0, 0.0), (1.0, 0.0)),
            ],
        };
        valid(vec![half]);
        // Two arcs meeting at both ends at right angles.
        let r = 2f64.sqrt();
        let lens = Loop {
            key: LoopKey(2),
            segments: vec![
                arc(20, circle((0.0, -1.0), r), FRAC_PI_4, 3.0 * FRAC_PI_4),
                arc(21, circle((0.0, 1.0), r), 5.0 * FRAC_PI_4, 7.0 * FRAC_PI_4),
            ],
        };
        valid(vec![lens]);
        // A line there and back: no area, all overlap.
        let flat = Loop {
            key: LoopKey(3),
            segments: vec![
                line(30, (0.0, 0.0), (1.0, 0.0)),
                line(31, (1.0, 0.0), (0.0, 0.0)),
            ],
        };
        assert_eq!(fault(vec![flat]), touching(30, 31));
    }

    #[test]
    fn ten_thousand_sides() {
        let n = 10_000;
        let pts: Vec<_> = (0..n)
            .map(|i| {
                let (s, c) = math::sin_cos(TAU * f64::from(i) / f64::from(n));
                (10.0 * c, 10.0 * s)
            })
            .collect();
        let p = valid(vec![polygon(1, 0, &pts)]);
        assert_eq!(p.lumps()[0].outer.segments.len(), 10_000);
    }

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
