//! The certified accuracy tests a segment or triangle must pass.
//!
//! A face is meshed in its parameter plane scaled by a [`Metric`], so
//! that lengths there roughly match lengths on the surface. A flat
//! triangle interpolating the surface at its corners deviates from the
//! surface over its parameter triangle `T` by at most `M R² / 2`, where
//! `R` is the radius of the smallest circle enclosing `T` and `M` bounds
//! the second derivative of the surface along every unit direction of the
//! scaled plane over `T` (S. Waldron, "The error in linear interpolation
//! at the vertices of a simplex", SIAM J. Numer. Anal. 35, 1998). The
//! bound holds both ways: every point of the triangle is that close to
//! the surface, and every point of the surface over `T` is that close to
//! the triangle. A chord of a curve over `[ta, tb]` deviates by at most
//! `|C''| (tb - ta)² / 8`, the same bound in one dimension.
//!
//! The normal (or tangent) can turn across a triangle (or segment) by at
//! most the turning rates of [`SurfaceBound`] (or `CurveBound`) times its
//! parameter spans. The angle test is waived for a piece shorter than the
//! chord tolerance in model space: a turn faster than that is a feature
//! smaller than the tolerance, and near a pole or an apex, where the
//! normal has no limit, the rates are unbounded or nearly so.

use crate::geom::{Curve, Point2, Point3, Surface, SurfaceBound};

/// Per-face scales from parameters to the plane a face is meshed in:
/// `(a u, b v)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metric {
    pub(crate) a: f64,
    pub(crate) b: f64,
}

impl Metric {
    pub(crate) fn scale(&self, p: Point2) -> Point2 {
        Point2::new(self.a * p.x, self.b * p.y)
    }

    pub(crate) fn unscale(&self, p: Point2) -> Point2 {
        Point2::new(p.x / self.a, p.y / self.b)
    }
}

/// What every piece of the mesh must meet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Targets {
    /// The deviation allowed for a triangle; segments get half.
    pub(crate) chord: f64,
    /// The turn allowed across a triangle or along an edge segment.
    pub(crate) angle: f64,
    /// The longest edge allowed in model space.
    pub(crate) max_edge: f64,
    /// Below this length in model space an unbounded turn is waived.
    pub(crate) waive: f64,
}

/// An upper bound on the second derivative along any unit direction of
/// the scaled plane: the largest eigenvalue of
/// `[[|Suu|/a², |Suv|/ab], [|Suv|/ab, |Svv|/b²]]`.
pub(crate) fn second_derivative(b: &SurfaceBound, m: Metric) -> f64 {
    let p = b.duu / (m.a * m.a);
    let q = b.duv / (m.a * m.b);
    let r = b.dvv / (m.b * m.b);
    let half = (p - r) / 2.0;
    (p + r) / 2.0 + crate::math::sqrt(half * half + q * q)
}

/// The radius of the smallest circle holding a triangle: half its longest
/// edge when it has an angle of 90° or more, else its circumradius.
pub(crate) fn enclosing_radius([a, b, c]: [Point2; 3]) -> f64 {
    let (ab, bc, ca) = (b - a, c - b, a - c);
    // An angle of 90° or more at a corner: its opposite edge is a
    // diameter.
    if -ab.dot(ca) <= 0.0 {
        return bc.length() / 2.0;
    }
    if -bc.dot(ab) <= 0.0 {
        return ca.length() / 2.0;
    }
    if -ca.dot(bc) <= 0.0 {
        return ab.length() / 2.0;
    }
    let area2 = ab.cross(-ca).abs();
    ab.length() * bc.length() * ca.length() / (2.0 * area2)
}

/// The parameter box of some points.
fn uv_box(pts: &[Point2]) -> [[f64; 2]; 2] {
    let mut out = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for p in pts {
        out[0] = [out[0][0].min(p.x), out[0][1].max(p.x)];
        out[1] = [out[1][0].min(p.y), out[1][1].max(p.y)];
    }
    out
}

