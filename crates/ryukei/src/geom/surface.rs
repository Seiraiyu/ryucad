use super::curve::check_length;
use super::{Frame, Point3, Rigid, UnitVector3, polar_angle};
use crate::Error;
use crate::math;
use crate::tol::ANGULAR_RESOLUTION;
use core::f64::consts::{FRAC_PI_2, TAU};

/// A surface in model space, evaluated by parameters `(u, v)`.
///
/// Phase 1 has only the analytic surfaces. NURBS will be another variant;
/// the analytic ones stay as fast paths with closed-form distances. Each
/// natural normal points away from the axis (or along `z` for a plane); a
/// face says whether it uses that normal or its reverse.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Surface {
    Plane(Plane),
    Cylinder(Cylinder),
    Cone(Cone),
    Sphere(Sphere),
    Torus(Torus),
}

/// The frame's xy plane: `origin + u x + v y`. The normal is `z`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    frame: Frame,
}

/// `origin + r (cos u x + sin u y) + v z`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cylinder {
    frame: Frame,
    radius: f64,
}

/// A single cone nappe, widening towards `+z`:
/// `origin + (r + v tan α)(cos u x + sin u y) + v z`.
///
/// `r` is the radius in the frame's xy plane and `α` the half-angle. The
/// apex is at `v = -r / tan α`, and `v` never goes below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    frame: Frame,
    radius: f64,
    half_angle: f64,
    sin: f64,
    cos: f64,
}

/// `origin + r (cos v (cos u x + sin u y) + sin v z)`, with `v` the
/// latitude in `[-π/2, π/2]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    frame: Frame,
    radius: f64,
}

/// A ring torus about `z`:
/// `origin + (R + r cos v)(cos u x + sin u y) + r sin v z`, with `R > r`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Torus {
    frame: Frame,
    major: f64,
    minor: f64,
}

impl Plane {
    pub fn new(frame: Frame) -> Self {
        Self { frame }
    }
}

impl Cylinder {
    pub fn new(frame: Frame, radius: f64) -> Result<Self, Error> {
        Ok(Self {
            frame,
            radius: check_length(radius)?,
        })
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }
}

impl Cone {
    /// Fails with `Degenerate` unless the half-angle is strictly between 0
    /// and π/2 by more than [`ANGULAR_RESOLUTION`].
    pub fn new(frame: Frame, radius: f64, half_angle: f64) -> Result<Self, Error> {
        let radius = check_length(radius)?;
        if !half_angle.is_finite() {
            return Err(Error::NonFinite);
        }
        if half_angle <= ANGULAR_RESOLUTION || half_angle >= FRAC_PI_2 - ANGULAR_RESOLUTION {
            return Err(Error::Degenerate);
        }
        let (sin, cos) = math::sin_cos(half_angle);
        Ok(Self {
            frame,
            radius,
            half_angle,
            sin,
            cos,
        })
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }

    pub fn half_angle(&self) -> f64 {
        self.half_angle
    }

    /// The `v` of the apex.
    pub fn apex_v(&self) -> f64 {
        -self.radius * self.cos / self.sin
    }
}

impl Sphere {
    pub fn new(frame: Frame, radius: f64) -> Result<Self, Error> {
        Ok(Self {
            frame,
            radius: check_length(radius)?,
        })
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }
}

impl Torus {
    /// Fails with `Degenerate` unless `major` exceeds `minor` by at least
    /// [`crate::tol::LINEAR_RESOLUTION`]: horn and spindle tori are not
    /// supported.
    pub fn new(frame: Frame, major: f64, minor: f64) -> Result<Self, Error> {
        let major = check_length(major)?;
        let minor = check_length(minor)?;
        check_length(major - minor)?;
        Ok(Self {
            frame,
            major,
            minor,
        })
    }

    pub fn major(&self) -> f64 {
        self.major
    }

    pub fn minor(&self) -> f64 {
        self.minor
    }
}

impl Surface {
    pub fn frame(&self) -> &Frame {
        match self {
            Self::Plane(s) => &s.frame,
            Self::Cylinder(s) => &s.frame,
            Self::Cone(s) => &s.frame,
            Self::Sphere(s) => &s.frame,
            Self::Torus(s) => &s.frame,
        }
    }

