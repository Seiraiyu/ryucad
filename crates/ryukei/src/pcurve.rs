//! Exact pcurves: an edge's curve in the parameter plane of a face's
//! surface, found from the geometry alone.
//!
//! [`exact`] knows two families, which cover every edge the primitives and
//! sweeps make:
//!
//! - **On a plane**, a line, circle, ellipse or planar NURBS maps to the
//!   same kind of curve in the plane's frame, with the same parameter. On
//!   a NURBS that is a parallelogram (degree 1 both ways, two control
//!   points each way, equal weights), whose parameters are an affine map
//!   of the plane, a line or NURBS maps the same way.
//! - **On any other surface**, an edge along a parameter line (a circle
//!   round an axis, a ruling of a cylinder or cone, a swept surface's
//!   generator moved along it) is a [`Line2`] whose parameter is an affine
//!   function of the edge's. The constant parameter and the affine map are
//!   read from projections of points inside the edge, where no pole or
//!   apex can make them ambiguous.
//!
//! Either way the result is accepted only if the surface takes it to
//! within the edge's tolerance of the edge at a fixed set of samples, so
//! an edge that fits neither family gets no pcurve rather than a wrong
//! one. [`fill`] gives every fin of a body that lacks one its exact
//! pcurve.

use crate::Error;
use crate::geom::{
    Circle2, Curve, Curve2, Ellipse2, Frame, Frame2, Line2, NurbsCurve2, Point2, Point3, Surface,
    Vector2,
};
use crate::topo::{Body, FinId, Pcurve};
use std::sync::Arc;

/// Samples at which a pcurve must fit its edge, both ends included.
const SAMPLES: usize = 17;

/// The pcurve of `curve` over `interval` on `surface`, as a curve and the
/// affine map from the edge's parameter to its own, when it is one of the
/// families above and fits within `tolerance` at every sample.
pub fn exact(
    surface: &Surface,
    curve: &Curve,
    interval: [f64; 2],
    tolerance: f64,
) -> Option<(Curve2, [f64; 2])> {
    let candidate = match surface {
        Surface::Plane(_) => on_plane(surface.frame()?, curve),
        _ => match affine(surface) {
            Some(a) => on_affine(&a, curve),
            None => iso_line(surface, curve, interval),
        },
    }?;
    fits(surface, curve, interval, tolerance, &candidate).then_some(candidate)
}

/// Gives every fin without a pcurve its exact one. Fails with `NoPcurve`,
/// naming the edge and face, at the first fin that has none, leaving the
/// fins before it filled.
pub fn fill(body: &mut Body) -> Result<(), Error> {
    let fins: Vec<FinId> = body
        .fins()
        .filter(|&f| body.fins[f].pcurve.is_none())
        .collect();
    for f in fins {
        let edge = body.fins[f].edge;
        let face = body.loops[body.fins[f].loop_].face;
        let e = &body.edges[edge];
        let surface = &body.surfaces[body.faces[face].surface];
        let found = exact(
            surface,
            &body.curves[e.curve],
            e.interval,
            e.tolerance.get(),
        );
        let (c, map) = found.ok_or(Error::NoPcurve { edge, face })?;
        let curve = body.add_curve2(c);
        body.fins[f].pcurve = Some(Pcurve { curve, map });
    }
    Ok(())
}

/// A planar curve in a plane's frame, with the same parameter. The curve
/// is assumed to lie in the plane; [`fits`] confirms it.
fn on_plane(frame: &Frame, curve: &Curve) -> Option<(Curve2, [f64; 2])> {
    let local = |p: Point3| {
        let v = frame.to_local(p);
        Point2::new(v.x, v.y)
    };
    let along =
        |d: crate::geom::Vector3| Vector2::new(d.dot(frame.x().get()), d.dot(frame.y().get()));
    let identity = [0.0, 1.0];
    let c2 = match curve {
        Curve::Line(l) => Curve2::Line2(Line2::new(local(l.origin()), along(l.dir().get())).ok()?),
        Curve::Circle(c) => {
            Curve2::Circle2(Circle2::new(frame2(frame, c.frame())?, c.radius()).ok()?)
        }
        Curve::Ellipse(e) => {
            Curve2::Ellipse2(Ellipse2::new(frame2(frame, e.frame())?, e.major(), e.minor()).ok()?)
        }
        Curve::Nurbs(n) => {
            let points = n.control_points().map(local).collect();
            let weights = Some(n.weights().to_vec());
            Curve2::Nurbs2(Arc::new(
                NurbsCurve2::new(n.knots().clone(), points, weights).ok()?,
            ))
        }
    };
    Some((c2, identity))
}