/// The normal's turn across a parameter box, if bounded.
fn turn(b: &SurfaceBound, [ru, rv]: [[f64; 2]; 2]) -> f64 {
    let along = |rate: f64, [lo, hi]: [f64; 2]| if hi > lo { rate * (hi - lo) } else { 0.0 };
    along(b.turn_u, ru) + along(b.turn_v, rv)
}

/// The longest distance between model-space points.
fn diameter(pts: &[Point3]) -> f64 {
    let mut d: f64 = 0.0;
    for (i, &p) in pts.iter().enumerate() {
        for &q in &pts[i + 1..] {
            d = d.max(p.distance(q));
        }
    }
    d
}

/// True when the flat triangle on `xyz` (the surface at the parameters
/// `uv`) meets the targets.
pub(crate) fn triangle_ok(
    surface: &Surface,
    metric: Metric,
    uv: [Point2; 3],
    xyz: [Point3; 3],
    t: &Targets,
) -> bool {
    let size = diameter(&xyz);
    if size > t.max_edge {
        return false;
    }
    let range = uv_box(&uv);
    let b = surface.derivative_bound(range);
    let r = enclosing_radius(uv.map(|p| metric.scale(p)));
    if second_derivative(&b, metric) * r * r / 2.0 > t.chord {
        return false;
    }
    let spin = turn(&b, range);
    spin <= t.angle || size < t.waive
}

/// True when the segment from `uv[0]` to `uv[1]` can be the edge of a
/// triangle that passes [`triangle_ok`]: a triangle on it whose third
/// corner lies in the circle on it as diameter has `R` half its length,
/// so the segment gets half the chord and half the angle. The bounds are
/// taken over the square holding that circle, where such a triangle
/// lies, not just along the segment.
pub(crate) fn face_segment_ok(
    surface: &Surface,
    metric: Metric,
    uv: [Point2; 2],
    xyz: [Point3; 2],
    t: &Targets,
) -> bool {
    let h = metric.scale(uv[0]).distance(metric.scale(uv[1]));
    let mid = Point2::new(0.5 * (uv[0].x + uv[1].x), 0.5 * (uv[0].y + uv[1].y));
    let (hu, hv) = (0.5 * h / metric.a, 0.5 * h / metric.b);
    let range = [[mid.x - hu, mid.x + hu], [mid.y - hv, mid.y + hv]];
    let b = surface.derivative_bound(range);
    if second_derivative(&b, metric) * h * h / 8.0 > t.chord / 2.0 {
        return false;
    }
    let spin = turn(&b, range);
    spin <= t.angle / 2.0 || diameter(&xyz) < t.waive
}