    pub fn point(&self, u: f64, v: f64) -> Point3 {
        match self {
            Self::Plane(s) => s.frame.to_world(u, v, 0.0),
            Self::Cylinder(s) => {
                s.frame.origin() + s.frame.radial(u) * s.radius + s.frame.z().get() * v
            }
            Self::Cone(s) => {
                let r = s.radius + v * s.sin / s.cos;
                s.frame.origin() + s.frame.radial(u) * r + s.frame.z().get() * v
            }
            Self::Sphere(s) => {
                let (sv, cv) = math::sin_cos(v);
                s.frame.origin() + (s.frame.radial(u) * cv + s.frame.z().get() * sv) * s.radius
            }
            Self::Torus(s) => {
                let (sv, cv) = math::sin_cos(v);
                s.frame.origin()
                    + s.frame.radial(u) * (s.major + s.minor * cv)
                    + s.frame.z().get() * (s.minor * sv)
            }
        }
    }

    /// The natural unit normal at `(u, v)`. At a cone's apex it is the
    /// limit along the generator at angle `u`.
    pub fn normal(&self, u: f64, v: f64) -> UnitVector3 {
        match self {
            Self::Plane(s) => s.frame.z(),
            Self::Cylinder(s) => UnitVector3::new_unchecked(s.frame.radial(u)),
            Self::Cone(s) => {
                UnitVector3::new_unchecked(s.frame.radial(u) * s.cos - s.frame.z().get() * s.sin)
            }
            Self::Sphere(Sphere { frame, .. }) | Self::Torus(Torus { frame, .. }) => {
                let (sv, cv) = math::sin_cos(v);
                UnitVector3::new_unchecked(frame.radial(u) * cv + frame.z().get() * sv)
            }
        }
    }

    /// The parameter periods in `u` and `v`.
    pub fn periods(&self) -> (Option<f64>, Option<f64>) {
        match self {
            Self::Plane(_) => (None, None),
            Self::Cylinder(_) | Self::Cone(_) | Self::Sphere(_) => (Some(TAU), None),
            Self::Torus(_) => (Some(TAU), Some(TAU)),
        }
    }

    /// For a surface that closes up on itself, the genus of that closed
    /// surface (sphere 0, torus 1). `None` for open surfaces. Only closed
    /// surfaces may carry a face with no loops.
    pub fn closed_genus(&self) -> Option<u32> {
        match self {
            Self::Sphere(_) => Some(0),
            Self::Torus(_) => Some(1),
            Self::Plane(_) | Self::Cylinder(_) | Self::Cone(_) => None,
        }
    }

    /// The distance from `p` to the nearest point of the surface.
    pub fn distance(&self, p: Point3) -> f64 {
        let q = self.frame().to_local(p);
        let rho = math::sqrt(q.x * q.x + q.y * q.y);
        match self {
            Self::Plane(_) => q.z.abs(),
            Self::Cylinder(s) => (rho - s.radius).abs(),
            Self::Cone(s) => {
                // In the half-plane through the axis and p: the generator
                // runs from (r, 0) in direction (sin α, cos α).
                let (w, h) = (rho - s.radius, q.z);
                if w * s.sin + h * s.cos >= -s.radius / s.sin {
                    (w * s.cos - h * s.sin).abs()
                } else {
                    let dh = h - s.apex_v();
                    math::sqrt(rho * rho + dh * dh)
                }
            }
            Self::Sphere(s) => (math::sqrt(rho * rho + q.z * q.z) - s.radius).abs(),
            Self::Torus(s) => {
                let w = rho - s.major;
                (math::sqrt(w * w + q.z * q.z) - s.minor).abs()
            }
        }
    }

    /// The parameters of the nearest point of the surface to `p`. Angles
    /// are in `[0, 2π)`; where the nearest point is not unique (on an axis,
    /// at a centre), angle 0 is chosen.
    pub fn params_of(&self, p: Point3) -> (f64, f64) {
        let q = self.frame().to_local(p);
        let rho = math::sqrt(q.x * q.x + q.y * q.y);
        let u = polar_angle(q.y, q.x);
        match self {
            Self::Plane(_) => (q.x, q.y),
            Self::Cylinder(_) => (u, q.z),
            Self::Cone(s) => {
                let slant = (rho - s.radius) * s.sin + q.z * s.cos;
                (u, (slant * s.cos).max(s.apex_v()))
            }
            Self::Sphere(_) => (u, math::atan2(q.z, rho)),
            Self::Torus(s) => (u, polar_angle(q.z, rho - s.major)),
        }
    }

