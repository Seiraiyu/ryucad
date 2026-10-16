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
}
