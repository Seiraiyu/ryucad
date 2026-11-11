//! Offsets that pass through their surface's centre (design 299's
//! inverted faces): a cylinder or a sphere offset inwards past its
//! radius, a torus past its minor radius, a pipe past its spine. `S + d N` is then a surface of
//! the same kind on the other side of the axis, centre or core circle,
//! of radius `|r + d|`, whose natural normal is `(1 - d κ₁)(1 - d κ₂) N`
//! by the parametrization: reversed against the original's for a
//! cylinder and a torus (one factor negative), the same for a sphere
//! (both negative).
//!
//! The global offset's envelope keeps these faces; distance selection
//! drops every piece of them, but they close the envelope.

use crate::geom::{Cylinder, Frame, Sphere, Surface, Torus};
use crate::sheet_metal::curve2::Affine2;
use crate::tol::LINEAR_RESOLUTION;
use core::f64::consts::PI;
use std::sync::Arc;

/// How an inverted offset's parameters relate to its surface's: a face's
/// pcurve `p` on the surface is `map(p)` on the offset, and the face's
/// sense flips where `flip`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Inversion {
    pub(crate) map: Option<Affine2>,
    pub(crate) flip: bool,
}

/// `s` moved by `d` along its natural normal where that passes through
/// its centre: `None` where it doesn't (the offset is regular, or the
/// surface isn't a cylinder, a sphere or a torus), `Some(None)` where the
/// offset collapses to within the resolution of the axis, centre or core
/// circle (or a torus would no longer be a ring), else the surface and
/// how its parameters relate.
pub(crate) fn inverted(s: &Surface, d: f64) -> Option<Option<(Surface, Inversion)>> {
    let r = match s {
        Surface::Cylinder(c) => c.radius(),
        Surface::Sphere(c) => c.radius(),
        Surface::Torus(t) => t.minor(),
        Surface::Pipe(p) => p.radius(),
        _ => return None,
    };
    let r2 = r + d;
    if r2 > LINEAR_RESOLUTION {
        return None;
    }
    if r2 >= -LINEAR_RESOLUTION {
        return Some(None);
    }
    let r2 = -r2;
    // The other side of the spine: the same frame, its angle `θ + π`, as
    // the torus's `v + π`. Refused where the new radius folds the pipe
    // (`Pipe::new`'s own check against the spine's curvature).
    if let Surface::Pipe(p) = s {
        return Some(p.with_radius(r2).ok().map(|c| {
            (
                Surface::Pipe(Arc::new(c)),
                Inversion {
                    map: Some(Affine2::diagonal([0.0, 1.0], [PI, 1.0])),
                    flip: false,
                },
            )
        }));
    }
    let frame = *s.frame().expect("an analytic surface");
    Some(match s {
        // Turned half a turn about its axis, so `u` stays.
        Surface::Cylinder(_) => turned(&frame)
            .and_then(|f| Cylinder::new(f, r2).ok())
            .map(|c| {
                (
                    Surface::Cylinder(c),
                    Inversion {
                        map: None,
                        flip: false,
                    },
                )
            }),
        // The antipode: `(u + π, -v)`, which turns the parameter plane
        // over, and the natural normal agrees with the original's.
        Surface::Sphere(_) => Sphere::new(frame, r2).ok().map(|c| {
            (
                Surface::Sphere(c),
                Inversion {
                    map: Some(Affine2::diagonal([PI, 1.0], [0.0, -1.0])),
                    flip: true,
                },
            )
        }),
        // The other side of the core circle: `(u, v + π)`.
        Surface::Torus(t) => {
            if r2 >= t.major() - LINEAR_RESOLUTION {
                None
            } else {
                Torus::new(frame, t.major(), r2).ok().map(|c| {
                    (
                        Surface::Torus(c),
                        Inversion {
                            map: Some(Affine2::diagonal([0.0, 1.0], [PI, 1.0])),
                            flip: false,
                        },
                    )
                })
            }
        }
        _ => unreachable!("matched above"),
    })
}

/// `frame` turned half a turn about its `z`.
fn turned(frame: &Frame) -> Option<Frame> {
    Frame::new(frame.origin(), frame.z().get(), -frame.x().get()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point2, Point3, Vector3};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    }

    /// The inverted offset holds `S(p) + d N(p)` at `map(p)`, and its
    /// natural normal there is `±N(p)` as `flip` and the parametrization
    /// say.
    fn holds(s: &Surface, d: f64, sign: f64) {
        let (o, inv) = inverted(s, d).unwrap().unwrap();
        for &(u, v) in &[(0.3, 0.2), (2.0, -0.7), (4.0, 1.1)] {
            let p = s.point(u, v) + s.normal(u, v).unwrap().get() * d;
            let q = inv
                .map
                .map_or(Point2::new(u, v), |m| m.point(Point2::new(u, v)));
            assert!(o.point(q.x, q.y).distance(p) < 1e-12, "{u} {v}");
            let n = s.normal(u, v).unwrap().get();
            let m = o.normal(q.x, q.y).unwrap().get();
            let m = if inv.flip { -m } else { m };
            assert!((m.dot(n) - sign).abs() < 1e-12, "{u} {v}");
        }
    }

    #[test]
    fn a_cylinder_past_its_axis_keeps_its_parameters() {
        let s = Surface::Cylinder(Cylinder::new(frame(), 1.0).unwrap());
        holds(&s, -3.0, -1.0);
    }

    #[test]
    fn a_sphere_past_its_centre_is_its_antipode() {
        let s = Surface::Sphere(Sphere::new(frame(), 1.0).unwrap());
        holds(&s, -2.5, 1.0);
    }

    #[test]
    fn a_torus_past_its_core_circle_turns_v_half_a_turn() {
        let s = Surface::Torus(Torus::new(frame(), 5.0, 1.0).unwrap());
        holds(&s, -2.0, -1.0);
    }

    #[test]
    fn a_pipe_past_its_spine_turns_its_angle_half_a_turn() {
        use crate::geom::pipe::tests::{R, on_plane, on_spline_plane};
        for p in [on_plane(), on_spline_plane()] {
            let s = Surface::Pipe(Arc::new(p));
            holds(&s, -(R + 0.05), -1.0);
            // At the spine, and past the spine's least radius of
            // curvature, it is refused.
            assert_eq!(inverted(&s, -R), Some(None));
            assert_eq!(inverted(&s, -(R + 5.0)), Some(None));
        }
    }

    #[test]
    fn a_regular_offset_or_a_collapse_isnt_inverted() {
        let s = Surface::Cylinder(Cylinder::new(frame(), 1.0).unwrap());
        assert!(inverted(&s, -0.5).is_none());
        assert_eq!(inverted(&s, -1.0), Some(None));
        let t = Surface::Torus(Torus::new(frame(), 2.0, 1.0).unwrap());
        assert_eq!(inverted(&t, -3.5), Some(None));
    }
}
