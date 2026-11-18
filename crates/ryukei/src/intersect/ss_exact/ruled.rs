//! A cylinder against a cone or another cylinder on any axes (plan 22.D,
//! design 724, question 1054).
//!
//! The cylinder's rulings `P(θ, t) = o + r (cos θ x + sin θ y) + t z`, put
//! into the other surface's implicit form, give `A t² + 2B(θ) t + C(θ)`
//! along each ruling: `A` a constant, `B` of degree 1 in `cos θ` and
//! `sin θ`, `C` of degree 2. Where `Δ(θ) = B² − AC` is positive the ruling
//! crosses the other surface twice, at `t± = (−B ± √Δ)/A`. Where it is
//! negative it crosses nowhere, and the form stays at least `|Δ|/|A|` from
//! zero all along it.
//!
//! - **The structure is certified.** In each of two charts, `θ = cπ +
//!   2 atan u` with `u = 2.5 (2s − 1)`, `(1 + u²)² Δ` is a quartic in `s`,
//!   formed in interval arithmetic from the frames as stored. Each chart
//!   is read over its half turn and a little past its seams, where `Δ`'s
//!   sign is certified (the seams turn by an eighth if not), so no root
//!   is at a chart's end or at infinity. [`Bernstein::roots`] isolates
//!   `Δ`'s roots, and between them `Δ` has one certified sign.
//! - **Arcs.** Each maximal arc of `θ` where `Δ > 0` gives one closed
//!   loop, its two branches joined at the arc's ends. `Δ > 0` all round
//!   gives two curves round the cylinder.
//! - **A touch.** A root that can't be isolated as simple, with `Δ < 0`
//!   either side, is a touch at its middle, if that point lies within
//!   [`LINEAR_RESOLUTION`] of the other surface (the module's tangency
//!   rule). One with `Δ > 0` on a side (a pinch) gives `None`.
//! - **Near misses.** Where `Δ < 0`, `N = Δ + |A| M` (`M` bounds how much
//!   the implicit form changes within the resolution of a point of the
//!   region) must have exactly one simple root beside each root of `Δ`
//!   bounding the stretch, and none in a stretch that is a whole turn.
//!   Otherwise `None`: a near touch is never dropped.
//! - **The cone's other nappe.** The apex must be certified off the
//!   cylinder, so each curve lies on one nappe. A curve certified on the
//!   other nappe is left out.
//! - **`A = 0`** (the cylinder's axis along a ruling of the cone, or
//!   parallel cylinders) is not this form's: `None`.
//! - **The curves** are cubic Hermite spans through exact points and
//!   tangents, halved until each span's middle is within a quarter
//!   resolution. A loop is parametrized by the angle `φ` about its arc's
//!   middle (`θ = θm + h cos φ`), so its turning points at the arc's
//!   ends are ordinary points. The fit is not trusted: `ss::with_pcurves`
//!   grows each branch's tolerance to its certified pcurve bounds.
//!
//! Not one of [`super::exact`]'s pairs: `ss::meet` tries it only after a
//! trace's fit fails (FTC 07 AP242's pin traces in chains no fit
//! certifies; census VII), never inside a re-trim rung's first build
//! (the tube contact's gate), so every pair that traces and fits keeps
//! its result. Design 724 placed it after a failed trace, but no input
//! fails the trace (the suites and 3,000 seeded grazes), so a failed
//! trace refuses as before. A unit a chart, a root and a span.

use super::Out;
use crate::geom::bernstein::{Bernstein, Root};
use crate::geom::{Box3, Curve, Frame, KnotVector, NurbsCurve, Point3, Surface, Vector3};
use crate::intersect::Failure;
use crate::intersect::ss::Raw;
use crate::interval::Interval;
use crate::math;
use crate::predicates::Sign;
use crate::tol::LINEAR_RESOLUTION;
use crate::work::Meter;
use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};
use std::sync::Arc;

/// A closed form's branches and touch points.
type Hit = (Vec<Raw>, Vec<Point3>);

