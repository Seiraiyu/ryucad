#![allow(unused)]

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
//! trace fails, never inside a re-trim rung's first build (the tube
//! contact's gate), so every pair that traces keeps its result. A unit a
//! chart, a root and a span.

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

/// See the module's doc. A stub until the pieces below are in.
pub(crate) fn ruled(
    _a: &Surface,
    _b: &Surface,
    _region: &Box3,
    _m: &Meter,
) -> Result<Option<(Vec<Raw>, Vec<Point3>)>, Failure> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
