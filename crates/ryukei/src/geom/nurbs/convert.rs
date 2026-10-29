//! Exact rational NURBS forms of the analytic curves and surfaces.

use super::{KnotVector, NurbsCurve, NurbsCurve2, NurbsSurface};
use crate::Error;
use crate::geom::{Circle, Curve, Curve2, Ellipse, Frame, Frame2, Line, Point3, Surface};
use crate::math;
use core::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, FRAC_PI_4, PI, SQRT_2, TAU};

/// tan(π/8).
const TAN_PI_8: f64 = SQRT_2 - 1.0;

/// How a converted direction's NURBS parameter relates to the analytic
/// one. Both maps are increasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reparam {
    /// The NURBS parameter is the analytic one.
    Identity,
    /// The analytic parameter is an angle, and each quarter turn
    /// `[kπ/2, (k+1)π/2]` is one rational quadratic arc whose knots are its
    /// ends. With `ψ` the angle from the quarter's middle and `s` the
    /// fraction of the quarter's knot span, `tan(ψ/2) = tan(π/8)(2s - 1)`,
    /// so the two parameters agree at every multiple of π/4.
    Angle,
}

impl Reparam {
    /// The NURBS parameter for an analytic one.
    pub fn to_nurbs(self, t: f64) -> f64 {
        match self {
            Self::Identity => t,
            Self::Angle => {
                let q = (t / FRAC_PI_2).floor() * FRAC_PI_2;
                let psi = t - q - FRAC_PI_4;
                let s = 0.5 * (1.0 + math::tan(psi / 2.0) / TAN_PI_8);
                q + s * FRAC_PI_2
            }
        }
    }

    /// The analytic parameter for a NURBS one.
    pub fn from_nurbs(self, s: f64) -> f64 {
        match self {
            Self::Identity => s,
            Self::Angle => {
                let q = (s / FRAC_PI_2).floor() * FRAC_PI_2;
                let frac = (s - q) / FRAC_PI_2;
                let psi = 2.0 * math::atan2(TAN_PI_8 * (2.0 * frac - 1.0), 1.0);
                q + FRAC_PI_4 + psi
            }
        }
    }
}

const H: f64 = FRAC_1_SQRT_2;

/// The eight control points of a full turn as `(cos, sin, weight)`
/// coefficients, in the periodic knot vector's order: the corner before
/// angle 0 comes first, then the point at angle 0.
pub(crate) const TURN: [(f64, f64, f64); 8] = [
    (1.0, -1.0, H),
    (1.0, 0.0, 1.0),
    (1.0, 1.0, H),
    (0.0, 1.0, 1.0),
    (-1.0, 1.0, H),
    (-1.0, 0.0, 1.0),
    (-1.0, -1.0, H),
    (0.0, -1.0, 1.0),
];

/// The knots of a full turn: a periodic quadratic with a double knot at
/// every quarter.
pub(crate) fn turn_knots() -> Result<KnotVector, Error> {
    let quarters = [0.0, FRAC_PI_2, PI, PI + FRAC_PI_2, TAU];
    KnotVector::periodic(2, &quarters, &[2; 5])
}

/// A full turn in `frame`'s xy plane with semi-axes `a` along `x` and `b`
/// along `y`.
fn turn(frame: &Frame, a: f64, b: f64) -> Result<NurbsCurve, Error> {
    let (o, x, y) = (frame.origin(), frame.x().get(), frame.y().get());
    let points = TURN
        .iter()
        .map(|&(c, s, _)| o + x * (a * c) + y * (b * s))
        .collect();
    let weights = TURN.iter().map(|&(.., w)| w).collect();
    NurbsCurve::new(turn_knots()?, points, Some(weights))
}

impl Line {
    /// The segment over `range` as a degree 1 NURBS. Fails like
    /// [`KnotVector::clamped`] for a range that is not increasing or too
    /// short, or with `OutsideSizeBox`.
    pub fn to_nurbs(&self, [t0, t1]: [f64; 2]) -> Result<(NurbsCurve, Reparam), Error> {
        let knots = KnotVector::clamped(1, &[t0, t1], &[2, 2])?;
        let at = |t: f64| self.origin() + self.dir().get() * t;
        let curve = NurbsCurve::new(knots, vec![at(t0), at(t1)], None)?;
        Ok((curve, Reparam::Identity))
    }
}

