//! The exact arithmetic of measurement: points with square roots in them,
//! directions an arc sweeps, and each kind of candidate distance compared
//! with a threshold.
//!
//! A threshold is given doubled, `dd = 2h`, so half nanometres stay whole.
//! Every comparison returns the [`Ordering`] of the candidate distance
//! against `h`, worked out from squares: both are non-negative.
//!
//! **Bit budgets.** Coordinates lie within ±2^40, so differences need 41
//! bits and squared lengths and radii 83; thresholds are capped at 2^44
//! (`dd`). A true end `(c|E|² + E√(R|E|²)) / |E|²` has integer parts of
//! 124 bits and a radicand of 166. The widest comparison is between two
//! true ends ([`pp`]): its `sign2` squares terms of about 420 bits twice,
//! about 1,690 bits, inside [`Int`]'s 2,048. Each function's comment gives
//! its own; `tests::extremes` runs every one at the limits.

use std::cmp::Ordering;

use ryu_board::geom::{Dir, V};

use super::core::{ArcP, Param, Pt, SegP};
use crate::surd::{sign1, sign2};
use crate::wide::Int;

fn i(v: i128) -> Int {
    Int::from(v)
}

/// A point `((nx + mx√x) / s, (ny + my√x) / s)`, `s > 0`.
#[derive(Clone, Copy, Debug)]
pub struct Fp {
    pub nx: Int,
    pub ny: Int,
    pub mx: Int,
    pub my: Int,
    pub x: Int,
    pub s: Int,
}

impl Fp {
    pub fn of(p: Pt) -> Fp {
        match p {
            Pt::At(p) => Fp {
                nx: i(p.x.into()),
                ny: i(p.y.into()),
                mx: Int::ZERO,
                my: Int::ZERO,
                x: Int::ZERO,
                s: i(1),
            },
            Pt::End { c, p, r2 } => {
                let e = p.minus(c);
                let l = e.len2();
                Fp {
                    nx: i(c.x.into()) * i(l),
                    ny: i(c.y.into()) * i(l),
                    mx: i(e.0),
                    my: i(e.1),
                    x: i(r2) * i(l),
                    s: i(l),
                }
            }
        }
    }
}

/// A parameter `(n + m√x) / s` along a line whose direction has squared
/// length `l`.
#[derive(Clone, Copy, Debug)]
pub struct Ft {
    pub n: Int,
    pub m: Int,
    pub x: Int,
    pub s: Int,
}

impl Ft {
    pub fn of(t: Param, l: i128) -> Ft {
        match t {
            Param::Zero => Ft {
                n: Int::ZERO,
                m: Int::ZERO,
                x: Int::ZERO,
                s: i(1),
            },
            Param::One => Ft {
                n: i(1),
                m: Int::ZERO,
                x: Int::ZERO,
                s: i(1),
            },
            Param::Lambda(r2) => Ft {
                n: Int::ZERO,
                m: i(1),
                x: i(r2) * i(l),
                s: i(l),
            },
        }
    }
}

/// The signs a direction `n + m√x` (both parts integer vectors) has
/// against integer vectors: its cross and dot products.
#[derive(Clone, Copy, Debug)]
pub struct Dirn {
    pub n: (Int, Int),
    pub m: (Int, Int),
    pub x: Int,
}

impl Dirn {
    pub fn int(v: V) -> Dirn {
        Dirn {
            n: (i(v.0), i(v.1)),
            m: (Int::ZERO, Int::ZERO),
            x: Int::ZERO,
        }
    }

    /// `a × self`.
    fn cross_from(&self, a: V) -> Ordering {
        let (ax, ay) = (i(a.0), i(a.1));
        sign1(
            ax * self.n.1 - ay * self.n.0,
            ax * self.m.1 - ay * self.m.0,
            self.x,
        )
    }

    /// `a · self`.
    fn dot_from(&self, a: V) -> Ordering {
        let (ax, ay) = (i(a.0), i(a.1));
        sign1(
            ax * self.n.0 + ay * self.n.1,
            ax * self.m.0 + ay * self.m.1,
            self.x,
        )
    }

    pub fn is_zero(&self) -> bool {
        sign1(self.n.0, self.m.0, self.x) == Ordering::Equal
            && sign1(self.n.1, self.m.1, self.x) == Ordering::Equal
    }
}

