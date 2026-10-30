//! Knot insertion (The NURBS Book, A5.1), refinement, clamping a periodic
//! spline to one period, and splitting. Results are always clamped.

use super::KnotVector;
use super::eval::{Coords, Spline};
use crate::tol::LINEAR_RESOLUTION;
use crate::{Error, NurbsError};

/// A control point and its weight. Interpolation happens in homogeneous
/// space, but the point is kept in Euclidean form so that control points
/// an operation doesn't touch are copied bit for bit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hom<V> {
    pub(crate) p: V,
    pub(crate) w: f64,
}

impl<V: Coords> Hom<V> {
    /// `(1 - alpha) a + alpha b` in homogeneous space.
    pub(crate) fn lerp(a: Self, b: Self, alpha: f64) -> Self {
        let w = a.w * (1.0 - alpha) + b.w * alpha;
        let p = (a.p * (a.w * (1.0 - alpha)) + b.p * (b.w * alpha)) / w;
        Self { p, w }
    }
}

/// `t` inserted `r` times into a flat knot vector of degree `p` and its
/// control points (A5.1). `t` must be in the domain and its multiplicity
/// plus `r` at most `p`.
pub(crate) fn insert<V: Coords>(
    flat: &[f64],
    p: usize,
    pw: &[Hom<V>],
    t: f64,
    r: usize,
) -> (Vec<f64>, Vec<Hom<V>>) {
    let k = flat.iter().rposition(|&u| u <= t).unwrap_or(0);
    let s = flat.iter().filter(|&&u| u == t).count();
    debug_assert!(k >= p && r + s <= p);
    let mut uq = Vec::with_capacity(flat.len() + r);
    uq.extend_from_slice(&flat[..=k]);
    uq.extend(core::iter::repeat_n(t, r));
    uq.extend_from_slice(&flat[k + 1..]);
    if r == 0 {
        return (uq, pw.to_vec());
    }
    let n = pw.len();
    let mut qw = vec![pw[0]; n + r];
    qw[..=k - p].copy_from_slice(&pw[..=k - p]);
    qw[k - s + r..].copy_from_slice(&pw[k - s..]);
    let mut rw = pw[k - p..=k - s].to_vec();
    let mut l = k - p;
    for j in 1..=r {
        l = k - p + j;
        for i in 0..=p - j - s {
            let alpha = (t - flat[l + i]) / (flat[i + k + 1] - flat[l + i]);
            rw[i] = Hom::lerp(rw[i], rw[i + 1], alpha);
        }
        qw[l] = rw[0];
        qw[k + r - j - s] = rw[p - j - s];
    }
    if l + 1 < k - s {
        qw[l + 1..k - s].copy_from_slice(&rw[1..k - s - l]);
    }
    (uq, qw)
}

/// The flat knots and control points of one direction in clamped form
/// over the domain: unchanged when clamped; for a periodic spline, the
/// domain ends raised to multiplicity `p` by insertion and the knots and
/// control points between them kept.
pub(crate) fn clamp<V: Coords>(knots: &KnotVector, pw: &[Hom<V>]) -> (Vec<f64>, Vec<Hom<V>>) {
    if !knots.is_periodic() {
        return (knots.flat().to_vec(), pw.to_vec());
    }
    let p = knots.degree();
    let n = pw.len();
    let mut flat = knots.flat().to_vec();
    let mut hom: Vec<_> = (0..n + p).map(|i| pw[i % n]).collect();
    let [k0, kr] = knots.domain();
    for t in [k0, kr] {
        let s = flat.iter().filter(|&&u| u == t).count();
        if s < p {
            (flat, hom) = insert(&flat, p, &hom, t, p - s);
        }
    }
    let a = flat.iter().position(|&u| u == k0).unwrap_or(p);
    let b = flat.iter().position(|&u| u == kr).unwrap_or(flat.len() - p);
    let mut out = Vec::with_capacity(b - a + p + 2);
    out.push(k0);
    out.extend_from_slice(&flat[a..b + p]);
    out.push(kr);
    (out, hom[a - 1..b].to_vec())
}

