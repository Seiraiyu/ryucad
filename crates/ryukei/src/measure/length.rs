//! Arc length with a certified bound.
//!
//! The range is cut into pieces until the unit tangent turns by at most
//! `θ` across each, by the curve's certified [`crate::geom::CurveBound`].
//! A piece's length then lies between its chord `c` and `c / cos θ`: every
//! tangent lies within `θ / 2` of the middle of the arc the tangents
//! sweep on the unit sphere, and so does the chord's direction, a positive
//! combination of them. The value is the Gauss–Legendre sum of `|C'|`,
//! clamped into the certified range, and the bound the farther end of it.

use super::Value;
use super::gauss;
use crate::geom::Curve;
use crate::math;

/// The most pieces a length is cut into.
const MAX_PIECES: usize = 1 << 14;

/// How far each piece may turn before it is cut: past half a radian the
/// bound is too loose to be worth having.
const MAX_TURN: f64 = 0.5;

/// The length of `curve` over `range`: exact for a line or a circle, else
/// certified with the error at most about `rel` times the length.
pub(crate) fn length(curve: &Curve, [t0, t1]: [f64; 2], rel: f64) -> Option<Value> {
    match curve {
        Curve::Line(_) => return Some(Value::Exact(t1 - t0)),
        Curve::Circle(c) => return Some(Value::Exact(c.radius() * (t1 - t0))),
        _ => {}
    }
    let mut pieces = vec![[t0, t1]];
    loop {
        let mut next = Vec::with_capacity(pieces.len() * 2);
        let (mut lo, mut hi, mut value) = (0.0, 0.0, 0.0);
        let mut done = true;
        for &[a, b] in &pieces {
            let c = curve.point(a).distance(curve.point(b));
            let theta = curve.derivative_bound([a, b]).turn * (b - a);
            let excess = if theta < MAX_TURN {
                c * (1.0 / math::cos(theta) - 1.0)
            } else {
                f64::INFINITY
            };
            if excess > rel * c && pieces.len() < MAX_PIECES {
                done = false;
                let m = 0.5 * (a + b);
                next.push([a, m]);
                next.push([m, b]);
            } else {
                next.push([a, b]);
            }
            lo += c;
            hi += c + excess;
            value += gauss::rule(8)
                .iter()
                .map(|&(s, w)| w * curve.eval(a + s * (b - a)).d1.length())
                .sum::<f64>()
                * (b - a);
        }
        if done || pieces.len() >= MAX_PIECES {
            if !hi.is_finite() {
                return None;
            }
            let value = value.clamp(lo, hi);
            return Some(Value::Bounded {
                value,
                bound: (value - lo).max(hi - value),
            });
        }
        pieces = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Ellipse, Frame, Line, Point3, Vector3};
    use core::f64::consts::{PI, TAU};

    #[test]
    fn lines_and_circles_are_exact() {
        let l = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(1.0, 1.0, 0.0)).unwrap());
        assert_eq!(length(&l, [0.5, 2.0], 1e-9), Some(Value::Exact(1.5)));
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let c = Curve::Circle(crate::geom::Circle::new(f, 2.0).unwrap());
        assert_eq!(length(&c, [0.0, PI], 1e-9), Some(Value::Exact(2.0 * PI)));
    }

    #[test]
    fn an_ellipse_is_bounded_about_its_true_length() {
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let e = Curve::Ellipse(Ellipse::new(f, 2.0, 1.0).unwrap());
        // The perimeter of a 2 × 1 ellipse, to 15 digits.
        let want = 9.688_448_220_547_675;
        let v = length(&e, [0.0, TAU], 1e-9).unwrap();
        assert!(v.holds(want, 0.0), "{v:?}");
        assert!((v.value() - want).abs() < 1e-12, "{v:?}");
        assert!(v.bound() < 1e-5 * want, "{v:?}");
        // A quarter is a quarter.
        let q = length(&e, [0.0, PI / 2.0], 1e-9).unwrap();
        assert!((q.value() - want / 4.0).abs() < 1e-12);
    }
}
