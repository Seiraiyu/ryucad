//! Silhouettes in closed form: planes, cylinders, cones, spheres, and
//! tori seen along their axis (design 326).

use crate::geom::{Circle, Curve, Curve2, Frame, Line, Line2, Point2, Surface, Vector2, Vector3};
use crate::intersect::Failure;
use crate::math;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::work::Meter;
use core::f64::consts::{FRAC_PI_2, PI, TAU};

/// A silhouette branch before trimming: a curve over a range covering the
/// face, its pcurve with the affine map from the curve's parameter, and
/// how far it may lie from the true silhouette.
#[derive(Clone, Debug)]
pub(super) struct Raw {
    pub(super) curve: Curve,
    pub(super) range: [f64; 2],
    pub(super) pcurve: (Curve2, [f64; 2]),
    pub(super) tolerance: f64,
}

/// A face's silhouette in closed form.
#[derive(Clone, Debug)]
pub(super) enum Closed {
    /// The surface is seen edge-on all over: no silhouette, its edges draw
    /// its image.
    EdgeOn,
    Branches(Vec<Raw>),
}

/// `x` moved by whole periods into `[start, start + period)`.
fn into(x: f64, start: f64, period: f64) -> f64 {
    start + (x - start).rem_euclid(period)
}

/// A range grown a hair each way, so the face's boundary crossings at its
/// ends lie inside it.
fn grown([a, b]: [f64; 2]) -> [f64; 2] {
    let h = (b - a) * 1e-6 + LINEAR_RESOLUTION;
    [a - h, b + h]
}

fn uncertain<E>(_: E) -> Failure {
    Failure::Uncertain
}

/// The line `u = u₀` of the parameter plane, `v` running.
fn along_v(u: f64) -> Result<Curve2, Failure> {
    Ok(Curve2::Line2(
        Line2::new(Point2::new(u, 0.0), Vector2::new(0.0, 1.0)).map_err(uncertain)?,
    ))
}

/// The line `v = v₀`, `u` running.
fn along_u(v: f64) -> Result<Curve2, Failure> {
    Ok(Curve2::Line2(
        Line2::new(Point2::new(0.0, v), Vector2::new(1.0, 0.0)).map_err(uncertain)?,
    ))
}