/// Splits clamped flat knots and control points at `t`, strictly inside
/// the domain: `t` raised to multiplicity `p`, then the two halves.
pub(crate) fn split_at<V: Coords>(
    flat: &[f64],
    p: usize,
    pw: &[Hom<V>],
    t: f64,
) -> [(Vec<f64>, Vec<Hom<V>>); 2] {
    let s = flat.iter().filter(|&&u| u == t).count();
    let (flat, hom) = insert(flat, p, pw, t, p - s.min(p));
    let k = flat.iter().rposition(|&u| u == t).unwrap_or(p);
    let mut left = flat[..=k].to_vec();
    left.push(t);
    let mut right = vec![t];
    right.extend_from_slice(&flat[k - p + 1..]);
    [
        (left, hom[..=k - p].to_vec()),
        (right, hom[k - p..].to_vec()),
    ]
}

/// Clamped flat knots and control points carried on past the start of
/// their domain by `m0` and past its end by `m1` (either may be 0): the
/// spline goes to Bézier form (every inner knot raised to multiplicity
/// `p`), and its end pieces, their own polynomials, are carried over the
/// margins by de Casteljau's algorithm outside `[0, 1]`. The spline is
/// unchanged over its own domain, its knots there kept bit for bit.
/// `None` where an extrapolated weight isn't positive.
pub(crate) fn extend_flat<V: Coords>(
    flat: &[f64],
    p: usize,
    pw: &[Hom<V>],
    [m0, m1]: [f64; 2],
) -> Option<(Vec<f64>, Vec<Hom<V>>)> {
    let (u0, u1) = (flat[0], flat[flat.len() - 1]);
    let mut inner: Vec<f64> = flat.iter().copied().filter(|&u| u > u0 && u < u1).collect();
    inner.dedup();
    let (mut flat, mut hom) = (flat.to_vec(), pw.to_vec());
    for &t in &inner {
        let s = flat.iter().filter(|&&u| u == t).count();
        if s < p {
            (flat, hom) = insert(&flat, p, &hom, t, p - s);
        }
    }
    let n = hom.len();
    if m0 > 0.0 {
        // The first piece over `[u0 - m0, k1]`: de Casteljau at `lam`
        // below it, its right part.
        let k1 = inner.first().copied().unwrap_or(u1);
        let lam = -m0 / (k1 - u0);
        let piece = de_casteljau(&hom[..=p], lam).1;
        hom[..=p].copy_from_slice(&piece);
        for u in &mut flat[..=p] {
            *u = u0 - m0;
        }
    }
    if m1 > 0.0 {
        // The last piece over `[kq, u1 + m1]`: its left part at `mu`
        // past it (from where it now starts, carried already when it is
        // the only piece).
        let start = if inner.is_empty() {
            flat[0]
        } else {
            inner[inner.len() - 1]
        };
        let mu = 1.0 + m1 / (u1 - start);
        let piece = de_casteljau(&hom[n - p - 1..], mu).0;
        hom[n - p - 1..].copy_from_slice(&piece);
        let len = flat.len();
        for u in &mut flat[len - p - 1..] {
            *u = u1 + m1;
        }
    }
    if hom.iter().any(|h| !(h.w > 0.0 && h.w.is_finite())) {
        return None;
    }
    Some((flat, hom))
}

/// The two parts of a Bézier piece either side of local parameter `t`
/// (anywhere, not only in `[0, 1]`), by de Casteljau's algorithm: the
/// left over `[0, t]`, the right over `[t, 1]`.
fn de_casteljau<V: Coords>(pts: &[Hom<V>], t: f64) -> (Vec<Hom<V>>, Vec<Hom<V>>) {
    let n = pts.len();
    let mut work = pts.to_vec();
    let mut left = vec![work[0]];
    let mut right = vec![work[n - 1]];
    for k in 1..n {
        for i in 0..n - k {
            work[i] = Hom::lerp(work[i], work[i + 1], t);
        }
        left.push(work[0]);
        right.push(work[n - 1 - k]);
    }
    right.reverse();
    (left, right)
}

/// Fails unless `t` is finite and strictly inside `[a, b]`.
fn check_inside(t: f64, [a, b]: [f64; 2]) -> Result<(), Error> {
    if !t.is_finite() {
        Err(Error::NonFinite)
    } else if t <= a || t >= b {
        Err(Error::InvalidNurbs(NurbsError::KnotOrder))
    } else {
        Ok(())
    }
}

/// Fails unless splitting `[a, b]` at `t` leaves two pieces no shorter
/// than [`LINEAR_RESOLUTION`].
pub(crate) fn check_split(t: f64, [a, b]: [f64; 2]) -> Result<(), Error> {
    if !t.is_finite() {
        Err(Error::NonFinite)
    } else if t - a < LINEAR_RESOLUTION || b - t < LINEAR_RESOLUTION {
        Err(Error::InvalidNurbs(NurbsError::ShortDomain))
    } else {
        Ok(())
    }
}