/// Which half-turn counterclockwise from `from` the direction lies in: 0
/// for `[0, π)`, 1 for `[π, 2π)` (ryu-board's rule, over roots).
fn half_turn(from: V, v: &Dirn) -> u8 {
    let c = v.cross_from(from);
    if c == Ordering::Greater || (c == Ordering::Equal && v.dot_from(from) == Ordering::Greater) {
        0
    } else {
        1
    }
}

/// Whether the arc passes the direction `v` from its centre, its ends
/// included: [`ryu_board::geom::Arc::sweeps`] for a direction with a root
/// in it.
pub fn sweeps(a: &ArcP, v: &Dirn) -> bool {
    if a.arc.is_full() {
        return true;
    }
    let s = a.arc.start.minus(a.arc.centre);
    let e = a.arc.end.minus(a.arc.centre);
    // Compares the counterclockwise angles from `from` to `v` and to `to`.
    let before = |from: V, to: V| -> bool {
        let hv = half_turn(from, v);
        let ht = half_turn(from, &Dirn::int(to));
        // v × to > 0 means v comes first.
        hv < ht || (hv == ht && v.cross_from(to) != Ordering::Greater)
    };
    match a.arc.dir {
        Dir::Ccw => before(s, e),
        Dir::Cw => before(e, s),
    }
}

/// `dd²` as an [`Int`].
fn dd2(dd: i128) -> Int {
    i(dd).square()
}

/// Two points against `h`: `|p - q|² (s1 s2)²` is
/// `A + B√x1 + C√x2 + D√(x1 x2)`. About 420-bit terms, squared twice in
/// `sign2`: 1,690 bits.
pub fn pp(p: &Fp, q: &Fp, dd: i128) -> Ordering {
    let ax = p.nx * q.s - q.nx * p.s;
    let ay = p.ny * q.s - q.ny * p.s;
    let (bx, by) = (p.mx * q.s, p.my * q.s);
    let (cx, cy) = (-(q.mx * p.s), -(q.my * p.s));
    let a = ax.square()
        + ay.square()
        + p.x * (bx.square() + by.square())
        + q.x * (cx.square() + cy.square());
    let two = i(2);
    let b = two * (ax * bx + ay * by);
    let c = two * (ax * cx + ay * cy);
    let d = two * (bx * cx + by * cy);
    let four = i(4);
    let ss = (p.s * q.s).square();
    sign2(
        four * a - dd2(dd) * ss,
        four * b,
        four * c,
        four * d,
        p.x,
        q.x,
    )
}

/// `p`'s parameter along `seg`'s line, as `(dn + dm√x) / (s l)`.
fn foot(p: &Fp, seg: &SegP) -> (Int, Int) {
    let (ux, uy) = (i(seg.u.0), i(seg.u.1));
    let (bx, by) = (i(seg.b.x.into()), i(seg.b.y.into()));
    let dn = ux * (p.nx - bx * p.s) + uy * (p.ny - by * p.s);
    let dm = ux * p.mx + uy * p.my;
    (dn, dm)
}

/// Whether `p`'s foot on `seg`'s line lies between its ends.
pub fn foot_within(p: &Fp, seg: &SegP) -> bool {
    let l = seg.u().len2();
    if l == 0 {
        return false;
    }
    let (dn, dm) = foot(p, seg);
    let sl = p.s * i(l);
    let t0 = Ft::of(seg.t0, l);
    let t1 = Ft::of(seg.t1, l);
    // t* - t0 >= 0 and t1 - t* >= 0, over s l t.s.
    sign2(
        dn * t0.s - t0.n * sl,
        dm * t0.s,
        -(t0.m * sl),
        Int::ZERO,
        p.x,
        t0.x,
    ) != Ordering::Less
        && sign2(
            t1.n * sl - dn * t1.s,
            -(dm * t1.s),
            t1.m * sl,
            Int::ZERO,
            p.x,
            t1.x,
        ) != Ordering::Less
}

