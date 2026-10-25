//! Forward-mode dual numbers: a value and its derivatives with respect to
//! `N` inputs. An equation written once over them gives its residual and
//! the exact gradient of the arithmetic that computes it, with no
//! derivative written by hand.

use ryukei::math;

/// A value and its derivatives with respect to `N` inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D<const N: usize> {
    pub v: f64,
    pub g: [f64; N],
}

impl<const N: usize> D<N> {
    pub fn constant(v: f64) -> D<N> {
        D { v, g: [0.0; N] }
    }

    /// Input `slot`, with value `v`.
    pub fn input(v: f64, slot: usize) -> D<N> {
        let mut g = [0.0; N];
        g[slot] = 1.0;
        D { v, g }
    }

    /// The function with value `v` and derivative `dv` at `self`, by the
    /// chain rule.
    pub fn map(self, v: f64, dv: f64) -> D<N> {
        let mut g = self.g;
        for x in &mut g {
            *x *= dv;
        }
        D { v, g }
    }

    pub fn sqrt(self) -> D<N> {
        let v = math::sqrt(self.v);
        // The derivative where the value is zero has no limit; zero keeps
        // a degenerate equation from poisoning the step with infinities.
        let dv = if v > 0.0 { 0.5 / v } else { 0.0 };
        self.map(v, dv)
    }

    pub fn scale(self, k: f64) -> D<N> {
        self.map(self.v * k, k)
    }
}

impl<const N: usize> std::ops::Add for D<N> {
    type Output = D<N>;
    fn add(self, o: D<N>) -> D<N> {
        let mut g = self.g;
        for (x, y) in g.iter_mut().zip(o.g) {
            *x += y;
        }
        D { v: self.v + o.v, g }
    }
}

impl<const N: usize> std::ops::Sub for D<N> {
    type Output = D<N>;
    fn sub(self, o: D<N>) -> D<N> {
        let mut g = self.g;
        for (x, y) in g.iter_mut().zip(o.g) {
            *x -= y;
        }
        D { v: self.v - o.v, g }
    }
}

impl<const N: usize> std::ops::Mul for D<N> {
    type Output = D<N>;
    fn mul(self, o: D<N>) -> D<N> {
        let mut g = [0.0; N];
        for (k, x) in g.iter_mut().enumerate() {
            *x = self.g[k] * o.v + self.v * o.g[k];
        }
        D { v: self.v * o.v, g }
    }
}

impl<const N: usize> std::ops::Div for D<N> {
    type Output = D<N>;
    fn div(self, o: D<N>) -> D<N> {
        let v = self.v / o.v;
        let mut g = [0.0; N];
        for (k, x) in g.iter_mut().enumerate() {
            *x = (self.g[k] - v * o.g[k]) / o.v;
        }
        D { v, g }
    }
}

impl<const N: usize> std::ops::Neg for D<N> {
    type Output = D<N>;
    fn neg(self) -> D<N> {
        self.scale(-1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_quotients_and_roots_differentiate() {
        let (x, y) = (D::<2>::input(3.0, 0), D::<2>::input(2.0, 1));
        let f = x * y / (x + y);
        // d/dx xy/(x+y) = y²/(x+y)², d/dy = x²/(x+y)².
        assert!((f.g[0] - 4.0 / 25.0).abs() < 1e-15);
        assert!((f.g[1] - 9.0 / 25.0).abs() < 1e-15);
        let r = (x * x).sqrt();
        assert_eq!((r.v, r.g), (3.0, [1.0, 0.0]));
        // No derivative at zero, rather than an infinite one.
        assert_eq!(D::<2>::constant(0.0).sqrt().g, [0.0, 0.0]);
        assert_eq!((-x).g, [-1.0, 0.0]);
        assert_eq!((x - y).v, 1.0);
        assert_eq!(x.map(9.0, 6.0).g, [6.0, 0.0]);
    }
}
