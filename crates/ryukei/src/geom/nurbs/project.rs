//! Global closest-point projection onto NURBS curves.
//!
//! The nearest point of a curve is a local minimum of the distance: a root
//! of `f = (C - p)·C'` where `f` goes from negative to positive, or a
//! domain end. On a Bézier piece with homogeneous form `C = A / W`,
//! `W³ f = (A - pW)·(A'W - AW')` is a polynomial whose Bernstein
//! coefficients are computed directly. Its sign follows them
//! (variation diminishing): a piece whose coefficients all have one sign
//! holds no root, and one whose coefficients change sign once holds exactly
//! one. Pieces are split until each root is isolated, and each is then
//! solved by Newton's method kept inside its bracket by bisection.

use super::KnotVector;
use super::bezier::BezierCurve;
use super::eval::{Coords, Spline};
use super::refine::Hom;
use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use core::ops::{Mul, Sub};

/// The deepest split while isolating the roots of one piece.
const MAX_SPLITS: u32 = 48;

/// Newton or bisection steps per root.
const MAX_STEPS: u32 = 64;

/// Below this fraction of the largest product term a coefficient counts
/// as zero: rounding can't decide its sign.
const ZERO: f64 = 1e-12;

/// `C(n, k)` in f64, by the multiplicative formula (exact while the
/// intermediate products are, far beyond degree 25).
pub(crate) fn binom(n: usize, k: usize) -> f64 {
    let k = k.min(n - k);
    (0..k).fold(1.0, |r, i| r * (n - i) as f64 / (i + 1) as f64)
}

/// The Bernstein coefficients of a polynomial's derivative.
pub(crate) fn deriv<T: Copy + Sub<Output = T> + Mul<f64, Output = T>>(c: &[T]) -> Vec<T> {
    let n = (c.len() - 1) as f64;
    c.windows(2).map(|w| (w[1] - w[0]) * n).collect()
}

/// The product of a vector-valued and a scalar Bernstein polynomial, and
/// the size of its largest term.
pub(crate) fn mul_by<V: Coords>(a: &[V], w: &[f64]) -> (Vec<V>, f64) {
    let (m, n) = (a.len() - 1, w.len() - 1);
    let mut c = vec![V::ZERO; m + n + 1];
    let mut big: f64 = 0.0;
    for (i, &ai) in a.iter().enumerate() {
        for (j, &wj) in w.iter().enumerate() {
            let k = binom(m, i) * binom(n, j) / binom(m + n, i + j);
            c[i + j] = c[i + j] + ai * (wj * k);
            big = big.max(math::sqrt(ai.dot(ai)) * wj * k);
        }
    }
    (c, big)
}

/// The dot product of two vector-valued Bernstein polynomials.
pub(crate) fn mul_dot<V: Coords>(a: &[V], b: &[V]) -> Vec<f64> {
    let (m, n) = (a.len() - 1, b.len() - 1);
    let mut c = vec![0.0; m + n + 1];
    for (i, &ai) in a.iter().enumerate() {
        for (j, &bj) in b.iter().enumerate() {
            let k = binom(m, i) * binom(n, j) / binom(m + n, i + j);
            c[i + j] += ai.dot(bj) * k;
        }
    }
    c
}

/// A scalar Bernstein polynomial split at `s` by de Casteljau.
pub(crate) fn split_scalar(c: &[f64], s: f64) -> (Vec<f64>, Vec<f64>) {
    let n = c.len();
    let mut q = c.to_vec();
    let mut left = vec![q[0]];
    let mut right = vec![q[n - 1]];
    for k in 1..n {
        for i in 0..n - k {
            q[i] = q[i] * (1.0 - s) + q[i + 1] * s;
        }
        left.push(q[0]);
        right.push(q[n - 1 - k]);
    }
    right.reverse();
    (left, right)
}

/// How a piece's coefficients bear on its roots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signs {
    /// Every coefficient is zero: the function vanishes on the piece.
    Zero,
    /// One strict sign throughout: no root.
    Excluded,
    /// The ends have opposite strict signs and the coefficients change sign
    /// once: exactly one root. `rising` when it goes from negative to
    /// positive.
    One { rising: bool },
    /// At most one sign change, with a zero at an end. Any root at the
    /// zero end is the boundary's business; inside there is exactly one
    /// root, `rising`, when the first coefficient rounding can sign is
    /// negative and the last positive, and none otherwise.
    Edge { rising: bool },
    /// Anything else.
    Unknown,
}

