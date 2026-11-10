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
//! A piece within the chord of its corners by first derivatives alone
//! (`small_enough`: its diameter plus `|S_u| Δu + |S_v| Δv`) passes too:
//! where the parameter runs fast along the surface, the second derivative
//! is mostly tangential and Waldron's bound far from the truth.
//!
//! The normal (or tangent) can turn across a triangle (or segment) by at
//! most the turning rates of [`SurfaceBound`] (or `CurveBound`) times its
//! parameter spans. The angle test is waived for a piece shorter than the
//! chord tolerance in model space: a turn faster than that is a feature
//! smaller than the tolerance, and near a pole or an apex, where the
//! normal has no limit, the rates are unbounded or nearly so.

use crate::geom::sheet::Sheet;
use crate::geom::{Curve, Point2, Point3, Surface, SurfaceBound};
use crate::interval::Interval;

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

/// The bounds a piece is tested against over a box of parameters: from
/// the surface's jets for a spline, an extrusion or a revolution (see
/// [`jet_bound`]), whose own `derivative_bound` costs several times more
/// (a revolution's encloses its curve's derivatives afresh, allocating,
/// each time), else `derivative_bound`. Both are certified.
pub(crate) fn surface_bound(surface: &Surface, range: [[f64; 2]; 2]) -> SurfaceBound {
    match surface {
        Surface::Nurbs(_) | Surface::Extrusion(_) | Surface::Revolution(_) => {
            jet_bound(surface, range).unwrap_or_else(|| surface.derivative_bound(range))
        }
        _ => surface.derivative_bound(range),
    }
}

