//! Exact reference signs for the robust predicates, and exact rationals
//! for checking interval arithmetic.
//!
//! Predicate references scale every f64 by 2^1074 into an exact `BigInt`
//! (every finite f64 is a multiple of 2^-1074) and evaluate the determinant
//! by plain cofactor expansion. The determinants are homogeneous, so the
//! common scale does not change the sign. Points are plain arrays so this
//! crate does not depend on ryukei.

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Signed, Zero};
use std::cmp::Ordering;

type Q = BigInt;

/// An exact rational, for references beyond the predicates.
pub type Rational = BigRational;

/// The exact value of a finite f64.
pub fn rational(x: f64) -> Rational {
    Rational::from_float(x).expect("finite input")
}

/// True when `lo <= r <= hi` exactly. Infinite bounds are unbounded.
pub fn within(lo: f64, hi: f64, r: &Rational) -> bool {
    (lo == f64::NEG_INFINITY || rational(lo) <= *r) && (hi == f64::INFINITY || *r <= rational(hi))
}

/// `x * 2^1074`, an exact integer for every finite f64.
fn q(x: f64) -> Q {
    assert!(x.is_finite(), "finite input");
    let scaled =
        Rational::from_float(x).unwrap() * Rational::from_integer(BigInt::from(1) << 1074u32);
    assert!(scaled.is_integer());
    scaled.to_integer()
}

fn sign(x: &Q) -> Ordering {
    if x.is_zero() {
        Ordering::Equal
    } else if x.is_positive() {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

fn sub2(a: [f64; 2], b: [f64; 2]) -> [Q; 2] {
    [q(a[0]) - q(b[0]), q(a[1]) - q(b[1])]
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [Q; 3] {
    [q(a[0]) - q(b[0]), q(a[1]) - q(b[1]), q(a[2]) - q(b[2])]
}

fn det2(a: &Q, b: &Q, c: &Q, d: &Q) -> Q {
    a * d - b * c
}

fn det3(m: &[[Q; 3]; 3]) -> Q {
    &m[0][0] * det2(&m[1][1], &m[1][2], &m[2][1], &m[2][2])
        - &m[0][1] * det2(&m[1][0], &m[1][2], &m[2][0], &m[2][2])
        + &m[0][2] * det2(&m[1][0], &m[1][1], &m[2][0], &m[2][1])
}

/// Sign of `| a-c ; b-c |`. Greater means a, b, c are counterclockwise.
pub fn orient2d(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Ordering {
    let [ax, ay] = sub2(a, c);
    let [bx, by] = sub2(b, c);
    sign(&det2(&ax, &ay, &bx, &by))
}

/// Sign of `| a-d ; b-d ; c-d |`. Greater means d is below the plane of
/// a, b, c, where a, b, c appear counterclockwise seen from above.
pub fn orient3d(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) -> Ordering {
    sign(&det3(&[sub3(a, d), sub3(b, d), sub3(c, d)]))
}

fn lift2(p: &[Q; 2]) -> Q {
    &p[0] * &p[0] + &p[1] * &p[1]
}

fn lift3(p: &[Q; 3]) -> Q {
    &p[0] * &p[0] + &p[1] * &p[1] + &p[2] * &p[2]
}

/// Greater means d is inside the circle through counterclockwise a, b, c.
pub fn incircle(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> Ordering {
    let rows = [a, b, c].map(|p| {
        let v = sub2(p, d);
        let l = lift2(&v);
        let [x, y] = v;
        [x, y, l]
    });
    sign(&det3(&rows))
}

/// Greater means e is inside the sphere through a, b, c, d when
/// `orient3d(a, b, c, d)` is Greater.
pub fn insphere(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3], e: [f64; 3]) -> Ordering {
    let rows: Vec<[Q; 4]> = [a, b, c, d]
        .iter()
        .map(|p| {
            let v = sub3(*p, e);
            let l = lift3(&v);
            let [x, y, z] = v;
            [x, y, z, l]
        })
        .collect();
    // Laplace expansion along the first column.
    let mut det = Q::from(0);
    for (i, row) in rows.iter().enumerate() {
        let minor: Vec<[Q; 3]> = rows
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, r)| [r[1].clone(), r[2].clone(), r[3].clone()])
            .collect();
        let m = det3(&[minor[0].clone(), minor[1].clone(), minor[2].clone()]);
        let term = &row[0] * m;
        if i % 2 == 0 {
            det += term;
        } else {
            det -= term;
        }
    }
    sign(&det)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::*;

    #[test]
    fn orient2d_basic() {
        assert_eq!(orient2d([0.0, 0.0], [1.0, 0.0], [0.0, 1.0]), Greater);
        assert_eq!(orient2d([0.0, 0.0], [0.0, 1.0], [1.0, 0.0]), Less);
        assert_eq!(orient2d([0.0, 0.0], [1.0, 1.0], [2.0, 2.0]), Equal);
    }

    #[test]
    fn orient3d_basic() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert_eq!(orient3d(a, b, c, [0.0, 0.0, -1.0]), Greater);
        assert_eq!(orient3d(a, b, c, [0.0, 0.0, 1.0]), Less);
        assert_eq!(orient3d(a, b, c, [5.0, 5.0, 0.0]), Equal);
    }

    #[test]
    fn incircle_basic() {
        let (a, b, c) = ([1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]);
        assert_eq!(incircle(a, b, c, [0.0, 0.0]), Greater);
        assert_eq!(incircle(a, b, c, [2.0, 0.0]), Less);
        assert_eq!(incircle(a, b, c, [0.0, -1.0]), Equal);
    }

    #[test]
    fn insphere_basic() {
        let (a, b, c, d) = (
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0],
        );
        assert_eq!(orient3d(a, b, c, d), Greater);
        assert_eq!(insphere(a, b, c, d, [0.0, 0.0, 0.0]), Greater);
        assert_eq!(insphere(a, b, c, d, [0.0, 0.0, 2.0]), Less);
        assert_eq!(insphere(a, b, c, d, [0.0, 0.0, 1.0]), Equal);
    }
}
