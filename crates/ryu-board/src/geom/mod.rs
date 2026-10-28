//! Integer geometry: the vocabulary every later stage measures in.
//!
//! Coordinates are integer nanometres and angles integer millidegrees.
//! Every coordinate on a board lies within ±[`COORD_LIMIT`], so the product
//! of two coordinate differences fits in `i128` exactly, which stage 1's
//! exact predicates rely on.

pub mod decimal;
mod edge;

pub use edge::{Arc, Dir, Edge, Segment};

/// Integer nanometres.
pub type Nm = i64;

/// The largest coordinate magnitude on a board: 2^40 nm, about 1.1 km.
pub const COORD_LIMIT: Nm = 1 << 40;

/// Integer millidegrees, counterclockwise, in `[0, 360000)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mdeg(i32);

impl Mdeg {
    pub const ZERO: Mdeg = Mdeg(0);
    /// One whole turn in millidegrees.
    pub const TURN: i64 = 360_000;

    /// Any whole number of millidegrees, reduced into `[0, 360000)`.
    pub fn new(v: i64) -> Mdeg {
        Mdeg(v.rem_euclid(Self::TURN) as i32)
    }

    pub fn get(self) -> i32 {
        self.0
    }

    /// The number of quarter turns, when the angle is a multiple of 90°.
    pub fn quarter_turns(self) -> Option<u8> {
        (self.0 % 90_000 == 0).then_some((self.0 / 90_000) as u8)
    }

    pub fn radians(self) -> f64 {
        f64::from(self.0) * (std::f64::consts::PI / 180_000.0)
    }

    /// The angle measured clockwise, as ODB++ writes rotations.
    pub fn from_clockwise(v: i64) -> Mdeg {
        Mdeg::new(-v)
    }
}

/// A point in integer nanometres, y up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Point {
    pub x: Nm,
    pub y: Nm,
}

impl Point {
    pub const ORIGIN: Point = Point { x: 0, y: 0 };

    pub const fn new(x: Nm, y: Nm) -> Point {
        Point { x, y }
    }

    /// Whether the point lies within ±[`COORD_LIMIT`].
    pub fn in_range(self) -> bool {
        self.x.abs() <= COORD_LIMIT && self.y.abs() <= COORD_LIMIT
    }

    /// `self - o` as an exact `i128` vector.
    pub fn minus(self, o: Point) -> V {
        V(
            i128::from(self.x) - i128::from(o.x),
            i128::from(self.y) - i128::from(o.y),
        )
    }

    pub fn offset(self, dx: Nm, dy: Nm) -> Point {
        Point::new(self.x + dx, self.y + dy)
    }
}

/// An exact vector between two points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct V(pub i128, pub i128);

impl V {
    pub fn cross(self, o: V) -> i128 {
        self.0 * o.1 - self.1 * o.0
    }

    pub fn dot(self, o: V) -> i128 {
        self.0 * o.0 + self.1 * o.1
    }

    pub fn len2(self) -> i128 {
        self.dot(self)
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0 && self.1 == 0
    }
}

/// `(b - a) × (c - a)`, exactly: positive when `a, b, c` turn counterclockwise.
pub fn orient(a: Point, b: Point, c: Point) -> i128 {
    b.minus(a).cross(c.minus(a))
}

/// `n / d` rounded to the nearest integer, ties to even. `d` must not be 0.
pub fn div_round(n: i128, d: i128) -> i128 {
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    let q = n.div_euclid(d);
    let r = n.rem_euclid(d);
    match (2 * r).cmp(&d) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => q + (q & 1),
    }
}

/// An `f64` rounded to the nearest nanometre, ties to even.
pub fn round_nm(v: f64) -> Nm {
    v.round_ties_even() as Nm
}

/// The smallest integer at least `sqrt(n)`.
pub fn ceil_sqrt(n: i128) -> i128 {
    let s = (n as u128).isqrt() as i128;
    if s * s < n { s + 1 } else { s }
}

/// The lower and upper bounds of a size centred on 0: an odd size puts its
/// extra nanometre on the positive side, so sizes stay exact.
pub fn span(size: Nm) -> (Nm, Nm) {
    let lo = -(size / 2);
    (lo, lo + size)
}

/// Half a size, rounded to the nearest nanometre, ties to even: a radius
/// from a diameter.
pub fn half(size: Nm) -> Nm {
    div_round(i128::from(size), 2) as Nm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_reduce_into_one_turn() {
        assert_eq!(Mdeg::new(-90_000).get(), 270_000);
        assert_eq!(Mdeg::new(720_000).get(), 0);
        assert_eq!(Mdeg::new(450_500).get(), 90_500);
        assert_eq!(Mdeg::new(180_000).quarter_turns(), Some(2));
        assert_eq!(Mdeg::new(45_000).quarter_turns(), None);
        assert_eq!(Mdeg::from_clockwise(90_000).get(), 270_000);
    }

    #[test]
    fn rounding_is_to_nearest_even() {
        assert_eq!(div_round(5, 2), 2);
        assert_eq!(div_round(7, 2), 4);
        assert_eq!(div_round(-5, 2), -2);
        assert_eq!(div_round(-7, 2), -4);
        assert_eq!(div_round(10, 3), 3);
        assert_eq!(div_round(11, 3), 4);
        assert_eq!(div_round(5, -2), -2);
        assert_eq!(round_nm(2.5), 2);
        assert_eq!(round_nm(-3.5), -4);
        assert_eq!(round_nm(1.4999), 1);
    }

    #[test]
    fn spans_keep_odd_sizes_exact() {
        assert_eq!(span(4), (-2, 2));
        assert_eq!(span(3), (-1, 2));
        assert_eq!(half(3), 2);
        assert_eq!(half(5), 2);
        assert_eq!(ceil_sqrt(16), 4);
        assert_eq!(ceil_sqrt(17), 5);
        assert_eq!(ceil_sqrt(0), 0);
    }

    #[test]
    fn products_at_the_limit_fit() {
        let a = Point::new(-COORD_LIMIT, -COORD_LIMIT);
        let b = Point::new(COORD_LIMIT, -COORD_LIMIT);
        let c = Point::new(COORD_LIMIT, COORD_LIMIT);
        assert_eq!(
            orient(a, b, c),
            4 * i128::from(COORD_LIMIT) * i128::from(COORD_LIMIT)
        );
        assert!(!Point::new(COORD_LIMIT + 1, 0).in_range());
    }
}