/// The signs of a polynomial's coefficients, in one pass.
pub(crate) fn signs(c: impl IntoIterator<Item = f64>, zero: f64) -> Signs {
    let (mut all_pos, mut all_neg, mut nonzero) = (true, true, false);
    let (mut first, mut last, mut prev, mut changes) = (None, 0i8, 0i8, 0);
    let mut first_sig = 0i8;
    for x in c {
        let s: i8 = if x > zero {
            1
        } else if x < -zero {
            -1
        } else {
            0
        };
        first.get_or_insert(s);
        last = s;
        all_pos &= s == 1;
        all_neg &= s == -1;
        if s != 0 {
            nonzero = true;
            if first_sig == 0 {
                first_sig = s;
            }
            if prev != 0 && prev != s {
                changes += 1;
            }
            prev = s;
        }
    }
    let first = first.unwrap_or(0);
    if !nonzero {
        Signs::Zero
    } else if all_pos || all_neg {
        Signs::Excluded
    } else if changes == 1 && first != 0 && last != 0 {
        Signs::One { rising: first < 0 }
    } else if changes <= 1 && (first == 0 || last == 0) {
        Signs::Edge {
            rising: first_sig < 0 && prev > 0,
        }
    } else {
        Signs::Unknown
    }
}

/// The sign of the first coefficient rounding can sign, or 0 if none.
fn lead(c: impl IntoIterator<Item = f64>, zero: f64) -> i8 {
    c.into_iter()
        .find(|x| x.abs() > zero)
        .map_or(0, |x| if x > 0.0 { 1 } else { -1 })
}

/// The Bernstein coefficients of `(A - pW)·(A'W - AW')` on a piece, and
/// the size below which a coefficient counts as zero. Writing the second
/// factor with `A - pW` in place of `A` gives the same polynomial with
/// smaller terms. The threshold scales with the terms before they cancel:
/// where the curve stands still (a collapsed control polygon), `A'W - AW'`
/// is rounding noise however small it is.
fn curve_poly<V: Coords>(pts: &[Hom<V>], p: V) -> (Vec<f64>, f64) {
    let a: Vec<V> = pts.iter().map(|h| (h.p - p) * h.w).collect();
    let w: Vec<f64> = pts.iter().map(|h| h.w).collect();
    let (da, big_da) = mul_by(&deriv(&a), &w);
    let (dw, big_dw) = mul_by(&a, &deriv(&w));
    let b: Vec<V> = da.iter().zip(&dw).map(|(&x, &y)| x - y).collect();
    let size = a.iter().map(|&x| math::sqrt(x.dot(x))).fold(0.0, f64::max);
    (mul_dot(&a, &b), ZERO * size * big_da.max(big_dw))
}

/// The knots where a spline may have a kink: interior knots of
/// multiplicity `degree` and, on a periodic spline, the seam if its
/// multiplicity is `degree`.
pub(crate) fn creases(knots: &KnotVector) -> Vec<f64> {
    let (k, m, p) = (knots.knots(), knots.mults(), knots.degree());
    let r = k.len() - 1;
    let seam = (knots.is_periodic() && m[0] == p).then_some(k[0]);
    seam.into_iter()
        .chain((1..r).filter(|&i| m[i] == p).map(|i| k[i]))
        .collect()
}

/// A periodic parameter in `[t0, t1)`: unchanged when it already is, so a
/// knot stays bit for bit itself. Clamped parameters are unchanged.
pub(crate) fn in_period(knots: &KnotVector, t: f64) -> f64 {
    let [t0, t1] = knots.domain();
    if !knots.is_periodic() || (t0..t1).contains(&t) {
        return t;
    }
    let t = knots.wrap(t);
    if t >= t1 { t0 } else { t }
}

/// `feet`, sorted by parameter, without any foot whose point is within
/// [`LINEAR_RESOLUTION`] of an earlier one: the same point found twice,
/// even from either side of a periodic seam.
pub(crate) fn distinct<T, V: Coords>(feet: Vec<Foot<T, V>>) -> Vec<Foot<T, V>> {
    let mut kept: Vec<Foot<T, V>> = Vec::with_capacity(feet.len());
    for foot in feet {
        let same = |k: &Foot<T, V>| {
            let d = foot.1 - k.1;
            d.dot(d) <= LINEAR_RESOLUTION * LINEAR_RESOLUTION
        };
        if !kept.iter().any(same) {
            kept.push(foot);
        }
    }
    kept
}

