//! Distances between two pieces: every place the least distance can be,
//! as candidates, each compared with a threshold exactly.
//!
//! Between a segment or an arc and another, the least distance is zero
//! where they meet, else it is reached at an end of one of them, or inside
//! both where the line between the two points is normal to both: for a
//! segment and an arc along the perpendicular from the arc's centre, for
//! two arcs along their line of centres. Every candidate is a distance
//! between a point of each piece, so none is below the least; their least
//! is the least distance.

use std::cmp::Ordering;

use ryu_board::geom::{Point, Rect, V};

use super::core::{ArcP, Core, Pt, SegP};
use super::exact::{self, Fp};
pub use crate::tree::gap2;

/// A place the least distance between two pieces may be.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// The pieces meet there.
    Zero((f64, f64)),
    /// Between two points.
    PP(Pt, Pt),
    /// A point and its foot on a segment.
    PSeg(Pt, SegP),
    /// A point and the arc along the ray from its centre.
    PArc(Pt, ArcP),
    /// The arc's centre and the arc: its radius.
    Centre(ArcP),
    /// A segment and an arc along the perpendicular from the centre, on
    /// side `σ` of the line's direction.
    SegArc(SegP, ArcP, i128),
    /// Two arcs along their line of centres.
    ArcArc(ArcP, ArcP, i128, i128),
}

/// A candidate, `swap` set when its first piece is the second argument.
#[derive(Clone, Copy, Debug)]
pub struct Cand {
    pub kind: Kind,
    pub swap: bool,
}

fn add(out: &mut Vec<Cand>, kind: Kind, swap: bool) {
    out.push(Cand { kind, swap });
}

fn ends(c: &Core) -> [Pt; 2] {
    match c {
        Core::Seg(s) => s.ends(),
        Core::Arc(a) => [Pt::At(a.arc.start), a.q],
    }
}

fn point_seg(p: Pt, s: &SegP, swap: bool, out: &mut Vec<Cand>) {
    if exact::foot_within(&Fp::of(p), s) {
        add(out, Kind::PSeg(p, *s), swap);
    }
}

fn point_arc(p: Pt, a: &ArcP, swap: bool, out: &mut Vec<Cand>) {
    let f = Fp::of(p);
    let d = exact::from_centre(&f, a);
    if d.is_zero() {
        add(out, Kind::Centre(*a), swap);
    } else if exact::sweeps(a, &d) {
        add(out, Kind::PArc(p, *a), swap);
    }
}

fn seg_arc(s: &SegP, a: &ArcP, swap: bool, out: &mut Vec<Cand>) {
    if let Some(at) = exact::seg_arc_meet(s, a) {
        add(out, Kind::Zero(at), swap);
    }
    let u = s.u();
    if u.len2() == 0 {
        return;
    }
    if !exact::foot_within(&Fp::of(Pt::At(a.arc.centre)), s) {
        return;
    }
    let perp = V(-u.1, u.0);
    for sigma in [1i128, -1] {
        if a.sweeps(V(sigma * perp.0, sigma * perp.1)) {
            add(out, Kind::SegArc(*s, *a, sigma), swap);
        }
    }
}

