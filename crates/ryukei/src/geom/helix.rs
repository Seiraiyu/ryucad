//! Helices: a point turning about an axis as it climbs it, at a radius
//! that grows linearly with height (a taper), and their NURBS forms
//! within a certified tolerance. A helix has no exact NURBS form, so it
//! is a curve to ryukei only as one of these: cubic Hermite spans in its
//! angle, whose values and first derivatives are the helix's own in
//! closed form, each span halved until Hermite's error bound is within
//! the tolerance asked for.

use crate::geom::{Frame, KnotVector, NurbsCurve, Point3, Vector3};
use crate::{Error, math};
use core::f64::consts::TAU;

/// Which way a helix turns as it climbs its axis: counterclockwise about
/// it seen from above (the right-hand rule), or clockwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Right,
    Left,
}

/// A helix about the `z` axis of `frame`: it starts at `frame`'s `x` at
/// `radius` from the axis, at the origin's height, and climbs `pitch`
/// along `z` each turn for `turns` turns (any positive number), its radius
/// growing by `tan(taper)` for each unit it climbs (shrinking where the
/// taper is negative). At angle `θ` from 0 to `2π turns` it is at
/// `origin + r(θ) (cos φ x + sin φ y) + (pitch θ / 2π) z`, where
/// `r(θ) = radius + (pitch θ / 2π) tan(taper)` and `φ` is `θ` for a
/// right hand and `-θ` for a left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Helix {
    pub frame: Frame,
    pub radius: f64,
    pub pitch: f64,
    pub taper: f64,
    pub turns: f64,
    pub hand: Hand,
}

/// Spans' halvings, at most: a span's bound falls 16-fold with each.
const MAX_DEPTH: u32 = 40;

/// Spans, at most: 10,000 turns of a helix a metre across at a tenth of
/// a micron take about 400,000.
const MAX_SPANS: usize = 4_000_000;

impl Helix {
    /// The angle at its end: `2π turns`.
    pub fn end(&self) -> f64 {
        TAU * self.turns
    }

    /// How far it climbs: `pitch × turns`.
    pub fn height(&self) -> f64 {
        self.pitch * self.turns
    }

    /// Its radius after turning `theta`.
    pub fn radius_at(&self, theta: f64) -> f64 {
        self.radius + self.rise(theta) * math::tan(self.taper)
    }

    /// How far it has climbed after turning `theta`.
    fn rise(&self, theta: f64) -> f64 {
        self.pitch * theta / TAU
    }

    /// How fast the radius grows per radian turned.
    fn spread(&self) -> f64 {
        self.pitch / TAU * math::tan(self.taper)
    }

    fn sign(&self) -> f64 {
        match self.hand {
            Hand::Right => 1.0,
            Hand::Left => -1.0,
        }
    }

    /// The point after turning `theta`, in closed form.
    pub fn point(&self, theta: f64) -> Point3 {
        let (s, c) = math::sin_cos(theta);
        let r = self.radius_at(theta);
        let f = &self.frame;
        f.origin()
            + f.x().get() * (r * c)
            + f.y().get() * (r * self.sign() * s)
            + f.z().get() * self.rise(theta)
    }

    /// Its first derivative in `theta`, in closed form.
    pub fn derivative(&self, theta: f64) -> Vector3 {
        let (s, c) = math::sin_cos(theta);
        let (r, k) = (self.radius_at(theta), self.spread());
        let f = &self.frame;
        f.x().get() * (k * c - r * s)
            + f.y().get() * (self.sign() * (k * s + r * c))
            + f.z().get() * (self.pitch / TAU)
    }

    /// Checks its numbers: finite, a positive radius, pitch and number of
    /// turns, a taper under a right angle either way, and a radius still
    /// positive at its end. `NonFinite` or `Degenerate` otherwise.
    fn check(&self) -> Result<(), Error> {
        let all = [self.radius, self.pitch, self.taper, self.turns];
        if !all.iter().all(|x| x.is_finite()) {
            return Err(Error::NonFinite);
        }
        let flat = self.taper.abs() >= core::f64::consts::FRAC_PI_2;
        if self.radius <= 0.0 || self.pitch <= 0.0 || self.turns <= 0.0 || flat {
            return Err(Error::Degenerate);
        }
        // The radius is linear in the angle: positive at both ends is
        // positive all along.
        if self.radius_at(self.end()) <= 0.0 {
            return Err(Error::Degenerate);
        }
        Ok(())
    }

