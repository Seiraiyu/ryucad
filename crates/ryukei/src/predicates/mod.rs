//! Robust geometric predicates with exact signs.
//!
//! Each predicate treats its f64 inputs as exact and returns the true sign
//! of a determinant. A floating-point filter with Shewchuk's proven error
//! bound answers almost every call; only when it cannot decide does the
//! predicate recompute the determinant exactly with [`expansion`]
//! arithmetic.
//!
//! The filter bound and the expansion stage both assume no underflow, which
//! holds when every coordinate is zero or at least 2^-100 in magnitude. Each
//! predicate checks that first and sends anything smaller straight to exact
//! big-integer arithmetic ([`bigint`]). The sign is exact for every finite
//! input inside the size box.
//!
//! Error bounds (Shewchuk 1997, "Adaptive Precision Floating-Point
//! Arithmetic and Fast Robust Geometric Predicates", section 4), with
//! `EPS = 2^-53` the unit roundoff: each is `(k + m*EPS) * EPS` times the
//! permanent (the determinant evaluated with absolute values), derived for
//! the exact evaluation order used below.

mod bigint;
pub(crate) mod expansion;

use crate::geom::{Point2, Point3};
use bigint::{Fixed, expansion_safe};
use expansion::Expansion;

/// Exact real arithmetic, so each determinant is written once and
/// evaluated with whichever representation is safe for the input.
trait Exact: Sized {
    /// `a - b`, exactly.
    fn diff(a: f64, b: f64) -> Self;
    fn add(&self, other: &Self) -> Self;
    fn sub(&self, other: &Self) -> Self;
    fn mul(&self, other: &Self) -> Self;
    fn signum(&self) -> f64;
}

impl Exact for Expansion {
    fn diff(a: f64, b: f64) -> Self {
        Expansion::diff(a, b)
    }
    fn add(&self, other: &Self) -> Self {
        Expansion::add(self, other)
    }
    fn sub(&self, other: &Self) -> Self {
        Expansion::sub(self, other)
    }
    fn mul(&self, other: &Self) -> Self {
        Expansion::mul(self, other)
    }
    fn signum(&self) -> f64 {
        Expansion::signum(self)
    }
}

impl Exact for Fixed {
    fn diff(a: f64, b: f64) -> Self {
        Fixed::from_f64(a).sub(&Fixed::from_f64(b))
    }
    fn add(&self, other: &Self) -> Self {
        Fixed::add(self, other)
    }
    fn sub(&self, other: &Self) -> Self {
        Fixed::sub(self, other)
    }
    fn mul(&self, other: &Self) -> Self {
        Fixed::mul(self, other)
    }
    fn signum(&self) -> f64 {
        Fixed::signum(self)
    }
}

const EPS: f64 = 1.0 / 9_007_199_254_740_992.0; // 2^-53
const CCW_BOUND: f64 = (3.0 + 16.0 * EPS) * EPS;
const O3D_BOUND: f64 = (7.0 + 56.0 * EPS) * EPS;

/// The exact sign of a predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sign {
    Negative,
    Zero,
    Positive,
}

impl Sign {
    fn of(x: f64) -> Self {
        if x > 0.0 {
            Self::Positive
        } else if x < 0.0 {
            Self::Negative
        } else {
            Self::Zero
        }
    }

    /// The sign if the filter can certify it: `|det| > bound`.
    fn filtered(det: f64, bound: f64) -> Option<Self> {
        (det > bound || -det > bound).then(|| Self::of(det))
    }
}

/// Positive when `a`, `b`, `c` are counterclockwise, negative when
/// clockwise, zero when collinear.
pub fn orient2d(a: Point2, b: Point2, c: Point2) -> Sign {
    if !expansion_safe(&[a.x, a.y, b.x, b.y, c.x, c.y]) {
        return orient2d_with::<Fixed>(a, b, c);
    }
    let detleft = (a.x - c.x) * (b.y - c.y);
    let detright = (a.y - c.y) * (b.x - c.x);
    let det = detleft - detright;
    Sign::filtered(det, CCW_BOUND * (detleft.abs() + detright.abs()))
        .unwrap_or_else(|| orient2d_with::<Expansion>(a, b, c))
}

fn orient2d_with<E: Exact>(a: Point2, b: Point2, c: Point2) -> Sign {
    let left = E::diff(a.x, c.x).mul(&E::diff(b.y, c.y));
    let right = E::diff(a.y, c.y).mul(&E::diff(b.x, c.x));
    Sign::of(left.sub(&right).signum())
}