/// Inserts each of `knots` once, in increasing order. Fails with
/// `NonFinite`, `KnotOrder` for a knot not strictly inside the domain, or
/// `KnotMultiplicity` when a knot would exceed multiplicity `p`.
pub(crate) fn refine_flat<V: Coords>(
    knots: &KnotVector,
    pw: &[Hom<V>],
    extra: &[f64],
) -> Result<(Vec<f64>, Vec<Hom<V>>), Error> {
    for &t in extra {
        check_inside(t, knots.domain())?;
    }
    let mut sorted = extra.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p = knots.degree();
    let (mut flat, mut hom) = clamp(knots, pw);
    for t in sorted {
        if flat.iter().filter(|&&u| u == t).count() >= p {
            return Err(Error::InvalidNurbs(NurbsError::KnotMultiplicity));
        }
        (flat, hom) = insert(&flat, p, &hom, t, 1);
    }
    Ok((flat, hom))
}

/// Two clamped splines of degree `p` joined end to start, the second
/// shifted in parameter by `shift`: the joint keeps multiplicity `p`, and
/// the second's first control point (the same point as the first's last)
/// is dropped.
fn join<V: Coords>(
    p: usize,
    (fa, ha): (Vec<f64>, Vec<Hom<V>>),
    (fb, hb): (Vec<f64>, Vec<Hom<V>>),
    shift: f64,
) -> (Vec<f64>, Vec<Hom<V>>) {
    let mut flat = fa[..fa.len() - 1].to_vec();
    flat.extend(fb[p + 1..].iter().map(|&u| u + shift));
    let mut hom = ha;
    hom.extend_from_slice(&hb[1..]);
    (flat, hom)
}

impl<V: Coords> Spline<V> {
    pub(crate) fn hom(&self) -> Vec<Hom<V>> {
        self.points
            .iter()
            .zip(self.weights.iter())
            .map(|(&p, &w)| Hom { p, w })
            .collect()
    }

    /// A clamped spline from flat knots and control points the kernel
    /// built itself.
    pub(crate) fn from_flat(p: usize, flat: &[f64], hom: &[Hom<V>]) -> Self {
        Self {
            knots: KnotVector::clamped_flat(p, flat),
            points: hom.iter().map(|h| h.p).collect(),
            weights: hom.iter().map(|h| h.w).collect(),
        }
    }

    pub(crate) fn clamped(&self) -> Self {
        if !self.knots.is_periodic() {
            return self.clone();
        }
        let (flat, hom) = clamp(&self.knots, &self.hom());
        Self::from_flat(self.knots.degree(), &flat, &hom)
    }

    pub(crate) fn refine(&self, extra: &[f64]) -> Result<Self, Error> {
        let (flat, hom) = refine_flat(&self.knots, &self.hom(), extra)?;
        Ok(Self::from_flat(self.knots.degree(), &flat, &hom))
    }