/// Bounds over a box from one enclosure of the surface's jets: `|S_kk|`
/// the magnitudes of their enclosures, and the unit normal's turning rates
/// `|w_k| / |w|` with `w = S_u × S_v` (the unit normal's derivative along
/// `k` is the part of `w_k` across the normal, over `|w|`), infinite where
/// `|w|` may vanish. `None` where the jets can't be enclosed.
pub(crate) fn jet_bound(surface: &Surface, range: [[f64; 2]; 2]) -> Option<SurfaceBound> {
    type V = [Interval; 3];
    let jets = jet_box(surface, range)?;
    let mag = |v: V| crate::math::sqrt(v.iter().map(|x| x.mag() * x.mag()).sum());
    let low = |v: V| crate::math::sqrt(v.iter().map(|x| x.square().lo()).sum());
    let cross = |a: V, b: V| -> V {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let add = |a: V, b: V| -> V { core::array::from_fn(|c| a[c] + b[c]) };
    let su: V = core::array::from_fn(|c| jets[c].d[0]);
    let sv: V = core::array::from_fn(|c| jets[c].d[1]);
    let h: [V; 3] = core::array::from_fn(|k| core::array::from_fn(|c| jets[c].h[k]));
    let w = low(cross(su, sv));
    let rate = |wk: V| {
        if w > 0.0 { mag(wk) / w } else { f64::INFINITY }
    };
    let b = SurfaceBound {
        duu: mag(h[0]),
        duv: mag(h[1]),
        dvv: mag(h[2]),
        turn_u: rate(add(cross(h[0], sv), cross(su, h[1]))),
        turn_v: rate(add(cross(h[1], sv), cross(su, h[2]))),
    };
    [b.duu, b.duv, b.dvv]
        .iter()
        .all(|x| x.is_finite())
        .then_some(b)
}

/// The surface's jets over a box, for a piece's bounds, which read only
/// its derivatives (the point's enclosure may be entire): a spline's by
/// [`crate::geom::NurbsSurface::derivative_box_small`], which encloses a
/// box small against its patch from a point value and a remainder rather
/// than cutting the patch's nets down to it (#305, #307, #308), else the
/// sheet's.
fn jet_box(surface: &Surface, range: [[f64; 2]; 2]) -> Option<crate::geom::jet::JVec<Interval>> {
    match surface {
        Surface::Nurbs(n) => Some(crate::geom::sheet::from_parts(
            n.derivative_box_small(range),
        )),
        _ => Sheet::of(surface).jet_box(range),
    }
}

/// The small-piece test, for a piece whose second-derivative bound is
/// loose (a spine's parameter running fast near its end makes `|S_uu|`
/// huge, almost all of it along the surface). With `c` a corner and `L`
/// the flat piece over the same parameters, for every `p` of the
/// parameter box `|S(p) - L(p)| <= |S(p) - S(c)| + |L(p) - L(c)|` (the
/// corner lies on the surface, within its edge's tolerance as
/// everywhere): the surface within `|S_u| Δu + |S_v| Δv` of `S(c)` (the
/// first derivatives bounded from the jets over the box, along the
/// straight path in the box from `c` to `p`), and the flat piece within
/// its diameter `size` of `L(c)`. True when the sum is within `chord`:
/// the same correspondence Waldron's bound certifies, so both ways.
pub(crate) fn small_enough(surface: &Surface, range: [[f64; 2]; 2], size: f64, chord: f64) -> bool {
    if size > chord {
        return false;
    }
    let Some(jets) = jet_box(surface, range) else {
        return false;
    };
    let mag = |k: usize| {
        crate::math::sqrt(
            (0..3)
                .map(|c| jets[c].d[k].mag() * jets[c].d[k].mag())
                .sum(),
        )
    };
    let reach = mag(0) * (range[0][1] - range[0][0]) + mag(1) * (range[1][1] - range[1][0]);
    size + reach <= chord
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

/// Whether a piece on `uv` (its corners at `xyz`) passes the angle test:
/// its normal turns by at most `angle`, else it is smaller than the
/// waiver. In a plane stretched from the speeds (see `uv::stretched`) a
/// triangle may run along one direction for many times the waiver while
/// lying well within it across, so the turn along each direction is
/// waived where the piece's extent that way (its corners measured along
/// the surface's unit tangent that way at its middle) is under the
/// waiver: there the turn is a feature smaller than the tolerance, as
/// for a small piece. Faces meshed at their speeds keep the whole-piece
/// rule.
fn angle_ok(
    surface: &Surface,
    stretched: bool,
    b: &SurfaceBound,
    range: [[f64; 2]; 2],
    xyz: &[Point3],
    angle: f64,
    waive: f64,
) -> bool {
    if diameter(xyz) < waive {
        return true;
    }
    // Waiving a direction only takes its term out of the sum, so a piece
    // whose whole turn passes passes waived too: the surface at its middle
    // is wanted only past that (#308: an evaluation per triangle tested).
    let whole = turn(b, range);
    if whole <= angle || !stretched {
        return whole <= angle;
    }
    let e = surface.eval(
        0.5 * (range[0][0] + range[0][1]),
        0.5 * (range[1][0] + range[1][1]),
    );
    let extent = |d: crate::geom::Vector3| match d.normalize() {
        Some(t) => {
            let t = t.get();
            let (lo, hi) = xyz
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                    let x = p.to_vector().dot(t);
                    (lo.min(x), hi.max(x))
                });
            hi - lo
        }
        None => f64::INFINITY,
    };
    let along = |rate: f64, [lo, hi]: [f64; 2], d| {
        if hi > lo && extent(d) >= waive {
            rate * (hi - lo)
        } else {
            0.0
        }
    };
    along(b.turn_u, range[0], e.du) + along(b.turn_v, range[1], e.dv) <= angle
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

/// What [`triangle_test`] found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Verdict {
    Pass,
    /// Longer than the longest edge allowed, or farther from the surface
    /// than the chord: refinement splits the longest edge in the plane.
    Long,
    /// Only the normal turns too far, by these rates per unit of `u` and
    /// of `v`: refinement splits the edge it turns most along.
    Turned {
        turn_u: f64,
        turn_v: f64,
    },
}

/// True when the flat triangle on `xyz` (the surface at the parameters
/// `uv`) meets the targets.
#[cfg(test)]
pub(crate) fn triangle_ok(
    surface: &Surface,
    metric: Metric,
    stretched: bool,
    uv: [Point2; 3],
    xyz: [Point3; 3],
    t: &Targets,
) -> bool {
    triangle_test(surface, metric, stretched, uv, xyz, t) == Verdict::Pass
}