    /// The surface moved by a rigid transform. Parameters are unchanged.
    pub fn transformed(&self, r: &Rigid) -> Self {
        let mut out = *self;
        let frame = match &mut out {
            Self::Plane(s) => &mut s.frame,
            Self::Cylinder(s) => &mut s.frame,
            Self::Cone(s) => &mut s.frame,
            Self::Sphere(s) => &mut s.frame,
            Self::Torus(s) => &mut s.frame,
        };
        *frame = frame.transformed(r);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vector3;
    use core::f64::consts::{FRAC_PI_4, PI};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    }

    fn all() -> [Surface; 5] {
        let f = frame();
        [
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, 2.0).unwrap()),
            Surface::Cone(Cone::new(f, 2.0, FRAC_PI_4).unwrap()),
            Surface::Sphere(Sphere::new(f, 2.0).unwrap()),
            Surface::Torus(Torus::new(f, 3.0, 1.0).unwrap()),
        ]
    }

    fn near(a: Point3, b: Point3) -> bool {
        a.distance(b) <= 1e-14
    }

    fn cone_apex_v(s: &Surface) -> f64 {
        match s {
            Surface::Cone(c) => c.apex_v(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn evaluation_at_known_parameters() {
        let [plane, cyl, cone, sphere, torus] = all();
        assert!(near(plane.point(1.0, -1.0), Point3::new(2.0, 1.0, 3.0)));
        assert!(near(cyl.point(0.0, 1.0), Point3::new(3.0, 2.0, 4.0)));
        assert!(near(cone.point(0.0, 1.0), Point3::new(4.0, 2.0, 4.0)));
        assert!(near(cone.point(1.0, -2.0), Point3::new(1.0, 2.0, 1.0)));
        assert!(near(sphere.point(PI, 0.0), Point3::new(-1.0, 2.0, 3.0)));
        assert!(near(
            sphere.point(0.0, FRAC_PI_2),
            Point3::new(1.0, 2.0, 5.0)
        ));
        assert!(near(torus.point(0.0, 0.0), Point3::new(5.0, 2.0, 3.0)));
        assert!(near(torus.point(0.0, PI), Point3::new(3.0, 2.0, 3.0)));
        assert!(near(
            torus.point(FRAC_PI_2, FRAC_PI_2),
            Point3::new(1.0, 5.0, 4.0)
        ));
    }

    #[test]
    fn natural_normals() {
        let [plane, cyl, cone, sphere, torus] = all();
        let close = |n: UnitVector3, v: Vector3| (n.get() - v).length() <= 1e-15;
        assert!(close(plane.normal(5.0, 5.0), Vector3::new(0.0, 0.0, 1.0)));
        assert!(close(cyl.normal(PI, 7.0), Vector3::new(-1.0, 0.0, 0.0)));
        let h = core::f64::consts::FRAC_1_SQRT_2;
        assert!(close(cone.normal(0.0, 0.0), Vector3::new(h, 0.0, -h)));
        assert!(close(
            sphere.normal(0.0, -FRAC_PI_2),
            Vector3::new(0.0, 0.0, -1.0)
        ));
        assert!(close(torus.normal(0.0, PI), Vector3::new(-1.0, 0.0, 0.0)));
    }

    #[test]
    fn closed_form_distances() {
        let [plane, cyl, cone, sphere, torus] = all();
        let centre = Point3::new(1.0, 2.0, 3.0);
        assert_eq!(plane.distance(Point3::new(9.0, 9.0, 1.0)), 2.0);
        assert_eq!(cyl.distance(centre), 2.0);
        assert_eq!(cyl.distance(Point3::new(1.0, 7.0, -40.0)), 3.0);
        assert_eq!(sphere.distance(centre), 2.0);
        assert_eq!(torus.distance(centre), 2.0);
        assert_eq!(torus.distance(Point3::new(4.0, 2.0, 3.0)), 1.0);
        // The apex is at v = -2, i.e. (1, 2, 1). Below it, the nearest
        // point is the apex itself.
        let apex = Point3::new(1.0, 2.0, 1.0);
        assert!(cone.distance(apex) <= 1e-15);
        assert!((cone.distance(Point3::new(1.0, 2.0, -2.0)) - 3.0).abs() <= 1e-15);
        // Inside the cone on the axis, above the apex: perpendicular to
        // the generator, (h - apex) sin α.
        let d = cone.distance(Point3::new(1.0, 2.0, 3.0));
        assert!((d - 2.0 * math::sin(FRAC_PI_4)).abs() <= 1e-15);
    }

    #[test]
    fn params_round_trip() {
        for s in all() {
            for (u, v) in [(0.0, 0.0), (0.5, 0.3), (2.0, -0.7), (4.0, 1.2), (6.0, 0.9)] {
                let p = s.point(u, v);
                assert!(s.distance(p) <= 1e-14, "{s:?} at {u}, {v}");
                let (pu, pv) = s.params_of(p);
                assert!(near(s.point(pu, pv), p), "{s:?} at {u}, {v}");
            }
        }
    }

    #[test]
    fn params_of_off_surface_points_land_on_the_nearest_point() {
        let [_, cyl, cone, sphere, torus] = all();
        let p = Point3::new(1.0, 7.0, 4.0);
        let (u, v) = cyl.params_of(p);
        assert_eq!((u, v), (FRAC_PI_2, 1.0));
        let (u, v) = sphere.params_of(Point3::new(1.0, 2.0, 9.0));
        assert_eq!((u, v), (0.0, FRAC_PI_2));
        let (_, v) = torus.params_of(Point3::new(3.5, 2.0, 3.0));
        assert_eq!(v, PI);
        // Below the apex the nearest point is the apex.
        let (_, v) = cone.params_of(Point3::new(1.0, 2.0, -5.0));
        assert_eq!(v, cone_apex_v(&cone));
    }

    #[test]
    fn periods_and_closedness() {
        let [plane, cyl, cone, sphere, torus] = all();
        assert_eq!(plane.periods(), (None, None));
        assert_eq!(cyl.periods(), (Some(TAU), None));
        assert_eq!(cone.periods(), (Some(TAU), None));
        assert_eq!(sphere.periods(), (Some(TAU), None));
        assert_eq!(torus.periods(), (Some(TAU), Some(TAU)));
        assert_eq!(plane.closed_genus(), None);
        assert_eq!(cyl.closed_genus(), None);
        assert_eq!(cone.closed_genus(), None);
        assert_eq!(sphere.closed_genus(), Some(0));
        assert_eq!(torus.closed_genus(), Some(1));
    }

    #[test]
    fn transformed_moves_the_frame() {
        let r = Rigid::translation(Vector3::new(0.0, 0.0, 10.0));
        for s in all() {
            let t = s.transformed(&r);
            assert!(near(
                t.point(0.5, 0.25),
                s.point(0.5, 0.25) + Vector3::new(0.0, 0.0, 10.0)
            ));
        }
    }

    #[test]
    fn constructors_reject_bad_input() {
        let f = Frame::WORLD;
        assert_eq!(Cylinder::new(f, 0.0), Err(Error::Degenerate));
        assert_eq!(Sphere::new(f, f64::NAN), Err(Error::NonFinite));
        assert_eq!(Cone::new(f, 1.0, 0.0), Err(Error::Degenerate));
        assert_eq!(Cone::new(f, 1.0, FRAC_PI_2), Err(Error::Degenerate));
        assert_eq!(Cone::new(f, 1.0, -0.5), Err(Error::Degenerate));
        assert_eq!(Cone::new(f, 1.0, f64::NAN), Err(Error::NonFinite));
        assert_eq!(Torus::new(f, 1.0, 1.0), Err(Error::Degenerate));
        assert_eq!(Torus::new(f, 1.0, 2.0), Err(Error::Degenerate));
        assert!(Torus::new(f, 1.0, 0.5).is_ok());
        let c = Cone::new(f, 2.0, FRAC_PI_4).unwrap();
        assert_eq!((c.radius(), c.half_angle()), (2.0, FRAC_PI_4));
        assert!((c.apex_v() + 2.0).abs() <= 1e-15);
    }
}
