//! The frame a sweep carries its profile in: `(f₁, f₂, t)` along a stretch
//! of a path curve, from a frame law and a twist.
//!
//! `t` is the path's unit tangent. A [`FrameLaw::Binormal`] frame takes
//! `e₂` along a fixed direction made perpendicular to `t`, in closed form.
//! A [`FrameLaw::Minimal`] frame is rotation-minimizing: double-reflection
//! frames (Wang, Jüttler, Zheng and Liu 2008) at fixed parameters, enough
//! of them that the tangent turns by at most [`MAX_TURN`] between
//! neighbours, give a reference vector `r` there, with the first two
//! derivatives a rotation-minimizing frame has (`r' = -(r · t') t`); between
//! them `r` is the quintic Hermite polynomial through those, and `e₁` is `r`
//! made perpendicular to `t`. So the frame is `C²` wherever the path is
//! `C³`, a closed form of `s` that interval arithmetic encloses over a
//! stretch, and exact by this definition: within about `MAX_TURN⁶` of
//! rotation-minimizing, but never a fit. In both, `e₂ = t × e₁`, and a
//! twist turns `(e₁, e₂)` about `t` by an angle linear in `s`: `f₁ = cos φ
//! e₁ + sin φ e₂`, `f₂ = cos φ e₂ - sin φ e₁`.

use crate::Error;
use crate::geom::bernstein::Bernstein;
use crate::geom::bound2::{IVec3, trig_box};
use crate::geom::jet::{self, JVec, Jet, Num};
use crate::geom::jet1::{self, J1Vec, Jet1};
use crate::geom::{Curve, UnitVector3, Vector3};
use crate::interval::Interval;
use crate::math;
use core::f64::consts::TAU;

/// How far the tangent turns, at most, between the samples of a minimal
/// frame (radians).
const MAX_TURN: f64 = 0.125;

/// The most samples a minimal frame takes in one piece of its path.
const MAX_PER_PIECE: usize = 64;

/// The deepest a stretch of path is halved while certifying a frame.
const MAX_DEPTH: u32 = 12;

/// The most stretches certifying a frame looks at before it gives up.
const MAX_STRETCHES: usize = 1 << 14;

/// How a sweep's frame turns along its path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameLaw {
    /// Rotation-minimizing: the frame turns only as the tangent does,
    /// never about it, so a profile moves along the path without spin.
    Minimal,
    /// `e₂` is the given direction made perpendicular to the tangent, so
    /// that direction stays in the profile's plane: a sweep up a helix
    /// about it keeps its profile level. Undefined where the tangent runs
    /// along the direction.
    Binormal(UnitVector3),
}

/// A moving frame along `range` of `path`, with a twist of `twist`
/// radians from its start to its end.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Carry {
    path: Curve,
    range: [f64; 2],
    law: Law,
    twist: f64,
}

#[derive(Clone, Debug, PartialEq)]
enum Law {
    Binormal(UnitVector3),
    /// The reference vector between consecutive samples, in order.
    Minimal(Vec<Quintic>),
}

/// A vector quintic over `range`, by its Bernstein coefficients in
/// `τ = (s - range[0]) / (range[1] - range[0])`.
#[derive(Clone, Debug, PartialEq)]
struct Quintic {
    range: [f64; 2],
    c: [Vector3; 6],
}

impl Quintic {
    /// The quintic with value, first and second derivatives (by `s`)
    /// `a` at the start and `b` at the end.
    fn hermite(range: [f64; 2], a: [Vector3; 3], b: [Vector3; 3]) -> Self {
        let h = range[1] - range[0];
        let c = [
            a[0],
            a[0] + a[1] * (h / 5.0),
            a[0] + a[1] * (2.0 * h / 5.0) + a[2] * (h * h / 20.0),
            b[0] - b[1] * (2.0 * h / 5.0) + b[2] * (h * h / 20.0),
            b[0] - b[1] * (h / 5.0),
            b[0],
        ];
        Self { range, c }
    }