#[cfg(test)]
thread_local! {
    /// How many times [`ruled`] was reached with a cylinder pair on this
    /// thread (tests only: the gate and the order are proved with it).
    pub(crate) static TRIED: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// The other surface: `x² + y² − (k + τ z)²` in its frame's local
/// coordinates, `τ = sin / cos` its half-angle's tangent (0 for a
/// cylinder).
#[derive(Clone, Copy, Debug)]
struct Quadric {
    frame: Frame,
    k: f64,
    sin: f64,
    cos: f64,
}

impl Quadric {
    fn of(s: &Surface) -> Option<Self> {
        let frame = *s.frame()?;
        match s {
            Surface::Cylinder(c) => Some(Self {
                frame,
                k: c.radius(),
                sin: 0.0,
                cos: 1.0,
            }),
            Surface::Cone(c) => {
                let (sin, cos) = math::sin_cos(c.half_angle());
                Some(Self {
                    frame,
                    k: c.radius(),
                    sin,
                    cos,
                })
            }
            _ => None,
        }
    }

    fn tan(&self) -> f64 {
        self.sin / self.cos
    }
}

/// The cylinder's rulings in the quadric's local coordinates: a ruling's
/// point is `U(θ) + t W`, `U(θ) = u0 + cos θ uc + sin θ us`. With `D =
/// diag(1, 1, −τ²)` and `e = (0, 0, −kτ)`, the quadric along a ruling is
/// `A t² + 2B t + C`: `A = W·DW`, `B = W·DU + e·W`, `C = U·DU + 2e·U − k²`.
#[derive(Clone, Copy, Debug)]
struct Rulings {
    cyl: Frame,
    r: f64,
    q: Quadric,
    u0: [f64; 3],
    uc: [f64; 3],
    us: [f64; 3],
    w: [f64; 3],
}

impl Rulings {
    fn new(cyl: Frame, r: f64, q: Quadric) -> Self {
        let axes = [q.frame.x().get(), q.frame.y().get(), q.frame.z().get()];
        let local = |v: Vector3| axes.map(|a| v.dot(a));
        Self {
            cyl,
            r,
            q,
            u0: local(cyl.origin() - q.frame.origin()),
            uc: local(cyl.x().get() * r),
            us: local(cyl.y().get() * r),
            w: local(cyl.z().get()),
        }
    }

    /// `a·Db`.
    fn dd(&self, a: [f64; 3], b: [f64; 3]) -> f64 {
        let t = self.q.tan();
        a[0] * b[0] + a[1] * b[1] - t * t * a[2] * b[2]
    }

    fn a(&self) -> f64 {
        self.dd(self.w, self.w)
    }

    /// `U(θ)` and its derivative.
    fn u(&self, th: f64) -> ([f64; 3], [f64; 3]) {
        let (s, c) = math::sin_cos(th);
        (
            core::array::from_fn(|i| self.u0[i] + c * self.uc[i] + s * self.us[i]),
            core::array::from_fn(|i| -s * self.uc[i] + c * self.us[i]),
        )
    }

    /// `B(θ)` and its derivative.
    fn b(&self, th: f64) -> (f64, f64) {
        let (u, du) = self.u(th);
        let kt = self.q.k * self.q.tan();
        (self.dd(self.w, u) - kt * self.w[2], self.dd(self.w, du))
    }

    /// `C(θ)` and its derivative.
    fn c(&self, th: f64) -> (f64, f64) {
        let (u, du) = self.u(th);
        let kt = self.q.k * self.q.tan();
        (
            self.dd(u, u) - 2.0 * kt * u[2] - self.q.k * self.q.k,
            2.0 * self.dd(u, du) - 2.0 * kt * du[2],
        )
    }

    /// `Δ(θ) = B² − AC` and its derivative.
    fn delta(&self, th: f64) -> (f64, f64) {
        let (b, db) = self.b(th);
        let (c, dc) = self.c(th);
        let a = self.a();
        (b * b - a * c, 2.0 * b * db - a * dc)
    }

    /// The cylinder's point at `(θ, t)`, from its own frame.
    fn point(&self, th: f64, t: f64) -> Point3 {
        let (s, c) = math::sin_cos(th);
        self.cyl.origin()
            + self.cyl.x().get() * (self.r * c)
            + self.cyl.y().get() * (self.r * s)
            + self.cyl.z().get() * t
    }

    /// `∂P/∂θ`.
    fn along(&self, th: f64) -> Vector3 {
        let (s, c) = math::sin_cos(th);
        self.cyl.x().get() * (-self.r * s) + self.cyl.y().get() * (self.r * c)
    }
}

/// The charts' reach: `u = U (2s − 1)` over `s ∈ [0, 1]`, so each chart
/// covers `±2 atan 2.5` (about ±136°) about its middle. `U² = 6.25` is
/// exact.
const U: f64 = 2.5;

fn iv(x: f64) -> Interval {
    Interval::point(x)
}

fn ivec(v: Vector3) -> [Interval; 3] {
    [iv(v.x), iv(v.y), iv(v.z)]
}

fn idot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A certified strict sign.
fn strict(x: Interval) -> Option<Sign> {
    x.sign().filter(|&s| s != Sign::Zero)
}

/// The angle at `s` in chart `c`.
fn theta_of(c: usize, s: f64) -> f64 {
    c as f64 * PI + 2.0 * math::atan2(U * (2.0 * s - 1.0), 1.0)
}

/// The `s` of angle `local` (taken from the chart's middle) in a chart.
fn s_of(local: f64) -> f64 {
    (math::tan(local / 2.0) / U + 1.0) / 2.0
}

/// `1 + u²`, `1 − u²` and `2u` in Bernstein form of degree 2.
fn basis() -> [Bernstein; 3] {
    [
        Bernstein::from_f64(&[7.25, -5.25, 7.25]),
        Bernstein::from_f64(&[-5.25, 7.25, -5.25]),
        Bernstein::from_f64(&[-5.0, 0.0, 5.0]),
    ]
}

/// The miss margin `M`. Within [`LINEAR_RESOLUTION`] of any point of
/// `region`, the quadric's form changes by less than `M`. Its gradient
/// `2(DL + e)` is at most `G = 2(max(1, τ²) L + kτ)` long, `L` being the
/// farthest the region reaches from the quadric's origin (plus the
/// resolution twice). Its second-order part `δ·Dδ` is at most
/// `max(1, τ²) δ²`. Rounded up by a millionth.
fn margin(q: &Quadric, region: &Box3) -> f64 {
    let (lo, hi) = (region.min(), region.max());
    let o = q.frame.origin();
    let far = (0..8)
        .map(|i| {
            let x = if i & 1 == 0 { lo.x } else { hi.x };
            let y = if i & 2 == 0 { lo.y } else { hi.y };
            let z = if i & 4 == 0 { lo.z } else { hi.z };
            Point3::new(x, y, z).distance(o)
        })
        .fold(0.0, f64::max)
        + 2.0 * LINEAR_RESOLUTION;
    let tau = q.tan();
    let big = (tau * tau).max(1.0);
    let g = 2.0 * (big * far + q.k * tau);
    (g * LINEAR_RESOLUTION + big * LINEAR_RESOLUTION * LINEAR_RESOLUTION) * (1.0 + 1e-6)
}

/// `(1 + u²)² Δ` and `(1 + u²)² N` in each chart, in `s` over `[0, 1]`.
struct Charts {
    delta: [Bernstein; 2],
    near: [Bernstein; 2],
}

/// The charts, in interval arithmetic from the frames as stored. `None`
/// where `A`'s sign can't be certified (`A = 0`: not this form's).
fn charts(rl: &Rulings, region: &Box3) -> Option<Charts> {
    let q = &rl.q;
    let axes = [q.frame.x().get(), q.frame.y().get(), q.frame.z().get()].map(ivec);
    let local = |v: [Interval; 3]| axes.map(|a| idot(v, a));
    let (o, p) = (rl.cyl.origin(), q.frame.origin());
    let u0 = local([iv(o.x) - iv(p.x), iv(o.y) - iv(p.y), iv(o.z) - iv(p.z)]);
    let r = iv(rl.r);
    let uc = local(ivec(rl.cyl.x().get())).map(|x| x * r);
    let us = local(ivec(rl.cyl.y().get())).map(|x| x * r);
    let w = local(ivec(rl.cyl.z().get()));
    let tau = iv(q.sin).checked_div(iv(q.cos))?;
    let (t2, kt, k2) = (tau * tau, iv(q.k) * tau, iv(q.k) * iv(q.k));
    let a = w[0] * w[0] + w[1] * w[1] - t2 * w[2] * w[2];
    let a_abs = match strict(a)? {
        Sign::Positive => a,
        _ => -a,
    };
    let m = iv(margin(q, region));
    let [p1, pc, ps] = basis();
    let dd = |x: &[Bernstein; 3], y: &[Bernstein; 3]| {
        x[0].mul(&y[0])
            .add(&x[1].mul(&y[1]))
            .sub(&x[2].mul(&y[2]).scale(t2))
    };
    let wb = w.map(|x| Bernstein::new(vec![x]));
    let chart = |sigma: f64| {
        let sg = iv(sigma);
        let uh: [Bernstein; 3] = core::array::from_fn(|i| {
            p1.scale(u0[i])
                .add(&pc.scale(uc[i] * sg))
                .add(&ps.scale(us[i] * sg))
        });
        let b = dd(&wb, &uh).sub(&p1.scale(kt * w[2]));
        let c = dd(&uh, &uh)
            .sub(&uh[2].mul(&p1).scale(iv(2.0) * kt))
            .sub(&p1.mul(&p1).scale(k2));
        let delta = b.mul(&b).sub(&c.scale(a));
        let near = delta.add(&p1.mul(&p1).scale(a_abs * m));
        (delta, near)
    };
    let ((d0, n0), (d1, n1)) = (chart(1.0), chart(-1.0));
    Some(Charts {
        delta: [d0, d1],
        near: [n0, n1],
    })
}

/// How far each chart's part runs past a seam, in `s`. The overlap's sign
/// is certified, so it holds no root.
const SEAM: f64 = 1e-6;

/// The seams' turns tried, in order: the first where `Δ` and `N` have
/// certified signs, the same in both charts, at both seams is used.
const TURNS: [f64; 3] = [0.0, FRAC_PI_4, -FRAC_PI_4];

/// The roots' brackets, in `s` over a chart's part.
const ROOT_WIDTH: f64 = 1e-14;

/// A root of `Δ` or `N`, by its bracket in `θ`. `Rise` and `Fall` are
/// simple roots where the sign goes from negative to positive and back;
/// `Unsure` a stretch whose sign can't be certified.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    Rise([f64; 2]),
    Fall([f64; 2]),
    Unsure([f64; 2]),
}