/// A surface `origin + u a + v b`.
struct Affine {
    origin: Point3,
    a: crate::geom::Vector3,
    b: crate::geom::Vector3,
}

impl Affine {
    /// The parameters of the point of the surface nearest `p`.
    fn local(&self, p: Point3) -> Point2 {
        let (x, y) = self.solve(p - self.origin);
        Point2::new(x, y)
    }

    /// The parameter step along the surface nearest the step `d`.
    fn solve(&self, d: crate::geom::Vector3) -> (f64, f64) {
        let (aa, ab, bb) = (self.a.dot(self.a), self.a.dot(self.b), self.b.dot(self.b));
        let (ad, bd) = (self.a.dot(d), self.b.dot(d));
        let det = aa * bb - ab * ab;
        ((bb * ad - ab * bd) / det, (aa * bd - ab * ad) / det)
    }
}

/// A NURBS parallelogram as an affine surface: degree 1 with two control
/// points each way, equal weights, and opposite sides equal within
/// resolution.
fn affine(surface: &Surface) -> Option<Affine> {
    let Surface::Nurbs(n) = surface else {
        return None;
    };
    if n.degrees() != [1, 1] || n.counts() != [2, 2] {
        return None;
    }
    let w = n.weights();
    if w.iter().any(|&x| x != w[0]) {
        return None;
    }
    let p = |i, j| n.control_point(i, j);
    let skew = (p(1, 1) - p(1, 0)) - (p(0, 1) - p(0, 0));
    if skew.length() > crate::tol::LINEAR_RESOLUTION {
        return None;
    }
    let [[u0, u1], [v0, v1]] = n.domain();
    let a = (p(1, 0) - p(0, 0)) / (u1 - u0);
    let b = (p(0, 1) - p(0, 0)) / (v1 - v0);
    let origin = p(0, 0) - a * u0 - b * v0;
    let det = a.dot(a) * b.dot(b) - a.dot(b) * a.dot(b);
    (det > 0.0).then_some(Affine { origin, a, b })
}

/// A line or NURBS on an affine surface: the line keeps its parameter up
/// to its speed in the parameter plane, the NURBS its control points'
/// images and its parameter.
fn on_affine(s: &Affine, curve: &Curve) -> Option<(Curve2, [f64; 2])> {
    match curve {
        Curve::Line(l) => {
            let (x, y) = s.solve(l.dir().get());
            let dir = Vector2::new(x, y);
            let line = Line2::new(s.local(l.origin()), dir).ok()?;
            Some((Curve2::Line2(line), [0.0, dir.length()]))
        }
        Curve::Nurbs(n) => {
            let points = n.control_points().map(|p| s.local(p)).collect();
            let weights = Some(n.weights().to_vec());
            let c = NurbsCurve2::new(n.knots().clone(), points, weights).ok()?;
            Some((Curve2::Nurbs2(Arc::new(c)), [0.0, 1.0]))
        }
        _ => None,
    }
}

/// A circle's or ellipse's frame in a plane's: its origin and `x` in the
/// plane, turning counterclockwise when the two normals agree.
fn frame2(plane: &Frame, f: &Frame) -> Option<Frame2> {
    let o = plane.to_local(f.origin());
    let x = f.x().get();
    let x = Vector2::new(x.dot(plane.x().get()), x.dot(plane.y().get()));
    let ccw = f.z().get().dot(plane.z().get()) > 0.0;
    Frame2::new(Point2::new(o.x, o.y), x, ccw).ok()
}