/// A candidate nearest point: parameter, point, distance.
pub(crate) type Foot<T, V> = (T, V, f64);

impl<V: Coords> Spline<V> {
    /// `f = (C - p)·C'` and its derivative at `t`, with the point.
    fn stationary_at(&self, p: V, t: f64) -> (f64, f64, V) {
        let [c, d1, d2] = self.eval(t);
        let r = c - p;
        (r.dot(d1), d1.dot(d1) + r.dot(d2), c)
    }

    /// The root of `f` in `[lo, hi]`, where `f` rises through zero (or,
    /// with `rising` false, falls through it): Newton's method, falling back to bisection whenever a step would
    /// leave the open bracket. The ends themselves are never evaluated: at
    /// a kink or a periodic seam the curve's derivative there is the other
    /// piece's. Stops when a step moves the point by less than a sixteenth
    /// of [`LINEAR_RESOLUTION`] (the point, not the parameter times the
    /// speed, since the speed can vanish where a curve folds back on
    /// itself), or when Newton's step doesn't move at all.
    fn solve(&self, p: V, mut lo: f64, mut hi: f64, rising: bool) -> f64 {
        let sign = if rising { 1.0 } else { -1.0 };
        let mut t = (lo + hi) / 2.0;
        let mut last: Option<V> = None;
        for _ in 0..MAX_STEPS {
            let (f, df, c) = self.stationary_at(p, t);
            let (f, df) = (f * sign, df * sign);
            if let Some(q) = last
                && math::sqrt((c - q).dot(c - q)) < LINEAR_RESOLUTION / 16.0
            {
                return t;
            }
            if f < 0.0 {
                lo = t;
            } else if f > 0.0 {
                hi = t;
            } else {
                return t;
            }
            let newton = t - f / df;
            let next = if df > 0.0 && (newton > lo && newton < hi || newton == t) {
                newton
            } else {
                (lo + hi) / 2.0
            };
            if next == t {
                return t;
            }
            last = Some(c);
            t = next;
        }
        t
    }

    /// The local minima of `f`'s piece polynomial `g` on `[a, b]`, pushed
    /// onto `out`; with `all`, its local maxima too.
    ///
    /// Only a whole piece can be found constant. After splits, coefficients
    /// near a root shrink with the interval until rounding can't sign them;
    /// such an interval, like one at the deepest split, is decided by `f`
    /// itself at its ends.
    fn isolate(
        &self,
        p: V,
        [a, b]: [f64; 2],
        (g, zero): (&[f64], f64),
        depth: u32,
        all: bool,
        out: &mut Vec<f64>,
    ) {
        let leaf = |out: &mut Vec<f64>| {
            let (fa, ..) = self.stationary_at(p, a);
            let (fb, ..) = self.stationary_at(p, b);
            if fa <= 0.0 && fb >= 0.0 {
                out.push(self.solve(p, a, b, true));
            } else if all && fa >= 0.0 && fb <= 0.0 {
                out.push(self.solve(p, a, b, false));
            }
        };
        match signs(g.iter().copied(), zero) {
            Signs::Excluded => {}
            // The distance is constant: every point is a minimum.
            Signs::Zero if depth == 0 => out.push(a),
            Signs::Zero => leaf(out),
            Signs::Edge { rising: true } | Signs::One { rising: true } => {
                out.push(self.solve(p, a, b, true))
            }
            // One falling root; an `Edge` that isn't rising may hold one
            // or none.
            Signs::One { rising: false } if all => out.push(self.solve(p, a, b, false)),
            Signs::Edge { rising: false }
                if all
                    && lead(g.iter().copied(), zero) > 0
                    && lead(g.iter().rev().copied(), zero) < 0 =>
            {
                out.push(self.solve(p, a, b, false))
            }
            Signs::Edge { .. } | Signs::One { .. } => {}
            Signs::Unknown if depth >= MAX_SPLITS => leaf(out),
            Signs::Unknown => {
                let (l, r) = split_scalar(g, 0.5);
                let m = a + (b - a) * 0.5;
                self.isolate(p, [a, m], (&l, zero), depth + 1, all, out);
                // A root on the split itself is no root of either half: it
                // shows as a zero at their shared end.
                let (before, after) = (
                    lead(l.iter().rev().copied(), zero),
                    lead(r.iter().copied(), zero),
                );
                if l[l.len() - 1].abs() <= zero
                    && ((before < 0 && after > 0) || (all && before > 0 && after < 0))
                {
                    out.push(m);
                }
                self.isolate(p, [m, b], (&r, zero), depth + 1, all, out);
            }
        }
    }