    /// The value and its first two derivatives by `s`, by de Casteljau's
    /// algorithm on the coefficients and their differences.
    fn at(&self, s: f64) -> [Vector3; 3] {
        let h = self.range[1] - self.range[0];
        let t = (s - self.range[0]) / h;
        let casteljau = |c: &[Vector3]| {
            let mut q = c.to_vec();
            for k in 1..q.len() {
                for i in 0..q.len() - k {
                    q[i] = q[i] * (1.0 - t) + q[i + 1] * t;
                }
            }
            q[0]
        };
        let d1: Vec<Vector3> = self.c.windows(2).map(|w| (w[1] - w[0]) * 5.0).collect();
        let d2: Vec<Vector3> = d1.windows(2).map(|w| (w[1] - w[0]) * 4.0).collect();
        [
            casteljau(&self.c),
            casteljau(&d1) / h,
            casteljau(&d2) / (h * h),
        ]
    }

    /// Enclosures of the value and its first two derivatives by `s` over
    /// `[lo, hi]` (inside the quintic's range), from the coefficients of
    /// the part over it.
    fn enclose(&self, [lo, hi]: [f64; 2]) -> [IVec3; 3] {
        let h = self.range[1] - self.range[0];
        // The part's ends, rounded outwards: the polynomial is defined
        // past its range, so a little more is still an enclosure.
        let ta = ((lo - self.range[0]) / h - 1e-14).max(0.0);
        let tb = ((hi - self.range[0]) / h + 1e-14).min(1.0);
        let ih = Interval::point(h);
        let mut out = [[Interval::point(0.0); 3]; 3];
        for k in 0..3 {
            let poly = Bernstein::from_f64(&self.c.map(|v| v.to_array()[k]));
            let d1 = poly.deriv();
            let d2 = d1.deriv();
            let per = |x: Interval, by: Interval| x.checked_div(by).expect("a positive span");
            let [value, first, second] = &mut out;
            value[k] = poly.part(ta, tb).range();
            first[k] = per(d1.part(ta, tb).range(), ih);
            second[k] = per(d2.part(ta, tb).range(), ih * ih);
        }
        out
    }
}

impl Quintic {
    /// [`Quintic::at`] and the third derivative by `s`.
    fn at3(&self, s: f64) -> [Vector3; 4] {
        let h = self.range[1] - self.range[0];
        let t = (s - self.range[0]) / h;
        let casteljau = |c: &[Vector3]| {
            let mut q = c.to_vec();
            for k in 1..q.len() {
                for i in 0..q.len() - k {
                    q[i] = q[i] * (1.0 - t) + q[i + 1] * t;
                }
            }
            q[0]
        };
        let d1: Vec<Vector3> = self.c.windows(2).map(|w| (w[1] - w[0]) * 5.0).collect();
        let d2: Vec<Vector3> = d1.windows(2).map(|w| (w[1] - w[0]) * 4.0).collect();
        let d3: Vec<Vector3> = d2.windows(2).map(|w| (w[1] - w[0]) * 3.0).collect();
        [
            casteljau(&self.c),
            casteljau(&d1) / h,
            casteljau(&d2) / (h * h),
            casteljau(&d3) / (h * h * h),
        ]
    }

    /// [`Quintic::enclose`] and the third derivative's enclosure.
    // `k` is a component of every row of `out`, not an index into one.
    #[allow(clippy::needless_range_loop)]
    fn enclose3(&self, [lo, hi]: [f64; 2]) -> [IVec3; 4] {
        let h = self.range[1] - self.range[0];
        let ta = ((lo - self.range[0]) / h - 1e-14).max(0.0);
        let tb = ((hi - self.range[0]) / h + 1e-14).min(1.0);
        let ih = Interval::point(h);
        let mut out = [[Interval::point(0.0); 3]; 4];
        for k in 0..3 {
            let poly = Bernstein::from_f64(&self.c.map(|v| v.to_array()[k]));
            let d1 = poly.deriv();
            let d2 = d1.deriv();
            let d3 = d2.deriv();
            let per = |x: Interval, by: Interval| x.checked_div(by).expect("a positive span");
            out[0][k] = poly.part(ta, tb).range();
            out[1][k] = per(d1.part(ta, tb).range(), ih);
            out[2][k] = per(d2.part(ta, tb).range(), ih * ih);
            out[3][k] = per(d3.part(ta, tb).range(), ih * ih * ih);
        }
        out
    }
}