/// True when the chord from `xyz[0]` to `xyz[1]` over the curve's
/// `[ta, tb]` meets the targets, with half the chord tolerance.
pub(crate) fn curve_segment_ok(
    curve: &Curve,
    [ta, tb]: [f64; 2],
    xyz: [Point3; 2],
    t: &Targets,
) -> bool {
    let size = xyz[0].distance(xyz[1]);
    if size > t.max_edge {
        return false;
    }
    let b = curve.derivative_bound([ta, tb]);
    let dt = tb - ta;
    if b.d2 * dt * dt / 8.0 > t.chord / 2.0 {
        return false;
    }
    let spin = b.turn * dt;
    spin <= t.angle || size < t.waive
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Cylinder, Frame, NurbsSurface, Point3, Sphere, Surface};
    use core::f64::consts::FRAC_PI_2;
    use std::sync::Arc;

    const UNIT: Metric = Metric { a: 1.0, b: 1.0 };

    fn targets(chord: f64, angle: f64) -> Targets {
        Targets {
            chord,
            angle,
            max_edge: f64::INFINITY,
            waive: chord,
        }
    }

    #[test]
    fn enclosing_circles() {
        let p = Point2::new;
        // Equilateral with side 2: the circumradius 2 / √3.
        let h = 3.0f64.sqrt();
        let r = enclosing_radius([p(0.0, 0.0), p(2.0, 0.0), p(1.0, h)]);
        assert!((r - 2.0 / h).abs() <= 1e-15);
        // Right-angled: half the hypotenuse.
        assert_eq!(
            enclosing_radius([p(0.0, 0.0), p(4.0, 0.0), p(0.0, 3.0)]),
            2.5
        );
        // Obtuse, in every corner order: half the longest edge.
        for t in [
            [p(0.0, 0.0), p(4.0, 0.0), p(2.0, 0.5)],
            [p(2.0, 0.5), p(0.0, 0.0), p(4.0, 0.0)],
            [p(4.0, 0.0), p(2.0, 0.5), p(0.0, 0.0)],
        ] {
            assert_eq!(enclosing_radius(t), 2.0);
        }
        // Collinear.
        assert_eq!(
            enclosing_radius([p(0.0, 0.0), p(1.0, 0.0), p(3.0, 0.0)]),
            1.5
        );
    }

    #[test]
    fn second_derivative_along_any_direction() {
        let b = SurfaceBound {
            duu: 2.0,
            duv: 0.0,
            dvv: 1.0,
            turn_u: 0.0,
            turn_v: 0.0,
        };
        assert_eq!(second_derivative(&b, UNIT), 2.0);
        // Scaling u by 2 divides |Suu| by 4.
        assert_eq!(second_derivative(&b, Metric { a: 2.0, b: 1.0 }), 1.0);
        let mixed = SurfaceBound { duv: 1.0, ..b };
        let m = second_derivative(&mixed, UNIT);
        assert!((m - (1.5 + 1.25f64.sqrt())).abs() <= 1e-15);
    }

    /// The largest distance, over a grid of barycentric points, between
    /// the surface and the flat triangle at the same parameters.
    fn deviation(s: &Surface, uv: [Point2; 3]) -> f64 {
        let xyz = uv.map(|p| s.point(p.x, p.y));
        let mut worst: f64 = 0.0;
        let n = 12;
        for i in 0..=n {
            for j in 0..=n - i {
                let (a, b) = (f64::from(i) / f64::from(n), f64::from(j) / f64::from(n));
                let c = 1.0 - a - b;
                let p = Point2::new(
                    a * uv[0].x + b * uv[1].x + c * uv[2].x,
                    a * uv[0].y + b * uv[1].y + c * uv[2].y,
                );
                let flat = Point3::ORIGIN
                    + xyz[0].to_vector() * a
                    + xyz[1].to_vector() * b
                    + xyz[2].to_vector() * c;
                worst = worst.max(s.point(p.x, p.y).distance(flat));
            }
        }
        worst
    }

    #[test]
    fn triangles_on_a_cylinder() {
        let s = Surface::Cylinder(Cylinder::new(Frame::WORLD, 2.0).unwrap());
        let m = Metric { a: 2.0, b: 1.0 };
        let tri = |du: f64| {
            let uv = [
                Point2::new(0.0, 0.0),
                Point2::new(du, 0.0),
                Point2::new(du / 2.0, du),
            ];
            (uv, uv.map(|p| s.point(p.x, p.y)))
        };
        let t = targets(1e-3, 0.5);
        let (uv, xyz) = tri(0.02);
        assert!(triangle_ok(&s, m, uv, xyz, &t));
        assert!(deviation(&s, uv) <= t.chord);
        let (uv, xyz) = tri(0.2);
        assert!(!triangle_ok(&s, m, uv, xyz, &t));
        // The angle alone: Δu = 0.02 turns the normal by 0.02.
        assert!(!triangle_ok(
            &s,
            m,
            tri(0.02).0,
            tri(0.02).1,
            &Targets {
                waive: 0.0,
                ..targets(1.0, 0.01)
            }
        ));
        // And the edge length.
        let short = Targets {
            max_edge: 0.01,
            ..t
        };
        assert!(!triangle_ok(&s, m, tri(0.02).0, tri(0.02).1, &short));
    }

    #[test]
    fn segments_of_a_circle() {
        let c = Curve::Circle(Circle::new(Frame::WORLD, 1.0).unwrap());
        let seg = |dt: f64| [c.point(0.0), c.point(dt)];
        let t = targets(1e-4, 0.5);
        // The sagitta of a chord over dt is about dt² / 8: it must be under
        // half the chord tolerance.
        assert!(curve_segment_ok(&c, [0.0, 0.019], seg(0.019), &t));
        assert!(!curve_segment_ok(&c, [0.0, 0.021], seg(0.021), &t));
        // The tangent turns by dt.
        assert!(!curve_segment_ok(
            &c,
            [0.0, 0.019],
            seg(0.019),
            &Targets {
                waive: 0.0,
                ..targets(1.0, 0.01)
            }
        ));
    }

    #[test]
    fn segments_on_a_face_get_half() {
        let s = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
        let seg = |du: f64| {
            let uv = [Point2::new(0.0, 0.0), Point2::new(du, 0.0)];
            (uv, uv.map(|p| s.point(p.x, p.y)))
        };
        let t = targets(1e-4, 0.5);
        // M h² / 8 <= chord / 2 with M = 1: h <= 0.02.
        let (uv, xyz) = seg(0.0199);
        assert!(face_segment_ok(&s, UNIT, uv, xyz, &t));
        let (uv, xyz) = seg(0.0201);
        assert!(!face_segment_ok(&s, UNIT, uv, xyz, &t));
    }

    #[test]
    fn unbounded_turning_is_waived_when_small() {
        // A NURBS sphere's pole: the normal's turn rate is unbounded there.
        let sphere = Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap());
        let (n, _) = sphere.to_nurbs([[0.0, 0.0], [0.0, 0.0]]).unwrap();
        let s = Surface::Nurbs(Arc::new(n));
        let tri = |h: f64| {
            let uv = [
                Point2::new(0.0, FRAC_PI_2 - h),
                Point2::new(h, FRAC_PI_2 - h),
                Point2::new(h / 2.0, FRAC_PI_2),
            ];
            (uv, uv.map(|p| s.point(p.x, p.y)))
        };
        let t = targets(1e-3, 0.5);
        let (uv, xyz) = tri(0.01);
        assert!(!triangle_ok(&s, UNIT, uv, xyz, &t));
        let (uv, xyz) = tri(0.0005);
        assert!(triangle_ok(&s, UNIT, uv, xyz, &t));
    }

    #[test]
    fn passing_triangles_are_within_the_chord() {
        // Random triangles on a wavy NURBS: whenever one passes, the
        // sampled deviation is within the chord.
        use crate::geom::KnotVector;
        let ku = KnotVector::clamped(3, &[0.0, 0.5, 2.0], &[4, 1, 4]).unwrap();
        let kv = KnotVector::clamped(2, &[0.0, 1.0, 3.0], &[3, 1, 3]).unwrap();
        let (nu, nv) = (ku.control_count(), kv.control_count());
        let mut points = Vec::new();
        let mut weights = Vec::new();
        for i in 0..nu {
            for j in 0..nv {
                let (x, y) = (i as f64, j as f64);
                points.push(Point3::new(x, y, ((x * 1.7 + y * 0.9) % 2.0) - 1.0));
                weights.push(0.5 + ((i * 7 + j * 3) % 5) as f64 * 0.3);
            }
        }
        let s = Surface::Nurbs(Arc::new(
            NurbsSurface::new([ku, kv], points, Some(weights)).unwrap(),
        ));
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let t = targets(1e-3, 1.0);
        let mut passed = 0;
        for _ in 0..400 {
            let size = 0.02 + 0.1 * next();
            let (u, v) = (next() * 1.8, next() * 2.8);
            let uv = [
                Point2::new(u, v),
                Point2::new(u + size * next(), v + size * next()),
                Point2::new(u + size * next(), v - size * next()),
            ];
            let xyz = uv.map(|p| s.point(p.x, p.y));
            if triangle_ok(&s, UNIT, uv, xyz, &t) {
                passed += 1;
                assert!(deviation(&s, uv) <= t.chord, "{uv:?}");
            }
        }
        assert!(passed > 20, "only {passed} passed");
    }
}
