//! Trimmed model-space curves as chains of rational Bézier pieces, with
//! the map from each piece's local parameter back to the curve's.
//!
//! A line is one degree 1 piece and a NURBS its own pieces cut to the
//! range, both with a local parameter affine in the curve's. A circle or
//! an ellipse is cut into equal rational quadratic arcs of at most a
//! quarter turn, whose local parameter `s` is the half-angle
//! substitution: `tan((t - m) / 2) = tan(h / 2) (2 s - 1)` on an arc of
//! half-angle `h` about the middle angle `m`.

use super::{BezierCurve, Curve, Frame, Hom, NurbsCurve, Vector3};
use crate::math;
use core::f64::consts::FRAC_PI_2;

/// One polynomial piece of a trimmed curve.
pub(crate) type Piece3 = BezierCurve<Vector3>;

/// A trimmed curve, `t0 < t1`, as Bézier pieces in increasing parameter.
#[derive(Clone, Debug)]
pub(crate) struct Chain3 {
    pub(crate) pieces: Vec<Piece3>,
    /// The pieces are circular or elliptic arcs, measured by angle.
    conic: bool,
}

impl Chain3 {
    /// `curve` over `[t0, t1]`, which must lie in a clamped curve's domain.
    pub(crate) fn new(curve: &Curve, t0: f64, t1: f64) -> Self {
        let (pieces, conic) = match curve {
            Curve::Line(_) => (
                vec![Piece3 {
                    range: [t0, t1],
                    pts: vec![
                        hom(curve.point(t0).to_vector(), 1.0),
                        hom(curve.point(t1).to_vector(), 1.0),
                    ],
                }],
                false,
            ),
            Curve::Circle(c) => (arc(c.frame(), c.radius(), c.radius(), t0, t1), true),
            Curve::Ellipse(e) => (arc(e.frame(), e.major(), e.minor(), t0, t1), true),
            Curve::Nurbs(n) => (nurbs(n, t0, t1), false),
        };
        Self { pieces, conic }
    }

    /// The curve's parameter at local parameter `s` of piece `i`.
    pub(crate) fn param(&self, i: usize, s: f64) -> f64 {
        let [a, b] = self.pieces[i].range;
        if s <= 0.0 {
            return a;
        }
        if s >= 1.0 {
            return b;
        }
        if self.conic {
            let (m, h) = ((a + b) / 2.0, (b - a) / 2.0);
            m + 2.0 * math::atan2(math::tan(h / 2.0) * (2.0 * s - 1.0), 1.0)
        } else {
            a + (b - a) * s
        }
    }

    /// The local parameter of piece `i` at the curve's parameter `t`,
    /// clamped to the piece.
    pub(crate) fn local(&self, i: usize, t: f64) -> f64 {
        let [a, b] = self.pieces[i].range;
        let s = if self.conic {
            let (m, h) = ((a + b) / 2.0, (b - a) / 2.0);
            (math::tan((t - m) / 2.0) / math::tan(h / 2.0) + 1.0) / 2.0
        } else {
            (t - a) / (b - a)
        };
        s.clamp(0.0, 1.0)
    }
}

fn hom(p: Vector3, w: f64) -> Hom<Vector3> {
    Hom { p, w }
}

/// Equal rational quadratic arcs of at most a quarter turn of
/// `origin + a cos t x + b sin t y` over `[t0, t1]`, as in
/// [`super::chain2`].
fn arc(f: &Frame, a: f64, b: f64, t0: f64, t1: f64) -> Vec<Piece3> {
    let at = |t: f64, scale: f64| {
        let (s, c) = math::sin_cos(t);
        f.origin().to_vector() + (f.x().get() * (a * c) + f.y().get() * (b * s)) / scale
    };
    let n = ((t1 - t0) / FRAC_PI_2).ceil().max(1.0);
    let step = (t1 - t0) / n;
    let w = math::cos(step / 2.0);
    let count = n as usize;
    (0..count)
        .map(|i| {
            let start = t0 + step * i as f64;
            let end = if i + 1 == count { t1 } else { start + step };
            Piece3 {
                range: [start, end],
                pts: vec![
                    hom(at(start, 1.0), 1.0),
                    hom(at((start + end) / 2.0, w), w),
                    hom(at(end, 1.0), 1.0),
                ],
            }
        })
        .collect()
}

