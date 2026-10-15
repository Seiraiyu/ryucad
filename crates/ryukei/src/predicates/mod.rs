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

use crate::geom::{Point2, Point3, Vector3};
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
const ICC_BOUND: f64 = (10.0 + 96.0 * EPS) * EPS;
const ISP_BOUND: f64 = (16.0 + 224.0 * EPS) * EPS;

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

/// Positive when `d` lies inside the circle through `a`, `b`, `c`, given
/// that `a`, `b`, `c` are counterclockwise (the sign flips if they are
/// clockwise). Negative outside, zero on the circle.
pub fn incircle(a: Point2, b: Point2, c: Point2, d: Point2) -> Sign {
    if !expansion_safe(&[a.x, a.y, b.x, b.y, c.x, c.y, d.x, d.y]) {
        return incircle_with::<Fixed>(a, b, c, d);
    }
    let (adx, ady) = (a.x - d.x, a.y - d.y);
    let (bdx, bdy) = (b.x - d.x, b.y - d.y);
    let (cdx, cdy) = (c.x - d.x, c.y - d.y);
    let (bdxcdy, cdxbdy) = (bdx * cdy, cdx * bdy);
    let (cdxady, adxcdy) = (cdx * ady, adx * cdy);
    let (adxbdy, bdxady) = (adx * bdy, bdx * ady);
    let alift = adx * adx + ady * ady;
    let blift = bdx * bdx + bdy * bdy;
    let clift = cdx * cdx + cdy * cdy;
    let det = alift * (bdxcdy - cdxbdy) + blift * (cdxady - adxcdy) + clift * (adxbdy - bdxady);
    let permanent = (bdxcdy.abs() + cdxbdy.abs()) * alift
        + (cdxady.abs() + adxcdy.abs()) * blift
        + (adxbdy.abs() + bdxady.abs()) * clift;
    Sign::filtered(det, ICC_BOUND * permanent)
        .unwrap_or_else(|| incircle_with::<Expansion>(a, b, c, d))
}

fn incircle_with<E: Exact>(a: Point2, b: Point2, c: Point2, d: Point2) -> Sign {
    let (adx, ady) = (E::diff(a.x, d.x), E::diff(a.y, d.y));
    let (bdx, bdy) = (E::diff(b.x, d.x), E::diff(b.y, d.y));
    let (cdx, cdy) = (E::diff(c.x, d.x), E::diff(c.y, d.y));
    let lift = |x: &E, y: &E| x.mul(x).add(&y.mul(y));
    let det = lift(&adx, &ady)
        .mul(&cross2(&bdx, &bdy, &cdx, &cdy))
        .add(&lift(&bdx, &bdy).mul(&cross2(&cdx, &cdy, &adx, &ady)))
        .add(&lift(&cdx, &cdy).mul(&cross2(&adx, &ady, &bdx, &bdy)));
    Sign::of(det.signum())
}