/// Every candidate between two pieces.
pub fn candidates(a: &Core, b: &Core) -> Vec<Cand> {
    let mut out = Vec::new();
    match (a, b) {
        (Core::Seg(s), Core::Seg(t)) => {
            if let Some((n, d)) = exact::seg_seg_cross(s, t) {
                let k = n as f64 / d as f64;
                add(
                    &mut out,
                    Kind::Zero((
                        s.b.x as f64 + k * s.u.0 as f64,
                        s.b.y as f64 + k * s.u.1 as f64,
                    )),
                    false,
                );
            }
            for e in s.ends() {
                point_seg(e, t, false, &mut out);
            }
            for e in t.ends() {
                point_seg(e, s, true, &mut out);
            }
        }
        (Core::Seg(s), Core::Arc(r)) => {
            seg_arc(s, r, false, &mut out);
            for e in s.ends() {
                point_arc(e, r, false, &mut out);
            }
            for e in ends(b) {
                point_seg(e, s, true, &mut out);
            }
        }
        (Core::Arc(r), Core::Seg(s)) => {
            seg_arc(s, r, true, &mut out);
            for e in s.ends() {
                point_arc(e, r, true, &mut out);
            }
            for e in ends(a) {
                point_seg(e, s, false, &mut out);
            }
        }
        (Core::Arc(r), Core::Arc(t)) => {
            if let Some(at) = exact::arc_arc_meet(r, t) {
                add(&mut out, Kind::Zero(at), false);
            }
            for e in ends(a) {
                point_arc(e, t, false, &mut out);
            }
            for e in ends(b) {
                point_arc(e, r, true, &mut out);
            }
            let w = t.arc.centre.minus(r.arc.centre);
            if w.len2() != 0 {
                for s1 in [1i128, -1] {
                    for s2 in [1i128, -1] {
                        if r.sweeps(V(s1 * w.0, s1 * w.1)) && t.sweeps(V(s2 * w.0, s2 * w.1)) {
                            add(&mut out, Kind::ArcArc(*r, *t, s1, s2), false);
                        }
                    }
                }
            }
        }
    }
    for p in ends(a) {
        for q in ends(b) {
            add(&mut out, Kind::PP(p, q), false);
        }
    }
    out
}

/// A candidate's distance against `h = dd / 2`. Exact.
pub fn compare(c: &Cand, dd: i128) -> Ordering {
    match &c.kind {
        Kind::Zero(_) => {
            if dd == 0 {
                Ordering::Equal
            } else {
                Ordering::Less
            }
        }
        Kind::PP(p, q) => exact::pp(&Fp::of(*p), &Fp::of(*q), dd),
        Kind::PSeg(p, s) => exact::pseg(&Fp::of(*p), s, dd),
        Kind::PArc(p, a) => exact::parc(&Fp::of(*p), a, dd),
        Kind::Centre(a) => exact::centre(a, dd),
        Kind::SegArc(s, a, sigma) => exact::seg_arc(s, a, *sigma, dd),
        Kind::ArcArc(a, b, s1, s2) => exact::arc_arc(a, b, *s1, *s2, dd),
    }
}

/// Whether boxes this far apart are farther than `h = dd / 2`.
pub fn boxes_apart(a: &Rect, b: &Rect, dd: i128) -> bool {
    4 * gap2(a, b) > dd * dd
}

fn pt_seg(p: P, a: P, b: P) -> f64 {
    let (ux, uy) = (b.0 - a.0, b.1 - a.1);
    let l = ux * ux + uy * uy;
    let t = if l == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * ux + (p.1 - a.1) * uy) / l).clamp(0.0, 1.0)
    };
    dist(p, (a.0 + t * ux, a.1 + t * uy))
}

fn orient(a: P, b: P, c: P) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Whether two orientations are certainly of one sign: away from zero by
/// far more than their rounding.
fn one_side(o1: f64, o2: f64, tol: f64) -> bool {
    o1 * o2 > 0.0 && o1.abs() > tol && o2.abs() > tol
}

/// Two segments: the least distance from an end of one to the other,
/// unless they may cross.
fn segs(p1: P, p2: P, q1: P, q2: P, m: f64) -> f64 {
    let tol = 1e-9 * (m * m + 1.0);
    let apart = one_side(orient(p1, p2, q1), orient(p1, p2, q2), tol)
        || one_side(orient(q1, q2, p1), orient(q1, q2, p2), tol);
    if !apart {
        return 0.0;
    }
    pt_seg(p1, q1, q2)
        .min(pt_seg(p2, q1, q2))
        .min(pt_seg(q1, p1, p2))
        .min(pt_seg(q2, p1, p2))
}

/// A segment and a whole circle.
fn seg_circle(p1: P, p2: P, c: P, rad: f64) -> f64 {
    let near = pt_seg(c, p1, p2);
    let far = dist(c, p1).max(dist(c, p2));
    if near > rad {
        near - rad
    } else if far < rad {
        rad - far
    } else {
        0.0
    }
}