    /// Every parameter where the distance from `p` is stationary, sorted:
    /// the roots of `f` on every piece, minima and maxima alike, and the
    /// knots where `f` changes sign (a root there, or a kink), with
    /// duplicates (points within [`LINEAR_RESOLUTION`]) removed. A clamped
    /// curve's domain ends are not included unless they are such points.
    pub(crate) fn stationary(&self, pieces: &[BezierCurve<V>], p: V) -> Vec<f64> {
        let mut ts = Vec::new();
        let mut ends = Vec::with_capacity(pieces.len());
        for piece in pieces {
            let (g, zero) = curve_poly(&piece.pts, p);
            self.isolate(p, piece.range, (&g, zero), 0, true, &mut ts);
            ends.push((
                lead(g.iter().copied(), zero),
                lead(g.iter().rev().copied(), zero),
            ));
        }
        let n = pieces.len();
        for i in 0..n {
            let j = if i + 1 < n {
                i + 1
            } else if self.knots.is_periodic() {
                0
            } else {
                continue;
            };
            let (before, after) = (ends[i].1, ends[j].0);
            if before != 0 && after != 0 && before != after {
                ts.push(pieces[i].range[1]);
            }
        }
        let mut feet: Vec<Foot<f64, V>> = ts
            .into_iter()
            .map(|t| {
                let t = in_period(&self.knots, t);
                (t, self.eval(t)[0], 0.0)
            })
            .collect();
        feet.sort_by(|x, y| x.0.total_cmp(&y.0));
        distinct(feet).into_iter().map(|f| f.0).collect()
    }

    /// The local minima of the distance from `p`, sorted by parameter, with
    /// duplicates (points within [`LINEAR_RESOLUTION`]) removed. With
    /// `prune`, pieces whose hull box is farther than the best point so far
    /// plus [`LINEAR_RESOLUTION`] are skipped: only minima that can tie
    /// for nearest are kept.
    pub(crate) fn minima(&self, pieces: &[BezierCurve<V>], p: V, prune: bool) -> Vec<Foot<f64, V>> {
        let mut ts = Vec::new();
        let dist = |q: V| math::sqrt((q - p).dot(q - p));
        let [t0, t1] = self.knots.domain();
        // The pieces' ends are points of the curve: the nearest bounds the
        // search from the start.
        let mut bound = pieces
            .iter()
            .flat_map(|x| [x.pts[0].p, x.pts[x.pts.len() - 1].p])
            .map(dist)
            .fold(f64::INFINITY, f64::min);
        // Each piece's polynomial and zero threshold; `None` where pruned.
        let mut polys = Vec::with_capacity(pieces.len());
        for piece in pieces {
            if prune {
                let (lo, hi) = piece.hull();
                if dist(p.max(lo).min(hi)) > bound + LINEAR_RESOLUTION {
                    polys.push(None);
                    continue;
                }
            }
            let (g, zero) = curve_poly(&piece.pts, p);
            let before = ts.len();
            self.isolate(p, piece.range, (&g, zero), 0, false, &mut ts);
            for &t in &ts[before..] {
                bound = bound.min(dist(self.eval(t)[0]));
            }
            polys.push(Some((g, zero)));
        }
        // A minimum can also sit on a piece boundary: where `f` turns from
        // negative to positive at the knot itself (a root there, or a kink
        // where `f` jumps), or at an end of a clamped domain where the curve
        // moves away from `p`. Each side's sign is that of the polynomial's
        // coefficient at that end, or of the nearest one rounding can sign:
        // where `C'` vanishes, `f` itself is only rounding there.
        let side = |i: usize, from_end: bool| {
            polys[i].as_ref().map(|(g, zero): &(Vec<f64>, f64)| {
                if from_end {
                    lead(g.iter().rev().copied(), *zero)
                } else {
                    lead(g.iter().copied(), *zero)
                }
            })
        };
        let n = pieces.len();
        let periodic = self.knots.is_periodic();
        for (i, piece) in pieces.iter().enumerate() {
            let next = if i + 1 < n {
                Some(i + 1)
            } else {
                periodic.then_some(0)
            };
            if let Some(j) = next
                && side(i, true) == Some(-1)
                && side(j, false) == Some(1)
            {
                ts.push(piece.range[1]);
            }
        }
        if !periodic {
            if side(0, false) == Some(1) {
                ts.push(t0);
            }
            if side(n - 1, true) == Some(-1) {
                ts.push(t1);
            }
        }
        if ts.is_empty() {
            ts.push(t0);
        }
        let mut feet: Vec<Foot<f64, V>> = ts
            .into_iter()
            .map(|t| {
                let t = in_period(&self.knots, t);
                let c = self.eval(t)[0];
                (t, c, dist(c))
            })
            .collect();
        feet.sort_by(|x, y| x.0.total_cmp(&y.0));
        distinct(feet)
    }
}

