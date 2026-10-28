//! Certified separation and crossings of trimmed plane curves.
//!
//! Two curves are apart when they are farther apart than
//! [`LINEAR_RESOLUTION`] everywhere except at the ends where they are
//! joined. This is decided on their Bézier pieces ([`super::chain2`]) by
//! explicit rules: control hulls are bounds, so a test that passes is a
//! proof, and a test that can't pass after enough subdivision counts as
//! touching, never as a guess.
//!
//! Where curves meet, [`crossings`] finds where. Two rational pieces
//! `A(s) / Wa(s)` and `B(t) / Wb(t)` meet where
//! `A(s) Wb(t) - B(t) Wa(s) = 0`, two polynomial equations in Bernstein
//! form ([`Bernstein2`]); their parameter square is subdivided until
//! every cell is certified to hold no solution, or exactly one (by
//! Krawczyk's test), or is small. Small cells left over are where the
//! curves touch or run together: each cluster of them must be certified
//! within the tolerance of both curves, and is then a touching point, or
//! a stretch where they overlap.

use super::bernstein2::{Bernstein2, krawczyk};
use super::chain2::{Chain2, Piece2};
use super::{Curve2, Vector2};
use crate::interval::Interval;
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

/// Where two curves meet, by their own parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Meet2 {
    /// A crossing, or a touch at a point.
    Point { ta: f64, tb: f64, at: Vector2 },
    /// The curves run within the tolerance of each other over these
    /// stretches.
    Overlap { ta: [f64; 2], tb: [f64; 2] },
}

/// The deepest subdivision of a pair of pieces' parameter square.
const MEET_DEPTH: u32 = 40;

/// A stretch where curves run within the tolerance of each other is an
/// overlap when it is longer than this many tolerances; shorter, it is a
/// touch.
const OVERLAP: f64 = 100.0;

/// The most cells one pair of pieces may be cut into.
const MEET_CELLS: usize = 1 << 16;

/// Every place two curves, each over a range of its parameter (`t1 < t0`
/// runs it backwards), meet, by their parameters; points within
/// [`OVERLAP`] tolerances of each other are one. Curves on one line,
/// circle or ellipse are settled in closed form. `None` when a place
/// where they come close can't be certified within `tol` of both.
pub(crate) fn crossings(
    a: (&Curve2, [f64; 2]),
    b: (&Curve2, [f64; 2]),
    tol: f64,
) -> Option<Vec<Meet2>> {
    // A spline that is a straight segment (a line read or converted as a
    // NURBS) is met as the line it runs along, its parameter mapped.
    let (la, lb) = (as_line(a.0), as_line(b.0));
    if la.is_some() || lb.is_some() {
        let id = |c: &Curve2| (c.clone(), [0.0, 1.0]);
        let ((ca, ma), (cb, mb)) = (la.unwrap_or_else(|| id(a.0)), lb.unwrap_or_else(|| id(b.0)));
        let fwd = |m: [f64; 2], r: [f64; 2]| r.map(|t| m[0] + m[1] * t);
        let back = |m: [f64; 2], t: f64| (t - m[0]) / m[1];
        let found = crossings((&ca, fwd(ma, a.1)), (&cb, fwd(mb, b.1)), tol)?;
        return Some(
            found
                .into_iter()
                .map(|m| match m {
                    Meet2::Point { ta, tb, at } => Meet2::Point {
                        ta: back(ma, ta),
                        tb: back(mb, tb),
                        at,
                    },
                    Meet2::Overlap { ta, tb } => Meet2::Overlap {
                        ta: ta.map(|t| back(ma, t)),
                        tb: tb.map(|t| back(mb, t)),
                    },
                })
                .collect(),
        );
    }
    if let Some(found) = same_carrier(a, b, tol) {
        return Some(found);
    }
    let ca = Chain2::new(a.0, a.1[0], a.1[1]);
    let cb = Chain2::new(b.0, b.1[0], b.1[1]);
    let mut out: Vec<Meet2> = Vec::new();
    for (i, pa) in ca.pieces.iter().enumerate() {
        let (alo, ahi) = pa.hull();
        for (j, pb) in cb.pieces.iter().enumerate() {
            let (blo, bhi) = pb.hull();
            let gap = (blo.x - ahi.x)
                .max(alo.x - bhi.x)
                .max(blo.y - ahi.y)
                .max(alo.y - bhi.y);
            if gap > tol {
                continue;
            }
            let system = System::new(pa, pb, tol);
            let mut cells = Vec::new();
            let mut budget = MEET_CELLS;
            system.solve(
                [0.0, 1.0],
                [0.0, 1.0],
                0,
                &mut out,
                &mut cells,
                &mut budget,
                (&ca, i),
                (&cb, j),
            )?;
            for cluster in clusters(cells) {
                out.push(system.cluster(&cluster, tol, (&ca, i), (&cb, j))?);
            }
        }
    }
    // Pieces meeting at a joint find the same point twice; and points
    // closer than a touch zone are one by the same rule.
    let mut merged: Vec<Meet2> = Vec::with_capacity(out.len());
    for m in out {
        let dup = match m {
            Meet2::Point { at, .. } => merged.iter().any(|n| match n {
                Meet2::Point { at: q, .. } => (*q - at).length() <= OVERLAP * tol,
                Meet2::Overlap { .. } => false,
            }),
            Meet2::Overlap { .. } => false,
        };
        if !dup {
            merged.push(m);
        }
    }
    Some(merged)
}

