//! The coincidence rule: when two analytic surfaces are the same surface.
//!
//! Directions agree within [`ANGULAR_RESOLUTION`] (measured by the sine
//! of the angle between them) and lengths and positions within
//! [`LINEAR_RESOLUTION`]:
//!
//! - planes with parallel or opposite normals, each origin within
//!   resolution of the other plane;
//! - cylinders with parallel axes, each origin within resolution of the
//!   other axis, and equal radii;
//! - cones with axes pointing the same way, apexes within resolution and
//!   equal half-angles;
//! - spheres with centres and radii within resolution;
//! - tori with parallel axes, centres within resolution and equal major
//!   and minor radii.
//!
//! The natural normals of coincident surfaces agree, except for planes,
//! whose normals may be opposite.

use crate::geom::{Frame, Point3, Surface, UnitVector3};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};

/// `Some(same)` when the surfaces are coincident, `same` when their
/// natural normals agree; `None` otherwise, and for any surface that is
/// not analytic.
pub(crate) fn coincident(a: &Surface, b: &Surface) -> Option<bool> {
    let close = |x: f64, y: f64| (x - y).abs() <= LINEAR_RESOLUTION;
    let near = |p: Point3, q: Point3| p.distance(q) <= LINEAR_RESOLUTION;
    match (a, b) {
        (Surface::Plane(_), Surface::Plane(_)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            let same = f.z().dot(g.z()) > 0.0;
            (parallel(f.z(), g.z())
                && off_plane(f, g.origin()) <= LINEAR_RESOLUTION
                && off_plane(g, f.origin()) <= LINEAR_RESOLUTION)
                .then_some(same)
        }
        (Surface::Cylinder(x), Surface::Cylinder(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            (parallel(f.z(), g.z())
                && off_axis(f, g.origin()) <= LINEAR_RESOLUTION
                && off_axis(g, f.origin()) <= LINEAR_RESOLUTION
                && close(x.radius(), y.radius()))
            .then_some(true)
        }
        (Surface::Cone(x), Surface::Cone(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            let apex = |f: &Frame, v: f64| f.origin() + f.z().get() * v;
            (f.z().dot(g.z()) > 0.0
                && parallel(f.z(), g.z())
                && near(apex(f, x.apex_v()), apex(g, y.apex_v()))
                && (x.half_angle() - y.half_angle()).abs() <= ANGULAR_RESOLUTION)
                .then_some(true)
        }
        (Surface::Sphere(x), Surface::Sphere(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            (near(f.origin(), g.origin()) && close(x.radius(), y.radius())).then_some(true)
        }
        (Surface::Torus(x), Surface::Torus(y)) => {
            let (f, g) = (a.frame()?, b.frame()?);
            (parallel(f.z(), g.z())
                && near(f.origin(), g.origin())
                && close(x.major(), y.major())
                && close(x.minor(), y.minor()))
            .then_some(true)
        }
        _ => None,
    }
}

/// Parallel or opposite within [`ANGULAR_RESOLUTION`].
pub(crate) fn parallel(u: UnitVector3, v: UnitVector3) -> bool {
    crate::tol::directions_parallel(u, v)
}

/// The distance of `p` from the plane of `f`'s xy axes.
pub(crate) fn off_plane(f: &Frame, p: Point3) -> f64 {
    (p - f.origin()).dot(f.z().get()).abs()
}

/// The distance of `p` from the line through `f`'s origin along its `z`.
pub(crate) fn off_axis(f: &Frame, p: Point3) -> f64 {
    let d = p - f.origin();
    (d - f.z().get() * d.dot(f.z().get())).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Cone, Cylinder, Plane, Sphere, Torus, Vector3};
    use core::f64::consts::FRAC_PI_4;

    fn frame(o: (f64, f64, f64), z: (f64, f64, f64)) -> Frame {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(1.0, 0.3, 0.0),
        )
        .unwrap()
    }

    #[test]
    fn planes() {
        let p = |o, z| Surface::Plane(Plane::new(frame(o, z)));
        let a = p((0.0, 0.0, 1.0), (0.0, 0.0, 1.0));
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0), (0.0, 0.0, 1.0))),
            Some(true)
        );
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0), (0.0, 0.0, -1.0))),
            Some(false)
        );
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0 + 0.5e-8), (0.0, 0.0, 1.0))),
            Some(true)
        );
        assert_eq!(
            coincident(&a, &p((5.0, 2.0, 1.0 + 2e-8), (0.0, 0.0, 1.0))),
            None
        );
        assert_eq!(coincident(&a, &p((0.0, 0.0, 1.0), (0.0, 1e-10, 1.0))), None);
    }

    #[test]
    fn surfaces_of_revolution() {
        let z = (0.0, 0.0, 1.0);
        let cyl = |o, z, r| Surface::Cylinder(Cylinder::new(frame(o, z), r).unwrap());
        let a = cyl((0.0, 0.0, 0.0), z, 1.0);
        // Shifted along the axis and turned round: the same cylinder.
        assert_eq!(
            coincident(&a, &cyl((0.0, 0.0, 7.0), (0.0, 0.0, -1.0), 1.0)),
            Some(true)
        );
        assert_eq!(coincident(&a, &cyl((0.0, 0.0, 0.0), z, 1.0 + 2e-8)), None);
        assert_eq!(coincident(&a, &cyl((2e-8, 0.0, 0.0), z, 1.0)), None);
        // Cones: the same apex and angle, whatever the reference radius.
        let c1 = Surface::Cone(Cone::new(frame((0.0, 0.0, 0.0), z), 1.0, FRAC_PI_4).unwrap());
        let c2 = Surface::Cone(Cone::new(frame((0.0, 0.0, 1.0), z), 2.0, FRAC_PI_4).unwrap());
        assert_eq!(coincident(&c1, &c2), Some(true));
        let flipped = Surface::Cone(
            Cone::new(frame((0.0, 0.0, -2.0), (0.0, 0.0, -1.0)), 1.0, FRAC_PI_4).unwrap(),
        );
        assert_eq!(coincident(&c1, &flipped), None);
        let s1 = Surface::Sphere(Sphere::new(frame((1.0, 2.0, 3.0), z), 2.0).unwrap());
        let s2 =
            Surface::Sphere(Sphere::new(frame((1.0, 2.0, 3.0), (1.0, 0.0, 0.0)), 2.0).unwrap());
        assert_eq!(coincident(&s1, &s2), Some(true));
        let t1 = Surface::Torus(Torus::new(frame((0.0, 0.0, 0.0), z), 3.0, 1.0).unwrap());
        let t2 =
            Surface::Torus(Torus::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, -1.0)), 3.0, 1.0).unwrap());
        assert_eq!(coincident(&t1, &t2), Some(true));
        assert_eq!(coincident(&t1, &s1), None);
    }
}