/// A parameter line with an affine parameter: one surface parameter held
/// at its value at the edge's middle, the other `a + b t`, from the
/// projections of the points a quarter and three quarters along. Those
/// points are never at a pole or apex of a valid face, and they are a
/// quarter of the edge from its middle, so unwrapping a periodic
/// parameter step by step is never ambiguous.
fn iso_line(surface: &Surface, curve: &Curve, [t0, t1]: [f64; 2]) -> Option<(Curve2, [f64; 2])> {
    let (pu, pv) = surface.periods();
    let periods = [pu, pv];
    let at = |f: f64| t0 + (t1 - t0) * f;
    let ts = [at(0.25), at(0.5), at(0.75)];
    let mut x: Vec<[f64; 2]> = Vec::with_capacity(3);
    for &t in &ts {
        let (u, v) = surface.project(curve.point(t)).param;
        let raw = [u, v];
        let next = match x.last() {
            Some(prev) => core::array::from_fn(|k| unwrap(raw[k], prev[k], periods[k])),
            None => raw,
        };
        x.push(next);
    }
    let spread = |k: usize| (x[2][k] - x[0][k]).abs();
    // The parameter that moves; the other must not.
    let k = if spread(0) >= spread(1) { 0 } else { 1 };
    let held = 1 - k;
    let b = (x[2][k] - x[0][k]) / (ts[2] - ts[0]);
    if b == 0.0 || !b.is_finite() {
        return None;
    }
    let a = x[1][k] - b * ts[1];
    // Line2 runs at unit speed along +k or -k from where the moving
    // parameter is `a`: s = |b| t.
    let mut origin = [0.0; 2];
    origin[k] = a;
    origin[held] = x[1][held];
    let mut dir = [0.0; 2];
    dir[k] = b.signum();
    let line = Line2::new(
        Point2::new(origin[0], origin[1]),
        Vector2::new(dir[0], dir[1]),
    )
    .ok()?;
    Some((Curve2::Line2(line), [0.0, b.abs()]))
}

/// `x` moved by whole periods to be as close as possible to `near`.
fn unwrap(x: f64, near: f64, period: Option<f64>) -> f64 {
    match period {
        Some(p) => x + ((near - x) / p).round() * p,
        None => x,
    }
}