/// A NURBS of degree 1 with two control points and equal weights as the
/// line through them (its parameter 0 at the first, the distance to the
/// second at the second), with the affine map from the spline's parameter
/// to the line's. `None` for any other curve.
fn as_line(c: &Curve2) -> Option<(Curve2, [f64; 2])> {
    let Curve2::Nurbs2(n) = c else {
        return None;
    };
    let w = n.weights();
    if n.degree() != 1 || w.len() != 2 || w[0] != w[1] {
        return None;
    }
    let mut pts = n.control_points();
    let (p, q) = (pts.next()?, pts.next()?);
    let [s0, s1] = n.domain();
    let line = super::Line2::through(p, q).ok()?;
    let m1 = p.distance(q) / (s1 - s0);
    Some((Curve2::Line2(line), [-s0 * m1, m1]))
}

/// Curves on one line, one circle or one ellipse, by rule (their
/// carriers within `tol` of each other): where their ranges overlap, in
/// closed form. `None` for any other pair.
fn same_carrier(
    (ca, ra): (&Curve2, [f64; 2]),
    (cb, rb): (&Curve2, [f64; 2]),
    tol: f64,
) -> Option<Vec<Meet2>> {
    let on_a = |t: f64| ca.distance(cb.point(t)) <= tol;
    let same = match (ca, cb) {
        (Curve2::Line2(_), Curve2::Line2(_)) => on_a(rb[0]) && on_a(rb[1]),
        (Curve2::Circle2(x), Curve2::Circle2(y)) => {
            (x.frame().origin() - y.frame().origin()).length() <= tol
                && (x.radius() - y.radius()).abs() <= tol
        }
        (Curve2::Ellipse2(x), Curve2::Ellipse2(y)) => {
            (x.frame().origin() - y.frame().origin()).length() <= tol
                && (x.major() - y.major()).abs() <= tol
                && (x.minor() - y.minor()).abs() <= tol
                && (0..8).all(|k| on_a(f64::from(k)))
        }
        _ => return None,
    };
    if !same {
        return None;
    }
    let period = ca.period();
    let to_a = |t: f64| ca.project(cb.point(t)).param;
    let (alo, ahi) = (ra[0].min(ra[1]), ra[0].max(ra[1]));
    let (blo, bhi) = (rb[0].min(rb[1]), rb[0].max(rb[1]));
    // The stretch of a's parameter b covers: from the end of b that comes
    // first as a's parameter grows, for b's length (one speed on one
    // carrier; a line's is mapped exactly).
    let mid = (blo + bhi) / 2.0;
    let forward = ca.eval(to_a(mid)).d1.dot(cb.eval(mid).d1) > 0.0;
    let stretches: Vec<[f64; 2]> = match period {
        None => {
            let (x, y) = (to_a(blo), to_a(bhi));
            vec![[alo.max(x.min(y)), ahi.min(x.max(y))]]
        }
        Some(p) => {
            let first = to_a(if forward { blo } else { bhi });
            let x0 = alo + (first - alo).rem_euclid(p);
            let x1 = x0 + (bhi - blo);
            let (one, two) = (
                [alo.max(x0), ahi.min(x1)],
                [alo.max(x0 - p), ahi.min(x1 - p)],
            );
            // Covering all of a, in two pieces that meet: one stretch.
            if one[1] - one[0] + two[1] - two[0] >= ahi - alo - 1e-12 {
                vec![[alo, ahi]]
            } else {
                vec![one, two]
            }
        }
    };
    let to_b = |t: f64| {
        let tb = cb.project(ca.point(t)).param;
        match cb.period() {
            Some(p) => blo + (tb - blo + 1e-12).rem_euclid(p) - 1e-12,
            None => tb,
        }
    };
    let mut out = Vec::new();
    for [lo, hi] in stretches {
        if hi < lo && ca.point(lo).distance(ca.point(hi)) > tol {
            continue;
        }
        if hi - lo >= ahi - alo - 1e-12 && period.is_some() {
            let start = to_b(lo);
            let sign = if forward { 1.0 } else { -1.0 };
            out.push(Meet2::Overlap {
                ta: [lo, hi],
                tb: [start, start + sign * (hi - lo)],
            });
            continue;
        }
        let (p, q) = (ca.point(lo), ca.point(hi));
        if p.distance(q) <= OVERLAP * tol {
            let t = (lo + hi) / 2.0;
            out.push(Meet2::Point {
                ta: t,
                tb: to_b(t),
                at: ca.point(t).to_vector(),
            });
        } else {
            out.push(Meet2::Overlap {
                ta: [lo, hi],
                tb: [to_b(lo), to_b(hi)],
            });
        }
    }
    Some(out)
}