/// Positive when `e` lies inside the sphere through `a`, `b`, `c`, `d`,
/// given that `orient3d(a, b, c, d)` is positive (the sign flips if it is
/// negative). Negative outside, zero on the sphere.
pub fn insphere(a: Point3, b: Point3, c: Point3, d: Point3, e: Point3) -> Sign {
    let coords = [a, b, c, d, e].map(Point3::to_array);
    if !expansion_safe(coords.as_flattened()) {
        return insphere_with::<Fixed>(a, b, c, d, e);
    }
    let (aex, aey, aez) = (a.x - e.x, a.y - e.y, a.z - e.z);
    let (bex, bey, bez) = (b.x - e.x, b.y - e.y, b.z - e.z);
    let (cex, cey, cez) = (c.x - e.x, c.y - e.y, c.z - e.z);
    let (dex, dey, dez) = (d.x - e.x, d.y - e.y, d.z - e.z);

    let (aexbey, bexaey) = (aex * bey, bex * aey);
    let (bexcey, cexbey) = (bex * cey, cex * bey);
    let (cexdey, dexcey) = (cex * dey, dex * cey);
    let (dexaey, aexdey) = (dex * aey, aex * dey);
    let (aexcey, cexaey) = (aex * cey, cex * aey);
    let (bexdey, dexbey) = (bex * dey, dex * bey);
    let ab = aexbey - bexaey;
    let bc = bexcey - cexbey;
    let cd = cexdey - dexcey;
    let da = dexaey - aexdey;
    let ac = aexcey - cexaey;
    let bd = bexdey - dexbey;

    let abc = aez * bc - bez * ac + cez * ab;
    let bcd = bez * cd - cez * bd + dez * bc;
    let cda = cez * da + dez * ac + aez * cd;
    let dab = dez * ab + aez * bd + bez * da;

    let alift = aex * aex + aey * aey + aez * aez;
    let blift = bex * bex + bey * bey + bez * bez;
    let clift = cex * cex + cey * cey + cez * cez;
    let dlift = dex * dex + dey * dey + dez * dez;

    let det = (dlift * abc - clift * dab) + (blift * cda - alift * bcd);

    let (aez, bez, cez, dez) = (aez.abs(), bez.abs(), cez.abs(), dez.abs());
    let p_ab = aexbey.abs() + bexaey.abs();
    let p_bc = bexcey.abs() + cexbey.abs();
    let p_cd = cexdey.abs() + dexcey.abs();
    let p_da = dexaey.abs() + aexdey.abs();
    let p_ac = aexcey.abs() + cexaey.abs();
    let p_bd = bexdey.abs() + dexbey.abs();
    let permanent = (p_cd * bez + p_bd * cez + p_bc * dez) * alift
        + (p_da * cez + p_ac * dez + p_cd * aez) * blift
        + (p_ab * dez + p_bd * aez + p_da * bez) * clift
        + (p_bc * aez + p_ac * bez + p_ab * cez) * dlift;
    Sign::filtered(det, ISP_BOUND * permanent)
        .unwrap_or_else(|| insphere_with::<Expansion>(a, b, c, d, e))
}

fn insphere_with<E: Exact>(a: Point3, b: Point3, c: Point3, d: Point3, e: Point3) -> Sign {
    let diff = |p: Point3| (E::diff(p.x, e.x), E::diff(p.y, e.y), E::diff(p.z, e.z));
    let (aex, aey, aez) = diff(a);
    let (bex, bey, bez) = diff(b);
    let (cex, cey, cez) = diff(c);
    let (dex, dey, dez) = diff(d);

    let ab = cross2(&aex, &aey, &bex, &bey);
    let bc = cross2(&bex, &bey, &cex, &cey);
    let cd = cross2(&cex, &cey, &dex, &dey);
    let da = cross2(&dex, &dey, &aex, &aey);
    let ac = cross2(&aex, &aey, &cex, &cey);
    let bd = cross2(&bex, &bey, &dex, &dey);

    let abc = aez.mul(&bc).sub(&bez.mul(&ac)).add(&cez.mul(&ab));
    let bcd = bez.mul(&cd).sub(&cez.mul(&bd)).add(&dez.mul(&bc));
    let cda = cez.mul(&da).add(&dez.mul(&ac)).add(&aez.mul(&cd));
    let dab = dez.mul(&ab).add(&aez.mul(&bd)).add(&bez.mul(&da));

    let lift = |x: &E, y: &E, z: &E| x.mul(x).add(&y.mul(y)).add(&z.mul(z));
    let det = lift(&dex, &dey, &dez)
        .mul(&abc)
        .sub(&lift(&cex, &cey, &cez).mul(&dab))
        .add(&lift(&bex, &bey, &bez).mul(&cda))
        .sub(&lift(&aex, &aey, &aez).mul(&bcd));
    Sign::of(det.signum())
}

/// `(x, y, z)` held exactly.
fn exact3<E: Exact>(x: f64, y: f64, z: f64) -> [E; 3] {
    [E::diff(x, 0.0), E::diff(y, 0.0), E::diff(z, 0.0)]
}

/// `r0 · (r1 × r2)`, exactly.
fn det3<E: Exact>(r0: &[E; 3], r1: &[E; 3], r2: &[E; 3]) -> E {
    r0[0]
        .mul(&cross2(&r1[1], &r1[2], &r2[1], &r2[2]))
        .add(&r0[1].mul(&cross2(&r1[2], &r1[0], &r2[2], &r2[0])))
        .add(&r0[2].mul(&cross2(&r1[0], &r1[1], &r2[0], &r2[1])))
}