/// Whether the surface takes the pcurve to within `tolerance` of the edge
/// at every sample.
pub(crate) fn fits(
    surface: &Surface,
    curve: &Curve,
    [t0, t1]: [f64; 2],
    tolerance: f64,
    (c2, map): &(Curve2, [f64; 2]),
) -> bool {
    (0..SAMPLES).all(|i| {
        let t = t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64;
        let s = map[0] + map[1] * t;
        let q = c2.point(s);
        surface.point(q.x, q.y).distance(curve.point(t)) <= tolerance
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{
        Circle, Cone, Cylinder, Ellipse, Extrusion, KnotVector, Line, NurbsCurve, Plane,
        Revolution, Rigid, Sphere, Torus, Vector3,
    };
    use crate::name::OpKey;
    use crate::primitive;
    use crate::tol::LINEAR_RESOLUTION;
    use core::f64::consts::{FRAC_PI_2, PI, TAU};

    const TOL: f64 = LINEAR_RESOLUTION;

    fn tilted() -> Frame {
        Frame::new(
            Point3::new(0.3, -0.2, 0.7),
            Vector3::new(0.2, -0.4, 1.0),
            Vector3::new(1.0, 0.3, 0.1),
        )
        .unwrap()
    }

    /// The largest distance between the surface's image of the pcurve and
    /// the edge, sampled finely.
    fn worst(surface: &Surface, curve: &Curve, [t0, t1]: [f64; 2]) -> f64 {
        let (c2, map) = exact(surface, curve, [t0, t1], TOL).expect("a pcurve");
        (0..=200)
            .map(|i| {
                let t = t0 + (t1 - t0) * i as f64 / 200.0;
                let q = c2.point(map[0] + map[1] * t);
                surface.point(q.x, q.y).distance(curve.point(t))
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn fitted_pcurves_hold_within_their_bound() {
        // A circle round a cylinder in its NURBS form, whose angle is no
        // longer the circle's: no exact pcurve, a fitted one.
        let f = tilted();
        let cyl = Surface::Cylinder(Cylinder::new(f, 0.5).unwrap());
        let (n, _) = cyl.to_nurbs([[0.0, TAU], [0.0, 1.0]]).unwrap();
        let nurbs = Surface::Nurbs(Arc::new(n));
        let circle = Curve::Circle(
            Circle::new(
                Frame::new(f.to_world(0.0, 0.0, 0.3), f.z().get(), f.x().get()).unwrap(),
                0.5,
            )
            .unwrap(),
        );
        let range = [0.5, 2.0];
        assert_eq!(exact(&nurbs, &circle, range, TOL), None);
        let (c2, bound) = fitted(&nurbs, &circle, range, TOL).unwrap();
        assert!((TOL..=10.0 * TOL).contains(&bound), "{bound}");
        assert_eq!(c2.domain(), range);
        for i in 0..=100 {
            let t = range[0] + (range[1] - range[0]) * i as f64 / 100.0;
            let q = c2.point(t);
            assert!(nurbs.point(q.x, q.y).distance(circle.point(t)) <= bound);
        }
        // A line off the surface gets nothing.
        let off = Curve::Line(
            Line::through(f.to_world(2.0, 0.0, 0.0), f.to_world(2.0, 1.0, 0.0)).unwrap(),
        );
        assert!(fitted(&nurbs, &off, [0.0, 1.0], TOL).is_none());
    }

    #[test]
    fn planar_curves_keep_their_kind_and_parameter() {
        let f = tilted();
        let plane = Surface::Plane(Plane::new(f));
        let p = |x: f64, y: f64| f.to_world(x, y, 0.0);
        let line = Curve::Line(Line::through(p(0.1, 0.2), p(0.9, -0.4)).unwrap());
        let (c2, map) = exact(&plane, &line, [0.0, 1.0], TOL).unwrap();
        assert!(matches!(c2, Curve2::Line2(_)));
        assert_eq!(map, [0.0, 1.0]);
        // A circle turning either way round the plane's normal.
        for z in [f.z().get(), -f.z().get()] {
            let cf = Frame::new(p(0.5, 0.5), z, f.x().get()).unwrap();
            let circle = Curve::Circle(Circle::new(cf, 0.4).unwrap());
            let (c2, _) = exact(&plane, &circle, [0.0, TAU], TOL).unwrap();
            let Curve2::Circle2(c) = c2 else {
                panic!("a circle")
            };
            assert_eq!(c.frame().is_ccw(), z.dot(f.z().get()) > 0.0);
            assert!(worst(&plane, &circle, [0.0, TAU]) <= 1e-15);
            let ellipse = Curve::Ellipse(Ellipse::new(cf, 0.4, 0.1).unwrap());
            assert!(worst(&plane, &ellipse, [1.0, 4.0]) <= 1e-15);
        }
        let knots = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let n = NurbsCurve::new(
            knots,
            vec![p(0.0, 0.0), p(0.5, 1.0), p(1.0, 0.0)],
            Some(vec![1.0, 0.5, 1.0]),
        )
        .unwrap();
        let n = Curve::Nurbs(Arc::new(n));
        assert!(worst(&plane, &n, [0.0, 1.0]) <= 1e-15);
        // A line leaving the plane gets nothing.
        let off = Curve::Line(Line::through(p(0.0, 0.0), f.to_world(0.0, 0.0, 1.0)).unwrap());
        assert_eq!(exact(&plane, &off, [0.0, 1.0], TOL), None);
    }

    #[test]
    fn nurbs_parallelograms_map_lines_and_nurbs_affinely() {
        let f = tilted();
        let plane = Surface::Plane(Plane::new(f));
        let (n, _) = plane.to_nurbs([[-1.0, 3.0], [0.5, 1.0]]).unwrap();
        let s = Surface::Nurbs(Arc::new(n));
        let p = |x: f64, y: f64| f.to_world(x, y, 0.0);
        let line = Curve::Line(Line::through(p(0.1, 0.6), p(0.9, 0.8)).unwrap());
        let (c2, map) = exact(&s, &line, [0.0, 0.5], TOL).unwrap();
        assert!(matches!(c2, Curve2::Line2(_)));
        assert!(map[1] > 0.0);
        assert!(worst(&s, &line, [0.0, 0.5]) <= 1e-14);
        let circle = Circle::new(
            Frame::new(p(1.0, 0.75), f.z().get(), f.x().get()).unwrap(),
            0.2,
        )
        .unwrap();
        let (cn, _) = circle.to_nurbs().unwrap();
        let cn = Curve::Nurbs(Arc::new(cn));
        assert!(worst(&s, &cn, [0.0, TAU]) <= 1e-14);
        // A circle itself has no exact pcurve on a parallelogram.
        assert_eq!(exact(&s, &Curve::Circle(circle), [0.0, TAU], TOL), None);
    }

    #[test]
    fn circles_round_an_axis_are_lines_in_u() {
        let f = tilted();
        let cyl = Surface::Cylinder(Cylinder::new(f, 0.5).unwrap());
        let cone = Surface::Cone(Cone::new(f, 0.5, 0.3).unwrap());
        let sphere = Surface::Sphere(Sphere::new(f, 0.5).unwrap());
        let torus = Surface::Torus(Torus::new(f, 0.5, 0.2).unwrap());
        let circle = |h: f64, r: f64, flip: bool| {
            let z = if flip { -f.z().get() } else { f.z().get() };
            let cf = Frame::new(f.to_world(0.0, 0.0, h), z, f.y().get()).unwrap();
            Curve::Circle(Circle::new(cf, r).unwrap())
        };
        let cone_r = |h: f64| 0.5 + h * crate::math::tan(0.3);
        for flip in [false, true] {
            for (s, c) in [
                (&cyl, circle(0.4, 0.5, flip)),
                (&cone, circle(0.4, cone_r(0.4), flip)),
                (&sphere, circle(0.3, 0.4, flip)),
                (&torus, circle(0.2, 0.5, flip)),
            ] {
                // A whole turn, a part across the seam, and a part.
                for i in [[0.0, TAU], [-1.0, 2.0], [2.0, 3.0]] {
                    assert!(worst(s, &c, i) <= 1e-12, "{s:?} {c:?} {i:?}");
                    let (c2, map) = exact(s, &c, i, TOL).unwrap();
                    let Curve2::Line2(l) = c2 else { panic!() };
                    assert_eq!(l.dir().y, 0.0);
                    assert!((map[1] - 1.0).abs() <= 1e-12);
                }
            }
        }
    }

    #[test]
    fn rulings_and_meridians_are_lines_in_v() {
        let f = tilted();
        let cyl = Surface::Cylinder(Cylinder::new(f, 0.5).unwrap());
        let along = |s: &Surface, u: f64, v0: f64, v1: f64| {
            Curve::Line(Line::through(s.point(u, v0), s.point(u, v1)).unwrap())
        };
        let c = along(&cyl, 1.0, 0.0, 0.8);
        assert!(worst(&cyl, &c, [0.0, 0.8]) <= 1e-12);
        // A cone's ruling runs faster than its v.
        let cone = Surface::Cone(Cone::new(f, 0.5, 0.3).unwrap());
        let c = along(&cone, 2.0, 0.8, 0.0);
        let len = cone.point(2.0, 0.8).distance(cone.point(2.0, 0.0));
        let (_, map) = exact(&cone, &c, [0.0, len], TOL).unwrap();
        assert!((map[1] - crate::math::cos(0.3)).abs() <= 1e-12);
        assert!(worst(&cone, &c, [0.0, len]) <= 1e-12);
        // A sphere's meridian from pole to pole, and a torus's tube circle.
        let sphere = Surface::Sphere(Sphere::new(f, 0.5).unwrap());
        let mf = Frame::new(f.origin(), f.radial(1.0 + FRAC_PI_2), f.radial(1.0)).unwrap();
        let meridian = Curve::Circle(Circle::new(mf, 0.5).unwrap());
        assert!(worst(&sphere, &meridian, [-FRAC_PI_2, FRAC_PI_2]) <= 1e-12);
        let torus = Surface::Torus(Torus::new(f, 0.5, 0.2).unwrap());
        let tf = Frame::new(
            f.origin() + f.radial(1.0) * 0.5,
            -f.radial(1.0 + FRAC_PI_2),
            f.radial(1.0),
        )
        .unwrap();
        let tube = Curve::Circle(Circle::new(tf, 0.2).unwrap());
        assert!(worst(&torus, &tube, [0.0, TAU]) <= 1e-12);
        assert!(worst(&torus, &tube, [PI, PI + 1.0]) <= 1e-12);
    }

    #[test]
    fn swept_surfaces_carry_their_generators() {
        let knots = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let n = NurbsCurve::new(
            knots,
            vec![
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(1.5, 0.0, 0.5),
                Point3::new(1.0, 0.0, 1.0),
            ],
            None,
        )
        .unwrap();
        let generator = Curve::Nurbs(Arc::new(n));
        let ext = Surface::Extrusion(Arc::new(
            Extrusion::new(generator.clone(), Vector3::new(0.0, 1.0, 0.2)).unwrap(),
        ));
        // The generator moved along the direction.
        let moved = match &ext {
            Surface::Extrusion(e) => {
                let d = e.dir().get() * 0.7;
                generator.transformed(&Rigid::translation(d))
            }
            _ => unreachable!(),
        };
        assert!(worst(&ext, &moved, [0.0, 1.0]) <= 1e-12);
        let rev = Surface::Revolution(Arc::new(
            Revolution::new(Frame::WORLD, generator.clone()).unwrap(),
        ));
        let turned = generator.transformed(&Rigid::rotation(crate::geom::UnitVector3::Z, 0.6));
        assert!(worst(&rev, &turned, [0.0, 1.0]) <= 1e-12);
        let arc = Curve::Circle(Circle::new(Frame::WORLD, 1.0).unwrap());
        assert!(worst(&rev, &arc, [0.0, TAU]) <= 1e-12);
    }

    #[test]
    fn a_curve_off_every_parameter_line_gets_nothing() {
        let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
        // A helix-like chord across the cylinder: neither parameter holds.
        let c = Curve::Line(
            Line::through(Point3::new(1.0, 0.0, 0.0), Point3::new(0.0, 1.0, 1.0)).unwrap(),
        );
        assert_eq!(exact(&cyl, &c, [0.0, 1.0], TOL), None);
    }

    #[test]
    fn fill_gives_every_primitive_fin_a_pcurve() {
        let placement = Rigid::from_frame(&tilted());
        let bodies = [
            primitive::block(OpKey(1), 1.0, 2.0, 3.0, &placement)
                .unwrap()
                .0,
            primitive::cylinder(OpKey(1), 0.5, 2.0, &placement)
                .unwrap()
                .0,
            primitive::cone(OpKey(1), 0.5, 2.0, &placement).unwrap().0,
            primitive::frustum(OpKey(1), 0.5, 0.2, 2.0, &placement)
                .unwrap()
                .0,
        ];
        for mut b in bodies {
            for f in b.fins().collect::<Vec<_>>() {
                b.fins[f].pcurve = None;
            }
            fill(&mut b).unwrap();
            assert!(b.fins().all(|f| b.fins[f].pcurve.is_some()));
        }
    }

    #[test]
    fn fill_names_the_fin_it_cannot_fill() {
        let (mut b, _) = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        for f in b.fins().collect::<Vec<_>>() {
            b.fins[f].pcurve = None;
        }
        let f = b.faces().next().unwrap();
        let cyl = b.add_surface(Surface::Cylinder(Cylinder::new(Frame::WORLD, 9.0).unwrap()));
        b.faces[f].surface = cyl;
        let Err(Error::NoPcurve { face, .. }) = fill(&mut b) else {
            panic!("no pcurve")
        };
        assert_eq!(face, f);
    }
}