/// The pieces of a NURBS over `[t0, t1]`: its own Bézier pieces (repeated
/// by the period for a periodic curve) cut to the range.
fn nurbs(n: &NurbsCurve, t0: f64, t1: f64) -> Vec<Piece3> {
    let own = n.pieces();
    let shifts: Vec<f64> = match n.period() {
        Some(p) => {
            let d0 = n.domain()[0];
            let first = ((t0 - d0) / p).floor() as i64 - 1;
            let last = ((t1 - d0) / p).ceil() as i64 + 1;
            (first..=last).map(|m| m as f64 * p).collect()
        }
        None => vec![0.0],
    };
    let mut out = Vec::new();
    for shift in shifts {
        for piece in own {
            let [a, b] = piece.range.map(|t| t + shift);
            let (c, d) = (t0.max(a), t1.min(b));
            if d <= c {
                continue;
            }
            let mut cut = piece.clone();
            let (u0, u1) = ((c - a) / (b - a), (d - a) / (b - a));
            if u1 < 1.0 {
                cut = cut.split(u1)[0].clone();
            }
            if u0 > 0.0 {
                cut = cut.split(u0 / u1)[1].clone();
            }
            cut.range = [c, d];
            out.push(cut);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Ellipse, KnotVector, Line, Point3};
    use core::f64::consts::{PI, TAU};
    use std::sync::Arc;

    /// The point of a piece at local parameter `s`.
    fn point(p: &Piece3, s: f64) -> Vector3 {
        if s <= 0.0 {
            return p.pts[0].p;
        }
        if s >= 1.0 {
            return p.pts[p.pts.len() - 1].p;
        }
        p.split(s)[1].pts[0].p
    }

    /// Every piece is the curve at the mapped parameter, and the map
    /// inverts.
    fn follows(curve: &Curve, t0: f64, t1: f64, count: usize) {
        let c = Chain3::new(curve, t0, t1);
        assert_eq!(c.pieces.len(), count);
        assert_eq!(c.pieces[0].range[0], t0);
        assert_eq!(c.pieces[count - 1].range[1], t1);
        for (i, p) in c.pieces.iter().enumerate() {
            for k in 0..=8 {
                let s = f64::from(k) / 8.0;
                let t = c.param(i, s);
                let q = curve.point(t).to_vector();
                assert!((point(p, s) - q).length() <= 1e-13, "piece {i} at {s}");
                assert!((c.local(i, t) - s).abs() <= 1e-12);
            }
        }
    }

    #[test]
    fn lines_circles_and_ellipses() {
        let line = Curve::Line(
            Line::new(Point3::new(1.0, 2.0, 3.0), Vector3::new(1.0, 1.0, 0.0)).unwrap(),
        );
        follows(&line, -1.0, 2.5, 1);
        let f = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 1.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let circle = Curve::Circle(Circle::new(f, 2.0).unwrap());
        follows(&circle, 0.0, TAU, 4);
        follows(&circle, 1.0, 1.0 + PI / 3.0, 1);
        let ellipse = Curve::Ellipse(Ellipse::new(f, 3.0, 1.0).unwrap());
        follows(&ellipse, -0.5, 4.0, 3);
    }

    #[test]
    fn nurbs_cut_to_the_range() {
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0, 2.5], &[4, 1, 2, 4]).unwrap();
        let pts = (0..7)
            .map(|i| Point3::new(f64::from(i), f64::from(i % 3), f64::from(i % 2)))
            .collect();
        let w = vec![1.0, 0.5, 2.0, 0.8, 1.5, 0.3, 1.0];
        let n = Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, Some(w)).unwrap()));
        follows(&n, 0.2, 1.7, 3);
        follows(&n, 0.0, 2.5, 3);
        let (whole, _) = Circle::new(Frame::WORLD, 1.5).unwrap().to_nurbs().unwrap();
        let periodic = Curve::Nurbs(Arc::new(whole));
        follows(&periodic, 5.0, 5.0 + PI, 3);
    }
}
