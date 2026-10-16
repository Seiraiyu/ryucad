//! Knot vectors: validation, the flat (expanded) form the algorithms run
//! on, span search and periodic wrapping.

use crate::tol::LINEAR_RESOLUTION;
use crate::{Error, NurbsError};

/// The highest degree a NURBS may have. Evaluation works in stack buffers
/// sized by it.
pub const MAX_DEGREE: usize = 25;

/// A validated knot vector for one parameter direction: the degree, the
/// distinct knots `k_0 < ... < k_r` and their multiplicities, clamped or
/// periodic. The domain is `[k_0, k_r]`.
///
/// **Clamped:** the end knots have multiplicity `degree + 1` and interior
/// knots at most `degree`. The flat knot vector repeats each knot by its
/// multiplicity, and it takes `Σ m - degree - 1` control points.
///
/// **Periodic:** every multiplicity is at most `degree`, and `k_r` is the
/// same knot as `k_0` one period later, so its multiplicity must equal
/// `k_0`'s and counts once. One period holds `n = m_0 + ... + m_(r-1)`
/// knots `u_0 <= ... <= u_(n-1)` (`u_0 = k_0`), extended periodically
/// (`u_(i+n) = u_i + period`). It takes `n` control points; the flat form
/// is `u_(-degree) ..= u_(n+degree)`, and control point `i mod n` weights
/// the basis function starting at flat knot `i`.
#[derive(Clone, Debug, PartialEq)]
pub struct KnotVector {
    degree: usize,
    knots: Box<[f64]>,
    mults: Box<[usize]>,
    periodic: bool,
    flat: Box<[f64]>,
}

fn invalid(e: NurbsError) -> Error {
    Error::InvalidNurbs(e)
}

impl KnotVector {
    /// A clamped knot vector. Fails with `NonFinite`, or `InvalidNurbs`:
    /// `Degree` outside `1..=MAX_DEGREE`, `KnotOrder` unless the knots
    /// strictly increase, `KnotMultiplicity` for the wrong count or
    /// multiplicities, `ShortDomain` for a domain shorter than
    /// [`LINEAR_RESOLUTION`].
    pub fn clamped(degree: usize, knots: &[f64], mults: &[usize]) -> Result<Self, Error> {
        check_common(degree, knots, mults)?;
        let (first, last) = (mults[0], mults[mults.len() - 1]);
        let interior = &mults[1..mults.len() - 1];
        if first != degree + 1 || last != degree + 1 || interior.iter().any(|&m| m > degree) {
            return Err(invalid(NurbsError::KnotMultiplicity));
        }
        let flat = knots
            .iter()
            .zip(mults)
            .flat_map(|(&k, &m)| core::iter::repeat_n(k, m))
            .collect();
        Ok(Self {
            degree,
            knots: knots.into(),
            mults: mults.into(),
            periodic: false,
            flat,
        })
    }

    /// A periodic knot vector, with the same failures as
    /// [`KnotVector::clamped`], plus `KnotMultiplicity` when the first and
    /// last multiplicities differ and `ControlCount` when one period holds
    /// fewer than `degree + 1` knots.
    pub fn periodic(degree: usize, knots: &[f64], mults: &[usize]) -> Result<Self, Error> {
        check_common(degree, knots, mults)?;
        let r = knots.len() - 1;
        if mults[0] != mults[r] || mults.iter().any(|&m| m > degree) {
            return Err(invalid(NurbsError::KnotMultiplicity));
        }
        let one: Vec<f64> = knots[..r]
            .iter()
            .zip(&mults[..r])
            .flat_map(|(&k, &m)| core::iter::repeat_n(k, m))
            .collect();
        let n = one.len();
        if n < degree + 1 {
            return Err(invalid(NurbsError::ControlCount));
        }
        let (k0, kr) = (knots[0], knots[r]);
        let period = kr - k0;
        // u_(qn + i) = (u_i - k_0) + (k_0 + q period), with the knots of
        // the domain itself (q = 0) copied exactly and the shift for q = 1
        // exactly k_r, so every knot in the domain is a given one.
        let base = |q: i64| match q {
            0 => k0,
            1 => kr,
            _ => k0 + q as f64 * period,
        };
        let p = degree as i64;
        let flat = (-p..=n as i64 + p)
            .map(|j| {
                let (q, i) = (j.div_euclid(n as i64), j.rem_euclid(n as i64) as usize);
                if q == 0 {
                    one[i]
                } else {
                    (one[i] - k0) + base(q)
                }
            })
            .collect();
        Ok(Self {
            degree,
            knots: knots.into(),
            mults: mults.into(),
            periodic: true,
            flat,
        })
    }

