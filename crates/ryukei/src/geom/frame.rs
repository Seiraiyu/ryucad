use super::{Point3, Rigid, UnitVector3, Vector3};
use crate::Error;
use crate::math;
use crate::tol::ANGULAR_RESOLUTION;
use core::f64::consts::TAU;

/// A right-handed orthonormal frame: an origin, an axis `z` and a reference
/// direction `x` perpendicular to it, with `y = z × x`.
///
/// Every analytic curve and surface sits in a frame, and the frame fixes
/// where its parameter zero is: angles are measured from `x`,
/// counterclockwise about `z`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    origin: Point3,
    x: UnitVector3,
    z: UnitVector3,
}

impl Frame {
    /// The model's own axes at the origin.
    pub const WORLD: Self = Self {
        origin: Point3::ORIGIN,
        x: UnitVector3::X,
        z: UnitVector3::Z,
    };

    /// The checked constructor. `z` is the axis; `x_hint` is made
    /// perpendicular to it and becomes the reference direction.
    ///
    /// Fails with `NonFinite` on NaN or infinity, `OutsideSizeBox` when the
    /// origin is outside the size box, and `Degenerate` when `z` or `x_hint`
    /// has no direction or they are parallel within [`ANGULAR_RESOLUTION`].
    pub fn new(origin: Point3, z: Vector3, x_hint: Vector3) -> Result<Self, Error> {
        Vector3::try_new(z.x, z.y, z.z)?;
        Vector3::try_new(x_hint.x, x_hint.y, x_hint.z)?;
        let origin = Point3::try_new(origin.x, origin.y, origin.z)?;
        let z = z.normalize().ok_or(Error::Degenerate)?;
        let x = x_hint.normalize().ok_or(Error::Degenerate)?;
        let x = perpendicular_unit(x.get(), z).ok_or(Error::Degenerate)?;
        Ok(Self { origin, x, z })
    }

    /// The frame a rigid placement carries the world frame to: the
    /// placement's translation and its rotated x and z axes.
    pub fn from_rigid(placement: &Rigid) -> Result<Self, Error> {
        let world = Self::WORLD.transformed(placement);
        let o = world.origin;
        Point3::try_new(o.x, o.y, o.z)?;
        Ok(world)
    }

    pub fn origin(&self) -> Point3 {
        self.origin
    }

    pub fn x(&self) -> UnitVector3 {
        self.x
    }

    pub fn y(&self) -> UnitVector3 {
        UnitVector3::new_unchecked(self.z.get().cross(self.x.get()))
    }

    pub fn z(&self) -> UnitVector3 {
        self.z
    }

    /// The same frame with its axis reversed and `x` kept, so angles run
    /// the other way.
    pub fn flipped(&self) -> Self {
        Self {
            origin: self.origin,
            x: self.x,
            z: -self.z,
        }
    }

    /// The frame moved by a rigid transform.
    pub fn transformed(&self, r: &Rigid) -> Self {
        Self {
            origin: r.apply_point(self.origin),
            x: r.apply_unit(self.x),
            z: r.apply_unit(self.z),
        }
    }

    /// The model point at local coordinates `(a, b, c)` along `x`, `y`, `z`.
    pub fn to_world(&self, a: f64, b: f64, c: f64) -> Point3 {
        self.origin + self.x.get() * a + self.y().get() * b + self.z.get() * c
    }

    /// The local coordinates of a model point along `x`, `y`, `z`.
    pub fn to_local(&self, p: Point3) -> Vector3 {
        let d = p - self.origin;
        Vector3::new(
            d.dot(self.x.get()),
            d.dot(self.y().get()),
            d.dot(self.z.get()),
        )
    }

    /// `cos(angle) x + sin(angle) y`: the radial direction at an angle.
    pub fn radial(&self, angle: f64) -> Vector3 {
        let (s, c) = math::sin_cos(angle);
        self.x.get() * c + self.y().get() * s
    }

    /// The radial direction at an angle and its derivative with respect to
    /// the angle, `-sin(angle) x + cos(angle) y`. The first is bit-equal to
    /// [`Frame::radial`].
    pub fn radial_and_turn(&self, angle: f64) -> (Vector3, Vector3) {
        let (s, c) = math::sin_cos(angle);
        let (x, y) = (self.x.get(), self.y().get());
        (x * c + y * s, x * -s + y * c)
    }
}

