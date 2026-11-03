//! An orthographic view: a direction and a frame in the image plane.

use crate::geom::{Point2, Point3, UnitVector3, Vector2, Vector3};
use crate::tol::ANGULAR_RESOLUTION;
use crate::{Error, ViewError};

/// An orthographic view. `towards` points from the model to the viewer;
/// `x` (right) and `y` (up) span the image plane, and `x × y = towards`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    towards: UnitVector3,
    x: UnitVector3,
    y: UnitVector3,
}

impl View {
    /// The view looking along `-towards` with `up` made square to it
    /// (`x = up × towards`, `y = towards × x`). Refused with `BadView`
    /// for a direction or `up` that isn't finite or has no length, or an
    /// `up` within [`ANGULAR_RESOLUTION`] of `±towards`.
    pub fn new(towards: Vector3, up: Vector3) -> Result<Self, Error> {
        let bad = || Error::InvalidView(ViewError::BadView);
        let finite = |v: Vector3| v.x.is_finite() && v.y.is_finite() && v.z.is_finite();
        if !finite(towards) || !finite(up) {
            return Err(bad());
        }
        let t = towards.normalize().ok_or_else(bad)?;
        let u = up.normalize().ok_or_else(bad)?;
        if u.get().cross(t.get()).length() <= ANGULAR_RESOLUTION {
            return Err(bad());
        }
        // `u × t` may be shorter than `LINEAR_RESOLUTION` (an `up` a few
        // nanoradians off the direction), so it is divided by its length
        // here rather than through `normalize`.
        let ux = u.get().cross(t.get());
        let x = UnitVector3::new_unchecked(ux / ux.length());
        let y = t.get().cross(x.get()).normalize().ok_or_else(bad)?;
        Ok(Self { towards: t, x, y })
    }

    /// From the model towards the viewer.
    pub fn towards(&self) -> UnitVector3 {
        self.towards
    }

    /// Image right.
    pub fn x(&self) -> UnitVector3 {
        self.x
    }

    /// Image up.
    pub fn y(&self) -> UnitVector3 {
        self.y
    }

    /// Where a point lands in the image plane.
    pub fn image(&self, p: Point3) -> Point2 {
        let v = p.to_vector();
        Point2::new(v.dot(self.x.get()), v.dot(self.y.get()))
    }

    /// A vector's image.
    pub fn image_vector(&self, v: Vector3) -> Vector2 {
        Vector2::new(v.dot(self.x.get()), v.dot(self.y.get()))
    }

    /// How far a point lies towards the viewer: nearer is larger.
    pub fn depth(&self, p: Point3) -> f64 {
        p.to_vector().dot(self.towards.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point3, Vector3};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-15
    }

    #[test]
    fn a_front_view_looks_along_y_with_x_right_and_z_up() {
        let v = View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        assert_eq!(v.x().get(), Vector3::new(1.0, 0.0, 0.0));
        assert_eq!(v.y().get(), Vector3::new(0.0, 0.0, 1.0));
        let p = v.image(Point3::new(0.3, 7.0, 0.2));
        assert!(close(p.x, 0.3) && close(p.y, 0.2));
        assert!(close(v.depth(Point3::new(0.3, 7.0, 0.2)), -7.0));
    }

    #[test]
    fn up_is_made_square_to_the_direction_and_the_frame_is_right_handed() {
        let v = View::new(Vector3::new(1.0, 2.0, 3.0), Vector3::new(0.1, 0.0, 1.0)).unwrap();
        let (x, y, t) = (v.x().get(), v.y().get(), v.towards().get());
        assert!(x.dot(t).abs() < 1e-15 && y.dot(t).abs() < 1e-15 && x.dot(y).abs() < 1e-15);
        assert!((x.cross(y) - t).length() < 1e-15);
        // Up keeps its side: y leans the way up does.
        assert!(y.dot(Vector3::new(0.1, 0.0, 1.0)) > 0.0);
    }

    #[test]
    fn bad_views_are_refused() {
        let bad = Err(crate::Error::InvalidView(crate::ViewError::BadView));
        let z = Vector3::new(0.0, 0.0, 1.0);
        assert_eq!(View::new(Vector3::new(f64::NAN, 0.0, 1.0), z), bad);
        assert_eq!(View::new(Vector3::ZERO, z), bad);
        assert_eq!(View::new(z, Vector3::ZERO), bad);
        assert_eq!(View::new(z, z), bad);
        assert_eq!(View::new(z, -z), bad);
        assert_eq!(View::new(z, Vector3::new(1e-12, 0.0, 1.0)), bad);
        assert!(View::new(z, Vector3::new(1e-9, 0.0, 1.0)).is_ok());
    }
}