/// The exact sign of the sum of `a · (b × c)` over the triangles
/// `[a, b, c]`: six times the volume a closed surface made of them
/// encloses, positive when they wind counterclockwise seen from outside.
pub fn volume_sign(triangles: &[[Point3; 3]]) -> Sign {
    let coords: Vec<f64> = triangles
        .iter()
        .flatten()
        .flat_map(|p| p.to_array())
        .collect();
    if expansion_safe(&coords) {
        volume_with::<Expansion>(triangles)
    } else {
        volume_with::<Fixed>(triangles)
    }
}

fn volume_with<E: Exact>(triangles: &[[Point3; 3]]) -> Sign {
    let total = triangles.iter().fold(E::diff(0.0, 0.0), |acc, t| {
        let [a, b, c] = t.map(|p| exact3::<E>(p.x, p.y, p.z));
        acc.add(&det3(&a, &b, &c))
    });
    Sign::of(total.signum())
}

/// The exact sign of the sum of `((b - a) × (c - a)) · n` over the
/// triangles `[a, b, c]`: positive when together they wind
/// counterclockwise about `n`.
pub fn winding_sign(triangles: &[[Point3; 3]], n: Vector3) -> Sign {
    let mut coords: Vec<f64> = triangles
        .iter()
        .flatten()
        .flat_map(|p| p.to_array())
        .collect();
    coords.extend(n.to_array());
    if expansion_safe(&coords) {
        winding_with::<Expansion>(triangles, n)
    } else {
        winding_with::<Fixed>(triangles, n)
    }
}

