//! Second-order jets in two variables, in floating point or intervals.
//!
//! A [`Jet`] carries a value with its gradient and Hessian by two
//! parameters, `(s, θ)` for a pipe. Arithmetic on jets applies the
//! product and chain rules, so a formula written once over jets gives a
//! function's derivatives up to second order exactly (in `f64`), or, over
//! [`Interval`]s whose inputs enclose the values and derivatives at every
//! point of a parameter box, enclosures of the derivatives over the whole
//! box. Each operation of the chain rule holds pointwise, so its interval
//! form encloses the true values at every point too.

use crate::interval::Interval;
use crate::math;
use core::ops::{Add, Mul, Neg, Sub};

/// What jets are made of: `f64` for values, [`Interval`] for enclosures.
pub(crate) trait Num:
    Copy + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Neg<Output = Self>
{
    fn of(x: f64) -> Self;
    /// `1 / x`; `None` where `x` is (or may be) zero.
    fn recip(self) -> Option<Self>;
    /// `√x`; `None` where `x` is (or may be) negative or zero, where the
    /// root has no derivative.
    fn sqrt(self) -> Option<Self>;
}

impl Num for f64 {
    fn of(x: f64) -> Self {
        x
    }

    fn recip(self) -> Option<Self> {
        (self != 0.0).then(|| 1.0 / self)
    }

    fn sqrt(self) -> Option<Self> {
        (self > 0.0).then(|| math::sqrt(self))
    }
}

impl Num for Interval {
    fn of(x: f64) -> Self {
        Interval::point(x)
    }

    fn recip(self) -> Option<Self> {
        Interval::point(1.0).checked_div(self)
    }

    fn sqrt(self) -> Option<Self> {
        if self.lo() <= 0.0 {
            return None;
        }
        Interval::sqrt(self)
    }
}

/// A value, its gradient `[d/ds, d/dθ]` and its Hessian
/// `[d²/ds², d²/ds dθ, d²/dθ²]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Jet<T> {
    pub(crate) v: T,
    pub(crate) d: [T; 2],
    pub(crate) h: [T; 3],
}

impl<T: Num> Jet<T> {
    pub(crate) fn constant(x: T) -> Self {
        let z = T::of(0.0);
        Self {
            v: x,
            d: [z; 2],
            h: [z; 3],
        }
    }

    /// A function of `s` alone, from its value and first two derivatives.
    pub(crate) fn of_s(v: T, d1: T, d2: T) -> Self {
        let z = T::of(0.0);
        Self {
            v,
            d: [d1, z],
            h: [d2, z, z],
        }
    }

    /// A function of `θ` alone, from its value and first two derivatives.
    pub(crate) fn of_t(v: T, d1: T, d2: T) -> Self {
        let z = T::of(0.0);
        Self {
            v,
            d: [z, d1],
            h: [z, z, d2],
        }
    }

    pub(crate) fn scale(self, k: f64) -> Self {
        let k = T::of(k);
        Self {
            v: self.v * k,
            d: self.d.map(|x| x * k),
            h: self.h.map(|x| x * k),
        }
    }

    /// `f(self)` from `f`, `f'` and `f''` at the value: the chain rule.
    fn compose(self, f0: T, f1: T, f2: T) -> Self {
        let [a, b] = self.d;
        Self {
            v: f0,
            d: [f1 * a, f1 * b],
            h: [
                f1 * self.h[0] + f2 * a * a,
                f1 * self.h[1] + f2 * a * b,
                f1 * self.h[2] + f2 * b * b,
            ],
        }
    }

    /// `1 / self`; `None` where the value may be zero.
    pub(crate) fn recip(self) -> Option<Self> {
        let r = self.v.recip()?;
        let r2 = r * r;
        Some(self.compose(r, -r2, T::of(2.0) * r2 * r))
    }

    /// `√self`; `None` where the value may be zero or negative.
    pub(crate) fn sqrt(self) -> Option<Self> {
        let q = self.v.sqrt()?;
        let inv = q.recip()?;
        let half = T::of(0.5) * inv;
        Some(self.compose(q, half, -(half * inv * T::of(0.5) * inv)))
    }
}

impl<T: Num> Add for Jet<T> {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self {
            v: self.v + o.v,
            d: [self.d[0] + o.d[0], self.d[1] + o.d[1]],
            h: [self.h[0] + o.h[0], self.h[1] + o.h[1], self.h[2] + o.h[2]],
        }
    }
}

impl<T: Num> Sub for Jet<T> {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        self + -o
    }
}

impl<T: Num> Neg for Jet<T> {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            v: -self.v,
            d: self.d.map(|x| -x),
            h: self.h.map(|x| -x),
        }
    }
}

impl<T: Num> Mul for Jet<T> {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        let ([a0, a1], [b0, b1]) = (self.d, o.d);
        Self {
            v: self.v * o.v,
            d: [a0 * o.v + self.v * b0, a1 * o.v + self.v * b1],
            h: [
                self.h[0] * o.v + T::of(2.0) * a0 * b0 + self.v * o.h[0],
                self.h[1] * o.v + a0 * b1 + a1 * b0 + self.v * o.h[1],
                self.h[2] * o.v + T::of(2.0) * a1 * b1 + self.v * o.h[2],
            ],
        }
    }
}