    /// A clamped knot vector from a flat one the kernel built itself (by
    /// knot insertion), already known to be valid.
    pub(crate) fn clamped_flat(degree: usize, flat: &[f64]) -> Self {
        let mut knots = Vec::new();
        let mut mults: Vec<usize> = Vec::new();
        for &u in flat {
            if knots.last() == Some(&u) {
                *mults.last_mut().unwrap_or(&mut 0) += 1;
            } else {
                knots.push(u);
                mults.push(1);
            }
        }
        debug_assert!(Self::clamped(degree, &knots, &mults).is_ok(), "{flat:?}");
        Self {
            degree,
            knots: knots.into(),
            mults: mults.into(),
            periodic: false,
            flat: flat.into(),
        }
    }

    pub fn degree(&self) -> usize {
        self.degree
    }

    /// The distinct knots.
    pub fn knots(&self) -> &[f64] {
        &self.knots
    }

    pub fn mults(&self) -> &[usize] {
        &self.mults
    }

    pub fn is_periodic(&self) -> bool {
        self.periodic
    }

    /// `[k_0, k_r]`.
    pub fn domain(&self) -> [f64; 2] {
        [self.knots[0], self.knots[self.knots.len() - 1]]
    }

    /// `k_r - k_0` for a periodic knot vector.
    pub fn period(&self) -> Option<f64> {
        let [a, b] = self.domain();
        self.periodic.then_some(b - a)
    }

    /// How many control points this knot vector takes.
    pub fn control_count(&self) -> usize {
        if self.periodic {
            self.flat.len() - 2 * self.degree - 1
        } else {
            self.flat.len() - self.degree - 1
        }
    }

    /// The flat knot vector the algorithms run on.
    pub fn flat(&self) -> &[f64] {
        &self.flat
    }

    /// How many control points the flat form takes: for a periodic vector,
    /// the first `degree` control points again after the last.
    pub fn flat_count(&self) -> usize {
        self.flat.len() - self.degree - 1
    }

    /// A periodic parameter brought into `[k_0, k_r)` (rounding may give
    /// `k_r` itself); a clamped one is returned as it is.
    pub fn wrap(&self, t: f64) -> f64 {
        match self.period() {
            Some(period) => {
                let k0 = self.knots[0];
                k0 + (t - k0).rem_euclid(period)
            }
            None => t,
        }
    }

    /// The flat index `i` of the knot span `[U_i, U_(i+1))` holding `t`,
    /// always a span of positive length inside the domain. Parameters
    /// before or after the domain get the first or last span, which
    /// extrapolates.
    pub fn span(&self, t: f64) -> usize {
        let u = &self.flat;
        let (p, n) = (self.degree, self.flat_count() - 1);
        if t >= u[n + 1] {
            return n;
        }
        if t < u[p] {
            let mut i = p;
            while u[i + 1] == u[i] {
                i += 1;
            }
            return i;
        }
        let (mut low, mut high) = (p, n + 1);
        let mut mid = (low + high) / 2;
        while t < u[mid] || t >= u[mid + 1] {
            if t < u[mid] {
                high = mid;
            } else {
                low = mid;
            }
            mid = (low + high) / 2;
        }
        mid
    }
}