/// `A(s) Wb(t) - B(t) Wa(s)`, by component, for two pieces.
struct System {
    x: Bernstein2,
    y: Bernstein2,
    /// The least weights, lower bounds on `Wa` and `Wb`.
    wa: f64,
    wb: f64,
    pa: Piece2,
    pb: Piece2,
    tol: f64,
}

impl System {
    fn new(pa: &Piece2, pb: &Piece2, tol: f64) -> Self {
        let (m, n) = (pa.pts.len() - 1, pb.pts.len() - 1);
        let p = Interval::point;
        let in_s =
            |f: &dyn Fn(usize) -> f64| Bernstein2::new(m, 0, (0..=m).map(|i| p(f(i))).collect());
        let in_t =
            |f: &dyn Fn(usize) -> f64| Bernstein2::new(0, n, (0..=n).map(|j| p(f(j))).collect());
        let ax = in_s(&|i| pa.pts[i].p.x * pa.pts[i].w);
        let ay = in_s(&|i| pa.pts[i].p.y * pa.pts[i].w);
        let wa = in_s(&|i| pa.pts[i].w);
        let bx = in_t(&|j| pb.pts[j].p.x * pb.pts[j].w);
        let by = in_t(&|j| pb.pts[j].p.y * pb.pts[j].w);
        let wb = in_t(&|j| pb.pts[j].w);
        Self {
            x: ax.mul(&wb).sub(&bx.mul(&wa)),
            y: ay.mul(&wb).sub(&by.mul(&wa)),
            wa: pa.pts.iter().map(|h| h.w).fold(f64::INFINITY, f64::min),
            wb: pb.pts.iter().map(|h| h.w).fold(f64::INFINITY, f64::min),
            pa: pa.clone(),
            pb: pb.clone(),
            tol,
        }
    }