impl Event {
    fn at(&self) -> [f64; 2] {
        match *self {
            Self::Rise(b) | Self::Fall(b) | Self::Unsure(b) => b,
        }
    }
}

/// `Δ`'s and `N`'s roots round the cylinder, in order from the seam at
/// `start` (chart 0 covers `[start, start + π]`, chart 1 the rest), with
/// the charts to read signs in.
struct Turn {
    start: f64,
    delta: Vec<Event>,
    near: Vec<Event>,
    charts: Charts,
}

impl Turn {
    /// The certified sign of `p` (`Δ`'s or `N`'s charts) at angle `theta`.
    fn sign_at(&self, p: &[Bernstein; 2], theta: f64) -> Option<Sign> {
        let mut th = theta;
        while th >= self.start + TAU {
            th -= TAU;
        }
        while th < self.start {
            th += TAU;
        }
        let (c, local) = if th < self.start + PI {
            (0, th)
        } else {
            (1, th - PI)
        };
        strict(p[c].eval(s_of(local)))
    }
}

/// The roots of `Δ` and `N` round the cylinder, read in two charts whose
/// seams have certified signs. `None` where no turn of the seams does.
fn turn(charts: Charts, m: &Meter) -> Result<Option<Turn>, Failure> {
    let near_seam =
        |p: &Bernstein, s: f64| strict(p.part((s - SEAM).max(0.0), (s + SEAM).min(1.0)).range());
    // The seam at `start` is chart 0's `lo` and chart 1's `hi`; the one at
    // `start + π` chart 0's `hi` and chart 1's `lo`.
    let signed = |p: &[Bernstein; 2], lo: f64, hi: f64| -> Option<()> {
        let (a0, a1) = (near_seam(&p[0], lo)?, near_seam(&p[1], hi)?);
        let (b0, b1) = (near_seam(&p[0], hi)?, near_seam(&p[1], lo)?);
        (a0 == a1 && b0 == b1).then_some(())
    };
    for gamma in TURNS {
        let (lo, hi) = (s_of(-FRAC_PI_2 + gamma), s_of(FRAC_PI_2 + gamma));
        if signed(&charts.delta, lo, hi).is_none() || signed(&charts.near, lo, hi).is_none() {
            continue;
        }
        let cut = [(lo - SEAM).max(0.0), (hi + SEAM).min(1.0)];
        let (mut delta, mut near) = (Vec::new(), Vec::new());
        for c in 0..2 {
            m.charge(1)?;
            for (p, out) in [(&charts.delta[c], &mut delta), (&charts.near[c], &mut near)] {
                for r in p.part(cut[0], cut[1]).roots(ROOT_WIDTH) {
                    m.charge(1)?;
                    let [a, b] = r.range();
                    let th = [a, b].map(|x| theta_of(c, cut[0] + (cut[1] - cut[0]) * x));
                    out.push(match r {
                        Root::Simple { rising: true, .. } => Event::Rise(th),
                        Root::Simple { .. } => Event::Fall(th),
                        Root::Unresolved { .. } => Event::Unsure(th),
                    });
                }
            }
        }
        return Ok(Some(Turn {
            start: -FRAC_PI_2 + gamma,
            delta,
            near,
            charts,
        }));
    }
    Ok(None)
}

/// Where the rulings cross: arcs `[θ0, θ1]` (`θ0 < θ1 < θ0 + 2π`) and
/// touches at `θ`, or every ruling (`round`).
#[derive(Clone, Debug, Default, PartialEq)]
struct Crossings {
    arcs: Vec<[f64; 2]>,
    touches: Vec<f64>,
    round: bool,
}

/// What `Δ`'s roots say, with `N`'s holding the near misses to account.
/// `None` where a gap's sign can't be certified, the signs don't
/// alternate, a root is unsure but for a touch (`Δ < 0` either side), or
/// a stretch where `Δ < 0` doesn't hold exactly one simple root of `N`
/// beside each root of `Δ` bounding it (none in a whole turn).
fn crossings(t: &Turn) -> Option<Crossings> {
    let ev = &t.delta;
    let n = ev.len();
    let mid = |b: [f64; 2]| (b[0] + b[1]) / 2.0;
    if n == 0 {
        let at = t.start + FRAC_PI_2;
        return match t.sign_at(&t.charts.delta, at)? {
            Sign::Positive if t.near.is_empty() => Some(Crossings {
                round: true,
                ..Crossings::default()
            }),
            Sign::Negative
                if t.near.is_empty() && t.sign_at(&t.charts.near, at) == Some(Sign::Negative) =>
            {
                Some(Crossings::default())
            }
            _ => None,
        };
    }
    // The gap after event `i` runs to event `i + 1` (round the turn).
    let gap = |i: usize| {
        let lo = ev[i].at()[1];
        let mut hi = ev[(i + 1) % n].at()[0];
        if i + 1 == n {
            hi += TAU;
        }
        [lo, hi]
    };
    let signs: Vec<Sign> = (0..n)
        .map(|i| {
            let [lo, hi] = gap(i);
            t.sign_at(&t.charts.delta, (lo + hi) / 2.0)
        })
        .collect::<Option<_>>()?;
    let mut out = Crossings::default();
    for i in 0..n {
        let (before, after) = (signs[(i + n - 1) % n], signs[i]);
        match ev[i] {
            Event::Rise(_) if before == Sign::Negative && after == Sign::Positive => {}
            Event::Fall(_) if before == Sign::Positive && after == Sign::Negative => {}
            Event::Unsure(b) if before == Sign::Negative && after == Sign::Negative => {
                out.touches.push(mid(b));
            }
            _ => return None,
        }
    }
    for i in 0..n {
        if let Event::Rise(b) = ev[i] {
            let Event::Fall(e) = ev[(i + 1) % n] else {
                return None;
            };
            let (t0, mut t1) = (mid(b), mid(e));
            if t1 <= t0 {
                t1 += TAU;
            }
            out.arcs.push([t0, t1]);
        }
    }
    // Near misses: each root of N simple, inside a gap where Δ < 0, two
    // to each such gap.
    let mut count = vec![0usize; n];
    for e in &t.near {
        let (Event::Rise(b) | Event::Fall(b)) = *e else {
            return None;
        };
        let k = (0..n).find(|&i| {
            let [lo, hi] = gap(i);
            let mut x = b[0];
            while x < lo {
                x += TAU;
            }
            while x >= lo + TAU {
                x -= TAU;
            }
            x > lo && x + (b[1] - b[0]) < hi
        })?;
        if signs[k] != Sign::Negative {
            return None;
        }
        count[k] += 1;
    }
    for i in 0..n {
        if signs[i] == Sign::Negative && count[i] != 2 {
            return None;
        }
    }
    Some(out)
}