/// The margin a lower bound worked from magnitudes up to `m` allows.
fn margin(m: f64) -> f64 {
    1.0 + 1e-9 * m
}

/// A lower bound on the distance from a piece to a box, its margin taken
/// off: a segment against the box's sides (zero where an end is inside),
/// an arc's whole circle against the box's nearest and farthest points.
pub fn lower_to_box(c: &Core, r: &Rect) -> f64 {
    let (x0, y0, x1, y1) = (
        r.min.x as f64,
        r.min.y as f64,
        r.max.x as f64,
        r.max.y as f64,
    );
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    let inside = |p: P| x0 - 1.0 <= p.0 && p.0 <= x1 + 1.0 && y0 - 1.0 <= p.1 && p.1 <= y1 + 1.0;
    let m0 = corners
        .iter()
        .fold(0.0f64, |m, p| m.max(p.0.abs()).max(p.1.abs()));
    match c {
        Core::Seg(s) => {
            let [a, b] = s.ends();
            let (p1, p2) = (a.approx(), b.approx());
            if inside(p1) || inside(p2) {
                return 0.0;
            }
            let m = m0
                .max(p1.0.abs())
                .max(p1.1.abs())
                .max(p2.0.abs())
                .max(p2.1.abs());
            let d = (0..4)
                .map(|k| segs(p1, p2, corners[k], corners[(k + 1) % 4], m))
                .fold(f64::MAX, f64::min);
            d - margin(m)
        }
        Core::Arc(a) => {
            let c = (a.arc.centre.x as f64, a.arc.centre.y as f64);
            let rad = ryu_math::sqrt(a.r2 as f64);
            let near = dist(c, (c.0.clamp(x0, x1), c.1.clamp(y0, y1)));
            let far = corners.iter().map(|&q| dist(c, q)).fold(0.0, f64::max);
            let d = if near > rad {
                near - rad
            } else if far < rad {
                rad - far
            } else {
                0.0
            };
            d - margin(m0.max(c.0.abs()).max(c.1.abs()) + rad)
        }
    }
}

/// A lower bound on the distance between two pieces, in floating point,
/// and the largest magnitude it was worked from.
///
/// Two segments: the least distance from an end of one to the other,
/// unless they may cross. A segment or an arc and an arc: the distance
/// to the arc's whole circle. Every value is a few roundings of numbers no
/// larger than the magnitude, so it is within `1e-12` of that of the
/// truth; [`core_cmp`] allows it a nanometre and `1e-9` of the
/// magnitude.
pub fn lower(a: &Core, b: &Core) -> (f64, f64) {
    fn seg_pts(s: &SegP) -> (P, P) {
        let [a, b] = s.ends();
        (a.approx(), b.approx())
    }
    fn circle(a: &ArcP) -> (P, f64) {
        (
            (a.arc.centre.x as f64, a.arc.centre.y as f64),
            ryu_math::sqrt(a.r2 as f64),
        )
    }
    fn mag(ps: &[P], r: f64) -> f64 {
        ps.iter().fold(r, |m, p| m.max(p.0.abs()).max(p.1.abs()))
    }
    match (a, b) {
        (Core::Seg(s), Core::Seg(t)) => {
            let ((p1, p2), (q1, q2)) = (seg_pts(s), seg_pts(t));
            let m = mag(&[p1, p2, q1, q2], 0.0);
            (segs(p1, p2, q1, q2, m), m)
        }
        (Core::Seg(s), Core::Arc(r)) | (Core::Arc(r), Core::Seg(s)) => {
            let (p1, p2) = seg_pts(s);
            let (c, rad) = circle(r);
            (seg_circle(p1, p2, c, rad), mag(&[p1, p2, c], rad))
        }
        (Core::Arc(r), Core::Arc(t)) => {
            let ((c1, r1), (c2, r2)) = (circle(r), circle(t));
            let dc = dist(c1, c2);
            let d = (dc - r1 - r2).max((r1 - r2).abs() - dc).max(0.0);
            (d, mag(&[c1, c2], r1.max(r2)))
        }
    }
}