/// Constant jets from a vector.
fn constant<T: Num>(v: Vector3) -> JVec<T> {
    v.to_array().map(|x| Jet::constant(T::of(x)))
}

/// Jets in `s` (the second variable) from a value and its first two
/// derivatives.
fn of_s<T: Num>(d: [[T; 3]; 3]) -> JVec<T> {
    core::array::from_fn(|k| Jet::of_t(d[0][k], d[1][k], d[2][k]))
}

/// The unit tangent as jets in `s`, from the path's first three
/// derivatives; `None` where the path may stop.
fn tangent<T: Num>(d: &[[T; 3]; 4]) -> Option<JVec<T>> {
    jet::unit(of_s([d[1], d[2], d[3]]))
}

/// The breaks of a curve strictly inside `range`, with its ends: knots of a
/// NURBS, eighths of a turn of a circle or an ellipse, none on a line.
pub(super) fn breaks(path: &Curve, [s0, s1]: [f64; 2]) -> Vec<f64> {
    let mut out = vec![s0];
    let mut add = |x: f64| {
        if x > s0 && x < s1 {
            out.push(x);
        }
    };
    match path {
        Curve::Line(_) => {}
        Curve::Circle(_) | Curve::Ellipse(_) => {
            let first = (s0 / (TAU / 8.0)).floor() as i64;
            let last = (s1 / (TAU / 8.0)).ceil() as i64;
            for k in first..=last {
                add(TAU / 8.0 * k as f64);
            }
        }
        Curve::Nurbs(n) => {
            let period = n.period();
            for p in n.pieces() {
                for shift in [-1.0, 0.0, 1.0] {
                    add(p.range[0] + period.map_or(0.0, |q| q * shift));
                }
            }
        }
    }
    out.push(s1);
    out.sort_by(f64::total_cmp);
    out.dedup();
    out
}

/// The angle between two vectors.
fn angle(a: Vector3, b: Vector3) -> f64 {
    math::atan2(a.cross(b).length(), a.dot(b))
}

/// How many samples a minimal frame takes over one piece of its path: the
/// tangent's turn over sixteen steps, in steps of [`MAX_TURN`], at least
/// one and at most [`MAX_PER_PIECE`].
fn samples(path: &Curve, [a, b]: [f64; 2]) -> usize {
    let t = |s: f64| path.jet3(s)[1];
    let mut turn = 0.0;
    for i in 0..16 {
        let (p, q) = (
            a + (b - a) * f64::from(i) / 16.0,
            a + (b - a) * f64::from(i + 1) / 16.0,
        );
        turn += angle(t(p), t(q));
    }
    let n = (turn / MAX_TURN).ceil();
    if n.is_finite() {
        (n as usize).clamp(1, MAX_PER_PIECE)
    } else {
        1
    }
}

/// `v` made perpendicular to the unit vector `t` and unit.
fn across(v: Vector3, t: Vector3) -> Option<Vector3> {
    (v - t * v.dot(t)).normalize().map(UnitVector3::get)
}

