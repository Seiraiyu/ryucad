//! The B-spline shared by NURBS curves in the plane and in space: its
//! validation and its rational evaluation with two derivatives (The NURBS
//! Book, A4.2).

use super::KnotVector;
use crate::geom::{Vector2, Vector3};
use crate::{Error, NurbsError};
use core::fmt::Debug;
use core::ops::{Add, Div, Mul, Sub};

/// What control points are made of: [`Vector2`] or [`Vector3`] (a point
/// as its offset from the origin).
pub(crate) trait Coords:
    Copy
    + Debug
    + PartialEq
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<f64, Output = Self>
    + Div<f64, Output = Self>
{
    const ZERO: Self;
}

impl Coords for Vector2 {
    const ZERO: Self = Vector2::ZERO;
}

impl Coords for Vector3 {
    const ZERO: Self = Vector3::ZERO;
}

/// A validated rational B-spline curve: its knots, its distinct control
/// points and their weights.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spline<V> {
    pub(crate) knots: KnotVector,
    pub(crate) points: Box<[V]>,
    pub(crate) weights: Box<[f64]>,
}

/// Checks a weight list against a control-point count: `None` means every
/// weight is 1.
pub(crate) fn check_weights(count: usize, weights: Option<Vec<f64>>) -> Result<Box<[f64]>, Error> {
    let weights = weights.unwrap_or_else(|| vec![1.0; count]);
    if weights.len() != count {
        return Err(Error::InvalidNurbs(NurbsError::ControlCount));
    }
    if weights.iter().any(|w| !w.is_finite()) {
        return Err(Error::NonFinite);
    }
    if weights.iter().any(|&w| w <= 0.0) {
        return Err(Error::InvalidNurbs(NurbsError::Weight));
    }
    Ok(weights.into())
}

impl<V: Coords> Spline<V> {
    /// Checks the counts and weights; the caller has checked the points.
    pub(crate) fn new(
        knots: KnotVector,
        points: Vec<V>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, Error> {
        if points.len() != knots.control_count() {
            return Err(Error::InvalidNurbs(NurbsError::ControlCount));
        }
        let weights = check_weights(points.len(), weights)?;
        Ok(Self {
            knots,
            points: points.into(),
            weights,
        })
    }

    /// The point and its first two derivatives at `t`: the weighted sums
    /// `A = Σ N w P` and `W = Σ N w` with their derivatives, then the
    /// quotient rule.
    pub(crate) fn eval(&self, t: f64) -> [V; 3] {
        let b = self.knots.basis(t);
        let p = self.knots.degree();
        let n = self.points.len();
        let mut a = [V::ZERO; 3];
        let mut w = [0.0; 3];
        for i in 0..=p {
            let idx = (b.span() - p + i) % n;
            let wi = self.weights[idx];
            let pw = self.points[idx] * wi;
            for k in 0..3 {
                let nk = b.values(k)[i];
                a[k] = a[k] + pw * nk;
                w[k] += wi * nk;
            }
        }
        let c = a[0] / w[0];
        let d1 = (a[1] - c * w[1]) / w[0];
        let d2 = (a[2] - d1 * (2.0 * w[1]) - c * w[2]) / w[0];
        [c, d1, d2]
    }
}