    /// The part over `[t0, t1]` as a clamped spline with the same
    /// parameters. On a periodic spline `t0` is first wrapped into the
    /// domain and the part may run on past its end, at most one period, so
    /// the result's domain is `[wrap(t0), wrap(t0) + t1 - t0]`. Fails with
    /// `NonFinite`, or `InvalidNurbs(KnotOrder)` for a range that is empty,
    /// leaves a clamped domain, or is longer than a period.
    pub(crate) fn part(&self, [t0, t1]: [f64; 2]) -> Result<Self, Error> {
        if !(t0.is_finite() && t1.is_finite()) {
            return Err(Error::NonFinite);
        }
        // A few ulps of the range's magnitude: what rounding leaves.
        let slack = |period: f64| 8.0 * f64::EPSILON * (t0.abs() + t1.abs() + period);
        let order = Error::InvalidNurbs(NurbsError::KnotOrder);
        let p = self.knots.degree();
        let [a, b] = self.knots.domain();
        let (s, e, (mut flat, mut hom)) = match self.knots.period() {
            None if t0 < a || t1 > b || t0 >= t1 => return Err(order),
            None => (t0, t1, clamp(&self.knots, &self.hom())),
            Some(period) if t0 >= t1 || t1 - t0 > period + slack(period) => return Err(order),
            Some(period) => {
                // A whole period mapped from another parameter may overshoot
                // by a rounding error: it is one period.
                let t1 = t0 + (t1 - t0).min(period);
                // The range moved by whole periods to start in the domain.
                // A start a rounding error past the domain's start (or
                // short of its end) is the start: a sliver of a piece
                // would collapse when shifted by the period.
                let w = self.knots.wrap(t0);
                let mut e = t1 + (w - t0);
                let slack = slack(period);
                let s = if w - a <= slack {
                    a
                } else if b - w <= slack {
                    e -= period;
                    a
                } else {
                    w
                };
                let (flat, hom) = clamp(&self.knots, &self.hom());
                let seated = if s > a {
                    let [head, tail] = split_at(&flat, p, &hom, s);
                    join(p, tail, head, period)
                } else {
                    (flat, hom)
                };
                (s, e, seated)
            }
        };
        // An end a rounding error from a knot is that knot: the sliver of
        // a piece between them would collapse when its knots are moved
        // (a circle's pcurve mapped onto an edge's range in USD's lowering,
        // #247).
        let near = slack(self.knots.period().unwrap_or(0.0));
        let snap = |t: f64| {
            flat.iter()
                .copied()
                .find(|&u| (u - t).abs() <= near)
                .unwrap_or(t)
        };
        let (s, e) = (snap(s), snap(e));
        if s >= e {
            return Err(order);
        }
        let (lo, hi) = (flat[0], flat[flat.len() - 1]);
        if s > lo {
            [_, (flat, hom)] = split_at(&flat, p, &hom, s);
        }
        if e < hi {
            [(flat, hom), _] = split_at(&flat, p, &hom, e);
        }
        Ok(Self::from_flat(p, &flat, &hom))
    }

    /// The spline run backwards in clamped form: `t` becomes `a + b - t`
    /// over the clamped domain `[a, b]`.
    pub(crate) fn reversed(&self) -> Self {
        let (flat, hom) = clamp(&self.knots, &self.hom());
        let (a, b) = (flat[0], flat[flat.len() - 1]);
        let flat: Vec<f64> = flat.iter().rev().map(|&u| (a + b) - u).collect();
        let hom: Vec<Hom<V>> = hom.into_iter().rev().collect();
        Self::from_flat(self.knots.degree(), &flat, &hom)
    }