/// The checks shared by both kinds: degree, finiteness, order, counts and
/// the domain length.
fn check_common(degree: usize, knots: &[f64], mults: &[usize]) -> Result<(), Error> {
    if degree == 0 || degree > MAX_DEGREE {
        return Err(invalid(NurbsError::Degree));
    }
    if knots.iter().any(|k| !k.is_finite()) {
        return Err(Error::NonFinite);
    }
    if knots.len() != mults.len() || knots.len() < 2 || mults.contains(&0) {
        return Err(invalid(NurbsError::KnotMultiplicity));
    }
    if knots.windows(2).any(|w| w[0] >= w[1]) {
        return Err(invalid(NurbsError::KnotOrder));
    }
    if knots[knots.len() - 1] - knots[0] < LINEAR_RESOLUTION {
        return Err(invalid(NurbsError::ShortDomain));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(e: NurbsError) -> Result<KnotVector, Error> {
        Err(Error::InvalidNurbs(e))
    }

    #[test]
    fn clamped_expands_by_multiplicity() {
        let k = KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 1, 3]).unwrap();
        assert_eq!(k.degree(), 2);
        assert_eq!(k.knots(), [0.0, 1.0, 2.0]);
        assert_eq!(k.mults(), [3, 1, 3]);
        assert!(!k.is_periodic());
        assert_eq!(k.flat(), [0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0]);
        assert_eq!(k.control_count(), 4);
        assert_eq!(k.flat_count(), 4);
        assert_eq!(k.domain(), [0.0, 2.0]);
        assert_eq!(k.period(), None);
        assert_eq!(k.wrap(7.5), 7.5);
    }

    #[test]
    fn clamped_spans() {
        let k = KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 1, 3]).unwrap();
        for (t, span) in [
            (0.0, 2),
            (0.5, 2),
            (1.0, 3),
            (1.999, 3),
            (2.0, 3),
            (-1.0, 2),
            (3.0, 3),
        ] {
            assert_eq!(k.span(t), span, "t={t}");
        }
    }

    #[test]
    fn periodic_extends_one_period_both_ways() {
        let k = KnotVector::periodic(2, &[0.0, 1.0, 2.0, 3.0], &[1, 1, 1, 1]).unwrap();
        assert!(k.is_periodic());
        assert_eq!(k.control_count(), 3);
        assert_eq!(k.flat_count(), 5);
        assert_eq!(k.flat(), [-2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(k.domain(), [0.0, 3.0]);
        assert_eq!(k.period(), Some(3.0));
        assert_eq!(k.wrap(4.5), 1.5);
        assert_eq!(k.wrap(-0.5), 2.5);
        assert_eq!(k.wrap(3.0), 0.0);
        assert_eq!(k.span(0.0), 2);
        assert_eq!(k.span(2.9), 4);
        assert_eq!(k.span(3.0), 4);
    }

    #[test]
    fn periodic_seam_multiplicity() {
        let k = KnotVector::periodic(2, &[0.0, 1.0, 2.0], &[2, 2, 2]).unwrap();
        assert_eq!(k.control_count(), 4);
        assert_eq!(k.flat(), [-1.0, -1.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0]);
        // The first span with any length.
        assert_eq!(k.span(0.0), 3);
        assert_eq!(k.span(-0.5), 3);
        assert_eq!(k.span(1.0), 5);
        assert_eq!(k.span(2.0), 5);
    }

    #[test]
    fn periodic_with_fewer_control_points_than_the_degree_wraps_repeatedly() {
        let k = KnotVector::periodic(3, &[0.0, 1.0, 2.0, 3.0, 4.0], &[1, 1, 1, 1, 1]).unwrap();
        assert_eq!(k.control_count(), 4);
        assert_eq!(k.flat().len(), 4 + 2 * 3 + 1);
        assert_eq!(k.flat()[0], -3.0);
        assert_eq!(k.flat()[10], 7.0);
    }

    #[test]
    fn the_seam_is_exact() {
        // 0.1 + 0.2 is not 0.3 in f64; the period end must still be the
        // given knot, bit for bit.
        let k = KnotVector::periodic(1, &[0.1, 0.2, 0.3], &[1, 1, 1]).unwrap();
        assert_eq!(k.domain(), [0.1, 0.3]);
        assert_eq!(k.flat()[1 + 2], 0.3);
    }

    #[test]
    fn knots_inside_the_domain_are_the_given_ones() {
        let knots = [
            -1.2712894970352684,
            -0.9490019974711028,
            0.28917250370679304,
            2.1,
        ];
        let k = KnotVector::periodic(2, &knots, &[2, 1, 2, 2]).unwrap();
        for kn in knots {
            assert!(k.flat().contains(&kn), "{kn} not in {:?}", k.flat());
        }
    }

    #[test]
    fn rejections() {
        use NurbsError::*;
        assert_eq!(KnotVector::clamped(0, &[0.0, 1.0], &[1, 1]), err(Degree));
        assert_eq!(KnotVector::clamped(26, &[0.0, 1.0], &[27, 27]), err(Degree));
        assert_eq!(
            KnotVector::clamped(1, &[0.0, 0.0, 1.0], &[2, 1, 2]),
            err(KnotOrder)
        );
        assert_eq!(
            KnotVector::clamped(1, &[0.0, f64::NAN], &[2, 2]),
            Err(Error::NonFinite)
        );
        assert_eq!(
            KnotVector::clamped(2, &[0.0, 1.0], &[2, 3]),
            err(KnotMultiplicity)
        );
        assert_eq!(
            KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 3, 3]),
            err(KnotMultiplicity)
        );
        assert_eq!(
            KnotVector::clamped(2, &[0.0, 1.0, 2.0], &[3, 0, 3]),
            err(KnotMultiplicity)
        );
        assert_eq!(
            KnotVector::clamped(2, &[0.0, 1.0], &[3]),
            err(KnotMultiplicity)
        );
        assert_eq!(KnotVector::clamped(1, &[0.0], &[2]), err(KnotMultiplicity));
        assert_eq!(
            KnotVector::periodic(2, &[0.0, 1.0, 2.0], &[1, 1, 2]),
            err(KnotMultiplicity)
        );
        assert_eq!(
            KnotVector::clamped(1, &[0.0, 1e-9], &[2, 2]),
            err(ShortDomain)
        );
        assert_eq!(
            KnotVector::periodic(3, &[0.0, 1.0], &[1, 1]),
            err(ControlCount)
        );
    }
}