/// A point against `h` across to its foot on `seg`'s line (the caller has
/// checked the foot lies on the segment): `cross(U, p - b)² / l`.
/// About 210-bit terms, squared once: 590 bits.
pub fn pseg(p: &Fp, seg: &SegP, dd: i128) -> Ordering {
    let (ux, uy) = (i(seg.u.0), i(seg.u.1));
    let (bx, by) = (i(seg.b.x.into()), i(seg.b.y.into()));
    let cn = ux * (p.ny - by * p.s) - uy * (p.nx - bx * p.s);
    let cm = ux * p.my - uy * p.mx;
    let l = i(seg.u().len2());
    let four = i(4);
    sign1(
        four * (cn.square() + cm.square() * p.x) - dd2(dd) * p.s.square() * l,
        i(8) * cn * cm,
        p.x,
    )
}

/// The direction from an arc's centre to `p`.
pub fn from_centre(p: &Fp, a: &ArcP) -> Dirn {
    let c = a.arc.centre;
    Dirn {
        n: (p.nx - i(c.x.into()) * p.s, p.ny - i(c.y.into()) * p.s),
        m: (p.mx, p.my),
        x: p.x,
    }
}

/// A point against `h` across to an arc's circle along the ray from its
/// centre (the caller has checked the arc sweeps that ray, and that `p`
/// isn't the centre): `|√A - √R|`. With `A' = A s² = α + β√x`, compares
/// `L = 4A' + 4s²R - dd²s²` with `8s√(A'R)`. About 255-bit terms, squared
/// twice: 1,020 bits.
pub fn parc(p: &Fp, a: &ArcP, dd: i128) -> Ordering {
    let v = from_centre(p, a);
    let two = i(2);
    let alpha = v.n.0.square() + v.n.1.square() + p.x * (v.m.0.square() + v.m.1.square());
    let beta = two * (v.n.0 * v.m.0 + v.n.1 * v.m.1);
    let r = i(a.r2);
    let s2 = p.s.square();
    let four = i(4);
    let l0 = four * alpha + four * s2 * r - dd2(dd) * s2;
    let l1 = four * beta;
    if sign1(l0, l1, p.x) != Ordering::Greater {
        return Ordering::Less;
    }
    let k = i(64) * s2 * r;
    sign1(
        l0.square() + l1.square() * p.x - k * alpha,
        two * l0 * l1 - k * beta,
        p.x,
    )
}

/// The centre against `h` across to its circle: `r`.
pub fn centre(a: &ArcP, dd: i128) -> Ordering {
    (i(4) * i(a.r2) - dd2(dd)).sign()
}

/// A segment's line and an arc's circle against `h` where the
/// perpendicular from the centre meets them (the caller has checked that
/// the arc sweeps `σ perp(U)` and the foot lies on the segment):
/// `|k - σ√(RL)| / √L`, `k = (b - c) · perp(U)`. About 170-bit terms,
/// squared once: 510 bits.
pub fn seg_arc(seg: &SegP, a: &ArcP, sigma: i128, dd: i128) -> Ordering {
    let u = seg.u();
    let perp = V(-u.1, u.0);
    let k = i(seg.b.minus(a.arc.centre).dot(perp));
    let l = i(u.len2());
    let r = i(a.r2);
    let four = i(4);
    sign1(
        four * k.square() + four * r * l - dd2(dd) * l,
        i(-8 * sigma) * k,
        r * l,
    )
}

/// Two arcs' circles against `h` along their line of centres (the caller
/// has checked that the arcs sweep `σ1 W` and `σ2 W`, `W = c2 - c1 ≠ 0`):
/// `|√Dc + σ2√R2 - σ1√R1|`. Over `4Dc`, a `sign2` in `x = Dc R1`,
/// `y = Dc R2`. About 175-bit terms, squared twice: 700 bits.
pub fn arc_arc(a: &ArcP, b: &ArcP, s1: i128, s2: i128, dd: i128) -> Ordering {
    let dc = i(b.arc.centre.minus(a.arc.centre).len2());
    let (r1, r2) = (i(a.r2), i(b.r2));
    let first = i(4) * dc * (dc + r1 + r2) - dd2(dd) * dc;
    sign2(
        first,
        i(-8 * s1) * dc,
        i(8 * s2) * dc,
        i(-8 * s1 * s2),
        dc * r1,
        dc * r2,
    )
}