impl Circle {
    /// The whole circle as a periodic rational quadratic: four quarter
    /// arcs. Fails with `OutsideSizeBox` when the control octagon, which
    /// reaches `r √2` from the centre, leaves the size box.
    pub fn to_nurbs(&self) -> Result<(NurbsCurve, Reparam), Error> {
        let r = self.radius();
        Ok((turn(self.frame(), r, r)?, Reparam::Angle))
    }
}

impl Ellipse {
    /// The whole ellipse, as for a circle, scaled along each axis.
    pub fn to_nurbs(&self) -> Result<(NurbsCurve, Reparam), Error> {
        Ok((
            turn(self.frame(), self.major(), self.minor())?,
            Reparam::Angle,
        ))
    }
}

impl Curve {
    /// The exact NURBS form of an analytic curve and how its parameter
    /// relates. `range` bounds an unbounded curve (a line); closed curves
    /// convert whole and ignore it; a NURBS is returned as it is.
    pub fn to_nurbs(&self, range: [f64; 2]) -> Result<(NurbsCurve, Reparam), Error> {
        match self {
            Self::Line(l) => l.to_nurbs(range),
            Self::Circle(c) => c.to_nurbs(),
            Self::Ellipse(e) => e.to_nurbs(),
            Self::Nurbs(n) => Ok(((**n).clone(), Reparam::Identity)),
        }
    }
}

/// A full turn in the plane, as for [`turn`].
fn turn2(frame: &Frame2, a: f64, b: f64) -> Result<NurbsCurve2, Error> {
    let (o, x, y) = (frame.origin(), frame.x(), frame.y());
    let points = TURN
        .iter()
        .map(|&(c, s, _)| o + x * (a * c) + y * (b * s))
        .collect();
    let weights = TURN.iter().map(|&(.., w)| w).collect();
    NurbsCurve2::new(turn_knots()?, points, Some(weights))
}

impl Curve2 {
    /// The exact NURBS form of a pcurve and how its parameter relates, as
    /// for [`Curve::to_nurbs`]: `range` bounds a line; a NURBS is returned
    /// as it is.
    pub fn to_nurbs(&self, [t0, t1]: [f64; 2]) -> Result<(NurbsCurve2, Reparam), Error> {
        match self {
            Self::Line2(l) => {
                let knots = KnotVector::clamped(1, &[t0, t1], &[2, 2])?;
                let at = |t: f64| l.origin() + l.dir() * t;
                let curve = NurbsCurve2::new(knots, vec![at(t0), at(t1)], None)?;
                Ok((curve, Reparam::Identity))
            }
            Self::Circle2(c) => Ok((turn2(c.frame(), c.radius(), c.radius())?, Reparam::Angle)),
            Self::Ellipse2(e) => Ok((turn2(e.frame(), e.major(), e.minor())?, Reparam::Angle)),
            Self::Nurbs2(n) => Ok(((**n).clone(), Reparam::Identity)),
        }
    }
}

/// A profile in a half-plane through the axis, as control points
/// `(distance from the axis, height, weight)`.
type Profile = Vec<(f64, f64, f64)>;

/// The surface swept by turning `profile` once about `frame`'s axis, with
/// `knots` along the profile (A8.1 in The NURBS Book): control point
/// `(i, j)` is turn corner `i` scaled by the profile's distance `j`, at
/// its height, weighted by both.
fn revolve(frame: &Frame, knots: KnotVector, profile: &Profile) -> Result<NurbsSurface, Error> {
    let (o, x, y, z) = (
        frame.origin(),
        frame.x().get(),
        frame.y().get(),
        frame.z().get(),
    );
    let mut points = Vec::with_capacity(TURN.len() * profile.len());
    let mut weights = Vec::with_capacity(points.capacity());
    for &(c, s, w) in &TURN {
        for &(rho, h, wp) in profile {
            points.push(o + x * (c * rho) + y * (s * rho) + z * h);
            weights.push(w * wp);
        }
    }
    NurbsSurface::new([turn_knots()?, knots], points, Some(weights))
}