/// The nearest of `feet`: of those within [`LINEAR_RESOLUTION`] of the
/// least distance, the first (they are sorted by parameter).
pub(crate) fn nearest<T: Copy, V: Copy>(feet: &[Foot<T, V>]) -> Foot<T, V> {
    let least = feet.iter().map(|f| f.2).fold(f64::INFINITY, f64::min);
    *feet
        .iter()
        .find(|f| f.2 <= least + LINEAR_RESOLUTION)
        .unwrap_or(&feet[0])
}

#[cfg(test)]
mod tests {
    use crate::geom::{Circle, Frame, KnotVector, NurbsCurve, Point3};
    use crate::tol::LINEAR_RESOLUTION;
    use core::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, PI, TAU};

    fn quarter() -> NurbsCurve {
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let p = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ];
        NurbsCurve::new(k, p, Some(vec![1.0, FRAC_1_SQRT_2, 1.0])).unwrap()
    }

    fn wavy() -> NurbsCurve {
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0, 2.5], &[4, 1, 2, 4]).unwrap();
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 3.0, -1.0),
            Point3::new(2.0, -1.0, 0.5),
            Point3::new(4.0, 2.0, 2.0),
            Point3::new(5.0, 0.0, 1.0),
            Point3::new(6.0, 1.0, -2.0),
            Point3::new(8.0, 0.0, 0.0),
        ];
        let w = vec![1.0, 0.5, 2.0, 0.8, 1.5, 0.3, 1.0];
        NurbsCurve::new(k, pts, Some(w)).unwrap()
    }

    /// The nearest of `n + 1` evenly spaced samples: an upper bound on the
    /// distance, and a starting guess for its parameter.
    fn sampled(c: &NurbsCurve, p: Point3, n: u32) -> (f64, f64) {
        let [a, b] = c.domain();
        (0..=n)
            .map(|i| {
                let t = a + (b - a) * f64::from(i) / f64::from(n);
                (c.point(t).distance(p), t)
            })
            .fold((f64::INFINITY, a), |x, y| if y.0 < x.0 { y } else { x })
    }

    #[test]
    fn symmetric_points_on_a_quarter_circle() {
        let c = quarter();
        let foot = c.project(Point3::new(2.0, 2.0, 0.0));
        assert!((foot.param - 0.5).abs() <= 1e-12);
        assert!((foot.distance - (8.0f64.sqrt() - 1.0)).abs() <= 1e-15);
        assert_eq!(foot.point, c.point(foot.param));
        let foot = c.project(Point3::new(0.1, 0.1, 0.0));
        assert!((foot.distance - (1.0 - 0.02f64.sqrt())).abs() <= 1e-15);
        assert_eq!(c.distance(Point3::new(0.1, 0.1, 0.0)), foot.distance);
    }

    #[test]
    fn beyond_an_end() {
        let c = quarter();
        let foot = c.project(Point3::new(3.0, -1.0, 0.0));
        assert_eq!(foot.param, 0.0);
        assert_eq!(c.local_minima(Point3::new(3.0, -1.0, 0.0)).len(), 1);
        let foot = c.project(Point3::new(-1.0, 5.0, 1.0));
        assert_eq!(foot.param, 1.0);
    }

    #[test]
    fn never_worse_than_dense_sampling() {
        let c = wavy();
        for p in [
            Point3::new(3.0, 1.0, 0.0),
            Point3::new(-2.0, 4.0, 1.0),
            Point3::new(4.5, 0.5, 1.5),
            Point3::new(9.0, -3.0, 0.0),
            Point3::new(2.0, 0.6, -0.4),
        ] {
            let foot = c.project(p);
            let (d, _) = sampled(&c, p, 20_000);
            assert!(foot.distance <= d + 1e-12, "{p:?}: {} > {d}", foot.distance);
            assert!(foot.distance >= d - 1e-3, "{p:?}");
            assert!(c.point(foot.param).distance(foot.point) == 0.0);
            assert!((foot.point.distance(p) - foot.distance).abs() <= 1e-15);
        }
    }

    #[test]
    fn every_local_minimum() {
        let c = wavy();
        let p = Point3::new(3.0, 1.0, 0.0);
        let minima = c.local_minima(p);
        assert!(minima.windows(2).all(|w| w[0].param < w[1].param));
        // Each sampled local minimum of the distance has a projection
        // within a sample step of it.
        let [a, b] = c.domain();
        let n = 20_000;
        let d = |i: u32| {
            c.point(a + (b - a) * f64::from(i) / f64::from(n))
                .distance(p)
        };
        for i in 1..n {
            if d(i) < d(i - 1) && d(i) < d(i + 1) {
                let t = a + (b - a) * f64::from(i) / f64::from(n);
                let near = minima
                    .iter()
                    .any(|m| (m.param - t).abs() <= 2.0 * (b - a) / f64::from(n));
                assert!(near, "sampled minimum at {t} missing from {minima:?}");
            }
        }
        // And the global one is among them.
        let foot = c.project(p);
        assert!(minima.iter().any(|m| m.param == foot.param));
    }

    #[test]
    fn every_stationary_point() {
        let c = wavy();
        let [a, b] = c.domain();
        let n = 20_000;
        for p in [
            Point3::new(3.0, 1.0, 0.0),
            Point3::new(4.0, 0.7, 0.9),
            Point3::new(-1.0, 2.0, 0.0),
        ] {
            let ts = c.stationary(p);
            assert!(ts.windows(2).all(|w| w[0] < w[1]));
            let d = |i: u32| {
                c.point(a + (b - a) * f64::from(i) / f64::from(n))
                    .distance(p)
            };
            for i in 1..n {
                let (x, y, z) = (d(i - 1), d(i), d(i + 1));
                if (y < x && y < z) || (y > x && y > z) {
                    let t = a + (b - a) * f64::from(i) / f64::from(n);
                    let near = ts
                        .iter()
                        .any(|&s| (s - t).abs() <= 2.0 * (b - a) / f64::from(n));
                    assert!(near, "sampled extremum at {t} missing from {ts:?}");
                }
            }
            // Every one is a root of (C - p)·C'.
            for &t in &ts {
                let e = c.eval(t);
                let f = (e.point - p).dot(e.d1);
                assert!(f.abs() <= 1e-6 * (e.point - p).length() * e.d1.length());
            }
        }
        // A full circle about a point inside it, off centre: one minimum
        // and one maximum.
        let circle = Circle::new(Frame::WORLD, 2.0).unwrap();
        let (n, _) = circle.to_nurbs().unwrap();
        let ts = n.stationary(Point3::new(0.5, 0.0, 0.0));
        assert_eq!(ts.len(), 2, "{ts:?}");
    }

    #[test]
    fn the_tip_of_a_kink() {
        // A V with its tip at the origin: a knot of multiplicity 2 on a
        // quadratic, where the tangent turns.
        let k = KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 2, 3]).unwrap();
        let pts = vec![
            Point3::new(-1.0, 1.0, 0.0),
            Point3::new(-0.5, 0.5, 0.0),
            Point3::ORIGIN,
            Point3::new(0.5, 0.5, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        ];
        let c = NurbsCurve::new(k, pts, None).unwrap();
        let below = Point3::new(0.0, -1.0, 0.0);
        let foot = c.project(below);
        assert_eq!((foot.param, foot.distance), (1.0, 1.0));
        assert_eq!(c.local_minima(below).len(), 1);
        // Above the tip, inside the V, the kink is a maximum: the nearest
        // points are on the arms.
        let above = Point3::new(0.0, 0.5, 0.0);
        assert!(c.local_minima(above).iter().all(|m| m.param != 1.0));
    }

    #[test]
    fn curves_that_fold_back() {
        // Out from P0 to a tip and back along the same line, so the speed
        // vanishes at the tip; found by fuzzing. The tip is the nearest
        // point to `p`.
        let case = |degree: usize, knots: [f64; 2], pts: Vec<Point3>, w: Vec<f64>, p: Point3| {
            let k = KnotVector::clamped(degree, &knots, &[degree + 1, degree + 1]).unwrap();
            let c = NurbsCurve::new(k, pts, Some(w)).unwrap();
            let foot = c.project(p);
            let (d, t) = sampled(&c, p, 20_000);
            assert!(
                foot.distance <= d + 1e-12,
                "{} > {d} (at {t})",
                foot.distance
            );
            assert!((foot.param - t).abs() <= 1e-3, "{} vs {t}", foot.param);
        };
        let q = Point3::new(6.0, 6.0, 6.0);
        case(
            2,
            [-2.7411764705882353, -1.031372549019608],
            vec![
                q,
                Point3::new(5.247058823529411, 6.0, -3.788235294117647),
                q,
            ],
            vec![5.0; 3],
            Point3::new(-6.0, -6.0, -6.0),
        );
        // The speed also vanishes at both ends.
        case(
            4,
            [3.0, 5.0],
            vec![q, q, Point3::new(6.0, 6.0, -0.4), q, q],
            vec![
                5.0,
                0.49647058823529416,
                0.06941176470588235,
                4.553529411764706,
                3.4276470588235295,
            ],
            Point3::new(5.952941176470588, 5.952941176470588, -6.0),
        );
        // The tip falls exactly where the search splits the piece.
        let r = Point3::new(-4.117647058823529, 6.0, -3.788235294117647);
        case(
            2,
            [3.0, 5.0],
            vec![
                r,
                Point3::new(-4.447058823529412, -5.7176470588235295, 6.0),
                r,
            ],
            vec![5.0; 3],
            Point3::new(-6.0, -6.0, -6.0),
        );
    }

    #[test]
    fn a_root_next_to_a_periodic_seam_that_is_a_kink() {
        // The last piece ends on the seam, where evaluating the curve gives
        // the first piece's derivative. Found by fuzzing.
        let k = KnotVector::periodic(
            2,
            &[3.0, 4.482352941176471, 6.474509803921569, 8.474509803921569],
            &[2, 1, 2, 2],
        )
        .unwrap();
        let q = Point3::new(6.0, 6.0, 6.0);
        let pts = vec![
            q,
            Point3::new(6.0, -5.952941176470588, -5.952941176470588),
            Point3::new(-5.952941176470588, -5.529411764705882, 6.0),
            q,
            q,
        ];
        let c = NurbsCurve::new(k, pts, Some(vec![5.0; 5])).unwrap();
        let p = Point3::new(6.0, 6.0, -5.952941176470588);
        let foot = c.project(p);
        let (d, _) = sampled(&c, p, 20_000);
        assert!(foot.distance <= d + 1e-12, "{foot:?}");
    }

    #[test]
    fn ties_go_to_the_lower_parameter() {
        // A full circle and its centre: every point is nearest.
        let circle = Circle::new(Frame::WORLD, 2.0).unwrap();
        let (c, _) = circle.to_nurbs().unwrap();
        let foot = c.project(Point3::ORIGIN);
        assert_eq!(foot.param, 0.0);
        assert!((foot.distance - 2.0).abs() <= 1e-15);
        // On the circle's axis, but off its plane.
        let foot = c.project(Point3::new(0.0, 0.0, 1.0));
        assert_eq!(foot.param, 0.0);
        // Inside but off the centre, there is one minimum, on the near
        // side.
        let m = c.local_minima(Point3::new(-0.5, 0.0, 0.0));
        assert_eq!(m.len(), 1);
        assert!((m[0].param - PI).abs() <= 1e-12);
        let foot = c.project(Point3::new(0.0, -0.5, 0.0));
        assert!((foot.param - 3.0 * FRAC_PI_2).abs() <= 1e-12);
    }

    #[test]
    fn periodic_parameters_stay_in_one_period() {
        let circle = Circle::new(Frame::WORLD, 2.0).unwrap();
        let (c, _) = circle.to_nurbs().unwrap();
        // Just below angle 0: the answer is near 2π, not negative.
        let p = Point3::new(5.0, -1e-3, 0.0);
        let foot = c.project(p);
        assert!(
            foot.param < TAU && foot.param > TAU - 1e-3,
            "{}",
            foot.param
        );
        let foot = c.project(Point3::new(5.0, 0.0, 0.0));
        assert!(
            foot.param == 0.0 || foot.param <= LINEAR_RESOLUTION,
            "{}",
            foot.param
        );
    }
}