/// The silhouette of `surface` seen along unit `t` over the parameter box
/// `rect`, where it has a closed form; `None` where it must be traced.
pub(super) fn closed_form(
    surface: &Surface,
    t: Vector3,
    rect: [[f64; 2]; 2],
    m: &Meter,
) -> Result<Option<Closed>, Failure> {
    let Some(f) = surface.frame() else {
        return Ok(None);
    };
    let (x, y, z) = (f.x().get(), f.y().get(), f.z().get());
    let (tx, ty, tz) = (t.dot(x), t.dot(y), t.dot(z));
    let axial = z.cross(t).length() <= ANGULAR_RESOLUTION;
    let u0 = math::atan2(-tx, ty);
    Ok(Some(match surface {
        Surface::Plane(_) => {
            if tz.abs() <= ANGULAR_RESOLUTION {
                Closed::EdgeOn
            } else {
                Closed::Branches(Vec::new())
            }
        }
        Surface::Cylinder(_) => {
            if axial {
                return Ok(Some(Closed::EdgeOn));
            }
            let mut out = Vec::new();
            for u in [u0, u0 + PI] {
                let u = into(u, rect[0][0], TAU);
                let line = Line::from_unit(surface.point(u, 0.0), f.z()).map_err(uncertain)?;
                out.push(Raw {
                    curve: Curve::Line(line),
                    range: grown(rect[1]),
                    pcurve: (along_v(u)?, [0.0, 1.0]),
                    tolerance: LINEAR_RESOLUTION,
                });
            }
            Closed::Branches(out)
        }
        Surface::Cone(c) => {
            let (sa, ca) = math::sin_cos(c.half_angle());
            let rho = math::sqrt(tx * tx + ty * ty);
            let k = sa / ca * tz / rho;
            if rho <= ANGULAR_RESOLUTION || !(k.abs() < 1.0 - ANGULAR_RESOLUTION) {
                return Ok(Some(Closed::Branches(Vec::new())));
            }
            let (phi, a) = (math::atan2(ty, tx), math::acos(k));
            let [v0, v1] = grown(rect[1]);
            let v0 = v0.max(c.apex_v());
            let mut out = Vec::new();
            for u in [phi + a, phi - a] {
                let u = into(u, rect[0][0], TAU);
                let dir = f.radial(u) * sa + z * ca;
                let line = Line::new(surface.point(u, 0.0), dir).map_err(uncertain)?;
                out.push(Raw {
                    curve: Curve::Line(line),
                    range: [v0 / ca, v1 / ca],
                    pcurve: (along_v(u)?, [0.0, ca]),
                    tolerance: LINEAR_RESOLUTION,
                });
            }
            Closed::Branches(out)
        }
        Surface::Sphere(s) => {
            let r = s.radius();
            let o = f.origin();
            if axial {
                let c = Circle::new(*f, r).map_err(uncertain)?;
                let a = rect[0][0];
                return Ok(Some(Closed::Branches(vec![Raw {
                    curve: Curve::Circle(c),
                    range: [a, a + TAU],
                    pcurve: (along_u(0.0)?, [0.0, 1.0]),
                    tolerance: LINEAR_RESOLUTION,
                }])));
            }
            if tz.abs() <= ANGULAR_RESOLUTION {
                let mut out = Vec::new();
                for u in [u0, u0 + PI] {
                    let u = into(u, rect[0][0], TAU);
                    let radial = f.radial(u);
                    let frame = Frame::new(o, radial.cross(z), radial).map_err(uncertain)?;
                    out.push(Raw {
                        curve: Curve::Circle(Circle::new(frame, r).map_err(uncertain)?),
                        range: [-FRAC_PI_2, FRAC_PI_2],
                        pcurve: (along_v(u)?, [0.0, 1.0]),
                        tolerance: LINEAR_RESOLUTION,
                    });
                }
                return Ok(Some(Closed::Branches(out)));
            }
            let hint = if x.cross(t).length() > 0.5 { x } else { y };
            let frame = Frame::new(o, t, hint).map_err(uncertain)?;
            let curve = Curve::Circle(Circle::new(frame, r).map_err(uncertain)?);
            let (pc, bound) = crate::intersect::fit_pcurve_within(
                surface,
                &curve,
                [0.0, TAU],
                LINEAR_RESOLUTION,
                512,
                m,
            )?;
            Closed::Branches(vec![Raw {
                curve,
                range: [0.0, TAU],
                pcurve: (Curve2::Nurbs2(std::sync::Arc::new(pc)), [0.0, 1.0]),
                tolerance: bound.max(LINEAR_RESOLUTION),
            }])
        }
        Surface::Torus(tr) if axial => {
            let a = rect[0][0];
            let mut out = Vec::new();
            for (v, radius) in [
                (0.0, tr.major() + tr.minor()),
                (PI, tr.major() - tr.minor()),
            ] {
                out.push(Raw {
                    curve: Curve::Circle(Circle::new(*f, radius).map_err(uncertain)?),
                    range: [a, a + TAU],
                    pcurve: (along_u(v)?, [0.0, 1.0]),
                    tolerance: LINEAR_RESOLUTION,
                });
            }
            Closed::Branches(out)
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Cone, Cylinder, Frame, Sphere, Torus};
    use core::f64::consts::FRAC_PI_4;

    fn rect() -> [[f64; 2]; 2] {
        [[0.0, TAU], [-1.0, 1.0]]
    }

    fn branches(s: &Surface, t: Vector3, rect: [[f64; 2]; 2]) -> Vec<Raw> {
        // unmetered: a unit test
        match closed_form(s, t, rect, &Meter::free()).unwrap() {
            Some(Closed::Branches(b)) => b,
            other => panic!("{other:?}"),
        }
    }

    /// Every branch lies on the surface, square to the view, its pcurve
    /// following it.
    fn sound(s: &Surface, t: Vector3, b: &[Raw]) {
        for raw in b {
            for i in 0..=16 {
                let x = raw.range[0] + (raw.range[1] - raw.range[0]) * i as f64 / 16.0;
                let (c2, map) = &raw.pcurve;
                let q = c2.point(map[0] + map[1] * x);
                assert!(s.point(q.x, q.y).distance(raw.curve.point(x)) <= raw.tolerance.max(1e-12));
                if let Some(n) = s.normal(q.x, q.y) {
                    assert!(n.get().dot(t).abs() <= 1e-9, "{x}");
                }
            }
        }
    }

    #[test]
    fn a_cylinder_has_two_rulings_or_is_edge_on() {
        let s = Surface::Cylinder(Cylinder::new(Frame::WORLD, 0.4).unwrap());
        let t = Vector3::new(0.3, -1.0, 0.2);
        let b = branches(&s, t / t.length(), rect());
        assert_eq!(b.len(), 2);
        sound(&s, t / t.length(), &b);
        // unmetered: a unit test
        let along = closed_form(&s, Vector3::new(0.0, 0.0, 1.0), rect(), &Meter::free()).unwrap();
        assert!(matches!(along, Some(Closed::EdgeOn)));
    }

    #[test]
    fn a_cone_has_two_rulings_from_its_apex_or_none() {
        let s = Surface::Cone(Cone::new(Frame::WORLD, 0.5, FRAC_PI_4 / 2.0).unwrap());
        let t = Vector3::new(1.0, -0.5, 0.2);
        let b = branches(&s, t / t.length(), [[0.0, TAU], [-2.0, 1.0]]);
        assert_eq!(b.len(), 2);
        sound(&s, t / t.length(), &b);
        assert!(branches(&s, Vector3::new(0.0, 0.0, 1.0), rect()).is_empty());
    }

    #[test]
    fn a_sphere_has_a_great_circle_seen_any_way() {
        let s = Surface::Sphere(Sphere::new(Frame::WORLD, 0.7).unwrap());
        let r = [
            [0.0, TAU],
            [-core::f64::consts::FRAC_PI_2, core::f64::consts::FRAC_PI_2],
        ];
        for (t, n) in [
            (Vector3::new(0.0, 0.0, 1.0), 1),
            (Vector3::new(0.0, -1.0, 0.0), 2),
            (Vector3::new(0.3, -0.4, 0.5), 1),
        ] {
            let t = t / t.length();
            let b = branches(&s, t, r);
            assert_eq!(b.len(), n);
            sound(&s, t, &b);
        }
    }

    #[test]
    fn a_torus_along_its_axis_has_two_circles_and_askew_is_traced() {
        let s = Surface::Torus(Torus::new(Frame::WORLD, 1.0, 0.25).unwrap());
        let b = branches(&s, Vector3::new(0.0, 0.0, -1.0), [[0.0, TAU], [0.0, TAU]]);
        assert_eq!(b.len(), 2);
        let mut radii: Vec<f64> = b
            .iter()
            .map(|r| r.curve.point(0.3).to_vector().length())
            .collect();
        radii.sort_by(f64::total_cmp);
        assert!((radii[0] - 0.75).abs() < 1e-15 && (radii[1] - 1.25).abs() < 1e-15);
        // unmetered: a unit test
        let askew = closed_form(&s, Vector3::new(0.0, 0.6, 0.8), rect(), &Meter::free()).unwrap();
        assert!(askew.is_none());
    }
}
