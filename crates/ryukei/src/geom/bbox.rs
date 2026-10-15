use super::{Affine, Point3, Rigid, Vector3};

/// An axis-aligned bounding box. Always non-empty: `min <= max` on each axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box3 {
    min: Point3,
    max: Point3,
}

impl Box3 {
    /// The box spanned by two opposite corners, in any order.
    pub fn new(a: Point3, b: Point3) -> Self {
        Self {
            min: a.min(b),
            max: a.max(b),
        }
    }

    pub fn from_point(p: Point3) -> Self {
        Self { min: p, max: p }
    }

    /// The smallest box holding every point, or `None` for no points.
    pub fn from_points(points: impl IntoIterator<Item = Point3>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = Self::from_point(it.next()?);
        Some(it.fold(first, |b, p| b.union(&Self::from_point(p))))
    }

    pub fn min(&self) -> Point3 {
        self.min
    }

    pub fn max(&self) -> Point3 {
        self.max
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// True when the boxes overlap or touch.
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
            && self.min.z <= other.max.z
            && other.min.z <= self.max.z
    }

    /// Grown by `d` on every side.
    pub fn grow(&self, d: f64) -> Self {
        let v = Vector3::new(d, d, d);
        Self {
            min: self.min - v,
            max: self.max + v,
        }
    }

    pub fn corners(&self) -> [Point3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Point3::new(a.x, a.y, a.z),
            Point3::new(b.x, a.y, a.z),
            Point3::new(a.x, b.y, a.z),
            Point3::new(b.x, b.y, a.z),
            Point3::new(a.x, a.y, b.z),
            Point3::new(b.x, a.y, b.z),
            Point3::new(a.x, b.y, b.z),
            Point3::new(b.x, b.y, b.z),
        ]
    }

    /// The box around the transformed corners (may be larger than the
    /// tightest box of the transformed contents).
    pub fn transform_rigid(&self, t: &Rigid) -> Self {
        Self::from_points(self.corners().map(|p| t.apply_point(p))).expect("8 corners")
    }

    pub fn transform_affine(&self, t: &Affine) -> Self {
        Self::from_points(self.corners().map(|p| t.apply_point(p))).expect("8 corners")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::UnitVector3;
    use core::f64::consts::FRAC_PI_4;

    #[test]
    fn new_orders_corners() {
        let b = Box3::new(Point3::new(1.0, -1.0, 2.0), Point3::new(-1.0, 1.0, 0.0));
        assert_eq!(b.min(), Point3::new(-1.0, -1.0, 0.0));
        assert_eq!(b.max(), Point3::new(1.0, 1.0, 2.0));
    }

    #[test]
    fn from_points_and_union() {
        assert!(Box3::from_points([]).is_none());
        let b =
            Box3::from_points([Point3::new(1.0, 0.0, 0.0), Point3::new(0.0, 2.0, -1.0)]).unwrap();
        assert_eq!(
            b,
            Box3::new(Point3::new(0.0, 0.0, -1.0), Point3::new(1.0, 2.0, 0.0))
        );
        let u = b.union(&Box3::from_point(Point3::new(5.0, 5.0, 5.0)));
        assert_eq!(u.max(), Point3::new(5.0, 5.0, 5.0));
    }

    #[test]
    fn intersects_including_touching() {
        let a = Box3::new(Point3::ORIGIN, Point3::new(1.0, 1.0, 1.0));
        let touching = Box3::new(Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, 1.0, 1.0));
        let apart = Box3::new(Point3::new(1.5, 0.0, 0.0), Point3::new(2.0, 1.0, 1.0));
        assert!(a.intersects(&touching));
        assert!(!a.intersects(&apart));
        assert!(a.grow(0.5).intersects(&apart));
    }

    #[test]
    fn transforms_cover_corners() {
        let a = Box3::new(Point3::ORIGIN, Point3::new(1.0, 1.0, 1.0));
        let r = Rigid::rotation(UnitVector3::Z, FRAC_PI_4);
        let rb = a.transform_rigid(&r);
        let h = core::f64::consts::SQRT_2 / 2.0;
        assert!((rb.min().x + h).abs() < 1e-15);
        assert!((rb.max().x - h).abs() < 1e-15);
        let sb = a.transform_affine(&Affine::scale(Vector3::new(2.0, -1.0, 1.0)));
        assert_eq!(
            sb,
            Box3::new(Point3::new(0.0, -1.0, 0.0), Point3::new(2.0, 0.0, 1.0))
        );
    }
}