/// The reference vector of a minimal frame from `seed` (made
/// perpendicular to the tangent at the start), as quintics between its
/// samples.
fn minimal(path: &Curve, range: [f64; 2], seed: Vector3) -> Result<Vec<Quintic>, Error> {
    let cuts = breaks(path, range);
    // Each sample, with the piece of path the span from it lies in.
    let mut params: Vec<(f64, [f64; 2])> = Vec::new();
    for w in cuts.windows(2) {
        let n = samples(path, [w[0], w[1]]);
        for i in 0..n {
            params.push((w[0] + (w[1] - w[0]) * i as f64 / n as f64, [w[0], w[1]]));
        }
    }
    // At a sample: the point, the tangent and its two derivatives, from
    // the piece given (so across a knot each side keeps its own).
    let local = |s: f64, piece: [f64; 2]| -> Result<(Vector3, [Vector3; 3]), Error> {
        let d = piece_jet3(path, s, piece).map(Vector3::to_array);
        let t = tangent(&d).ok_or(Error::Degenerate)?;
        let part = |f: &dyn Fn(&Jet<f64>) -> f64| Vector3::new(f(&t[0]), f(&t[1]), f(&t[2]));
        Ok((
            Vector3::new(d[0][0], d[0][1], d[0][2]),
            [part(&|j| j.v), part(&|j| j.d[1]), part(&|j| j.h[2])],
        ))
    };
    // A rotation-minimizing frame's reference vector and its derivatives.
    let jets = |r: Vector3, t: [Vector3; 3]| {
        let (rt1, rt2) = (r.dot(t[1]), r.dot(t[2]));
        [r, t[0] * -rt1, t[0] * -rt2 - t[1] * rt1]
    };
    let (s0, piece0) = params[0];
    let (mut x, mut t) = local(s0, piece0)?;
    let mut r = across(seed, t[0]).ok_or(Error::Degenerate)?;
    let mut out = Vec::with_capacity(params.len());
    let ends: Vec<f64> = params[1..]
        .iter()
        .map(|p| p.0)
        .chain(core::iter::once(range[1]))
        .collect();
    for (&(a, piece), &b) in params.iter().zip(&ends) {
        // The start's jets on this span's piece, then the end's.
        let start = jets(r, local(a, piece)?.1);
        let (x1, t1) = local(b, piece)?;
        // Reflect in the plane bisecting the chord, then in the one
        // taking the reflected tangent onto the new one.
        let v1 = x1 - x;
        let c1 = v1.dot(v1);
        let (rl, tl) = if c1 > 0.0 {
            (
                r - v1 * (2.0 * v1.dot(r) / c1),
                t[0] - v1 * (2.0 * v1.dot(t[0]) / c1),
            )
        } else {
            (r, t[0])
        };
        let v2 = t1[0] - tl;
        let c2 = v2.dot(v2);
        let next = if c2 > 0.0 {
            rl - v2 * (2.0 * v2.dot(rl) / c2)
        } else {
            rl
        };
        r = across(next, t1[0]).ok_or(Error::Degenerate)?;
        (x, t) = (x1, t1);
        out.push(Quintic::hermite([a, b], start, jets(r, t)));
    }
    Ok(out)
}

/// The path's point and first three derivatives at `s` on its piece
/// `piece`: a NURBS's from that Bézier piece alone (its derivatives at a
/// knot from that side), any other curve's as it is.
fn piece_jet3(path: &Curve, s: f64, piece: [f64; 2]) -> [Vector3; 4] {
    let Curve::Nurbs(n) = path else {
        return path.jet3(s);
    };
    let mid = (piece[0] + piece[1]) / 2.0;
    let found = n.pieces().iter().find(|p| {
        let [a, b] = p.range;
        a <= mid && mid <= b
    });
    let Some(p) = found else {
        return path.jet3(s);
    };
    let [a, b] = p.range;
    let h = b - a;
    let x = (s - a) / h;
    let hom: Vec<[f64; 4]> = p
        .pts
        .iter()
        .map(|q| {
            let v = q.p * q.w;
            [v.x, v.y, v.z, q.w]
        })
        .collect();
    let at = |c: &[[f64; 4]]| -> [f64; 4] {
        if c.is_empty() {
            return [0.0; 4];
        }
        let mut q = c.to_vec();
        for k in 1..q.len() {
            for i in 0..q.len() - k {
                q[i] = core::array::from_fn(|j| q[i][j] * (1.0 - x) + q[i + 1][j] * x);
            }
        }
        q[0]
    };
    let diff = |c: &[[f64; 4]]| -> Vec<[f64; 4]> {
        let m = c.len().saturating_sub(1) as f64;
        c.windows(2)
            .map(|w| core::array::from_fn(|j| (w[1][j] - w[0][j]) * m / h))
            .collect()
    };
    let h1 = diff(&hom);
    let h2 = diff(&h1);
    let h3 = diff(&h2);
    let [a0, a1, a2, a3] = [at(&hom), at(&h1), at(&h2), at(&h3)];
    let v = |c: [f64; 4]| Vector3::new(c[0], c[1], c[2]);
    let (w0, w1, w2, w3) = (a0[3], a1[3], a2[3], a3[3]);
    let c0 = v(a0) / w0;
    let c1 = (v(a1) - c0 * w1) / w0;
    let c2 = (v(a2) - c1 * (2.0 * w1) - c0 * w2) / w0;
    let c3 = (v(a3) - c2 * (3.0 * w1) - c1 * (3.0 * w2) - c0 * w3) / w0;
    [c0, c1, c2, c3]
}

