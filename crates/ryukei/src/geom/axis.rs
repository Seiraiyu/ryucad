//! An axis to turn about: a point and a direction.

use super::{Point3, Rigid, UnitVector3, Vector3};
use crate::Error;

/// A line with no parameter: what a revolve turns about. Turning by a
/// positive angle is counterclockwise seen from the tip of `dir`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    origin: Point3,
    dir: UnitVector3,
}

impl Axis {
    /// Fails with `NonFinite`, `OutsideSizeBox` (origin), or `Degenerate`
    /// when `dir` has no direction.
    pub fn new(origin: Point3, dir: Vector3) -> Result<Self, Error> {
        Vector3::try_new(dir.x, dir.y, dir.z)?;
        let origin = Point3::try_new(origin.x, origin.y, origin.z)?;
        let dir = dir.normalize().ok_or(Error::Degenerate)?;
        Ok(Self { origin, dir })
    }

    pub fn origin(&self) -> Point3 {
        self.origin
    }

    pub fn dir(&self) -> UnitVector3 {
        self.dir
    }

    /// The axis moved by a rigid transform.
    pub fn transformed(&self, r: &Rigid) -> Self {
        Self {
            origin: r.apply_point(self.origin),
            dir: r.apply_unit(self.dir),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction() {
        let a = Axis::new(Point3::new(1.0, 2.0, 3.0), Vector3::new(0.0, 0.0, 2.0)).unwrap();
        assert_eq!(a.origin(), Point3::new(1.0, 2.0, 3.0));
        assert_eq!(a.dir(), UnitVector3::Z);
        let o = Point3::ORIGIN;
        assert_eq!(Axis::new(o, Vector3::ZERO), Err(Error::Degenerate));
        let nan = Vector3::new(f64::NAN, 0.0, 1.0);
        assert_eq!(Axis::new(o, nan), Err(Error::NonFinite));
        let far = Point3::new(600.0, 0.0, 0.0);
        assert_eq!(
            Axis::new(far, Vector3::new(1.0, 0.0, 0.0)),
            Err(Error::OutsideSizeBox)
        );
        let moved = a.transformed(&Rigid::translation(Vector3::new(0.0, 0.0, 1.0)));
        assert_eq!(moved.origin(), Point3::new(1.0, 2.0, 4.0));
        assert_eq!(moved.dir(), UnitVector3::Z);
    }
}