/// Hermite spans a curve starts with (a power of two, so `π` is a break).
const START_SPANS: usize = 32;
/// The most spans a curve may take.
const MAX_SPANS: usize = 1 << 12;

impl Rulings {
    /// The loop over arc `[t0, t1]` at `φ` (`θ = θm + h cos φ`, the `+`
    /// branch for `φ` in `(0, π)`): its point and tangent. At `φ = 0` and
    /// `π`, the arc's ends, `√Δ`'s factor `|sin φ|` is taken to its limit.
    fn on_loop(&self, [t0, t1]: [f64; 2], phi: f64) -> (Point3, Vector3) {
        let (mid, h) = ((t0 + t1) / 2.0, (t1 - t0) / 2.0);
        let (sp, cp) = math::sin_cos(phi);
        let th = mid + h * cp;
        let dth = -h * sp;
        let (b, db) = self.b(th);
        let (d, dd) = self.delta(th);
        let (s, ds) = if phi == 0.0 {
            (0.0, math::sqrt((-dd * h / 2.0).max(0.0)))
        } else if phi == PI {
            (0.0, -math::sqrt((dd * h / 2.0).max(0.0)))
        } else {
            let root = math::sqrt(d.max(0.0));
            let side = if sp > 0.0 { 1.0 } else { -1.0 };
            let ds = if root > 0.0 {
                side * dd * dth / (2.0 * root)
            } else {
                0.0
            };
            (side * root, ds)
        };
        let a = self.a();
        let (t, dt) = ((-b + s) / a, (-db * dth + ds) / a);
        (
            self.point(th, t),
            self.along(th) * dth + self.cyl.z().get() * dt,
        )
    }

    /// The curve round the cylinder on side `side` (±1) at `θ`, where
    /// every ruling crosses.
    fn round(&self, side: f64, th: f64) -> (Point3, Vector3) {
        let (b, db) = self.b(th);
        let (d, dd) = self.delta(th);
        let root = math::sqrt(d.max(0.0));
        let ds = if root > 0.0 {
            side * dd / (2.0 * root)
        } else {
            0.0
        };
        let a = self.a();
        let (t, dt) = ((-b + side * root) / a, (-db + ds) / a);
        (self.point(th, t), self.along(th) + self.cyl.z().get() * dt)
    }
}

/// A closed curve through `at`'s points and tangents over `[0, 2π]`, as
/// cubic Hermite spans: [`START_SPANS`] to begin, each halved until the
/// cubic's middle is within a quarter resolution of `at`'s point there
/// (a check, not a certificate: `ss::with_pcurves` certifies). Its last
/// point is its first, bit for bit. `None` past [`MAX_SPANS`]. A unit a
/// span tried.
fn hermite(
    at: &dyn Fn(f64) -> (Point3, Vector3),
    m: &Meter,
) -> Result<Option<NurbsCurve>, Failure> {
    let step = TAU / START_SPANS as f64;
    let mut todo: Vec<[f64; 2]> = (0..START_SPANS)
        .rev()
        .map(|k| [step * k as f64, step * (k + 1) as f64])
        .collect();
    let first = at(0.0);
    let mut breaks = vec![0.0];
    let mut ends = Vec::new();
    let mut tried = 0;
    while let Some([a, b]) = todo.pop() {
        m.charge(1)?;
        tried += 1;
        if breaks.len() > MAX_SPANS || tried > 4 * MAX_SPANS {
            return Ok(None);
        }
        let pa = if a == 0.0 { first } else { at(a) };
        let pb = if b == TAU { first } else { at(b) };
        let w = b - a;
        let mid = pa.0 + (pb.0 - pa.0) * 0.5 + (pa.1 - pb.1) * (w / 8.0);
        let c = (a + b) / 2.0;
        if mid.distance(at(c).0) > LINEAR_RESOLUTION / 4.0 {
            if c <= a || c >= b {
                return Ok(None);
            }
            todo.push([c, b]);
            todo.push([a, c]);
            continue;
        }
        breaks.push(b);
        ends.push((pa, pb));
    }
    let spans = ends.len();
    let mut points = Vec::with_capacity(3 * spans + 1);
    points.push(first.0);
    for (i, ((pa, da), (pb, db))) in ends.into_iter().enumerate() {
        let w = breaks[i + 1] - breaks[i];
        points.push(pa + da * (w / 3.0));
        points.push(pb - db * (w / 3.0));
        points.push(pb);
    }
    let mut mults = vec![3; spans + 1];
    mults[0] = 4;
    mults[spans] = 4;
    let Ok(knots) = KnotVector::clamped(3, &breaks, &mults) else {
        return Ok(None);
    };
    Ok(NurbsCurve::new(knots, points, None).ok())
}

impl Rulings {
    /// The sign of `k + τ z` at `p` in the quadric's local coordinates:
    /// which nappe of a cone `p` is on (positive: the cone's own). Always
    /// positive for a cylinder.
    fn nappe(&self, p: Point3) -> Option<Sign> {
        let q = &self.q;
        let o = q.frame.origin();
        let d = [iv(p.x) - iv(o.x), iv(p.y) - iv(o.y), iv(p.z) - iv(o.z)];
        let tau = iv(q.sin).checked_div(iv(q.cos))?;
        strict(iv(q.k) + tau * idot(d, ivec(q.frame.z().get())))
    }