/// Whether two segments cross where their lines aren't parallel, at
/// parameters inside both, and the crossing as `(num1, den)` along the
/// first.
pub fn seg_seg_cross(p: &SegP, q: &SegP) -> Option<(i128, i128)> {
    let (u1, u2) = (p.u(), q.u());
    let mut den = u1.cross(u2);
    if den == 0 {
        return None;
    }
    let w = q.b.minus(p.b);
    let mut n1 = w.cross(u2);
    let mut n2 = w.cross(u1);
    if den < 0 {
        den = -den;
        n1 = -n1;
        n2 = -n2;
    }
    let within = |n: i128, seg: &SegP| {
        let l = seg.u().len2();
        let (t0, t1) = (Ft::of(seg.t0, l), Ft::of(seg.t1, l));
        let (n, den) = (i(n), i(den));
        sign1(n * t0.s - den * t0.n, -(den * t0.m), t0.x) != Ordering::Less
            && sign1(den * t1.n - n * t1.s, den * t1.m, t1.x) != Ordering::Less
    };
    (within(n1, p) && within(n2, q)).then_some((n1, den))
}

/// Where a segment meets an arc, if it does: the meeting nearer the
/// segment's start, in floating point.
pub fn seg_arc_meet(seg: &SegP, a: &ArcP) -> Option<(f64, f64)> {
    let u = seg.u();
    let l = u.len2();
    if l == 0 {
        return None;
    }
    let av = seg.b.minus(a.arc.centre);
    let beta = u.dot(av);
    let gamma = i(av.len2()) - i(a.r2);
    let delta = i(beta).square() - i(l) * gamma;
    if delta.sign() == Ordering::Less {
        return None;
    }
    let (t0, t1) = (Ft::of(seg.t0, l), Ft::of(seg.t1, l));
    let (lb, bt) = (i(l), i(beta));
    for sigma in [-1i128, 1] {
        let sg = i(sigma);
        // t = (-β + σ√Δ) / L between t0 and t1.
        let after = sign2(
            -(bt * t0.s) - lb * t0.n,
            sg * t0.s,
            -(lb * t0.m),
            Int::ZERO,
            delta,
            t0.x,
        ) != Ordering::Less;
        let before = sign2(
            lb * t1.n + bt * t1.s,
            -(sg * t1.s),
            lb * t1.m,
            Int::ZERO,
            delta,
            t1.x,
        ) != Ordering::Less;
        if !(after && before) {
            continue;
        }
        let dir = Dirn {
            n: (i(av.0) * lb - bt * i(u.0), i(av.1) * lb - bt * i(u.1)),
            m: (sg * i(u.0), sg * i(u.1)),
            x: delta,
        };
        if sweeps(a, &dir) {
            let d = approx_root(delta);
            let t = (-(beta as f64) + sigma as f64 * d) / l as f64;
            return Some((
                seg.b.x as f64 + t * u.0 as f64,
                seg.b.y as f64 + t * u.1 as f64,
            ));
        }
    }
    None
}

/// Where two arcs on circles with different centres meet, if they do.
pub fn arc_arc_meet(a: &ArcP, b: &ArcP) -> Option<(f64, f64)> {
    let w = b.arc.centre.minus(a.arc.centre);
    let dc = w.len2();
    if dc == 0 {
        return None;
    }
    let (r1, r2) = (i(a.r2), i(b.r2));
    let base = i(dc) + r1 - r2;
    let k = i(4) * i(dc) * r1 - base.square();
    if k.sign() == Ordering::Less {
        return None;
    }
    let other = r1 - r2 - i(dc);
    let perp = (i(-w.1), i(w.0));
    for sigma in [1i128, -1] {
        let sg = i(sigma);
        let m = (sg * perp.0, sg * perp.1);
        let d1 = Dirn {
            n: (base * i(w.0), base * i(w.1)),
            m,
            x: k,
        };
        let d2 = Dirn {
            n: (other * i(w.0), other * i(w.1)),
            m,
            x: k,
        };
        if sweeps(a, &d1) && sweeps(b, &d2) {
            let kf = approx_root(k);
            let bf = (dc as f64 + a.r2 as f64 - b.r2 as f64) / (2.0 * dc as f64);
            let hf = sigma as f64 * kf / (2.0 * dc as f64);
            let c = a.arc.centre;
            return Some((
                c.x as f64 + bf * w.0 as f64 - hf * w.1 as f64,
                c.y as f64 + bf * w.1 as f64 + hf * w.0 as f64,
            ));
        }
    }
    None
}

