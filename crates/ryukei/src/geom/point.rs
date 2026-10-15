use super::{Vector2, Vector3};
use crate::Error;
use crate::tol;
use core::ops::{Add, AddAssign, Sub, SubAssign};

/// A position in the plane. Point - Point is a [`Vector2`]; Point + Point
/// does not exist.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

/// A position in model space (meters).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Point3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Point2 {
    pub const ORIGIN: Self = Self::new(0.0, 0.0);

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn to_vector(self) -> Vector2 {
        Vector2::new(self.x, self.y)
    }

    /// `self + (other - self) * t`.
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Point3 {
    pub const ORIGIN: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// The checked constructor for coordinates from outside the kernel:
    /// rejects NaN and infinity, and points outside the size box.
    pub fn try_new(x: f64, y: f64, z: f64) -> Result<Self, Error> {
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            return Err(Error::NonFinite);
        }
        let p = Self::new(x, y, z);
        if tol::in_size_box(p) {
            Ok(p)
        } else {
            Err(Error::OutsideSizeBox)
        }
    }

    pub fn to_vector(self) -> Vector3 {
        Vector3::new(self.x, self.y, self.z)
    }

    pub fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// `self + (other - self) * t`.
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }

    pub fn distance(self, other: Self) -> f64 {
        (other - self).length()
    }

    pub fn min(self, other: Self) -> Self {
        Self::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    pub fn max(self, other: Self) -> Self {
        Self::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }
}

impl Sub for Point2 {
    type Output = Vector2;
    fn sub(self, rhs: Self) -> Vector2 {
        Vector2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Add<Vector2> for Point2 {
    type Output = Self;
    fn add(self, rhs: Vector2) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub<Vector2> for Point2 {
    type Output = Self;
    fn sub(self, rhs: Vector2) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Sub for Point3 {
    type Output = Vector3;
    fn sub(self, rhs: Self) -> Vector3 {
        Vector3::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Add<Vector3> for Point3 {
    type Output = Self;
    fn add(self, rhs: Vector3) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl AddAssign<Vector3> for Point3 {
    fn add_assign(&mut self, rhs: Vector3) {
        *self = *self + rhs;
    }
}

impl Sub<Vector3> for Point3 {
    type Output = Self;
    fn sub(self, rhs: Vector3) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl SubAssign<Vector3> for Point3 {
    fn sub_assign(&mut self, rhs: Vector3) {
        *self = *self - rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_minus_point_is_vector() {
        let v: Vector3 = Point3::new(4.0, 6.0, 8.0) - Point3::new(1.0, 2.0, 3.0);
        assert_eq!(v, Vector3::new(3.0, 4.0, 5.0));
        let w: Vector2 = Point2::new(3.0, 1.0) - Point2::new(1.0, 1.0);
        assert_eq!(w, Vector2::new(2.0, 0.0));
    }

    #[test]
    fn point_plus_vector_is_point() {
        let mut p = Point3::new(1.0, 2.0, 3.0) + Vector3::new(1.0, 1.0, 1.0);
        assert_eq!(p, Point3::new(2.0, 3.0, 4.0));
        p -= Vector3::new(2.0, 3.0, 4.0);
        assert_eq!(p, Point3::ORIGIN);
        assert_eq!(
            Point2::ORIGIN + Vector2::new(1.0, 2.0),
            Point2::new(1.0, 2.0)
        );
    }

    #[test]
    fn try_new_validates() {
        assert_eq!(
            Point3::try_new(1.0, 2.0, 3.0),
            Ok(Point3::new(1.0, 2.0, 3.0))
        );
        assert_eq!(Point3::try_new(f64::NAN, 0.0, 0.0), Err(Error::NonFinite));
        assert_eq!(
            Point3::try_new(0.0, f64::INFINITY, 0.0),
            Err(Error::NonFinite)
        );
        assert_eq!(
            Point3::try_new(0.0, 0.0, -501.0),
            Err(Error::OutsideSizeBox)
        );
    }

    #[test]
    fn lerp_distance_min_max() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(2.0, 4.0, -4.0);
        assert_eq!(a.lerp(b, 0.5), Point3::new(1.0, 2.0, -2.0));
        assert_eq!(a.distance(b), 6.0);
        assert_eq!(a.min(b), Point3::new(0.0, 0.0, -4.0));
        assert_eq!(a.max(b), Point3::new(2.0, 4.0, 0.0));
    }
}