    /// Whether the cone's apex is certified off the cylinder, so no curve
    /// passes from one nappe to the other (always, for a cylinder).
    fn apex_off(&self) -> bool {
        let q = &self.q;
        if q.sin == 0.0 {
            return true;
        }
        let Some(v) = (iv(q.k) * iv(q.cos)).checked_div(iv(q.sin)) else {
            return false;
        };
        let (o, z, c) = (q.frame.origin(), q.frame.z().get(), self.cyl.origin());
        let d = [
            iv(o.x) - iv(z.x) * v - iv(c.x),
            iv(o.y) - iv(z.y) * v - iv(c.y),
            iv(o.z) - iv(z.z) * v - iv(c.z),
        ];
        let (x, y) = (ivec(self.cyl.x().get()), ivec(self.cyl.y().get()));
        strict(idot(d, x).square() + idot(d, y).square() - iv(self.r).square()).is_some()
    }
}

/// The closed form for a cylinder against a cone, or against a cylinder
/// of another radius or on a skew axis: its branches and touch points.
/// `None` where any of it can't be certified (see the module's doc), and
/// for any other pair.
pub(crate) fn ruled(
    a: &Surface,
    b: &Surface,
    region: &Box3,
    m: &Meter,
) -> Result<Option<Hit>, Failure> {
    let (cyl, other) = match (a, b) {
        (Surface::Cylinder(x), Surface::Cylinder(y)) if y.radius() < x.radius() => (b, a),
        (Surface::Cylinder(_), Surface::Cylinder(_) | Surface::Cone(_)) => (a, b),
        (Surface::Cone(_), Surface::Cylinder(_)) => (b, a),
        _ => return Ok(None),
    };
    #[cfg(test)]
    TRIED.with(|n| n.set(n.get() + 1));
    let Surface::Cylinder(c) = cyl else {
        return Ok(None);
    };
    let (Some(frame), Some(q)) = (cyl.frame(), Quadric::of(other)) else {
        return Ok(None);
    };
    let rl = Rulings::new(*frame, c.radius(), q);
    if !rl.apex_off() {
        return Ok(None);
    }
    let Some(charts) = charts(&rl, region) else {
        return Ok(None);
    };
    let Some(turn) = turn(charts, m)? else {
        return Ok(None);
    };
    let Some(found) = crossings(&turn) else {
        return Ok(None);
    };
    let mut out = Out::new(region);
    for &th in &found.touches {
        let p = rl.point(th, -rl.b(th).0 / rl.a());
        match rl.nappe(p) {
            Some(Sign::Positive) => {}
            Some(_) => continue,
            None => return Ok(None),
        }
        if other.distance(p) > LINEAR_RESOLUTION {
            return Ok(None);
        }
        out.point(p);
    }
    let mut curves = Vec::new();
    for &arc in &found.arcs {
        curves.push(hermite(&|phi| rl.on_loop(arc, phi), m)?);
    }
    if found.round {
        for side in [1.0, -1.0] {
            curves.push(hermite(&|th| rl.round(side, th), m)?);
        }
    }
    for curve in curves {
        let Some(curve) = curve else {
            return Ok(None);
        };
        let curve = Curve::Nurbs(Arc::new(curve));
        match rl.nappe(curve.point(PI)) {
            Some(Sign::Positive) => out.closed(curve, false, 0.0),
            Some(_) => {}
            None => return Ok(None),
        }
    }
    Ok(Some(out.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free() -> Meter<'static> {
        // unmetered: unit tests of a closed form.
        Meter::free()
    }
    use crate::geom::{Cone, Cylinder};

    fn frame(o: (f64, f64, f64), z: (f64, f64, f64), x: (f64, f64, f64)) -> Frame {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(x.0, x.1, x.2),
        )
        .unwrap()
    }

    /// The quadratic along a ruling is the other surface's implicit form
    /// at the ruling's points, in floats, at 64 angles and 5 heights.
    #[test]
    fn a_rulings_quadratic_is_the_quadric_along_it() {
        let cyl = frame((0.1, -0.2, 0.3), (0.2, 1.0, 0.1), (1.0, 0.0, 0.0));
        let cone = frame((0.0, 0.0, 0.0), (0.0, 0.1, 1.0), (1.0, 0.0, 0.0));
        let other = Surface::Cone(Cone::new(cone, 1.2, 0.3).unwrap());
        let rl = Rulings::new(cyl, 0.4, Quadric::of(&other).unwrap());
        let implicit = crate::intersect::implicit::Implicit::of(&other).unwrap();
        for k in 0..64 {
            let th = TAU * f64::from(k) / 64.0;
            let (b, _) = rl.b(th);
            let (c, _) = rl.c(th);
            for t in [-2.0, -0.5, 0.0, 0.7, 3.0] {
                let p = rl.point(th, t);
                let f = implicit.value(implicit.local(p.to_vector()));
                let want = (f.lo() + f.hi()) / 2.0;
                let got = rl.a() * t * t + 2.0 * b * t + c;
                assert!((got - want).abs() < 1e-12, "{th} {t}: {got} against {want}");
            }
        }
        // Derivatives by θ, against central differences.
        for k in 0..16 {
            let th = TAU * f64::from(k) / 16.0;
            let e = 1e-6;
            let num = |f: &dyn Fn(f64) -> (f64, f64)| (f(th + e).0 - f(th - e).0) / (2.0 * e);
            assert!((rl.b(th).1 - num(&|x| rl.b(x))).abs() < 1e-6);
            assert!((rl.c(th).1 - num(&|x| rl.c(x))).abs() < 1e-6);
            assert!((rl.delta(th).1 - num(&|x| rl.delta(x))).abs() < 1e-6);
        }
    }

    /// Each chart's quartic, at 33 values of `s`, holds `(1 + u²)² Δ(θ)`
    /// computed in floats; `N` holds it plus `(1 + u²)² |A| M`.
    #[test]
    fn the_charts_hold_delta() {
        let cyl = frame((0.1, -0.2, 0.3), (0.2, 1.0, 0.1), (1.0, 0.0, 0.0));
        let cone = frame((0.0, 0.0, 0.0), (0.0, 0.1, 1.0), (1.0, 0.0, 0.0));
        let other = Surface::Cone(Cone::new(cone, 1.2, 0.3).unwrap());
        let q = Quadric::of(&other).unwrap();
        let rl = Rulings::new(cyl, 0.4, q);
        let region = Box3::new(Point3::new(-5.0, -5.0, -5.0), Point3::new(5.0, 5.0, 5.0));
        let ch = charts(&rl, &region).unwrap();
        let m = margin(&q, &region);
        assert!(m > 0.0 && m < 1e-5, "{m}");
        for c in 0..2 {
            for k in 0..=32 {
                let s = f64::from(k) / 32.0;
                let u = U * (2.0 * s - 1.0);
                let w = (1.0 + u * u) * (1.0 + u * u);
                let want = rl.delta(theta_of(c, s)).0 * w;
                let got = ch.delta[c].eval(s);
                let slack = 1e-9 * want.abs().max(1.0);
                assert!(
                    got.lo() - slack <= want && want <= got.hi() + slack,
                    "{c} {s}: {got:?} against {want}"
                );
                let near = ch.near[c].eval(s);
                let want_near = want + w * rl.a().abs() * m;
                assert!(near.lo() - slack <= want_near && want_near <= near.hi() + slack);
            }
        }
    }

    /// A pipe of radius 0.5 along x whose axis lies on the wall of a
    /// cylinder of radius 1 along z (the pipe's frame x is world y,
    /// exactly). Its rulings cross where `|1 + cos θ / 2| < 1`, `θ` in
    /// `(π/2, 3π/2)`, and are tangent at `θ = ±π/2`: the first charts'
    /// seams. So the seams turn, and `Δ` rises at π/2 and falls at 3π/2,
    /// read in order from the turned seam.
    #[test]
    fn a_root_on_a_chart_seam_turns_the_seams() {
        let wall = Surface::Cylinder(
            Cylinder::new(
                frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0)),
                1.0,
            )
            .unwrap(),
        );
        let rl = Rulings::new(
            frame((0.0, 1.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)),
            0.5,
            Quadric::of(&wall).unwrap(),
        );
        let region = Box3::new(
            Point3::new(-10.0, -10.0, -10.0),
            Point3::new(10.0, 10.0, 10.0),
        );
        let turn = turn(charts(&rl, &region).unwrap(), &free())
            .unwrap()
            .unwrap();
        assert_ne!(turn.start, -FRAC_PI_2, "the seams turned");
        assert_eq!(turn.delta.len(), 2, "{:?}", turn.delta);
        for e in &turn.delta {
            let [a, b] = e.at();
            assert!(b >= a && b - a < 1e-9, "{e:?}");
            let (s, c) = math::sin_cos((a + b) / 2.0);
            assert!(c.abs() < 1e-9 && (s.abs() - 1.0).abs() < 1e-9, "{e:?}");
            match e {
                Event::Rise(_) => assert!(s > 0.0, "the rulings start crossing at π/2"),
                Event::Fall(_) => assert!(s < 0.0, "and stop at 3π/2"),
                Event::Unsure(_) => panic!("{e:?}"),
            }
        }
    }

    /// Hand-made turns: events and charts whose signs are known.
    fn turn_of(delta: Vec<Event>, near: Vec<Event>, d: [f64; 3], n: [f64; 3]) -> Turn {
        // Constant-sign charts are enough where the test's gaps are read.
        let quad = |c: [f64; 3]| Bernstein::from_f64(&c);
        Turn {
            start: -FRAC_PI_2,
            delta,
            near,
            charts: Charts {
                delta: [quad(d), quad(d)],
                near: [quad(n), quad(n)],
            },
        }
    }

    /// One arc, `Δ > 0` over `(0, π)` and `< 0` over `(π, 2π)` (charts
    /// linear in `s`, their roots at `θ = 0` and `π`): its stretch where
    /// `Δ < 0` must hold exactly two simple roots of `N`, one beside each
    /// of `Δ`'s (plan 22.D's M5b). None or four, and it refuses.
    #[test]
    fn a_stretch_where_the_rulings_miss_holds_two_roots_of_n() {
        let b = |x: f64| [x - 1e-12, x + 1e-12];
        let lin = |c: [f64; 2]| Bernstein::from_f64(&c);
        let with = |near: Vec<Event>| Turn {
            start: -FRAC_PI_2,
            delta: vec![Event::Rise(b(0.0)), Event::Fall(b(PI))],
            near,
            charts: Charts {
                delta: [lin([-1.0, 1.0]), lin([1.0, -1.0])],
                near: [lin([-1.0, 1.0]), lin([1.0, -1.0])],
            },
        };
        let two = vec![Event::Fall(b(PI + 0.01)), Event::Rise(b(-0.01))];
        let found = crossings(&with(two)).expect("one arc");
        assert_eq!(found.arcs, [[0.0, PI]]);
        assert!(found.touches.is_empty() && !found.round);
        assert_eq!(crossings(&with(vec![])), None, "no root of N beside Δ's");
        let four = vec![
            Event::Fall(b(PI + 0.01)),
            Event::Rise(b(PI + 1.0)),
            Event::Fall(b(PI + 1.5)),
            Event::Rise(b(-0.01)),
        ];
        assert_eq!(
            crossings(&with(four)),
            None,
            "a near touch inside the stretch"
        );
    }

    #[test]
    fn signs_round_the_turn_make_arcs_touches_or_nothing() {
        let b = |x: f64| [x - 1e-12, x + 1e-12];
        // Every ruling crosses: Δ > 0 all round, no roots.
        let all = turn_of(vec![], vec![], [1.0; 3], [1.0; 3]);
        assert!(crossings(&all).unwrap().round);
        // None crosses, and none comes near: Δ < 0, N < 0, no roots.
        let none = turn_of(vec![], vec![], [-1.0; 3], [-1.0; 3]);
        assert_eq!(crossings(&none), Some(Crossings::default()));
        // None crosses, but N has roots: a near touch somewhere.
        let close = turn_of(
            vec![],
            vec![Event::Rise(b(0.1)), Event::Fall(b(0.2))],
            [-1.0; 3],
            [-1.0; 3],
        );
        assert_eq!(crossings(&close), None);
    }

    /// A loop's points lie on both surfaces, its ends at the arc's ends,
    /// and the Hermite fit closes on itself.
    #[test]
    fn a_loop_runs_both_branches_and_closes() {
        let wall = Surface::Cylinder(
            Cylinder::new(
                frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0)),
                1.0,
            )
            .unwrap(),
        );
        let rl = Rulings::new(
            frame((0.0, 1.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)),
            0.5,
            Quadric::of(&wall).unwrap(),
        );
        let arc = [FRAC_PI_2, 3.0 * FRAC_PI_2];
        let pipe = Surface::Cylinder(Cylinder::new(rl.cyl, 0.5).unwrap());
        for k in 0..=256 {
            let phi = TAU * f64::from(k) / 256.0;
            let (p, d) = rl.on_loop(arc, phi);
            assert!(
                wall.distance(p) < 1e-12 && pipe.distance(p) < 1e-12,
                "{phi}: {p:?}"
            );
            assert!(d.length().is_finite());
        }
        let curve = hermite(&|phi| rl.on_loop(arc, phi), &free())
            .unwrap()
            .unwrap();
        let c = Curve::Nurbs(Arc::new(curve));
        assert_eq!(c.point(0.0), c.point(TAU));
        for k in 0..=1024 {
            let p = c.point(TAU * f64::from(k) / 1024.0);
            assert!(wall.distance(p) < 2e-8 && pipe.distance(p) < 2e-8);
        }
        // Round: a pipe of radius 0.5 along x through the axis of the wall.
        let rl = Rulings::new(
            frame((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)),
            0.5,
            Quadric::of(&wall).unwrap(),
        );
        for side in [1.0, -1.0] {
            let curve = hermite(&|th| rl.round(side, th), &free()).unwrap().unwrap();
            let c = Curve::Nurbs(Arc::new(curve));
            for k in 0..=1024 {
                let p = c.point(TAU * f64::from(k) / 1024.0);
                assert!(wall.distance(p) < 2e-8);
                assert_eq!(p.x > 0.0, side > 0.0, "{p:?}");
            }
        }
    }

    fn region() -> Box3 {
        Box3::new(
            Point3::new(-10.0, -10.0, -10.0),
            Point3::new(10.0, 10.0, 10.0),
        )
    }

    fn cylinder(o: (f64, f64, f64), z: (f64, f64, f64), r: f64) -> Surface {
        let x = if z.0.abs() < 0.9 {
            (1.0, 0.0, 0.0)
        } else {
            (0.0, 1.0, 0.0)
        };
        Surface::Cylinder(Cylinder::new(frame(o, z, x), r).unwrap())
    }

    fn cone(o: (f64, f64, f64), z: (f64, f64, f64), r: f64, a: f64) -> Surface {
        let x = if z.0.abs() < 0.9 {
            (1.0, 0.0, 0.0)
        } else {
            (0.0, 1.0, 0.0)
        };
        Surface::Cone(Cone::new(frame(o, z, x), r, a).unwrap())
    }

    /// `ruled` either way round, the same answer; and any other pair, or
    /// a pair of planes and cones, is not its.
    #[test]
    fn ruled_takes_cylinders_with_cones_or_cylinders_only() {
        let m = free();
        let pin = cylinder((0.0, 0.0, 0.3), (1.0, 0.0, 0.0), 0.5);
        let bore = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 2.0, 5f64.to_radians());
        let one = ruled(&pin, &bore, &region(), &m).unwrap().unwrap();
        let two = ruled(&bore, &pin, &region(), &m).unwrap().unwrap();
        assert_eq!(one, two);
        assert_eq!(one.0.len(), 2);
        let plane = Surface::Plane(crate::geom::Plane::new(frame(
            (0.0, 0.0, 0.0),
            (0.0, 0.0, 1.0),
            (1.0, 0.0, 0.0),
        )));
        assert_eq!(ruled(&plane, &bore, &region(), &m), Ok(None));
        assert_eq!(ruled(&bore, &bore, &region(), &m), Ok(None));
    }

    /// Inside `region()` less a millimetre on every side.
    fn inner(p: Point3) -> bool {
        let r = region().grow(-1e-3);
        let (lo, hi) = (r.min(), r.max());
        (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && (lo.z..=hi.z).contains(&p.z)
    }

    /// Every crossing of 1,024 rulings, found by walking each one through
    /// the implicit form of `other` (both nappes) and keeping those on
    /// `other` itself.
    fn sampled(cyl: &Surface, other: &Surface) -> Vec<Point3> {
        let Surface::Cylinder(c) = cyl else { panic!() };
        let f = cyl.frame().unwrap();
        let imp = crate::intersect::implicit::Implicit::of(other).unwrap();
        let value = |p: Point3| {
            let v = imp.value(imp.local(p.to_vector()));
            (v.lo() + v.hi()) / 2.0
        };
        let mut out = Vec::new();
        for k in 0..1024 {
            let th = TAU * (f64::from(k) + 0.5) / 1024.0;
            let (s, co) = math::sin_cos(th);
            let base =
                f.origin() + f.x().get() * (c.radius() * co) + f.y().get() * (c.radius() * s);
            let at = |t: f64| base + f.z().get() * t;
            let steps = 8192;
            let (lo, hi) = (-30.0, 30.0);
            let mut prev = value(at(lo));
            for i in 1..=steps {
                let t1 = lo + (hi - lo) * f64::from(i) / f64::from(steps);
                let now = value(at(t1));
                if (prev < 0.0) != (now < 0.0) {
                    let (mut a, mut b) = (t1 - (hi - lo) / f64::from(steps), t1);
                    for _ in 0..80 {
                        let m = (a + b) / 2.0;
                        if (value(at(m)) < 0.0) == (prev < 0.0) {
                            a = m
                        } else {
                            b = m
                        }
                    }
                    let p = at((a + b) / 2.0);
                    if other.distance(p) <= 1e-7 && inner(p) {
                        out.push(p);
                    }
                }
                prev = now;
            }
        }
        out
    }

    /// Every branch within 2e-8 of both surfaces at 1,024 points; every
    /// sampled crossing within 1e-6 of a branch; and the branch count.
    fn agrees(cyl: &Surface, other: &Surface, branches: usize) {
        let (raws, points) = ruled(cyl, other, &region(), &free())
            .unwrap()
            .expect("a closed form");
        assert!(points.is_empty(), "{points:?}");
        assert_eq!(raws.len(), branches);
        for r in &raws {
            for k in 0..=1024 {
                let p = r
                    .curve
                    .point(r.range[0] + (r.range[1] - r.range[0]) * f64::from(k) / 1024.0);
                assert!(
                    cyl.distance(p) <= 2e-8 && other.distance(p) <= 2e-8,
                    "{p:?}"
                );
            }
        }
        let crossings = sampled(cyl, other);
        assert!(!crossings.is_empty());
        for p in crossings {
            assert!(
                raws.iter().any(|r| r.curve.distance(p) <= 1e-6),
                "missed {p:?}"
            );
        }
    }

    #[test]
    fn a_pin_through_a_bore_meets_it_in_two_curves_round_the_pin() {
        let pin = cylinder((0.0, 0.0, 0.3), (1.0, 0.0, 0.0), 0.5);
        let bore = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 2.0, 5f64.to_radians());
        agrees(&pin, &bore, 2);
    }

    #[test]
    fn a_pin_through_a_bores_wall_meets_it_in_one_loop() {
        // Axis at y = −2, on the wall: the rulings toward the bore's axis
        // cross, the arc running across θ = 0 (the frame's x is world y).
        let pin = Surface::Cylinder(
            Cylinder::new(
                frame((0.0, -2.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)),
                0.5,
            )
            .unwrap(),
        );
        let bore = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 2.0, 5f64.to_radians());
        agrees(&pin, &bore, 1);
    }

    #[test]
    fn a_rod_wider_than_a_bore_meets_it_in_two_loops() {
        let rod = cylinder((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), 1.5);
        let bore = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, 10f64.to_radians());
        agrees(&rod, &bore, 2);
    }

    #[test]
    fn skew_cylinders_of_unequal_radii() {
        let big = cylinder((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0);
        // Through: the small one's rulings all cross the big one.
        let through = cylinder((0.0, 0.3, 0.0), (1.0, 0.0, 0.3), 0.4);
        agrees(&through, &big, 2);
        // Either way round, the smaller one is ruled.
        assert_eq!(
            ruled(&big, &through, &region(), &free()),
            ruled(&through, &big, &region(), &free())
        );
        // And it is the smaller one's rulings that are read: each branch
        // runs round the small cylinder, its parameter the angle about the
        // small one's axis (plan 22.D's M12; ruling the big one gives two
        // loops on it, parametrized about their arcs instead).
        let (raws, _) = ruled(&big, &through, &region(), &free()).unwrap().unwrap();
        let f = through.frame().unwrap();
        for r in &raws {
            for k in 0..16 {
                let th = TAU * (f64::from(k) + 0.5) / 16.0;
                let d = r.curve.point(th) - f.origin();
                let at = math::atan2(d.dot(f.y().get()), d.dot(f.x().get()));
                let off = (at - th).rem_euclid(TAU);
                assert!(off.min(TAU - off) < 1e-9, "{th}: {at}");
            }
        }
        // Partly: an axis 0.8 off, the rulings on its far side miss.
        let partly = cylinder((0.0, 0.8, 0.0), (1.0, 0.0, 0.3), 0.4);
        agrees(&partly, &big, 1);
    }

    #[test]
    fn a_root_on_a_chart_seam_still_gives_its_loop() {
        let wall = Surface::Cylinder(
            Cylinder::new(
                frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 0.0)),
                1.0,
            )
            .unwrap(),
        );
        let pipe = Surface::Cylinder(
            Cylinder::new(
                frame((0.0, 1.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)),
                0.5,
            )
            .unwrap(),
        );
        agrees(&pipe, &wall, 1);
    }

    /// A cone of radius 1 about z (10°), and a cylinder of radius 0.25
    /// along y, tangent to it at (1, 0, 0) from outside (`out = 1`) or
    /// inside (`out = −1`), moved `gap` further out along the normal.
    fn grazing(out: f64, gap: f64) -> (Surface, Surface) {
        let a = 10f64.to_radians();
        let (s, c) = math::sin_cos(a);
        let n = Vector3::new(c, 0.0, -s);
        let p = Point3::new(1.0, 0.0, 0.0) + n * (out * 0.25 + gap);
        let cyl = Surface::Cylinder(
            Cylinder::new(Frame::new(p, Vector3::new(0.0, 1.0, 0.0), n).unwrap(), 0.25).unwrap(),
        );
        (cyl, cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, a))
    }

    #[test]
    fn an_outside_graze_is_a_touch() {
        let (cyl, bore) = grazing(1.0, 0.0);
        let (raws, points) = ruled(&cyl, &bore, &region(), &free())
            .unwrap()
            .expect("a touch");
        assert!(raws.is_empty(), "{raws:?}");
        assert_eq!(points.len(), 1);
        assert!(
            points[0].distance(Point3::new(1.0, 0.0, 0.0)) <= 1e-8,
            "{:?}",
            points[0]
        );
    }

    #[test]
    fn a_pair_kept_apart_meets_nowhere() {
        let (cyl, bore) = grazing(1.0, 0.1);
        assert_eq!(
            ruled(&cyl, &bore, &region(), &free()),
            Ok(Some((vec![], vec![])))
        );
    }

    /// Kept apart by less than the resolution: a touch or `None`, never
    /// nothing.
    #[test]
    fn a_near_miss_within_the_resolution_is_never_nothing() {
        let (cyl, bore) = grazing(1.0, 5e-9);
        match ruled(&cyl, &bore, &region(), &free()).unwrap() {
            None => {}
            Some((raws, points)) => {
                assert!(raws.is_empty());
                assert_eq!(points.len(), 1, "a touch");
            }
        }
    }

    /// Inside the bore, tangent: the two branches cross at the touch (a
    /// pinch), which the form refuses.
    #[test]
    fn an_inside_graze_is_a_pinch_and_refused() {
        let (cyl, bore) = grazing(-1.0, 0.0);
        assert_eq!(ruled(&cyl, &bore, &region(), &free()), Ok(None));
    }

    /// `A = 0`: the cylinder's axis along one of the cone's rulings, or
    /// two parallel cylinders.
    #[test]
    fn an_axis_along_a_ruling_is_not_this_forms() {
        let a = 20f64.to_radians();
        let (s, c) = math::sin_cos(a);
        let bore = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, a);
        let along = cylinder((3.0, 0.0, 0.0), (s, 0.0, c), 0.2);
        assert_eq!(ruled(&along, &bore, &region(), &free()), Ok(None));
        // At 45° the axis along a ruling makes `A` round to 5.6e-17,
        // whose sign isn't certified: the charts refuse at once, before
        // anything later could (plan 22.D's M8).
        let steep = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, FRAC_PI_4);
        let along = cylinder((3.0, 0.0, 0.0), (1.0, 0.0, 1.0), 0.2);
        let Surface::Cylinder(c) = &along else {
            unreachable!()
        };
        let rl = Rulings::new(
            *along.frame().unwrap(),
            c.radius(),
            Quadric::of(&steep).unwrap(),
        );
        assert!(charts(&rl, &region()).is_none());
        assert_eq!(ruled(&along, &steep, &region(), &free()), Ok(None));
        let one = cylinder((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0);
        let two = cylinder((0.5, 0.0, 0.0), (0.0, 0.0, 1.0), 0.3);
        assert_eq!(ruled(&two, &one, &region(), &free()), Ok(None));
    }

    /// Below the apex the rulings cross only the other nappe, which the
    /// cone doesn't have: nothing. Above, the cone itself: two curves.
    #[test]
    fn the_cones_other_nappe_is_left_out() {
        // Radius 1 at z = 0, 45°: the apex at z = −1, the cone above it.
        let bore = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, FRAC_PI_4);
        let below = cylinder((0.0, 0.0, -3.0), (1.0, 0.0, 0.0), 0.3);
        assert_eq!(
            ruled(&below, &bore, &region(), &free()),
            Ok(Some((vec![], vec![])))
        );
        let above = cylinder((0.0, 0.0, 1.0), (1.0, 0.0, 0.0), 0.3);
        agrees(&above, &bore, 2);
        // Through the apex: not this form's.
        let apex = cylinder((0.0, 0.3, -1.0), (1.0, 0.0, 0.0), 0.3);
        assert_eq!(ruled(&apex, &bore, &region(), &free()), Ok(None));
        // Refused by the apex's own check, not only by what follows it
        // (plan 22.D's M10); and the cylinders above and below are off it.
        let off = |c: &Surface| {
            let Surface::Cylinder(k) = c else {
                unreachable!()
            };
            Rulings::new(*c.frame().unwrap(), k.radius(), Quadric::of(&bore).unwrap()).apex_off()
        };
        assert!(!off(&apex));
        assert!(off(&above) && off(&below));
    }
}