/// `v` made perpendicular to `axis` and normalized: two Gram-Schmidt passes,
/// so the result stays orthogonal even when `v` is nearly parallel. `None`
/// when `v` is parallel to `axis` within [`ANGULAR_RESOLUTION`].
fn perpendicular_unit(v: Vector3, axis: UnitVector3) -> Option<UnitVector3> {
    let a = axis.get();
    let p = v - a * a.dot(v);
    let len = p.length();
    if len <= ANGULAR_RESOLUTION {
        return None;
    }
    let p = p / len;
    let p = p - a * a.dot(p);
    Some(UnitVector3::new_unchecked(p / p.length()))
}

/// The angle of `(x, y)` from the positive x axis, in `[0, 2π)`. The origin
/// maps to 0.
pub(crate) fn polar_angle(y: f64, x: f64) -> f64 {
    let a = math::atan2(y, x);
    let a = if a < 0.0 { a + TAU } else { a + 0.0 };
    if a >= TAU { 0.0 } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::{FRAC_PI_2, PI};

    fn close(a: Vector3, b: Vector3) -> bool {
        (a - b).length() <= 1e-15
    }

    #[test]
    fn new_orthogonalizes_the_reference_direction() {
        let f = Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 2.0),
            Vector3::new(1.0, 0.0, 1.0),
        )
        .unwrap();
        assert_eq!(f.origin(), Point3::new(1.0, 2.0, 3.0));
        assert_eq!(f.z(), UnitVector3::Z);
        assert_eq!(f.x(), UnitVector3::X);
        assert_eq!(f.y(), UnitVector3::Y);
    }

    #[test]
    fn nearly_parallel_hint_still_gives_an_orthonormal_frame() {
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1e-9, 0.0, 1.0),
        )
        .unwrap();
        assert!(f.x().dot(f.z()).abs() <= 1e-16);
        assert!(close(f.x().get(), Vector3::new(1.0, 0.0, 0.0)));
    }

    #[test]
    fn new_rejects_bad_input() {
        let o = Point3::ORIGIN;
        let z = Vector3::new(0.0, 0.0, 1.0);
        assert_eq!(Frame::new(o, z, z), Err(Error::Degenerate));
        assert_eq!(Frame::new(o, Vector3::ZERO, z), Err(Error::Degenerate));
        assert_eq!(
            Frame::new(o, z, Vector3::new(f64::NAN, 0.0, 0.0)),
            Err(Error::NonFinite)
        );
        assert_eq!(
            Frame::new(Point3::new(600.0, 0.0, 0.0), z, Vector3::new(1.0, 0.0, 0.0)),
            Err(Error::OutsideSizeBox)
        );
    }

    #[test]
    fn from_rigid_carries_the_axes() {
        let r = Rigid::translation(Vector3::new(1.0, 0.0, 0.0))
            .then_after(&Rigid::rotation(UnitVector3::Z, FRAC_PI_2));
        let f = Frame::from_rigid(&r).unwrap();
        assert_eq!(f.origin(), Point3::new(1.0, 0.0, 0.0));
        assert!(close(f.x().get(), Vector3::new(0.0, 1.0, 0.0)));
        assert!(close(f.y().get(), Vector3::new(-1.0, 0.0, 0.0)));
        assert_eq!(f.z(), UnitVector3::Z);
        let far = Rigid::translation(Vector3::new(0.0, 0.0, 501.0));
        assert_eq!(Frame::from_rigid(&far), Err(Error::OutsideSizeBox));
    }

    #[test]
    fn local_and_world_round_trip() {
        let f = Frame::new(
            Point3::new(1.0, 1.0, 1.0),
            Vector3::new(1.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        )
        .unwrap();
        let p = f.to_world(0.5, -2.0, 3.0);
        assert!(close(f.to_local(p), Vector3::new(0.5, -2.0, 3.0)));
        assert!(close(f.radial(0.0), f.x().get()));
        assert!(close(f.radial(FRAC_PI_2), f.y().get()));
        let g = f.flipped();
        assert_eq!(g.z(), -f.z());
        assert!(close(g.y().get(), -f.y().get()));
    }

    #[test]
    fn polar_angle_range() {
        assert_eq!(polar_angle(0.0, 1.0), 0.0);
        assert_eq!(polar_angle(-0.0, 1.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(polar_angle(0.0, 0.0), 0.0);
        assert_eq!(polar_angle(1.0, 0.0), FRAC_PI_2);
        assert_eq!(polar_angle(0.0, -1.0), PI);
        assert_eq!(polar_angle(-1.0, 0.0), 3.0 * FRAC_PI_2);
        // Tiny negative angles round up to 2π, which wraps to 0.
        assert_eq!(polar_angle(-1e-300, 1.0), 0.0);
    }
}
