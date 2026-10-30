//! Third-order jets in one variable, in floating point or intervals.
//!
//! A [`Jet1`] carries a value with its first three derivatives by one
//! parameter, the path's `s` for a sweep's frame. Arithmetic applies
//! Leibniz's rule and the quotient and root rules to third order, so a
//! formula written once over these jets gives its derivatives up to
//! third order exactly (in `f64`), or enclosures of them over a stretch
//! (over [`Interval`]s whose inputs enclose the values and derivatives at
//! every point of it), as [`super::jet::Jet`] does to second order in two
//! variables.

use super::jet::Num;
use core::ops::{Add, Mul, Neg, Sub};

/// A value and its first three derivatives, `[f, f', f'', f''']`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Jet1<T>(pub(crate) [T; 4]);

impl<T: Num> Jet1<T> {
    pub(crate) fn constant(x: T) -> Self {
        let z = T::of(0.0);
        Self([x, z, z, z])
    }

    /// `f / g` by the quotient rule, solved order by order: `q = f / g`,
    /// `q' = (f' - q g') / g`, `q'' = (f'' - 2 q' g' - q g'') / g`,
    /// `q''' = (f''' - 3 q'' g' - 3 q' g'' - q g''') / g`. `None` where `g`
    /// may be zero.
    pub(crate) fn div(self, g: Self) -> Option<Self> {
        let r = g.0[0].recip()?;
        let [f0, f1, f2, f3] = self.0;
        let [_, g1, g2, g3] = g.0;
        let (two, three) = (T::of(2.0), T::of(3.0));
        let q0 = f0 * r;
        let q1 = (f1 - q0 * g1) * r;
        let q2 = (f2 - two * q1 * g1 - q0 * g2) * r;
        let q3 = (f3 - three * q2 * g1 - three * q1 * g2 - q0 * g3) * r;
        Some(Self([q0, q1, q2, q3]))
    }

    /// `√self`, from `s² = x`: `s' = x' / 2s`, `s'' = (x'' - 2 s'²) / 2s`,
    /// `s''' = (x''' - 6 s' s'') / 2s`. `None` where the value may be zero
    /// or negative.
    pub(crate) fn sqrt(self) -> Option<Self> {
        let s0 = self.0[0].sqrt()?;
        let half = (T::of(2.0) * s0).recip()?;
        let [_, x1, x2, x3] = self.0;
        let s1 = x1 * half;
        let s2 = (x2 - T::of(2.0) * s1 * s1) * half;
        let s3 = (x3 - T::of(6.0) * s1 * s2) * half;
        Some(Self([s0, s1, s2, s3]))
    }
}

impl<T: Num> Add for Jet1<T> {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self(core::array::from_fn(|k| self.0[k] + o.0[k]))
    }
}

impl<T: Num> Sub for Jet1<T> {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self(core::array::from_fn(|k| self.0[k] - o.0[k]))
    }
}

impl<T: Num> Neg for Jet1<T> {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.map(|x| -x))
    }
}

impl<T: Num> Mul for Jet1<T> {
    type Output = Self;
    /// Leibniz's rule to third order.
    fn mul(self, o: Self) -> Self {
        let ([a0, a1, a2, a3], [b0, b1, b2, b3]) = (self.0, o.0);
        let (two, three) = (T::of(2.0), T::of(3.0));
        Self([
            a0 * b0,
            a1 * b0 + a0 * b1,
            a2 * b0 + two * a1 * b1 + a0 * b2,
            a3 * b0 + three * a2 * b1 + three * a1 * b2 + a0 * b3,
        ])
    }
}

/// A vector of jets.
pub(crate) type J1Vec<T> = [Jet1<T>; 3];

/// A vector's value and first three derivatives as jets.
pub(crate) fn of<T: Num>(d: [[T; 3]; 4]) -> J1Vec<T> {
    core::array::from_fn(|k| Jet1([d[0][k], d[1][k], d[2][k], d[3][k]]))
}

/// A constant vector.
pub(crate) fn constant<T: Num>(v: [T; 3]) -> J1Vec<T> {
    v.map(Jet1::constant)
}