/// The least distance between two pieces against `h = dd / 2`. Exact.
pub fn core_cmp(a: &Core, b: &Core, dd: i128) -> Ordering {
    if boxes_apart(&a.bounds(), &b.bounds(), dd) {
        return Ordering::Greater;
    }
    let (low, m) = lower(a, b);
    if low > dd as f64 / 2.0 + margin(m) {
        return Ordering::Greater;
    }
    let mut best = Ordering::Greater;
    // Nothing is nearer than touching: at a zero threshold that settles it.
    let floor = if dd == 0 {
        Ordering::Equal
    } else {
        Ordering::Less
    };
    for c in candidates(a, b) {
        best = best.min(compare(&c, dd));
        if best == floor {
            break;
        }
    }
    best
}

type P = (f64, f64);

fn dist(a: P, b: P) -> f64 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    ryu_math::sqrt(dx * dx + dy * dy)
}

fn on_ray(c: Point, r2: i128, towards: P) -> P {
    let (cx, cy) = (c.x as f64, c.y as f64);
    let (dx, dy) = (towards.0 - cx, towards.1 - cy);
    let k = ryu_math::sqrt(r2 as f64) / ryu_math::sqrt(dx * dx + dy * dy);
    (cx + dx * k, cy + dy * k)
}

fn foot_on(p: P, s: &SegP) -> P {
    let (ux, uy) = (s.u.0 as f64, s.u.1 as f64);
    let (bx, by) = (s.b.x as f64, s.b.y as f64);
    let t = ((p.0 - bx) * ux + (p.1 - by) * uy) / (ux * ux + uy * uy);
    (bx + t * ux, by + t * uy)
}

