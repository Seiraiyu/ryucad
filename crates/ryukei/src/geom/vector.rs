use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A displacement in the plane.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vector2 {
    pub x: f64,
    pub y: f64,
}

/// A displacement in model space (meters).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A direction: a [`Vector3`] of length 1 (within rounding). Only built by
/// [`Vector3::normalize`] and the axis constants.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(transparent)]
pub struct UnitVector3(Vector3);

impl Vector2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn dot(self, rhs: Self) -> f64 {
        self.x * rhs.x + self.y * rhs.y
    }

    /// The z component of the 3D cross product.
    pub fn cross(self, rhs: Self) -> f64 {
        self.x * rhs.y - self.y * rhs.x
    }

    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    pub fn length(self) -> f64 {
        math::sqrt(self.length_squared())
    }
}

impl Vector3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// Summed x, then y, then z; never fused.
    pub fn dot(self, rhs: Self) -> f64 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    pub fn cross(self, rhs: Self) -> Self {
        Self::new(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }

    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    pub fn length(self) -> f64 {
        math::sqrt(self.length_squared())
    }

    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y), self.z.min(rhs.z))
    }

    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y), self.z.max(rhs.z))
    }

    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs(), self.z.abs())
    }

    /// `self + (other - self) * t`.
    pub fn lerp(self, other: Self, t: f64) -> Self {
        self + (other - self) * t
    }

    /// The direction of `self`, or `None` when it is shorter than
    /// [`LINEAR_RESOLUTION`].
    pub fn normalize(self) -> Option<UnitVector3> {
        let len = self.length();
        (len > LINEAR_RESOLUTION).then(|| UnitVector3(self / len))
    }
}

impl UnitVector3 {
    pub const X: Self = Self(Vector3::new(1.0, 0.0, 0.0));
    pub const Y: Self = Self(Vector3::new(0.0, 1.0, 0.0));
    pub const Z: Self = Self(Vector3::new(0.0, 0.0, 1.0));

    pub fn get(self) -> Vector3 {
        self.0
    }

    pub fn dot(self, rhs: Self) -> f64 {
        self.0.dot(rhs.0)
    }
}

impl Neg for UnitVector3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl From<UnitVector3> for Vector3 {
    fn from(u: UnitVector3) -> Self {
        u.0
    }
}

macro_rules! vector_ops {
    ($t:ident { $($f:ident),+ }) => {
        impl Add for $t {
            type Output = Self;
            fn add(self, rhs: Self) -> Self { Self { $($f: self.$f + rhs.$f),+ } }
        }
        impl AddAssign for $t {
            fn add_assign(&mut self, rhs: Self) { *self = *self + rhs; }
        }
        impl Sub for $t {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self { Self { $($f: self.$f - rhs.$f),+ } }
        }
        impl SubAssign for $t {
            fn sub_assign(&mut self, rhs: Self) { *self = *self - rhs; }
        }
        impl Neg for $t {
            type Output = Self;
            fn neg(self) -> Self { Self { $($f: -self.$f),+ } }
        }
        impl Mul<f64> for $t {
            type Output = Self;
            fn mul(self, s: f64) -> Self { Self { $($f: self.$f * s),+ } }
        }
        impl Mul<$t> for f64 {
            type Output = $t;
            fn mul(self, v: $t) -> $t { v * self }
        }
        impl Div<f64> for $t {
            type Output = Self;
            fn div(self, s: f64) -> Self { Self { $($f: self.$f / s),+ } }
        }
    };
}

vector_ops!(Vector2 { x, y });
vector_ops!(Vector3 { x, y, z });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(4.0, 5.0, 6.0);
        assert_eq!(a + b, Vector3::new(5.0, 7.0, 9.0));
        assert_eq!(b - a, Vector3::new(3.0, 3.0, 3.0));
        assert_eq!(-a, Vector3::new(-1.0, -2.0, -3.0));
        assert_eq!(a * 2.0, 2.0 * a);
        assert_eq!(b / 2.0, Vector3::new(2.0, 2.5, 3.0));
        assert_eq!(Vector2::new(1.0, 2.0) * 3.0, Vector2::new(3.0, 6.0));
    }

    #[test]
    fn dot_cross_length() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(4.0, 5.0, 6.0);
        assert_eq!(a.dot(b), 32.0);
        assert_eq!(a.cross(b), Vector3::new(-3.0, 6.0, -3.0));
        assert_eq!(Vector3::new(2.0, 3.0, 6.0).length(), 7.0);
        assert_eq!(Vector2::new(1.0, 0.0).cross(Vector2::new(0.0, 1.0)), 1.0);
        assert_eq!(Vector2::new(3.0, 4.0).length(), 5.0);
    }

    #[test]
    fn min_max_abs_lerp() {
        let a = Vector3::new(1.0, -5.0, 3.0);
        let b = Vector3::new(-1.0, 5.0, 3.0);
        assert_eq!(a.min(b), Vector3::new(-1.0, -5.0, 3.0));
        assert_eq!(a.max(b), Vector3::new(1.0, 5.0, 3.0));
        assert_eq!(a.abs(), Vector3::new(1.0, 5.0, 3.0));
        assert_eq!(a.lerp(b, 0.5), Vector3::new(0.0, 0.0, 3.0));
    }

    #[test]
    fn normalize_respects_resolution() {
        let u = Vector3::new(0.0, 3.0, 4.0).normalize().unwrap();
        assert_eq!(u.get(), Vector3::new(0.0, 0.6, 0.8));
        assert!(
            Vector3::new(LINEAR_RESOLUTION, 0.0, 0.0)
                .normalize()
                .is_none()
        );
        assert!(
            Vector3::new(2.0 * LINEAR_RESOLUTION, 0.0, 0.0)
                .normalize()
                .is_some()
        );
        assert!(Vector3::ZERO.normalize().is_none());
    }

    #[test]
    fn unit_axes() {
        assert_eq!(UnitVector3::X.dot(UnitVector3::Y), 0.0);
        assert_eq!((-UnitVector3::Z).get(), Vector3::new(0.0, 0.0, -1.0));
        assert_eq!(Vector3::from(UnitVector3::Y), Vector3::new(0.0, 1.0, 0.0));
    }
}
