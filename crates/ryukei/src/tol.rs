//! The kernel-wide tolerance model. Model space is SI meters.
//!
//! These are fixed for every body. Unit conversion and models larger than
//! the size box are ryucad's job at its boundary.

use crate::Error;
use crate::geom::{Box3, Point3, UnitVector3};

/// Two points closer than this are the same point (10 nm).
pub const LINEAR_RESOLUTION: f64 = 1e-8;

/// Two directions closer than this (in radians) are parallel.
pub const ANGULAR_RESOLUTION: f64 = 1e-11;

/// All model geometry lies in `[-SIZE_BOX_HALF, SIZE_BOX_HALF]` on each axis.
pub const SIZE_BOX_HALF: f64 = 500.0;

/// True when `a` and `b` are within [`LINEAR_RESOLUTION`].
pub fn points_coincident(a: Point3, b: Point3) -> bool {
    (b - a).length() <= LINEAR_RESOLUTION
}

/// True when a length is within [`LINEAR_RESOLUTION`] of zero.
pub fn is_zero_length(x: f64) -> bool {
    x.abs() <= LINEAR_RESOLUTION
}

/// True when `u` and `v` are parallel or antiparallel within
/// [`ANGULAR_RESOLUTION`] (compared via the sine of the angle).
pub fn directions_parallel(u: UnitVector3, v: UnitVector3) -> bool {
    u.get().cross(v.get()).length() <= ANGULAR_RESOLUTION
}

/// The size box itself.
pub fn size_box() -> Box3 {
    let h = SIZE_BOX_HALF;
    Box3::new(Point3::new(-h, -h, -h), Point3::new(h, h, h))
}

/// True when `p` lies inside the size box (boundary included).
pub fn in_size_box(p: Point3) -> bool {
    [p.x, p.y, p.z].iter().all(|c| c.abs() <= SIZE_BOX_HALF)
}

/// A per-entity tolerance (edges, vertices): never below
/// [`LINEAR_RESOLUTION`].
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Tolerance(f64);

impl Tolerance {
    pub const RESOLUTION: Self = Self(LINEAR_RESOLUTION);

    pub fn new(value: f64) -> Result<Self, Error> {
        if !value.is_finite() {
            Err(Error::NonFinite)
        } else if value < LINEAR_RESOLUTION {
            Err(Error::Degenerate)
        } else {
            Ok(Self(value))
        }
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vector3;

    #[test]
    fn coincidence_at_the_boundary() {
        let a = Point3::new(1.0, 0.0, 0.0);
        assert!(points_coincident(a, a));
        assert!(points_coincident(a, Point3::new(1.0, 0.5e-8, 0.0)));
        assert!(!points_coincident(a, Point3::new(1.0, 2e-8, 0.0)));
        assert!(is_zero_length(-1e-8));
        assert!(!is_zero_length(1.0000001e-8));
    }

    #[test]
    fn parallel_directions() {
        let u = UnitVector3::X;
        assert!(directions_parallel(u, u));
        assert!(directions_parallel(u, -u));
        let tilted = |a: f64| Vector3::new(1.0, a, 0.0).normalize().unwrap();
        assert!(directions_parallel(u, tilted(0.5e-11)));
        assert!(!directions_parallel(u, tilted(2e-11)));
    }

    #[test]
    fn size_box() {
        assert!(in_size_box(Point3::new(500.0, -500.0, 0.0)));
        assert!(!in_size_box(Point3::new(500.000001, 0.0, 0.0)));
        assert!(!in_size_box(Point3::new(0.0, f64::NAN, 0.0)));
    }

    #[test]
    fn tolerance_floor() {
        assert_eq!(Tolerance::new(1e-6).unwrap().get(), 1e-6);
        assert_eq!(Tolerance::new(LINEAR_RESOLUTION), Ok(Tolerance::RESOLUTION));
        assert_eq!(Tolerance::new(1e-9), Err(Error::Degenerate));
        assert_eq!(Tolerance::new(f64::NAN), Err(Error::NonFinite));
        assert_eq!(Tolerance::new(f64::INFINITY), Err(Error::NonFinite));
    }
}