fn winding_with<E: Exact>(triangles: &[[Point3; 3]], n: Vector3) -> Sign {
    let n = exact3::<E>(n.x, n.y, n.z);
    let total = triangles.iter().fold(E::diff(0.0, 0.0), |acc, &[a, b, c]| {
        let u = [E::diff(b.x, a.x), E::diff(b.y, a.y), E::diff(b.z, a.z)];
        let v = [E::diff(c.x, a.x), E::diff(c.y, a.y), E::diff(c.z, a.z)];
        acc.add(&det3(&n, &u, &v))
    });
    Sign::of(total.signum())
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

    #[test]
    fn incircle_basic() {
        let (a, b, c) = (p2(1.0, 0.0), p2(0.0, 1.0), p2(-1.0, 0.0));
        assert_eq!(incircle(a, b, c, p2(0.0, 0.0)), Positive);
        assert_eq!(incircle(a, b, c, p2(2.0, 0.0)), Negative);
        assert_eq!(incircle(a, b, c, p2(0.0, -1.0)), Zero);
        // Just inside the unit circle, at the edge of f64 precision.
        assert_eq!(incircle(a, b, c, p2(0.0, -1.0 + f64::EPSILON)), Positive);
    }

    #[test]
    fn insphere_basic() {
        let (a, b, c, d) = (
            p3(1.0, 0.0, 0.0),
            p3(0.0, 1.0, 0.0),
            p3(-1.0, 0.0, 0.0),
            p3(0.0, 0.0, -1.0),
        );
        assert_eq!(orient3d(a, b, c, d), Positive);
        assert_eq!(insphere(a, b, c, d, p3(0.0, 0.0, 0.0)), Positive);
        assert_eq!(insphere(a, b, c, d, p3(0.0, 0.0, 2.0)), Negative);
        assert_eq!(insphere(a, b, c, d, p3(0.0, 0.0, 1.0)), Zero);
        assert_eq!(
            insphere(a, b, c, d, p3(0.0, 0.0, 1.0 - f64::EPSILON)),
            Positive
        );
    }

    #[test]
    fn tiny_circles_and_spheres_use_big_integers() {
        let s = 1e-160;
        let (a2, b2, c2) = (p2(s, 0.0), p2(0.0, s), p2(-s, 0.0));
        assert_eq!(incircle(a2, b2, c2, p2(0.0, 0.0)), Positive);
        assert_eq!(incircle(a2, b2, c2, p2(0.0, -s)), Zero);
        let (a3, b3, c3, d3) = (
            p3(s, 0.0, 0.0),
            p3(0.0, s, 0.0),
            p3(-s, 0.0, 0.0),
            p3(0.0, 0.0, -s),
        );
        assert_eq!(insphere(a3, b3, c3, d3, p3(0.0, 0.0, 0.0)), Positive);
        assert_eq!(insphere(a3, b3, c3, d3, p3(0.0, 0.0, 2.0 * s)), Negative);
    }

    #[test]
    fn circle_and_sphere_stages_agree_on_easy_input() {
        let (a, b, c, d) = (
            p3(1.0, 2.0, 3.0),
            p3(-4.0, 5.0, 1.0),
            p3(2.0, -1.0, 0.5),
            p3(0.1, 0.2, 9.0),
        );
        let e = p3(0.3, 0.4, 0.5);
        assert_eq!(
            insphere(a, b, c, d, e),
            insphere_with::<Expansion>(a, b, c, d, e)
        );
        assert_eq!(
            insphere(a, b, c, d, e),
            insphere_with::<Fixed>(a, b, c, d, e)
        );
        let (a2, b2, c2, d2) = (p2(1.0, 2.0), p2(-4.0, 5.0), p2(2.0, -1.0), p2(0.1, 0.2));
        assert_eq!(
            incircle(a2, b2, c2, d2),
            incircle_with::<Expansion>(a2, b2, c2, d2)
        );
        assert_eq!(
            incircle(a2, b2, c2, d2),
            incircle_with::<Fixed>(a2, b2, c2, d2)
        );
    }

    /// The unit right-angled tetrahedron's faces, counterclockwise seen
    /// from outside.
    fn tetrahedron(scale: f64) -> Vec<[Point3; 3]> {
        let v = [
            p3(0.0, 0.0, 0.0),
            p3(scale, 0.0, 0.0),
            p3(0.0, scale, 0.0),
            p3(0.0, 0.0, scale),
        ];
        [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]]
            .iter()
            .map(|t| t.map(|i| v[i]))
            .collect()
    }

    #[test]
    fn volume_sign_of_closed_surfaces() {
        assert_eq!(volume_sign(&tetrahedron(1.0)), Positive);
        let inside_out: Vec<_> = tetrahedron(1.0)
            .iter()
            .map(|&[a, b, c]| [a, c, b])
            .collect();
        assert_eq!(volume_sign(&inside_out), Negative);
        assert_eq!(volume_sign(&[]), Zero);
        // Coordinates far below 2^-100 take the big-integer path.
        assert_eq!(volume_sign(&tetrahedron(1e-200)), Positive);
        // Away from the origin the volume is the same, so the sign is too.
        let moved: Vec<_> = tetrahedron(1.0)
            .iter()
            .map(|t| t.map(|p| p3(p.x + 300.0, p.y - 250.0, p.z + 0.1)))
            .collect();
        assert_eq!(volume_sign(&moved), Positive);
    }

    #[test]
    fn winding_sign_about_a_normal() {
        let t = [[p3(0.0, 0.0, 5.0), p3(1.0, 0.0, 5.0), p3(0.0, 1.0, 5.0)]];
        let up = Vector3::new(0.0, 0.0, 1.0);
        assert_eq!(winding_sign(&t, up), Positive);
        assert_eq!(winding_sign(&t, -up), Negative);
        assert_eq!(winding_sign(&t, Vector3::new(1.0, 0.0, 0.0)), Zero);
        // A square with a clockwise hole still winds counterclockwise.
        let sq = [
            [p3(0.0, 0.0, 0.0), p3(4.0, 0.0, 0.0), p3(4.0, 4.0, 0.0)],
            [p3(0.0, 0.0, 0.0), p3(4.0, 4.0, 0.0), p3(0.0, 4.0, 0.0)],
            [p3(1.0, 1.0, 0.0), p3(1.0, 3.0, 0.0), p3(3.0, 3.0, 0.0)],
            [p3(1.0, 1.0, 0.0), p3(3.0, 3.0, 0.0), p3(3.0, 1.0, 0.0)],
        ];
        assert_eq!(winding_sign(&sq, up), Positive);
        let tiny = [[
            p3(0.0, 0.0, 0.0),
            p3(1e-300, 0.0, 0.0),
            p3(0.0, 1e-300, 0.0),
        ]];
        assert_eq!(winding_sign(&tiny, up), Positive);
    }
}