/// A vector of jets.
pub(crate) type JVec<T> = [Jet<T>; 3];

pub(crate) fn add<T: Num>(a: JVec<T>, b: JVec<T>) -> JVec<T> {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub<T: Num>(a: JVec<T>, b: JVec<T>) -> JVec<T> {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a` times the jet `k`.
pub(crate) fn times<T: Num>(a: JVec<T>, k: Jet<T>) -> JVec<T> {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn dot<T: Num>(a: JVec<T>, b: JVec<T>) -> Jet<T> {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross<T: Num>(a: JVec<T>, b: JVec<T>) -> JVec<T> {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `a / |a|`; `None` where `a` may vanish.
pub(crate) fn unit<T: Num>(a: JVec<T>) -> Option<JVec<T>> {
    Some(times(a, dot(a, a).sqrt()?.recip()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(s, θ)` as jets at a point.
    fn vars<T: Num>(s: T, t: T) -> (Jet<T>, Jet<T>) {
        let (one, z) = (T::of(1.0), T::of(0.0));
        (Jet::of_s(s, one, z), Jet::of_t(t, one, z))
    }

    /// `√(s² + 3 s θ) / (1 + θ²)`, written over jets.
    fn f<T: Num>(s: Jet<T>, t: Jet<T>) -> Jet<T> {
        let num = (s * s + s * t.scale(3.0)).sqrt().unwrap();
        let den = (Jet::constant(T::of(1.0)) + t * t).recip().unwrap();
        num * den
    }

    #[test]
    fn derivatives_of_a_formula() {
        let (s0, t0) = (1.5, 0.4);
        let (s, t) = vars(s0, t0);
        let j = f(s, t);
        // Central differences of the plain function.
        let g = |s: f64, t: f64| math::sqrt(s * s + 3.0 * s * t) / (1.0 + t * t);
        let e = 1e-4;
        let ds = (g(s0 + e, t0) - g(s0 - e, t0)) / (2.0 * e);
        let dt = (g(s0, t0 + e) - g(s0, t0 - e)) / (2.0 * e);
        let dss = (g(s0 + e, t0) - 2.0 * g(s0, t0) + g(s0 - e, t0)) / (e * e);
        let dtt = (g(s0, t0 + e) - 2.0 * g(s0, t0) + g(s0, t0 - e)) / (e * e);
        let dst = (g(s0 + e, t0 + e) - g(s0 + e, t0 - e) - g(s0 - e, t0 + e) + g(s0 - e, t0 - e))
            / (4.0 * e * e);
        assert!((j.v - g(s0, t0)).abs() < 1e-15);
        assert!(
            (j.d[0] - ds).abs() < 1e-7 && (j.d[1] - dt).abs() < 1e-7,
            "{:?} {ds} {dt}",
            j.d
        );
        assert!((j.h[0] - dss).abs() < 1e-5, "{} {dss}", j.h[0]);
        assert!((j.h[1] - dst).abs() < 1e-5, "{} {dst}", j.h[1]);
        assert!((j.h[2] - dtt).abs() < 1e-5, "{} {dtt}", j.h[2]);
    }

    #[test]
    fn interval_jets_enclose_the_values_over_a_box() {
        let iv = |a: f64, b: f64| Interval::new(a, b).unwrap();
        let (s, t) = vars(iv(1.0, 2.0), iv(0.2, 0.5));
        let j = f(s, t);
        for i in 0..=10 {
            for k in 0..=10 {
                let (s0, t0) = (1.0 + f64::from(i) / 10.0, 0.2 + 0.3 * f64::from(k) / 10.0);
                let (a, b) = vars(s0, t0);
                let x = f(a, b);
                assert!(j.v.contains(x.v));
                assert!(j.d[0].contains(x.d[0]) && j.d[1].contains(x.d[1]));
                for m in 0..3 {
                    assert!(j.h[m].contains(x.h[m]), "{m} at {s0}, {t0}");
                }
            }
        }
    }

    #[test]
    fn vectors() {
        let (s, t) = vars(0.3, 1.1);
        let a = [s, t, Jet::constant(2.0)];
        let b = [t, s * s, s];
        let u = unit(cross(a, b)).unwrap();
        assert!((dot(u, u).v - 1.0).abs() < 1e-15);
        // The derivative of a unit vector is perpendicular to it.
        let along = |k: usize| u.map(|x| x.d[k]);
        for k in 0..2 {
            let d = along(k);
            let n = u.map(|x| x.v);
            assert!((d[0] * n[0] + d[1] * n[1] + d[2] * n[2]).abs() < 1e-15);
        }
        assert_eq!(add(a, b)[0].v, sub(a, times(b, Jet::constant(-1.0)))[0].v);
        // Zero has no direction, and no root with a derivative.
        let zero = [Jet::constant(0.0); 3];
        assert!(unit(zero).is_none());
        assert!(
            Jet::constant(Interval::new(-1.0, 1.0).unwrap())
                .recip()
                .is_none()
        );
    }
}