impl Surface {
    /// The exact NURBS form of an analytic or swept surface and how each
    /// parameter relates. `ranges` bounds the unbounded directions (both
    /// for a plane, `v` for a cylinder, cone or extrusion); closed
    /// directions and a sphere's `v` convert whole and ignore their range;
    /// a NURBS is returned as it is. An extrusion is its curve's NURBS
    /// (see [`Curve::to_nurbs`]) times a degree 1 `v`; a revolution is the
    /// whole turn times its curve's NURBS, whose control points are
    /// measured in the frame's xz plane. A cone's `v` range
    /// must not go below its apex (`Degenerate`). A pipe has no exact
    /// NURBS form (`Degenerate`). Otherwise fails like [`Line::to_nurbs`].
    pub fn to_nurbs(&self, ranges: [[f64; 2]; 2]) -> Result<(NurbsSurface, [Reparam; 2]), Error> {
        let [[u0, u1], [v0, v1]] = ranges;
        let line = |a: f64, b: f64| KnotVector::clamped(1, &[a, b], &[2, 2]);
        let f = self.frame().unwrap_or(&Frame::WORLD);
        let swept = [Reparam::Angle, Reparam::Identity];
        let round = [Reparam::Angle, Reparam::Angle];
        match self {
            Self::Plane(_) => {
                let points = [u0, u1]
                    .iter()
                    .flat_map(|&u| [v0, v1].map(|v| f.to_world(u, v, 0.0)))
                    .collect::<Vec<Point3>>();
                let s = NurbsSurface::new([line(u0, u1)?, line(v0, v1)?], points, None)?;
                Ok((s, [Reparam::Identity; 2]))
            }
            Self::Cylinder(c) => {
                let r = c.radius();
                let profile = vec![(r, v0, 1.0), (r, v1, 1.0)];
                Ok((revolve(f, line(v0, v1)?, &profile)?, swept))
            }
            Self::Cone(c) => {
                if v0 < c.apex_v() {
                    return Err(Error::Degenerate);
                }
                let (sin, cos) = math::sin_cos(c.half_angle());
                let rho = |v: f64| (c.radius() + v * sin / cos).max(0.0);
                let profile = vec![(rho(v0), v0, 1.0), (rho(v1), v1, 1.0)];
                Ok((revolve(f, line(v0, v1)?, &profile)?, swept))
            }
            Self::Sphere(s) => {
                let r = s.radius();
                let knots = KnotVector::clamped(2, &[-FRAC_PI_2, 0.0, FRAC_PI_2], &[3, 2, 3])?;
                let profile = vec![
                    (0.0, -r, 1.0),
                    (r, -r, H),
                    (r, 0.0, 1.0),
                    (r, r, H),
                    (0.0, r, 1.0),
                ];
                Ok((revolve(f, knots, &profile)?, round))
            }
            Self::Torus(t) => {
                let (big, r) = (t.major(), t.minor());
                let profile = TURN
                    .iter()
                    .map(|&(c, s, w)| (big + r * c, r * s, w))
                    .collect();
                Ok((revolve(f, turn_knots()?, &profile)?, round))
            }
            Self::Nurbs(n) => Ok(((**n).clone(), [Reparam::Identity; 2])),
            Self::Extrusion(e) => {
                let (c, r) = e.curve().to_nurbs([u0, u1])?;
                let d = e.dir().get();
                let mut points = Vec::with_capacity(2 * c.control_points().len());
                let mut weights = Vec::with_capacity(points.capacity());
                for (q, &w) in c.control_points().zip(c.weights()) {
                    for v in [v0, v1] {
                        points.push(q + d * v);
                        weights.push(w);
                    }
                }
                let s =
                    NurbsSurface::new([c.knots().clone(), line(v0, v1)?], points, Some(weights))?;
                Ok((s, [r, Reparam::Identity]))
            }
            // A pipe about a curved spine has no exact rational form.
            Self::Pipe(_) | Self::Sweep(_) | Self::Offset(_) => Err(Error::Degenerate),
            Self::Revolution(s) => {
                let (c, r) = s.curve().to_nurbs([v0, v1])?;
                let f = s.frame();
                let (o, x, z) = (f.origin(), f.x().get(), f.z().get());
                let profile = c
                    .control_points()
                    .zip(c.weights())
                    .map(|(q, &w)| ((q - o).dot(x), (q - o).dot(z), w))
                    .collect();
                Ok((
                    revolve(f, c.knots().clone(), &profile)?,
                    [Reparam::Angle, r],
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{
        Circle, Cone, Curve, Cylinder, Ellipse, Frame, Line, Plane, Point3, Sphere, Surface, Torus,
        Vector3,
    };
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.3, -0.2, 1.0),
            Vector3::new(1.0, 0.5, 0.0),
        )
        .unwrap()
    }

    #[test]
    fn identity_reparam() {
        assert_eq!(Reparam::Identity.to_nurbs(1.25), 1.25);
        assert_eq!(Reparam::Identity.from_nurbs(-3.5), -3.5);
    }

    #[test]
    fn angle_reparam_fixes_the_quarters() {
        let r = Reparam::Angle;
        for k in -2..=4 {
            let q = f64::from(k) * FRAC_PI_2;
            assert!((r.to_nurbs(q) - q).abs() <= 1e-15, "k={k}");
            assert!((r.from_nurbs(q) - q).abs() <= 1e-15, "k={k}");
            // The middle of a quarter is the middle of its knot span.
            assert!((r.to_nurbs(q + FRAC_PI_4) - (q + FRAC_PI_4)).abs() <= 1e-15);
        }
        let mut last = r.to_nurbs(-PI);
        for i in 1..=400 {
            let t = -PI + 3.0 * PI * f64::from(i) / 400.0;
            let s = r.to_nurbs(t);
            assert!(s > last, "not increasing at {t}");
            assert!((r.from_nurbs(s) - t).abs() <= 1e-14, "round trip at {t}");
            last = s;
        }
    }

    #[test]
    fn a_line_segment() {
        let l = Line::new(Point3::new(1.0, 2.0, 3.0), Vector3::new(1.0, 2.0, 2.0)).unwrap();
        let (n, r) = l.to_nurbs([-1.0, 2.0]).unwrap();
        assert_eq!(r, Reparam::Identity);
        assert_eq!(n.degree(), 1);
        assert_eq!(n.domain(), [-1.0, 2.0]);
        let c = Curve::Line(l);
        for t in [-1.0, 0.0, 0.7, 2.0] {
            assert!(n.point(t).distance(c.point(t)) <= 1e-15);
        }
        assert_eq!(
            l.to_nurbs([2.0, -1.0]),
            Err(crate::Error::InvalidNurbs(crate::NurbsError::KnotOrder))
        );
    }

    /// The analytic curve and its conversion agree through the
    /// reparameterization, over two turns.
    fn agree(c: &Curve, n: &NurbsCurve, r: Reparam) {
        for i in 0..=200 {
            let t = -TAU + 2.0 * TAU * f64::from(i) / 200.0;
            let d = n.point(r.to_nurbs(t)).distance(c.point(t));
            assert!(d <= 1e-14, "{c:?} at {t}: {d}");
        }
    }

    #[test]
    fn a_circle() {
        let circle = Circle::new(frame(), 2.0).unwrap();
        let (n, r) = circle.to_nurbs().unwrap();
        assert_eq!(r, Reparam::Angle);
        assert_eq!(n.degree(), 2);
        assert_eq!(n.period(), Some(TAU));
        assert_eq!(n.control_points().len(), 8);
        let h = core::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(n.weights(), [h, 1.0, h, 1.0, h, 1.0, h, 1.0]);
        let c = Curve::Circle(circle);
        // The on-curve control points are the quarter points.
        let pts: Vec<_> = n.control_points().collect();
        assert!(pts[1].distance(c.point(0.0)) <= 1e-15);
        assert!(pts[3].distance(c.point(FRAC_PI_2)) <= 1e-15);
        agree(&c, &n, r);
    }

    #[test]
    fn an_ellipse() {
        let e = Ellipse::new(frame(), 3.0, 1.25).unwrap();
        let (n, r) = e.to_nurbs().unwrap();
        assert_eq!(r, Reparam::Angle);
        agree(&Curve::Ellipse(e), &n, r);
    }

    #[test]
    fn through_the_curve_enum() {
        let c = Curve::Circle(Circle::new(frame(), 2.0).unwrap());
        let (n, r) = c.to_nurbs([0.0, 1.0]).unwrap();
        assert_eq!(n.period(), Some(TAU));
        agree(&c, &n, r);
        let l = Curve::Line(Line::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap());
        let (n, _) = l.to_nurbs([0.0, 1.0]).unwrap();
        assert_eq!(n.domain(), [0.0, 1.0]);
    }

    /// The analytic surface and its conversion agree through the
    /// reparameterizations at a grid of parameters, in points and in
    /// normals.
    fn agree_surface(s: &Surface, ranges: [[f64; 2]; 2]) {
        let (n, [ru, rv]) = s.to_nurbs(ranges).unwrap();
        let [[u0, u1], [v0, v1]] = ranges;
        for i in 0..=16 {
            for j in 0..=16 {
                let u = u0 + (u1 - u0) * f64::from(i) / 16.0;
                let v = v0 + (v1 - v0) * f64::from(j) / 16.0;
                let (a, b) = (ru.to_nurbs(u), rv.to_nurbs(v));
                let d = n.point(a, b).distance(s.point(u, v));
                assert!(d <= 1e-14, "{s:?} at ({u}, {v}): {d}");
                if let Some(m) = s.principal_curvatures(u, v).and(s.normal(u, v)) {
                    let k = n.normal(a, b).unwrap();
                    assert!((k.get() - m.get()).length() <= 1e-12, "{s:?} at ({u}, {v})");
                }
            }
        }
    }

    #[test]
    fn every_surface_converts_exactly() {
        let f = frame();
        let around = [-TAU, TAU];
        agree_surface(&Surface::Plane(Plane::new(f)), [[-2.0, 3.0], [1.0, 4.0]]);
        agree_surface(
            &Surface::Cylinder(Cylinder::new(f, 2.0).unwrap()),
            [around, [-1.0, 3.0]],
        );
        let cone = Cone::new(f, 2.0, 0.4).unwrap();
        // From the apex up: a pole row at the bottom.
        agree_surface(&Surface::Cone(cone), [around, [cone.apex_v(), 3.0]]);
        agree_surface(
            &Surface::Sphere(Sphere::new(f, 2.0).unwrap()),
            [around, [-FRAC_PI_2, FRAC_PI_2]],
        );
        agree_surface(
            &Surface::Torus(Torus::new(f, 3.0, 1.0).unwrap()),
            [around, around],
        );
    }

    #[test]
    fn surface_conversion_shapes() {
        let f = frame();
        let sphere = Surface::Sphere(Sphere::new(f, 2.0).unwrap());
        let (n, r) = sphere.to_nurbs([[0.0, 0.0], [0.0, 0.0]]).unwrap();
        assert_eq!(r, [Reparam::Angle, Reparam::Angle]);
        assert_eq!(n.counts(), [8, 5]);
        assert_eq!(n.periods(), (Some(TAU), None));
        assert_eq!(n.domain()[1], [-FRAC_PI_2, FRAC_PI_2]);
        // The poles are collapsed rows.
        let south = n.control_point(0, 0);
        assert!((0..8).all(|i| n.control_point(i, 0) == south));
        assert!(south.distance(sphere.point(0.0, -FRAC_PI_2)) <= 1e-15);
        let torus = Surface::Torus(Torus::new(f, 3.0, 1.0).unwrap());
        let (n, _) = torus.to_nurbs([[0.0, 0.0], [0.0, 0.0]]).unwrap();
        assert_eq!(n.counts(), [8, 8]);
        assert_eq!(n.periods(), (Some(TAU), Some(TAU)));
        let plane = Surface::Plane(Plane::new(f));
        let (n, r) = plane.to_nurbs([[0.0, 1.0], [0.0, 2.0]]).unwrap();
        assert_eq!(r, [Reparam::Identity, Reparam::Identity]);
        assert_eq!((n.degrees(), n.counts()), ([1, 1], [2, 2]));
    }

    #[test]
    fn surface_conversion_rejections() {
        let f = frame();
        let cone = Cone::new(f, 2.0, 0.4).unwrap();
        let below = [[0.0, 1.0], [cone.apex_v() - 0.5, 1.0]];
        assert_eq!(
            Surface::Cone(cone).to_nurbs(below),
            Err(crate::Error::Degenerate)
        );
        assert_eq!(
            Surface::Plane(Plane::new(f)).to_nurbs([[1.0, 0.0], [0.0, 1.0]]),
            Err(crate::Error::InvalidNurbs(crate::NurbsError::KnotOrder))
        );
    }

    #[test]
    fn control_points_must_fit_the_size_box() {
        // The corners of the control octagon reach r √2 from the centre.
        let f = Frame::new(
            Point3::new(100.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 1.0, 0.0),
        )
        .unwrap();
        let c = Circle::new(f, 399.0).unwrap();
        assert_eq!(c.to_nurbs(), Err(crate::Error::OutsideSizeBox));
    }
}