/// `√v` in floating point.
fn approx_root(v: Int) -> f64 {
    ryu_math::sqrt(v.to_f64())
}

/// Whether two exact points are the same point.
pub fn same(p: &Fp, q: &Fp) -> bool {
    pp(p, q, 0) == Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::geom::{Arc, COORD_LIMIT, Point};

    const L: i64 = COORD_LIMIT;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn at(x: i64, y: i64) -> Fp {
        Fp::of(Pt::At(p(x, y)))
    }

    fn arc(start: Point, end: Point, centre: Point, dir: Dir) -> ArcP {
        ArcP::new(Arc {
            start,
            end,
            centre,
            dir,
        })
    }

    #[test]
    fn points_and_segments() {
        assert_eq!(pp(&at(0, 0), &at(3, 4), 10), Ordering::Equal);
        assert_eq!(pp(&at(0, 0), &at(3, 4), 9), Ordering::Greater);
        let s = SegP::new(p(0, 0), p(10, 0));
        assert!(foot_within(&at(3, 4), &s));
        assert!(!foot_within(&at(13, 4), &s));
        assert_eq!(pseg(&at(3, 4), &s, 8), Ordering::Equal);
        assert_eq!(pseg(&at(3, 4), &s, 9), Ordering::Less);
        // Crossing at the first's middle: t = 50 / 100.
        let t = SegP::new(p(5, -5), p(5, 5));
        assert_eq!(seg_seg_cross(&s, &t), Some((50, 100)));
        assert_eq!(seg_seg_cross(&s, &SegP::new(p(5, 1), p(5, 5))), None);
    }

    #[test]
    fn arcs_and_their_directions() {
        let q = arc(p(100, 0), p(0, 100), p(0, 0), Dir::Ccw);
        assert!(sweeps(&q, &Dirn::int(V(1, 1))));
        assert!(!sweeps(&q, &Dirn::int(V(-1, 1))));
        // (√2, 1) is inside the quarter, (√2, -1) isn't.
        let root = |y: i128| Dirn {
            n: (Int::ZERO, Int::from(y)),
            m: (Int::from(1i128), Int::ZERO),
            x: Int::from(2i128),
        };
        assert!(sweeps(&q, &root(1)));
        assert!(!sweeps(&q, &root(-1)));
        // (0, 150) is 50 off the arc along the ray; the centre 100.
        assert_eq!(parc(&at(0, 150), &q, 100), Ordering::Equal);
        assert_eq!(centre(&q, 200), Ordering::Equal);
        // A line 120 up, across the perpendicular from the centre: 20.
        let half = arc(p(100, 0), p(-100, 0), p(0, 0), Dir::Ccw);
        let line = SegP::new(p(-50, 120), p(50, 120));
        assert_eq!(seg_arc(&line, &half, 1, 40), Ordering::Equal);
        assert!(seg_arc_meet(&SegP::new(p(-200, 50), p(200, 50)), &half).is_some());
        assert!(seg_arc_meet(&line, &half).is_none());
        // Circles of 100 and 50, centres 170 apart: 20 along the line.
        let a = arc(p(100, 0), p(100, 0), p(0, 0), Dir::Ccw);
        let b = arc(p(220, 0), p(220, 0), p(170, 0), Dir::Ccw);
        assert_eq!(arc_arc(&a, &b, 1, -1, 40), Ordering::Equal);
        assert!(arc_arc_meet(&a, &b).is_none());
        let c = arc(p(130, 0), p(130, 0), p(80, 0), Dir::Ccw);
        assert!(arc_arc_meet(&a, &c).is_some());
    }

    #[test]
    fn two_true_ends() {
        // Two irrational ends: the widest comparison.
        let q1 = Fp::of(Pt::end(
            p(-L, -L),
            p(-L + 3, L),
            4 * (L as i128) * (L as i128),
        ));
        let q2 = Fp::of(Pt::end(
            p(L, L),
            p(L, -L + 7),
            4 * (L as i128) * (L as i128),
        ));
        assert_eq!(pp(&q1, &q1, 0), Ordering::Equal);
        assert_eq!(pp(&q1, &q2, 0), Ordering::Greater);
        assert_eq!(pp(&q1, &q2, 1 << 44), Ordering::Less);
    }
}