/// The componentwise hull of two sets of jets.
fn hull(a: JVec<Interval>, b: JVec<Interval>) -> JVec<Interval> {
    core::array::from_fn(|k| Jet {
        v: a[k].v.hull(b[k].v),
        d: [a[k].d[0].hull(b[k].d[0]), a[k].d[1].hull(b[k].d[1])],
        h: [
            a[k].h[0].hull(b[k].h[0]),
            a[k].h[1].hull(b[k].h[1]),
            a[k].h[2].hull(b[k].h[2]),
        ],
    })
}

/// How far `to` is turned about their common third axis from `from`: the
/// angle of `to`'s first axis in `from`'s first two. A closed path's
/// frame closes when the frame at its end is turned by a whole number of
/// turns from the one at its start.
pub(crate) fn turned(from: [Vector3; 3], to: [Vector3; 3]) -> f64 {
    math::atan2(to[0].dot(from[1]), to[0].dot(from[0]))
}

/// The path's point and the frame's three axes, as jets in `s`.
pub(crate) type Moving<T> = (JVec<T>, [JVec<T>; 3]);

/// The path's point and the frame's three axes, as third-order jets in
/// `s`.
pub(crate) type Moving3<T> = (J1Vec<T>, [J1Vec<T>; 3]);

/// The componentwise hull of two third-order jet vectors.
fn hull1(a: J1Vec<Interval>, b: J1Vec<Interval>) -> J1Vec<Interval> {
    core::array::from_fn(|k| Jet1(core::array::from_fn(|i| a[k].0[i].hull(b[k].0[i]))))
}

impl Carry {
    /// The frame along `range` of `path` by `law`, with `twist` radians
    /// of twist over the range. A minimal frame starts from `seed` made
    /// perpendicular to the tangent; a binormal frame ignores it.
    ///
    /// Fails with `NonFinite`, or `Degenerate`: for a range that is empty,
    /// outside a clamped path's domain or longer than a periodic one's
    /// period, a seed along the tangent, or a frame that can't be
    /// certified defined over the whole range (the path's speed may
    /// vanish, or its tangent may run along a binormal law's direction).
    /// That is checked over the path's pieces (and a minimal frame's
    /// stretches between samples), halved up to 12 times, at most 16,384
    /// stretches in all.
    pub(crate) fn new(
        path: Curve,
        range: [f64; 2],
        seed: Vector3,
        law: FrameLaw,
        twist: f64,
    ) -> Result<Self, Error> {
        let carry = Self::build(path, range, seed, law, twist)?;
        carry.certify()?;
        Ok(carry)
    }

    /// The frame as [`Carry::new`] makes it, without certifying it: for a
    /// frame moved or reflected with its path, defined wherever the
    /// original was.
    pub(crate) fn build(
        path: Curve,
        range: [f64; 2],
        seed: Vector3,
        law: FrameLaw,
        twist: f64,
    ) -> Result<Self, Error> {
        let [s0, s1] = range;
        if !(s0.is_finite() && s1.is_finite() && twist.is_finite()) {
            return Err(Error::NonFinite);
        }
        let ok = match path.period() {
            Some(p) => s1 - s0 <= p,
            None => {
                let [d0, d1] = path.domain();
                d0 <= s0 && s1 <= d1
            }
        };
        if !(s0 < s1 && ok) {
            return Err(Error::Degenerate);
        }
        let law = match law {
            FrameLaw::Binormal(b) => Law::Binormal(b),
            FrameLaw::Minimal => Law::Minimal(minimal(&path, range, seed)?),
        };
        Ok(Self {
            path,
            range,
            law,
            twist,
        })
    }

    pub(crate) fn path(&self) -> &Curve {
        &self.path
    }

    pub(crate) fn range(&self) -> [f64; 2] {
        self.range
    }

    pub(crate) fn twist(&self) -> f64 {
        self.twist
    }

    /// Where a minimal frame's samples are, between its ends.
    #[cfg(test)]
    fn samples(&self) -> Vec<f64> {
        match &self.law {
            Law::Binormal(_) => Vec::new(),
            Law::Minimal(q) => q[1..].iter().map(|q| q.range[0]).collect(),
        }
    }