pub(crate) fn add<T: Num>(a: J1Vec<T>, b: J1Vec<T>) -> J1Vec<T> {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub<T: Num>(a: J1Vec<T>, b: J1Vec<T>) -> J1Vec<T> {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a` times the jet `k`.
pub(crate) fn times<T: Num>(a: J1Vec<T>, k: Jet1<T>) -> J1Vec<T> {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn dot<T: Num>(a: J1Vec<T>, b: J1Vec<T>) -> Jet1<T> {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross<T: Num>(a: J1Vec<T>, b: J1Vec<T>) -> J1Vec<T> {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `a / |a|`; `None` where `a` may vanish.
pub(crate) fn unit<T: Num>(a: J1Vec<T>) -> Option<J1Vec<T>> {
    let len = dot(a, a).sqrt()?;
    Some([a[0].div(len)?, a[1].div(len)?, a[2].div(len)?])
}

/// The `k`th derivative's vector.
pub(crate) fn order<T: Num>(a: &J1Vec<T>, k: usize) -> [T; 3] {
    [a[0].0[k], a[1].0[k], a[2].0[k]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::Interval;
    use crate::math;

    /// `s` as a jet.
    fn var<T: Num>(s: T) -> Jet1<T> {
        Jet1([s, T::of(1.0), T::of(0.0), T::of(0.0)])
    }

    /// `√(s² + 1) / (2 + s)`, written over jets.
    fn f<T: Num>(s: Jet1<T>) -> Jet1<T> {
        let one = Jet1::constant(T::of(1.0));
        let two = Jet1::constant(T::of(2.0));
        (s * s + one).sqrt().unwrap().div(two + s).unwrap()
    }

    #[test]
    fn derivatives_of_a_formula() {
        let s0 = 0.7;
        let j = f(var(s0));
        let g = |s: f64| math::sqrt(s * s + 1.0) / (2.0 + s);
        // Central differences, each order from five points.
        let e = 1e-3;
        let [m2, m1, p0, p1, p2] = [-2.0, -1.0, 0.0, 1.0, 2.0].map(|k| g(s0 + k * e));
        let d1 = (p1 - m1) / (2.0 * e);
        let d2 = (p1 - 2.0 * p0 + m1) / (e * e);
        let d3 = (p2 - 2.0 * p1 + 2.0 * m1 - m2) / (2.0 * e * e * e);
        assert!((j.0[0] - p0).abs() < 1e-15);
        assert!((j.0[1] - d1).abs() < 1e-6, "{} {d1}", j.0[1]);
        assert!((j.0[2] - d2).abs() < 1e-5, "{} {d2}", j.0[2]);
        assert!((j.0[3] - d3).abs() < 1e-4, "{} {d3}", j.0[3]);
    }

    #[test]
    fn interval_jets_enclose_the_values_over_a_stretch() {
        let j = f(var(Interval::new(0.5, 1.5).unwrap()));
        for i in 0..=20 {
            let s = 0.5 + f64::from(i) / 20.0;
            let x = f(var(s));
            for k in 0..4 {
                assert!(j.0[k].contains(x.0[k]), "order {k} at {s}");
            }
        }
    }

    #[test]
    fn a_unit_vectors_derivative_is_perpendicular_to_it() {
        let s = var(0.4);
        let a = [s, s * s, Jet1::constant(1.0)];
        let u = unit(a).unwrap();
        let n = order(&u, 0);
        let d = order(&u, 1);
        assert!((n[0] * n[0] + n[1] * n[1] + n[2] * n[2] - 1.0).abs() < 1e-15);
        assert!((n[0] * d[0] + n[1] * d[1] + n[2] * d[2]).abs() < 1e-15);
        assert!(unit(constant([0.0; 3])).is_none());
        let c = cross(a, times(a, Jet1::constant(2.0)));
        assert_eq!(order(&c, 3), [0.0; 3]);
        assert_eq!(order(&add(a, sub(a, a)), 2), order(&a, 2));
    }
}