/// A candidate's distance and its two points (on the first piece, then
/// the second), in floating point: for estimates and nearest points.
pub fn approx(c: &Cand) -> (f64, P, P) {
    let (d, p, q) = match &c.kind {
        Kind::Zero(at) => (0.0, *at, *at),
        Kind::PP(p, q) => {
            let (p, q) = (p.approx(), q.approx());
            (dist(p, q), p, q)
        }
        Kind::PSeg(p, s) => {
            let p = p.approx();
            let f = foot_on(p, s);
            (dist(p, f), p, f)
        }
        Kind::PArc(p, a) => {
            let p = p.approx();
            let q = on_ray(a.arc.centre, a.r2, p);
            (dist(p, q), p, q)
        }
        Kind::Centre(a) => {
            let c = (a.arc.centre.x as f64, a.arc.centre.y as f64);
            let s = (a.arc.start.x as f64, a.arc.start.y as f64);
            (dist(c, s), c, s)
        }
        Kind::SegArc(s, a, sigma) => {
            let c = a.arc.centre;
            let side = (
                c.x as f64 - *sigma as f64 * s.u.1 as f64,
                c.y as f64 + *sigma as f64 * s.u.0 as f64,
            );
            let y = on_ray(c, a.r2, side);
            let x = foot_on(y, s);
            (dist(x, y), x, y)
        }
        Kind::ArcArc(a, b, s1, s2) => {
            let (c1, c2) = (a.arc.centre, b.arc.centre);
            let w = ((c2.x - c1.x) as f64, (c2.y - c1.y) as f64);
            let y1 = on_ray(
                c1,
                a.r2,
                (
                    c1.x as f64 + *s1 as f64 * w.0,
                    c1.y as f64 + *s1 as f64 * w.1,
                ),
            );
            let y2 = on_ray(
                c2,
                b.r2,
                (
                    c2.x as f64 + *s2 as f64 * w.0,
                    c2.y as f64 + *s2 as f64 * w.1,
                ),
            );
            (dist(y1, y2), y1, y2)
        }
    };
    if c.swap { (d, q, p) } else { (d, p, q) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::core::edge_cores;
    use ryu_board::geom::{Arc, COORD_LIMIT, Dir};

    const L: i64 = COORD_LIMIT;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn seg(a: Point, b: Point) -> Core {
        Core::Seg(SegP::new(a, b))
    }

    fn arc(start: Point, end: Point, centre: Point, dir: Dir) -> Core {
        Core::Arc(ArcP::new(Arc {
            start,
            end,
            centre,
            dir,
        }))
    }

    /// The exact distance's place against `h`, at `h` and a nanometre
    /// either side.
    fn exactly(a: &Core, b: &Core, d: i128) {
        assert_eq!(core_cmp(a, b, 2 * d), Ordering::Equal, "{a:?} {b:?} at {d}");
        assert_eq!(core_cmp(a, b, 2 * d - 1), Ordering::Greater);
        assert_eq!(core_cmp(a, b, 2 * d + 1), Ordering::Less);
        assert_eq!(core_cmp(b, a, 2 * d), Ordering::Equal);
    }

    #[test]
    fn segments() {
        let a = seg(p(0, 0), p(10, 0));
        exactly(&a, &seg(p(3, 4), p(3, 9)), 4);
        exactly(&a, &seg(p(13, 4), p(20, 9)), 5);
        // Crossing, touching at an end, overlapping along a line.
        assert_eq!(core_cmp(&a, &seg(p(5, -5), p(5, 5)), 0), Ordering::Equal);
        assert_eq!(core_cmp(&a, &seg(p(10, 0), p(10, 5)), 0), Ordering::Equal);
        assert_eq!(core_cmp(&a, &seg(p(5, 0), p(15, 0)), 0), Ordering::Equal);
        // Parallel, apart: 3.
        exactly(&a, &seg(p(-5, 3), p(50, 3)), 3);
        // A point.
        exactly(&a, &seg(p(4, 7), p(4, 7)), 7);
        assert_eq!(core_cmp(&a, &seg(p(4, 0), p(4, 0)), 0), Ordering::Equal);
    }

    #[test]
    fn a_point_and_arcs() {
        let q = arc(p(100, 0), p(0, 100), p(0, 0), Dir::Ccw);
        // Along the ray: 100 off the arc at (200, 200)? |(200,200)| = 282.8.
        exactly(&q, &seg(p(0, 150), p(0, 150)), 50);
        exactly(&q, &seg(p(0, 0), p(0, 0)), 100);
        // Not swept: to the nearer end (100, 0) from (100, -30).
        exactly(&q, &seg(p(100, -30), p(100, -30)), 30);
        // The clockwise arc is the other three quarters: (0,150) is 50
        // from its end, along the y axis.
        let cw = arc(p(100, 0), p(0, 100), p(0, 0), Dir::Cw);
        exactly(&cw, &seg(p(0, 150), p(0, 150)), 50);
        // (-100, -100) is √20000 - 100 = 41.42 off it, along the ray.
        let x = seg(p(-100, -100), p(-100, -100));
        assert_eq!(core_cmp(&cw, &x, 2 * 41), Ordering::Greater);
        assert_eq!(core_cmp(&cw, &x, 2 * 42), Ordering::Less);
        assert_eq!(core_cmp(&q, &x, 2 * 42), Ordering::Greater);
    }

    #[test]
    fn segments_and_arcs() {
        let q = arc(p(100, 0), p(-100, 0), p(0, 0), Dir::Ccw);
        // A line tangent to the top: distance 0, never crossing.
        assert_eq!(
            core_cmp(&q, &seg(p(-50, 100), p(50, 100)), 0),
            Ordering::Equal
        );
        // A line above the top: 20 across the perpendicular from the
        // centre.
        exactly(&q, &seg(p(-50, 120), p(50, 120)), 20);
        // A chord crossing the arc.
        assert_eq!(
            core_cmp(&q, &seg(p(-200, 50), p(200, 50)), 0),
            Ordering::Equal
        );
        // Below, the lower half isn't there: nearest are the ends.
        exactly(&q, &seg(p(-100, -40), p(100, -40)), 40);
    }

    #[test]
    fn two_arcs() {
        let a = arc(p(100, 0), p(100, 0), p(0, 0), Dir::Ccw);
        // Circles of 100 and 50 with centres 170 apart: 20 apart.
        let b = arc(p(220, 0), p(220, 0), p(170, 0), Dir::Ccw);
        exactly(&a, &b, 20);
        // Tangent outside, centres 150 apart.
        let c = arc(p(200, 0), p(200, 0), p(150, 0), Dir::Ccw);
        assert_eq!(core_cmp(&a, &c, 0), Ordering::Equal);
        assert_eq!(core_cmp(&a, &c, 1), Ordering::Less);
        // Inside, concentric: 100 - 30.
        let d = arc(p(30, 0), p(30, 0), p(0, 0), Dir::Ccw);
        exactly(&a, &d, 70);
        // Tangent inside: radius 40 centred at (60, 0).
        let e = arc(p(100, 0), p(100, 0), p(60, 0), Dir::Ccw);
        assert_eq!(core_cmp(&a, &e, 0), Ordering::Equal);
        // Crossing circles.
        let f = arc(p(130, 0), p(130, 0), p(80, 0), Dir::Ccw);
        assert_eq!(core_cmp(&a, &f, 0), Ordering::Equal);
    }

    /// Arcs at the limits whose written ends are off their circles, so
    /// their true ends carry 166-bit radicands.
    fn extreme_arcs() -> Vec<Core> {
        let mut out = Vec::new();
        for (start, end, centre, dir) in [
            (p(L, -L), p(-L + 3, L), p(-L, -L), Dir::Ccw),
            (p(-L, L), p(L, -L + 7), p(L, L), Dir::Cw),
            (p(L, 0), p(-L + 1, 1), p(0, 0), Dir::Ccw),
            (p(0, -L), p(1, L - 5), p(0, 0), Dir::Cw),
        ] {
            edge_cores(
                &ryu_board::geom::Edge::Arc(Arc {
                    start,
                    end,
                    centre,
                    dir,
                }),
                &mut out,
            );
        }
        out.push(Core::Seg(SegP::new(p(-L, L), p(L, -L))));
        out.push(Core::Seg(SegP::new(p(L, L), p(L, L))));
        out
    }

    /// Every candidate at the limits stays inside the budget: the widest
    /// product here needs 26 of the 32 limbs (1,664 bits).
    #[test]
    fn extremes() {
        let cores = extreme_arcs();
        // Joins are there: the true ends are irrational.
        assert!(cores.len() > 6);
        for a in &cores {
            for b in &cores {
                for c in candidates(a, b) {
                    for dd in [0i128, 1, 1 << 44] {
                        compare(&c, dd);
                    }
                }
            }
        }
    }

    #[test]
    fn irrational_ends() {
        // Radius 1000, written end 1 nm outside: the true end is (0, 1000).
        let q = arc(p(1000, 0), p(0, 1001), p(0, 0), Dir::Ccw);
        exactly(&q, &seg(p(0, 1100), p(0, 1100)), 100);
        // A quarter arc whose true end is irrational: written (3, 4)·200,
        // radius 999: true end (599.4, 799.2).
        let r = arc(p(999, 0), p(600, 800), p(0, 0), Dir::Ccw);
        // From (599, 900): along the ray it is not swept? (599,900) is at
        // 56.35°, beyond the end's 53.13°: nearest is the true end,
        // √(0.16 + 100.8²) = 100.8008 nm.
        let x = seg(p(599, 900), p(599, 900));
        assert_eq!(core_cmp(&r, &x, 2 * 101), Ordering::Less);
        assert_eq!(core_cmp(&r, &x, 2 * 100 + 1), Ordering::Greater);
        assert_eq!(core_cmp(&r, &x, 2 * 100 + 2), Ordering::Less);
        let (d, _, _) = candidates(&r, &x).iter().map(approx).fold(
            (f64::MAX, (0.0, 0.0), (0.0, 0.0)),
            |m, c| {
                if c.0 < m.0 { c } else { m }
            },
        );
        assert!((d - 100.8008).abs() < 1e-3, "{d}");
    }
}