    /// Solves on the cell `s × t`, pushing certified points to `out` and
    /// cells it can't settle to `cells`; `None` when the budget runs out.
    #[allow(clippy::too_many_arguments)]
    fn solve(
        &self,
        s: [f64; 2],
        t: [f64; 2],
        depth: u32,
        out: &mut Vec<Meet2>,
        cells: &mut Vec<([f64; 2], [f64; 2])>,
        budget: &mut usize,
        (a, i): (&Chain2, usize),
        (b, j): (&Chain2, usize),
    ) -> Option<()> {
        *budget = budget.checked_sub(1)?;
        let (gx, gy) = (self.x.part(s, t), self.y.part(s, t));
        if !gx.range().contains(0.0) || !gy.range().contains(0.0) {
            return Some(());
        }
        match krawczyk(&self.x, &self.y, s, t) {
            Some(false) => return Some(()),
            Some(true) => {
                let (u, v) = self.newton(s, t);
                let at = point(&self.pa, u);
                out.push(Meet2::Point {
                    ta: a.param(i, u),
                    tb: b.param(j, v),
                    at,
                });
                return Some(());
            }
            None => {}
        }
        // Where the curves are certified within the tolerance over the
        // whole cell, subdividing can't tell more: they touch or run
        // together there.
        if depth == MEET_DEPTH || self.gap(&gx, &gy) <= self.tol {
            cells.push((s, t));
            return Some(());
        }
        let (ms, mt) = ((s[0] + s[1]) / 2.0, (t[0] + t[1]) / 2.0);
        for ss in [[s[0], ms], [ms, s[1]]] {
            for tt in [[t[0], mt], [mt, t[1]]] {
                self.solve(ss, tt, depth + 1, out, cells, budget, (a, i), (b, j))?;
            }
        }
        Some(())
    }

    /// The solution in a certified cell, by Newton's method from its
    /// middle, kept inside it.
    fn newton(&self, s: [f64; 2], t: [f64; 2]) -> (f64, f64) {
        let mid = |x: Interval| (x.lo() + x.hi()) / 2.0;
        let (xs, xt, ys, yt) = (
            self.x.deriv_s(),
            self.x.deriv_t(),
            self.y.deriv_s(),
            self.y.deriv_t(),
        );
        let (mut u, mut v) = ((s[0] + s[1]) / 2.0, (t[0] + t[1]) / 2.0);
        for _ in 0..40 {
            let (fx, fy) = (mid(self.x.eval(u, v)), mid(self.y.eval(u, v)));
            let (a, b) = (mid(xs.eval(u, v)), mid(xt.eval(u, v)));
            let (c, d) = (mid(ys.eval(u, v)), mid(yt.eval(u, v)));
            let det = a * d - b * c;
            if det == 0.0 {
                break;
            }
            let (du, dv) = ((d * fx - b * fy) / det, (a * fy - c * fx) / det);
            u = (u - du).clamp(s[0], s[1]);
            v = (v - dv).clamp(t[0], t[1]);
            if du.abs() + dv.abs() < 1e-16 {
                break;
            }
        }
        (u, v)
    }

    /// A bound on the distance between the curves over a cell, from its
    /// pieces of the system.
    fn gap(&self, gx: &Bernstein2, gy: &Bernstein2) -> f64 {
        math_hypot(gx.range().mag(), gy.range().mag()) / (self.wa * self.wb)
    }