/// [`triangle_ok`], saying which test failed.
pub(crate) fn triangle_test(
    surface: &Surface,
    metric: Metric,
    stretched: bool,
    uv: [Point2; 3],
    xyz: [Point3; 3],
    t: &Targets,
) -> Verdict {
    let size = diameter(&xyz);
    if size > t.max_edge {
        return Verdict::Long;
    }
    let range = uv_box(&uv);
    let b = surface_bound(surface, range);
    let r = enclosing_radius(uv.map(|p| metric.scale(p)));
    if second_derivative(&b, metric) * r * r / 2.0 > t.chord
        && !small_enough(surface, range, size, t.chord)
    {
        return Verdict::Long;
    }
    if angle_ok(surface, stretched, &b, range, &xyz, t.angle, t.waive) {
        Verdict::Pass
    } else {
        Verdict::Turned {
            turn_u: b.turn_u,
            turn_v: b.turn_v,
        }
    }
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
    stretched: bool,
    uv: [Point2; 2],
    xyz: [Point3; 2],
    t: &Targets,
) -> bool {
    let h = metric.scale(uv[0]).distance(metric.scale(uv[1]));
    let mid = Point2::new(0.5 * (uv[0].x + uv[1].x), 0.5 * (uv[0].y + uv[1].y));
    let (hu, hv) = (0.5 * h / metric.a, 0.5 * h / metric.b);
    let range = [[mid.x - hu, mid.x + hu], [mid.y - hv, mid.y + hv]];
    let b = surface_bound(surface, range);
    if second_derivative(&b, metric) * h * h / 8.0 > t.chord / 2.0
        && !small_enough(surface, range, xyz[0].distance(xyz[1]), t.chord / 2.0)
    {
        return false;
    }
    angle_ok(surface, stretched, &b, range, &xyz, t.angle / 2.0, t.waive)
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
        assert!(triangle_ok(&s, m, false, uv, xyz, &t));
        assert!(deviation(&s, uv) <= t.chord);
        let (uv, xyz) = tri(0.2);
        assert!(!triangle_ok(&s, m, false, uv, xyz, &t));
        // The angle alone: Δu = 0.02 turns the normal by 0.02.
        assert!(!triangle_ok(
            &s,
            m,
            false,
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
        assert!(!triangle_ok(&s, m, false, tri(0.02).0, tri(0.02).1, &short));
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
        assert!(face_segment_ok(&s, UNIT, false, uv, xyz, &t));
        let (uv, xyz) = seg(0.0201);
        assert!(!face_segment_ok(&s, UNIT, false, uv, xyz, &t));
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
        assert!(!triangle_ok(&s, UNIT, false, uv, xyz, &t));
        let (uv, xyz) = tri(0.0005);
        assert!(triangle_ok(&s, UNIT, false, uv, xyz, &t));
    }

    /// A degree 3 rational curve over three spans, with `x > 0`: a profile
    /// to revolve about z, or to extrude along z.
    fn profile() -> Curve {
        use crate::geom::{KnotVector, NurbsCurve};
        let k = KnotVector::clamped(3, &[0.0, 0.4, 1.0, 2.5], &[4, 1, 2, 4]).unwrap();
        let pts = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.6, 0.0, 0.5),
            Point3::new(0.7, 0.0, 1.2),
            Point3::new(1.4, 0.0, 2.0),
            Point3::new(1.1, 0.0, 2.6),
            Point3::new(1.9, 0.0, 3.1),
            Point3::new(1.2, 0.0, 4.0),
        ];
        let w = vec![1.0, 0.5, 2.0, 0.8, 1.5, 0.3, 1.0];
        Curve::Nurbs(Arc::new(NurbsCurve::new(k, pts, Some(w)).unwrap()))
    }

    /// `|S_kk|` and the unit normal's turning rates at `(u, v)`.
    fn sampled(s: &Surface, u: f64, v: f64) -> [f64; 5] {
        let e = s.eval(u, v);
        let w = e.du.cross(e.dv);
        let n = w.normalize().unwrap().get();
        let rate = |wk: crate::geom::Vector3| (wk - n * wk.dot(n)).length() / w.length();
        [
            e.duu.length(),
            e.duv.length(),
            e.dvv.length(),
            rate(e.duu.cross(e.dv) + e.du.cross(e.duv)),
            rate(e.duv.cross(e.dv) + e.du.cross(e.dvv)),
        ]
    }

    #[test]
    fn jet_bounds_hold_at_samples() {
        use crate::geom::{Extrusion, Revolution, Vector3};
        let turned = profile();
        // The profile laid in the xy plane, for an extrusion along z.
        let flat = turned.transformed(&crate::geom::Rigid::rotation(
            Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(),
            -core::f64::consts::FRAC_PI_2,
        ));
        let surfaces = [
            Surface::Revolution(Arc::new(Revolution::new(Frame::WORLD, turned).unwrap())),
            Surface::Extrusion(Arc::new(
                Extrusion::new(flat, Vector3::new(0.2, 0.1, 1.0)).unwrap(),
            )),
        ];
        let mut seed = 7u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for s in &surfaces {
            let [[u0, u1], [v0, v1]] = s.domain();
            let (u0, u1) = (u0.max(-1.0), u1.min(1.0).max(u0.max(-1.0) + 0.1));
            let (v0, v1) = (v0.max(-1.0), v1.min(3.0));
            for _ in 0..40 {
                let (du, dv) = (0.02 + 0.3 * next(), 0.02 + 0.3 * next());
                let u = u0 + (u1 - u0 - du).max(0.0) * next();
                let v = v0 + (v1 - v0 - dv).max(0.0) * next();
                let range = [[u, u + du], [v, v + dv]];
                let b = jet_bound(s, range).expect("a bound");
                let bounds = [b.duu, b.duv, b.dvv, b.turn_u, b.turn_v];
                for i in 0..=6 {
                    for j in 0..=6 {
                        let at =
                            sampled(s, u + du * f64::from(i) / 6.0, v + dv * f64::from(j) / 6.0);
                        for k in 0..5 {
                            assert!(at[k] <= bounds[k] * (1.0 + 1e-12), "{s:?} {range:?}: {k}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn small_pieces_are_within_the_chord() {
        // A sphere and a revolved spline, pieces a few millimetres across
        // in parameter boxes of every shape: whenever the small-piece
        // test passes, the sampled deviation is within the chord.
        use crate::geom::Revolution;
        let surfaces = [
            Surface::Sphere(Sphere::new(Frame::WORLD, 1.0).unwrap()),
            Surface::Revolution(Arc::new(Revolution::new(Frame::WORLD, profile()).unwrap())),
        ];
        let mut seed = 3u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let chord = 1e-2;
        let mut passed = 0;
        for s in &surfaces {
            for _ in 0..400 {
                let (u, v) = (next() * 3.0, 0.2 + next());
                let (du, dv) = (1e-3 + 1e-2 * next(), 1e-3 + 1e-2 * next());
                let uv = [
                    Point2::new(u, v),
                    Point2::new(u + du, v + dv * next()),
                    Point2::new(u + du * next(), v + dv),
                ];
                let xyz = uv.map(|p| s.point(p.x, p.y));
                if small_enough(s, uv_box(&uv), diameter(&xyz), chord) {
                    passed += 1;
                    assert!(deviation(s, uv) <= chord, "{s:?} {uv:?}");
                }
            }
            // Larger than the chord: never by this test.
            let uv = [
                Point2::new(0.0, 0.5),
                Point2::new(0.2, 0.5),
                Point2::new(0.0, 0.7),
            ];
            let xyz = uv.map(|p| s.point(p.x, p.y));
            assert!(!small_enough(s, uv_box(&uv), diameter(&xyz), chord));
        }
        assert!(passed > 40, "only {passed}");
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
            if triangle_ok(&s, UNIT, false, uv, xyz, &t) {
                passed += 1;
                assert!(deviation(&s, uv) <= t.chord, "{uv:?}");
            }
        }
        assert!(passed > 20, "only {passed} passed");
    }
}