    /// Hermite's error bound for the cubic span over `[a, b]`:
    /// `h⁴/384 · max|C⁽⁴⁾|`, where `|C⁽⁴⁾| = sqrt(r² + 16 k²)` (the axial
    /// part is linear and drops out; `k` is the radius's growth per
    /// radian) and `r` is largest at an end, since it is linear. Two-point
    /// Hermite interpolation's Peano kernel keeps one sign, so the bound
    /// holds for the vector, not only per coordinate. A few ulps of the
    /// points' size are added for rounding.
    fn span_bound(&self, a: f64, b: f64) -> f64 {
        let h = b - a;
        let r = self.radius_at(a).abs().max(self.radius_at(b).abs());
        let k = self.spread();
        let d4 = (r * r + 16.0 * k * k).sqrt();
        let o = self.frame.origin();
        let size = o.x.abs().max(o.y.abs()).max(o.z.abs()) + r + self.rise(b).abs();
        let h2 = h * h;
        h2 * h2 / 384.0 * d4 + 8.0 * f64::EPSILON * size
    }

    /// Its NURBS form within `tolerance` of it, and the bound certified:
    /// cubic Hermite spans in the angle `θ` (the curve's parameter, from 0
    /// to [`Helix::end`]), a quarter turn each to start, each halved until
    /// its bound is within the tolerance; knots of multiplicity three at
    /// the spans' ends, so the values and first derivatives there are the
    /// helix's own, its ends exactly. Fails with `NonFinite` or
    /// `Degenerate` for numbers [`Helix`] can't take, or a tolerance that
    /// isn't positive or that rounding alone exceeds; `OutsideSizeBox`
    /// where it leaves the size box.
    pub fn to_nurbs(&self, tolerance: f64) -> Result<(NurbsCurve, f64), Error> {
        self.check()?;
        if !(tolerance.is_finite() && tolerance > 0.0) {
            return Err(Error::Degenerate);
        }
        let end = self.end();
        let quarters = (self.turns * 4.0).ceil().max(1.0);
        if quarters > MAX_SPANS as f64 {
            return Err(Error::Degenerate);
        }
        let n = quarters as usize;
        let at = |i: usize| {
            if i == n {
                end
            } else {
                end * (i as f64) / (n as f64)
            }
        };
        // Each quarter halved as it needs, its spans' ends in order.
        let mut cuts = vec![0.0];
        let mut bound: f64 = 0.0;
        for i in 0..n {
            let mut stack = vec![(at(i), at(i + 1), 0u32)];
            while let Some((a, b, depth)) = stack.pop() {
                let e = self.span_bound(a, b);
                if e <= tolerance {
                    cuts.push(b);
                    bound = bound.max(e);
                    continue;
                }
                let m = 0.5 * (a + b);
                if depth >= MAX_DEPTH || !(a < m && m < b) || cuts.len() + stack.len() > MAX_SPANS {
                    return Err(Error::Degenerate);
                }
                // The right half waits under the left, so cuts come out
                // in order.
                stack.push((m, b, depth + 1));
                stack.push((a, m, depth + 1));
            }
        }
        let mut points = Vec::with_capacity(3 * (cuts.len() - 1) + 1);
        points.push(self.point(0.0));
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let h = b - a;
            let (pb, db) = (self.point(b), self.derivative(b));
            let (pa, da) = (*points.last().expect("a start"), self.derivative(a));
            points.push(pa + da * (h / 3.0));
            points.push(pb - db * (h / 3.0));
            points.push(pb);
        }
        let mut mults = vec![3; cuts.len()];
        mults[0] = 4;
        *mults.last_mut().expect("two cuts") = 4;
        let knots = KnotVector::clamped(3, &cuts, &mults)?;
        Ok((NurbsCurve::new(knots, points, None)?, bound))
    }
}

#[cfg(test)]
mod tests;