    /// A cluster of cells as a touch or an overlap, when every cell is
    /// certified to keep the curves within `tol` of each other. By rule,
    /// it is an overlap when it runs more than [`OVERLAP`] times the
    /// tolerance along either curve; shorter, it is a touch, the zone
    /// around a tangency where rounding can't sign the system.
    fn cluster(
        &self,
        cells: &[([f64; 2], [f64; 2])],
        tol: f64,
        (a, i): (&Chain2, usize),
        (b, j): (&Chain2, usize),
    ) -> Option<Meet2> {
        for (s, t) in cells {
            if self.gap(&self.x.part(*s, *t), &self.y.part(*s, *t)) > tol {
                return None;
            }
        }
        let s = cells.iter().fold([1.0f64, 0.0f64], |r, c| {
            [r[0].min(c.0[0]), r[1].max(c.0[1])]
        });
        let t = cells.iter().fold([1.0f64, 0.0f64], |r, c| {
            [r[0].min(c.1[0]), r[1].max(c.1[1])]
        });
        let (p0, p1) = (point(&self.pa, s[0]), point(&self.pa, s[1]));
        let (q0, q1) = (point(&self.pb, t[0]), point(&self.pb, t[1]));
        let long = OVERLAP * tol;
        if (p1 - p0).length() <= long && (q1 - q0).length() <= long {
            let (u, v) = self.newton(s, t);
            return Some(Meet2::Point {
                ta: a.param(i, u),
                tb: b.param(j, v),
                at: point(&self.pa, u),
            });
        }
        Some(Meet2::Overlap {
            ta: [a.param(i, s[0]), a.param(i, s[1])],
            tb: [b.param(j, t[0]), b.param(j, t[1])],
        })
    }
}

fn math_hypot(x: f64, y: f64) -> f64 {
    crate::math::hypot(x, y)
}

/// The point of a piece at local parameter `s`.
fn point(p: &Piece2, s: f64) -> Vector2 {
    if s <= 0.0 {
        return p.pts[0].p;
    }
    if s >= 1.0 {
        return p.pts[p.pts.len() - 1].p;
    }
    p.split(s)[1].pts[0].p
}