/// Positive when `d` lies below the plane through `a`, `b`, `c`, where
/// "below" means `a`, `b`, `c` appear counterclockwise seen from above.
/// Negative when above, zero when coplanar. (Shewchuk's convention.)
pub fn orient3d(a: Point3, b: Point3, c: Point3, d: Point3) -> Sign {
    if !expansion_safe(&[a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z, d.x, d.y, d.z]) {
        return orient3d_with::<Fixed>(a, b, c, d);
    }
    let (adx, ady, adz) = (a.x - d.x, a.y - d.y, a.z - d.z);
    let (bdx, bdy, bdz) = (b.x - d.x, b.y - d.y, b.z - d.z);
    let (cdx, cdy, cdz) = (c.x - d.x, c.y - d.y, c.z - d.z);
    let (bdxcdy, cdxbdy) = (bdx * cdy, cdx * bdy);
    let (cdxady, adxcdy) = (cdx * ady, adx * cdy);
    let (adxbdy, bdxady) = (adx * bdy, bdx * ady);
    let det = adz * (bdxcdy - cdxbdy) + bdz * (cdxady - adxcdy) + cdz * (adxbdy - bdxady);
    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * adz.abs()
        + (cdxady.abs() + adxcdy.abs()) * bdz.abs()
        + (adxbdy.abs() + bdxady.abs()) * cdz.abs();
    Sign::filtered(det, O3D_BOUND * permanent)
        .unwrap_or_else(|| orient3d_with::<Expansion>(a, b, c, d))
}

/// `x1 * y2 - x2 * y1`, exactly.
fn cross2<E: Exact>(x1: &E, y1: &E, x2: &E, y2: &E) -> E {
    x1.mul(y2).sub(&x2.mul(y1))
}

fn orient3d_with<E: Exact>(a: Point3, b: Point3, c: Point3, d: Point3) -> Sign {
    let (adx, ady, adz) = (E::diff(a.x, d.x), E::diff(a.y, d.y), E::diff(a.z, d.z));
    let (bdx, bdy, bdz) = (E::diff(b.x, d.x), E::diff(b.y, d.y), E::diff(b.z, d.z));
    let (cdx, cdy, cdz) = (E::diff(c.x, d.x), E::diff(c.y, d.y), E::diff(c.z, d.z));
    let det = adz
        .mul(&cross2(&bdx, &bdy, &cdx, &cdy))
        .add(&bdz.mul(&cross2(&cdx, &cdy, &adx, &ady)))
        .add(&cdz.mul(&cross2(&adx, &ady, &bdx, &bdy)));
    Sign::of(det.signum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use Sign::*;

    fn p2(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn p3(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    #[test]
    fn orient2d_basic() {
        assert_eq!(orient2d(p2(0.0, 0.0), p2(1.0, 0.0), p2(0.0, 1.0)), Positive);
        assert_eq!(orient2d(p2(0.0, 0.0), p2(0.0, 1.0), p2(1.0, 0.0)), Negative);
        assert_eq!(orient2d(p2(0.0, 0.0), p2(1.0, 1.0), p2(2.0, 2.0)), Zero);
    }

    #[test]
    fn orient2d_needs_exact_stage() {
        // Collinear in exact arithmetic but rounded floats say otherwise.
        let a = p2(0.5, 0.5);
        let b = p2(12.0, 12.0);
        let c = p2(24.0, 24.0);
        let d = p2(0.5 + f64::EPSILON / 2.0, 0.5);
        assert_eq!(orient2d(a, b, c), Zero);
        assert_eq!(orient2d(d, b, c), Negative);
        assert_eq!(orient2d(p2(0.5, 0.5 + f64::EPSILON / 2.0), b, c), Positive);
    }

    #[test]
    fn orient3d_basic() {
        let (a, b, c) = (p3(0.0, 0.0, 0.0), p3(1.0, 0.0, 0.0), p3(0.0, 1.0, 0.0));
        assert_eq!(orient3d(a, b, c, p3(0.0, 0.0, -1.0)), Positive);
        assert_eq!(orient3d(a, b, c, p3(0.0, 0.0, 1.0)), Negative);
        assert_eq!(orient3d(a, b, c, p3(0.3, 0.7, 0.0)), Zero);
        assert_eq!(orient3d(a, b, c, p3(0.3, 0.7, -1e-300)), Positive);
    }

    #[test]
    fn tiny_coordinates_use_big_integers() {
        // Subnormal coordinates: expansion products would underflow to 0.
        let t = f64::from_bits(1); // 2^-1074
        assert_eq!(orient2d(p2(0.0, 0.0), p2(t, 0.0), p2(0.0, t)), Positive);
        assert_eq!(orient2d(p2(0.0, 0.0), p2(t, t), p2(2.0 * t, 2.0 * t)), Zero);
        let (a, b, c) = (p3(0.0, 0.0, 0.0), p3(t, 0.0, 0.0), p3(0.0, t, 0.0));
        assert_eq!(orient3d(a, b, c, p3(0.0, 0.0, -t)), Positive);
    }

    #[test]
    fn orient_stages_agree_on_easy_input() {
        let (a, b, c, d) = (
            p3(1.0, 2.0, 3.0),
            p3(-4.0, 5.0, 1.0),
            p3(2.0, -1.0, 0.5),
            p3(0.1, 0.2, 9.0),
        );
        assert_eq!(orient3d(a, b, c, d), orient3d_with::<Expansion>(a, b, c, d));
        assert_eq!(orient3d(a, b, c, d), orient3d_with::<Fixed>(a, b, c, d));
        let (a2, b2, c2) = (p2(1.0, 2.0), p2(-4.0, 5.0), p2(2.0, -1.0));
        assert_eq!(orient2d(a2, b2, c2), orient2d_with::<Expansion>(a2, b2, c2));
        assert_eq!(orient2d(a2, b2, c2), orient2d_with::<Fixed>(a2, b2, c2));
    }
}