    /// Where the frame's closed form changes inside the range: a minimal
    /// frame's samples and the path's breaks.
    pub(crate) fn cuts(&self) -> Vec<f64> {
        let mut out = breaks(&self.path, self.range);
        if let Law::Minimal(q) = &self.law {
            out.extend(q.iter().map(|q| q.range[0]));
        }
        out.sort_by(f64::total_cmp);
        out.dedup();
        out
    }

    /// The stretches searches start from: the path's pieces in the range.
    pub(crate) fn stretches(&self) -> Vec<[f64; 2]> {
        breaks(&self.path, self.range)
            .windows(2)
            .map(|w| [w[0], w[1]])
            .collect()
    }

    fn certify(&self) -> Result<(), Error> {
        let mut stack: Vec<([f64; 2], u32)> =
            self.stretches().into_iter().map(|r| (r, 0)).collect();
        let mut seen = 0usize;
        while let Some((r, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_STRETCHES {
                return Err(Error::Degenerate);
            }
            if self.jet_box(r).is_some() {
                continue;
            }
            if depth == MAX_DEPTH {
                return Err(Error::Degenerate);
            }
            let m = (r[0] + r[1]) / 2.0;
            stack.push(([r[0], m], depth + 1));
            stack.push(([m, r[1]], depth + 1));
        }
        Ok(())
    }

    /// Whether the range is a whole period of a periodic path. Its frame
    /// is periodic only when it closes ([`turned`]): the caller twists it
    /// so.
    pub(crate) fn closed(&self) -> bool {
        self.path.period() == Some(self.range[1] - self.range[0])
    }

    /// `s` taken into the range round a closed path; the range's own
    /// parameters (its end too) are kept.
    fn wrap(&self, s: f64) -> f64 {
        let [s0, s1] = self.range;
        if self.closed() && !(s0..=s1).contains(&s) {
            s0 + (s - s0).rem_euclid(s1 - s0)
        } else {
            s
        }
    }

    /// The twist angle at `s`, and its rate.
    fn phi(&self, s: f64) -> (f64, f64) {
        let [s0, s1] = self.range;
        let rate = self.twist / (s1 - s0);
        (rate * (s - s0), rate)
    }

    /// The axes from the path's derivatives, the reference vector's jets
    /// (a minimal frame's) and `(cos φ, sin φ, φ')`.
    fn axes<T: Num>(
        &self,
        d: &[[T; 3]; 4],
        r: Option<[[T; 3]; 3]>,
        (c, s, rate): (T, T, f64),
    ) -> Option<[JVec<T>; 3]> {
        let t = tangent(d)?;
        // Both from the path's own derivative, never its unit tangent:
        // `b × c'` and `(c' × r) × c'` are polynomial in it, so interval
        // enclosures stay tight.
        let cp = of_s([d[1], d[2], d[3]]);
        let e1 = match (&self.law, r) {
            (Law::Binormal(b), _) => jet::unit(jet::cross(constant(b.get()), cp))?,
            (Law::Minimal(_), Some(r)) => {
                let r = of_s(r);
                jet::unit(jet::cross(jet::cross(cp, r), cp))?
            }
            (Law::Minimal(_), None) => return None,
        };
        let e2 = jet::cross(t, e1);
        let k = T::of(rate);
        let cj = Jet::of_t(c, -s * k, -c * k * k);
        let sj = Jet::of_t(s, c * k, -s * k * k);
        let f1 = jet::add(jet::times(e1, cj), jet::times(e2, sj));
        let f2 = jet::sub(jet::times(e2, cj), jet::times(e1, sj));
        Some([f1, f2, t])
    }

    /// [`Carry::axes`] to third order, from the path's first four
    /// derivatives and the reference vector's first three: what a
    /// sweep's offset needs.
    fn axes3<T: Num>(
        &self,
        d: &[[T; 3]; 5],
        r: Option<[[T; 3]; 4]>,
        (c, s, rate): (T, T, f64),
    ) -> Option<[J1Vec<T>; 3]> {
        let cp = jet1::of([d[1], d[2], d[3], d[4]]);
        let t = jet1::unit(cp)?;
        let e1 = match (&self.law, r) {
            (Law::Binormal(b), _) => {
                let b = jet1::constant(b.get().to_array().map(T::of));
                jet1::unit(jet1::cross(b, cp))?
            }
            (Law::Minimal(_), Some(r)) => {
                let r = jet1::of(r);
                jet1::unit(jet1::cross(jet1::cross(cp, r), cp))?
            }
            (Law::Minimal(_), None) => return None,
        };
        let e2 = jet1::cross(t, e1);
        let k = T::of(rate);
        let cj = Jet1([c, -s * k, -c * k * k, s * k * k * k]);
        let sj = Jet1([s, c * k, -s * k * k, -c * k * k * k]);
        let f1 = jet1::add(jet1::times(e1, cj), jet1::times(e2, sj));
        let f2 = jet1::sub(jet1::times(e2, cj), jet1::times(e1, sj));
        Some([f1, f2, t])
    }

    /// The path's point and the frame at `s` to third order, in floating
    /// point; `None` where the frame is undefined there.
    pub(crate) fn jets3_at(&self, s: f64) -> Option<Moving3<f64>> {
        let s = self.wrap(s);
        let d = self.path.jet4(s).map(Vector3::to_array);
        let r = self.quintic(s).map(|q| q.at3(s).map(Vector3::to_array));
        let (phi, rate) = self.phi(s);
        let (sin, cos) = math::sin_cos(phi);
        let axes = self.axes3(&d, r, (cos, sin, rate))?;
        Some((jet1::of([d[0], d[1], d[2], d[3]]), axes))
    }

    /// Enclosures of [`Carry::jets3_at`] over every `s` in `[a, b]`;
    /// `None` where the frame can't be certified defined over it.
    pub(crate) fn jet3_box(&self, range: [f64; 2]) -> Option<Moving3<Interval>> {
        let mut out: Option<Moving3<Interval>> = None;
        for [lo, hi] in self.spans(range) {
            let d = self.path.jet4_box([lo, hi]);
            let r = match &self.law {
                Law::Binormal(_) => None,
                Law::Minimal(_) => Some(self.quintic((lo + hi) / 2.0)?.enclose3([lo, hi])),
            };
            let (p0, rate) = self.phi(lo);
            let (p1, _) = self.phi(hi);
            let (cos, sin) = trig_box([p0.min(p1), p0.max(p1)]);
            let axes = self.axes3(&d, r, (cos, sin, rate))?;
            let made = (jet1::of([d[0], d[1], d[2], d[3]]), axes);
            out = Some(match out {
                None => made,
                Some((c, f)) => (
                    hull1(c, made.0),
                    [
                        hull1(f[0], made.1[0]),
                        hull1(f[1], made.1[1]),
                        hull1(f[2], made.1[2]),
                    ],
                ),
            });
        }
        out
    }

    /// The stretches a box of `s` covers, each within one of a minimal
    /// frame's quintics: [`Carry::pieces`] cut at its samples.
    fn spans(&self, range: [f64; 2]) -> Vec<[f64; 2]> {
        let mut out = Vec::new();
        for [a, b] in self.pieces(range) {
            match &self.law {
                Law::Binormal(_) => out.push([a, b]),
                Law::Minimal(q) => {
                    // The first and last quintics reach on past the range.
                    let n = q.len();
                    out.extend(
                        q.iter()
                            .enumerate()
                            .map(|(k, q)| {
                                let lo = if k == 0 {
                                    f64::NEG_INFINITY
                                } else {
                                    q.range[0]
                                };
                                let hi = if k + 1 == n {
                                    f64::INFINITY
                                } else {
                                    q.range[1]
                                };
                                [lo, hi]
                            })
                            .filter(|r| r[1] >= a && r[0] <= b)
                            .map(|r| [a.max(r[0]), b.min(r[1])]),
                    );
                }
            }
        }
        out
    }

    /// The frame moved by a rigid transform, its path and a minimal
    /// frame's reference vectors moved with it rather than sampled again
    /// ([`Carry::build`]): the same frame up to rounding, cheaply, for
    /// enclosures taken in a cell's own axes. Not certified again.
    pub(crate) fn moved(&self, r: &crate::geom::Rigid) -> Self {
        let law = match &self.law {
            Law::Binormal(b) => Law::Binormal(r.apply_unit(*b)),
            Law::Minimal(q) => Law::Minimal(
                q.iter()
                    .map(|q| Quintic {
                        range: q.range,
                        c: q.c.map(|v| r.apply_vector(v)),
                    })
                    .collect(),
            ),
        };
        Self {
            path: self.path.transformed(r),
            range: self.range,
            law,
            twist: self.twist,
        }
    }

    /// The reference vector's quintic over the span holding `s` (the
    /// first or last past the range's ends).
    fn quintic(&self, s: f64) -> Option<&Quintic> {
        let Law::Minimal(q) = &self.law else {
            return None;
        };
        let k = q.partition_point(|q| q.range[1] < s).min(q.len() - 1);
        q.get(k)
    }

    /// The path's point and the frame at `s`, as jets in `s` in floating
    /// point; `None` where the frame is undefined there (only outside the
    /// range, which was certified).
    pub(crate) fn jets_at(&self, s: f64) -> Option<Moving<f64>> {
        let s = self.wrap(s);
        let d = self.path.jet3(s).map(Vector3::to_array);
        let r = self.quintic(s).map(|q| q.at(s).map(Vector3::to_array));
        let (phi, rate) = self.phi(s);
        let (sin, cos) = math::sin_cos(phi);
        let axes = self.axes(&d, r, (cos, sin, rate))?;
        Some((of_s([d[0], d[1], d[2]]), axes))
    }

    /// The axes' values at `s`.
    pub(crate) fn axes_at(&self, s: f64) -> Option<[Vector3; 3]> {
        let (_, a) = self.jets_at(s)?;
        Some(a.map(|j| Vector3::new(j[0].v, j[1].v, j[2].v)))
    }

    /// The stretches of the range a box of `s` covers: clamped to it, or
    /// wrapped round a closed path.
    fn pieces(&self, [a, b]: [f64; 2]) -> Vec<[f64; 2]> {
        let [s0, s1] = self.range;
        if !self.closed() {
            return vec![[a.clamp(s0, s1), b.clamp(s0, s1)]];
        }
        let p = s1 - s0;
        if b - a >= p {
            return vec![[s0, s1]];
        }
        let a0 = self.wrap(a);
        let b0 = a0 + (b - a);
        if b0 <= s1 {
            vec![[a0, b0]]
        } else {
            vec![[a0, s1], [s0, b0 - p]]
        }
    }

    /// Enclosures of the path's point and the frame's jets over every `s`
    /// in `[a, b]`; `None` where the frame can't be certified defined
    /// over it.
    pub(crate) fn jet_box(&self, range: [f64; 2]) -> Option<Moving<Interval>> {
        let mut out: Option<Moving<Interval>> = None;
        for [a, b] in self.pieces(range) {
            let spans: Vec<[f64; 2]> = match &self.law {
                Law::Binormal(_) => vec![[a, b]],
                Law::Minimal(q) => {
                    // The first and last quintics reach on past the range.
                    let n = q.len();
                    q.iter()
                        .enumerate()
                        .map(|(k, q)| {
                            let lo = if k == 0 {
                                f64::NEG_INFINITY
                            } else {
                                q.range[0]
                            };
                            let hi = if k + 1 == n {
                                f64::INFINITY
                            } else {
                                q.range[1]
                            };
                            [lo, hi]
                        })
                        .filter(|r| r[1] >= a && r[0] <= b)
                        .map(|r| [a.max(r[0]), b.min(r[1])])
                        .collect()
                }
            };
            for [lo, hi] in spans {
                let d = self.path.jet_box([lo, hi]);
                let r = match &self.law {
                    Law::Binormal(_) => None,
                    Law::Minimal(_) => Some(self.quintic((lo + hi) / 2.0)?.enclose([lo, hi])),
                };
                let (p0, rate) = self.phi(lo);
                let (p1, _) = self.phi(hi);
                let (cos, sin) = trig_box([p0.min(p1), p0.max(p1)]);
                let axes = self.axes(&d, r, (cos, sin, rate))?;
                let made = (of_s([d[0], d[1], d[2]]), axes);
                out = Some(match out {
                    None => made,
                    Some((c, f)) => (
                        hull(c, made.0),
                        [
                            hull(f[0], made.1[0]),
                            hull(f[1], made.1[1]),
                            hull(f[2], made.1[2]),
                        ],
                    ),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