/// Cells grouped by touching.
fn clusters(cells: Vec<([f64; 2], [f64; 2])>) -> Vec<Vec<([f64; 2], [f64; 2])>> {
    let mut groups: Vec<Vec<([f64; 2], [f64; 2])>> = Vec::new();
    for c in cells {
        let touch = |d: &([f64; 2], [f64; 2])| {
            c.0[0] <= d.0[1] && d.0[0] <= c.0[1] && c.1[0] <= d.1[1] && d.1[0] <= c.1[1]
        };
        let hits: Vec<usize> = (0..groups.len())
            .filter(|&g| groups[g].iter().any(touch))
            .collect();
        let mut merged = vec![c];
        for &g in hits.iter().rev() {
            merged.extend(groups.remove(g));
        }
        groups.push(merged);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight degree 1 spline overlapping a line is met as that line:
    /// one overlap, in the spline's own parameters.
    #[test]
    fn a_straight_spline_overlapping_a_line() {
        use crate::geom::{KnotVector, NurbsCurve2};
        let k = KnotVector::clamped(1, &[2.0, 4.0], &[2, 2]).unwrap();
        let seg =
            NurbsCurve2::new(k, vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)], None).unwrap();
        let seg = Curve2::Nurbs2(std::sync::Arc::new(seg));
        let line = Curve2::Line2(
            super::super::Line2::through(Point2::new(0.5, 0.0), Point2::new(3.0, 0.0)).unwrap(),
        );
        let found = crossings((&seg, [2.0, 4.0]), (&line, [0.0, 2.5]), 1e-8).unwrap();
        let [Meet2::Overlap { ta, tb }] = found[..] else {
            panic!("{found:?}");
        };
        assert!(
            (ta[0] - 3.0).abs() <= 1e-12 && (ta[1] - 4.0).abs() <= 1e-12,
            "{ta:?}"
        );
        assert!(
            tb[0].abs() <= 1e-12 && (tb[1] - 0.5).abs() <= 1e-12,
            "{tb:?}"
        );
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

    use crate::geom::{Circle2, Curve2, Frame2, Line2, Point2};
    use core::f64::consts::TAU;

    fn line(a: (f64, f64), b: (f64, f64)) -> (Curve2, [f64; 2]) {
        let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
        (
            Curve2::Line2(Line2::through(a, b).unwrap()),
            [0.0, a.distance(b)],
        )
    }

    fn circle((x, y): (f64, f64), r: f64, t0: f64, t1: f64) -> (Curve2, [f64; 2]) {
        let f = Frame2::new(Point2::new(x, y), Vector2::new(1.0, 0.0), true).unwrap();
        (Curve2::Circle2(Circle2::new(f, r).unwrap()), [t0, t1])
    }

    fn meet(a: (Curve2, [f64; 2]), b: (Curve2, [f64; 2])) -> Vec<Meet2> {
        crossings((&a.0, a.1), (&b.0, b.1), LINEAR_RESOLUTION).unwrap()
    }

    fn points(m: &[Meet2]) -> Vec<Vector2> {
        m.iter()
            .map(|m| match *m {
                Meet2::Point { at, .. } => at,
                Meet2::Overlap { .. } => panic!("an overlap in {m:?}"),
            })
            .collect()
    }

    #[test]
    fn lines_and_circles_cross() {
        let found = meet(line((0.0, 0.0), (2.0, 2.0)), line((0.0, 2.0), (2.0, 0.0)));
        let p = points(&found);
        assert_eq!(p.len(), 1);
        assert!((p[0] - Vector2::new(1.0, 1.0)).length() < 1e-14);
        let Meet2::Point { ta, tb, .. } = found[0] else {
            unreachable!()
        };
        assert!((ta - 2f64.sqrt()).abs() < 1e-13 && (tb - 2f64.sqrt()).abs() < 1e-13);
        // A line through a circle: two crossings, at the right angles.
        let found = meet(
            line((-3.0, 0.5), (3.0, 0.5)),
            circle((0.0, 0.0), 1.0, 0.0, TAU),
        );
        assert_eq!(found.len(), 2);
        for m in &found {
            let Meet2::Point { tb, at, .. } = *m else {
                panic!()
            };
            let (s, c) = crate::math::sin_cos(tb);
            assert!((at - Vector2::new(c, s)).length() < 1e-12);
        }
        // Two circles; and apart ones.
        let p = points(&meet(
            circle((0.0, 0.0), 1.0, 0.0, TAU),
            circle((1.0, 0.5), 1.0, 0.0, TAU),
        ));
        assert_eq!(p.len(), 2);
        assert!(
            meet(
                circle((0.0, 0.0), 1.0, 0.0, TAU),
                circle((3.0, 0.0), 1.0, 0.0, TAU)
            )
            .is_empty()
        );
    }

    #[test]
    fn touches_ends_and_overlaps() {
        // A line tangent to a circle touches it once.
        let p = points(&meet(
            line((-3.0, 1.0), (3.0, 1.0)),
            circle((0.0, 0.0), 1.0, 0.0, TAU),
        ));
        assert_eq!(p.len(), 1);
        assert!((p[0] - Vector2::new(0.0, 1.0)).length() < 1e-6);
        // Two segments sharing an end.
        let p = points(&meet(
            line((0.0, 0.0), (1.0, 0.0)),
            line((1.0, 0.0), (1.0, 1.0)),
        ));
        assert_eq!(p.len(), 1);
        assert!((p[0] - Vector2::new(1.0, 0.0)).length() < 1e-9);
        // Collinear segments overlapping on [1, 2], whichever way they run.
        let found = meet(line((0.0, 0.0), (2.0, 0.0)), line((1.0, 0.0), (3.0, 0.0)));
        assert_eq!(
            found,
            [Meet2::Overlap {
                ta: [1.0, 2.0],
                tb: [0.0, 1.0]
            }]
        );
        let found = meet(line((0.0, 0.0), (2.0, 0.0)), line((3.0, 0.0), (1.0, 0.0)));
        assert_eq!(
            found,
            [Meet2::Overlap {
                ta: [1.0, 2.0],
                tb: [2.0, 1.0]
            }]
        );
        // End to end on one line: a point.
        assert_eq!(
            points(&meet(
                line((0.0, 0.0), (1.0, 0.0)),
                line((1.0, 0.0), (2.0, 0.0))
            ))
            .len(),
            1
        );
        // Arcs of one circle overlapping on [0.5, 1], one of them run
        // backwards and across the seam.
        let found = meet(
            circle((0.0, 0.0), 1.0, 0.0, 1.0),
            circle((0.0, 0.0), 1.0, 0.5, 2.0),
        );
        assert_eq!(found.len(), 1);
        let Meet2::Overlap { ta, tb } = found[0] else {
            panic!("{found:?}")
        };
        assert!(
            (ta[0] - 0.5).abs() < 1e-12 && (ta[1] - 1.0).abs() < 1e-12,
            "{ta:?}"
        );
        assert!(
            (tb[0] - 0.5).abs() < 1e-12 && (tb[1] - 1.0).abs() < 1e-12,
            "{tb:?}"
        );
        let found = meet(
            circle((0.0, 0.0), 1.0, -0.5, 0.5),
            circle((0.0, 0.0), 1.0, TAU + 0.2, TAU - 1.0),
        );
        let Meet2::Overlap { ta, .. } = found[0] else {
            panic!("{found:?}")
        };
        assert!(
            (ta[0] + 0.5).abs() < 1e-12 && (ta[1] - 0.2).abs() < 1e-12,
            "{ta:?}"
        );
    }

    #[test]
    fn random_pairs_agree_with_polylines() {
        use crate::geom::{KnotVector, NurbsCurve2};
        use std::sync::Arc;
        let mut seed = 99u64;
        let mut r = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let curve = |r: &mut dyn FnMut() -> f64| -> (Curve2, [f64; 2]) {
            match (r() * 3.0) as usize {
                0 => line(
                    (r() * 4.0 - 2.0, r() * 4.0 - 2.0),
                    (r() * 4.0 - 2.0, r() * 4.0 - 2.0),
                ),
                1 => {
                    let t0 = r() * 6.0;
                    circle(
                        (r() * 2.0 - 1.0, r() * 2.0 - 1.0),
                        0.3 + r() * 1.5,
                        t0,
                        t0 + 0.5 + r() * 5.0,
                    )
                }
                _ => {
                    let k = KnotVector::clamped(3, &[0.0, 1.0], &[4, 4]).unwrap();
                    let pts = (0..4)
                        .map(|_| Point2::new(r() * 4.0 - 2.0, r() * 4.0 - 2.0))
                        .collect();
                    (
                        Curve2::Nurbs2(Arc::new(NurbsCurve2::new(k, pts, None).unwrap())),
                        [0.0, 1.0],
                    )
                }
            }
        };
        let polyline = |c: &(Curve2, [f64; 2])| -> Vec<Vector2> {
            (0..=600)
                .map(|k| {
                    c.0.point(c.1[0] + (c.1[1] - c.1[0]) * f64::from(k) / 600.0)
                        .to_vector()
                })
                .collect()
        };
        let mut uncertain = 0;
        for _ in 0..200 {
            let (a, b) = (curve(&mut r), curve(&mut r));
            let Some(found) = crossings((&a.0, a.1), (&b.0, b.1), LINEAR_RESOLUTION) else {
                uncertain += 1;
                continue;
            };
            for m in &found {
                if let Meet2::Point { ta, tb, at } = *m {
                    assert!((a.0.point(ta).to_vector() - at).length() < 1e-6);
                    assert!((b.0.point(tb).to_vector() - at).length() < 1e-6);
                }
            }
            // Every crossing of the polylines is near a point found.
            let (pa, pb) = (polyline(&a), polyline(&b));
            for u in pa.windows(2) {
                for v in pb.windows(2) {
                    let d = (u[1] - u[0]).cross(v[1] - v[0]);
                    if d == 0.0 {
                        continue;
                    }
                    let w = v[0] - u[0];
                    let (s, t) = (w.cross(v[1] - v[0]) / d, w.cross(u[1] - u[0]) / d);
                    if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
                        let x = u[0] + (u[1] - u[0]) * s;
                        let near = found.iter().any(|m| match *m {
                            Meet2::Point { at, .. } => (at - x).length() < 1e-2,
                            Meet2::Overlap { .. } => true,
                        });
                        assert!(near, "missed {x:?} in {found:?}");
                    }
                }
            }
        }
        assert_eq!(uncertain, 0);
    }
}