    pub(crate) fn split(&self, t: f64) -> Result<[Self; 2], Error> {
        check_split(t, self.knots.domain())?;
        let p = self.knots.degree();
        let (flat, hom) = clamp(&self.knots, &self.hom());
        Ok(split_at(&flat, p, &hom, t).map(|(f, h)| Self::from_flat(p, &f, &h)))
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::{KnotVector, NurbsCurve, Point3};
    use crate::{Error, NurbsError};
    use core::f64::consts::FRAC_1_SQRT_2;

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

    fn periodic() -> NurbsCurve {
        let k = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
        let pts = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.5),
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(0.0, -1.0, -0.5),
        ];
        NurbsCurve::new(k, pts, Some(vec![1.0, 2.0, 1.0, 0.5])).unwrap()
    }

    /// The two curves agree at 101 samples of `[a, b]`.
    fn same_shape(c: &NurbsCurve, d: &NurbsCurve, [a, b]: [f64; 2]) -> bool {
        (0..=100).all(|i| {
            let t = a + (b - a) * f64::from(i) / 100.0;
            c.point(t).distance(d.point(t)) <= 1e-14
        })
    }

    #[test]
    fn refining_keeps_the_shape() {
        let c = quarter();
        let r = c.refine(&[0.5]).unwrap();
        assert_eq!(r.knots().knots(), [0.0, 0.5, 1.0]);
        assert_eq!(r.knots().mults(), [3, 1, 3]);
        assert_eq!(r.control_points().len(), 4);
        assert!(same_shape(&c, &r, [0.0, 1.0]));
        let w = wavy();
        let r = w.refine(&[0.4, 0.4, 2.0, 0.1, 2.0]).unwrap();
        assert_eq!(r.knots().knots(), [0.0, 0.1, 0.4, 1.0, 2.0, 2.5]);
        assert_eq!(r.knots().mults(), [4, 1, 3, 2, 2, 4]);
        assert!(same_shape(&w, &r, [0.0, 2.5]));
    }

    #[test]
    fn refine_rejections() {
        let c = quarter();
        let invalid = |e| Err(Error::InvalidNurbs(e));
        assert_eq!(c.refine(&[0.0]), invalid(NurbsError::KnotOrder));
        assert_eq!(c.refine(&[1.5]), invalid(NurbsError::KnotOrder));
        assert_eq!(c.refine(&[f64::NAN]), Err(Error::NonFinite));
        assert_eq!(
            c.refine(&[0.5, 0.5, 0.5]),
            invalid(NurbsError::KnotMultiplicity)
        );
    }

    #[test]
    fn clamping_a_periodic_curve_keeps_one_period() {
        let c = periodic();
        let k = c.to_clamped();
        assert!(!k.knots().is_periodic());
        assert_eq!(k.domain(), c.domain());
        assert!(same_shape(&c, &k, [0.0, 4.0]));
        assert_eq!(k.control_points().next(), Some(k.point(0.0)));
        let d = k.to_clamped();
        assert_eq!(d, k);
    }

    #[test]
    fn splitting() {
        let w = wavy();
        let (a, b) = w.split(1.7).unwrap();
        assert_eq!(a.domain(), [0.0, 1.7]);
        assert_eq!(b.domain(), [1.7, 2.5]);
        assert!(same_shape(&w, &a, [0.0, 1.7]));
        assert!(same_shape(&w, &b, [1.7, 2.5]));
        // At an existing knot, and on a periodic curve.
        let (a, b) = w.split(1.0).unwrap();
        assert!(a.point(1.0).distance(b.point(1.0)) <= 1e-14);
        let p = periodic();
        let (a, b) = p.split(2.5).unwrap();
        assert!(same_shape(&p, &a, [0.0, 2.5]));
        assert!(same_shape(&p, &b, [2.5, 4.0]));
        assert_eq!(
            w.split(1e-9),
            Err(Error::InvalidNurbs(NurbsError::ShortDomain))
        );
        assert_eq!(
            w.split(2.5),
            Err(Error::InvalidNurbs(NurbsError::ShortDomain))
        );
    }

    /// A part whose range ends a rounding error from a knot ends on the
    /// knot: no sliver of a piece is left to collapse (#247).
    #[test]
    fn a_part_a_rounding_error_from_a_knot_ends_on_it() {
        let w = wavy();
        let up = |t: f64| f64::from_bits(t.to_bits() + 1);
        let down = |t: f64| f64::from_bits(t.to_bits() - 1);
        let r = w.part([0.1, up(1.0)]).unwrap();
        assert_eq!(r.knots().knots(), [0.1, 0.4, 1.0]);
        assert!(same_shape(&w, &r, [0.1, 1.0]));
        let r = w.part([down(0.4), 2.0]).unwrap();
        assert_eq!(r.knots().knots(), [0.4, 1.0, 2.0]);
        let p = periodic();
        let r = p.part([0.5, up(up(3.0))]).unwrap();
        assert_eq!(r.knots().knots(), [0.5, 1.0, 2.0, 3.0]);
        assert!(same_shape(&p, &r, [0.5, 3.0]));
        // A range a rounding error long is empty.
        assert_eq!(
            w.part([1.0, up(1.0)]),
            Err(Error::InvalidNurbs(NurbsError::KnotOrder))
        );
    }

    /// A curve carried past both ends is the same curve over its own
    /// domain, its end pieces' polynomials beyond: the extension leaves
    /// each end with the end piece's value and first two derivatives.
    #[test]
    fn extended_curves_carry_their_end_pieces_on() {
        // A sixteenth of the end pieces past each end, as the
        // intersectors carry a spline (further, a weight may fall to zero).
        for (c, [m0, m1]) in [(quarter(), [0.0625, 0.0625]), (wavy(), [0.025, 0.093_75])] {
            let [a, b] = c.domain();
            let e = c.extended([m0, m1]).unwrap();
            assert_eq!(e.domain(), [a - m0, b + m1]);
            for i in 0..=32 {
                let t = a + (b - a) * f64::from(i) / 32.0;
                assert!(e.point(t).distance(c.point(t)) <= 1e-14, "{t}");
            }
            for t in [a, b] {
                let (x, z) = (e.eval(t), c.eval(t));
                assert!((x.d1 - z.d1).length() <= 1e-12 * (1.0 + z.d1.length()));
                assert!((x.d2 - z.d2).length() <= 1e-9 * (1.0 + z.d2.length()));
            }
        }
        assert_eq!(periodic().extended([0.1, 0.1]), None);
    }
}
