//! The geometry of a chain of chamfers.
//!
//! A chamfer is set back from its edge along each face: its rail on a
//! face is where a sphere of the setback swept along the edge meets the
//! face, the point at that distance from the edge in each plane normal
//! to it, on the face's side. The curve the sections follow, the
//! carrier, is the edge's line, circle or ellipse, or for a fitted edge
//! the branch of its faces' intersection through it, which runs on past
//! its vertices. Along a line between planes and cylinders, and along a
//! circle round coaxial faces, the rails are lines and circles, and the
//! chamfer is a plane, a cylinder or a cone; along anything else both
//! rails are fitted on shared knots by the carrier's parameter
//! ([`crate::intersect::fit_on`]), certified on their faces, and the
//! chamfer is the ruled NURBS between them, which holds them exactly.
//!
//! A distance and an angle give the other setback as
//! `d sin θ / sin(α + θ)`, `α` the angle of the wedge cut or filled, only
//! where `α` is the same all along: a line between planes, a circle round
//! coaxial faces.

use super::Chamfer;
use super::chain::Chain;
use super::convex::{Bend, BendFail, bend};
use super::end::on_edge;
use super::plan::{Joint2, Partial, Section, SmoothAt};
use super::rail::{Rail, circle_rail, line_rail};
use super::spec::Spec;
use super::spine::{nearest, supports};
use crate::geom::{
    Cone, Curve, Curve2, Cylinder, Frame, KnotVector, Line, Line2, NurbsCurve, NurbsSurface, Plane,
    Point2, Point3, Surface, Vector2, Vector3,
};
use crate::intersect::coincide;
use crate::intersect::{self, Failure, SurfaceHit, fit_on, fit_pcurve};
use crate::math;
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, Sense};
use crate::{BlendError, Error};
use core::f64::consts::PI;
use std::sync::Arc;

/// The carrier, rails and setbacks of the chamfer segment of `edges`
/// (chain positions).
pub(crate) fn partial(
    body: &Body,
    specs: &[Spec],
    chain: &Chain,
    edges: Vec<usize>,
    chamfer: &Chamfer,
    closed: bool,
) -> Result<Partial, Error> {
    let (k, forward) = chain.edges[edges[0]];
    let spec = &specs[k];
    let name = spec.name;
    let convex = match bend(body, spec.edge) {
        Ok(Bend::Convex) => true,
        Ok(Bend::Concave) => false,
        Ok(Bend::Smooth) => return Err(BlendError::SmoothEdge { edge: name }.into()),
        Err(BendFail::Changes) => return Err(BlendError::ConvexityChanges { edge: name }.into()),
        Err(BendFail::Uncertain) => return Err(BlendError::Uncertain { at: name }.into()),
    };
    let _ = convex;
    let rail_side = chain.rail_side(specs, k);
    let sides = [rail_side, 1 - rail_side];
    let faces = sides.map(|i| spec.faces[i]);
    // Carried past their domains where the faces reach their edges, so
    // rails and carriers run on past the faces' ends.
    let carried = supports(body, spec);
    let surfaces = sides.map(|i| carried[i].clone());
    let (carrier, span) = carrier(body, spec, &carried, forward, closed)?;
    let setbacks = setbacks(body, spec, faces, chamfer, &carrier)?;
    let reach = 2.0 * setbacks[0].max(setbacks[1]);
    let span = match &carrier {
        Curve::Line(_) => [span[0] - 5.0 * reach, span[1] + 5.0 * reach],
        _ => span,
    };
    // Which way each face's fin on the edge runs along the chain.
    let along = sides.map(|i| {
        let fwd = body.fins[spec.fins[i]].sense == Sense::Forward;
        if fwd == forward { 1.0 } else { -1.0 }
    });
    let sense = faces.map(|f| {
        if body.faces[f].sense == Sense::Forward {
            1.0
        } else {
            -1.0
        }
    });
    let point = |t: f64, side: usize| -> Option<Point3> {
        setback(
            &surfaces[side],
            sense[side],
            &carrier,
            t,
            along[side],
            setbacks[side],
        )
    };
    let uncertain = || -> Error { BlendError::Uncertain { at: name }.into() };
    let exact: Vec<Option<(Curve, [f64; 2], f64)>> = (0..2)
        .map(|side| {
            let foot = |t: f64| point(t, side).unwrap_or(Point3::ORIGIN);
            match &carrier {
                Curve::Line(_) => line_rail(&foot, span),
                Curve::Circle(c) => circle_rail(c, &foot, span),
                _ => None,
            }
        })
        .collect();
    let (rails, ruled) = if let [Some(a), Some(b)] = &exact[..] {
        let mut rails = Vec::with_capacity(2);
        for (side, (curve, map, off)) in [a, b].into_iter().enumerate() {
            let interval = span.map(|t| map[0] + map[1] * t);
            let tol = off.max(LINEAR_RESOLUTION);
            let (pc, pmap) = crate::pcurve::exact(&surfaces[side], curve, interval, tol)
                .ok_or_else(uncertain)?;
            rails.push(Rail {
                curve: curve.clone(),
                map: *map,
                pcurve: (pc, pmap),
                tolerance: tol,
                on_blend: None,
            });
        }
        (rails, None)
    } else {
        let h = (span[1] - span[0]) * 1e-6;
        let at = |t: f64| -> Option<[(Point3, Vector3); 2]> {
            let mut out = [(Point3::ORIGIN, Vector3::ZERO); 2];
            for (side, o) in out.iter_mut().enumerate() {
                let p = point(t, side)?;
                let d = (point(t + h, side)? - point(t - h, side)?) / (2.0 * h);
                *o = (p, d);
            }
            Some(out)
        };
        let fit = fit_on([&surfaces[0], &surfaces[1]], span, &at).map_err(|f| match f {
            Failure::Unsupported => BlendError::Unsupported { at: name },
            Failure::Uncertain => BlendError::Uncertain { at: name },
        })?;
        let [a, b] = fit.curves;
        let mut rails = Vec::with_capacity(2);
        for (side, c) in [&a, &b].into_iter().enumerate() {
            let tol = fit.bounds[side];
            let curve = Curve::Nurbs(Arc::new(c.clone()));
            // Where either face has no implicit form, the rails' pcurves
            // come from the fit, certified with it.
            let (pc, bound) = match &fit.pcurves[side] {
                Some(x) => x.clone(),
                None => fit_pcurve(&surfaces[side], &curve, span).map_err(|_| uncertain())?,
            };
            let v = if side == 0 { 0.0 } else { 1.0 };
            let line =
                Line2::new(Point2::new(0.0, v), Vector2::new(1.0, 0.0)).map_err(|_| uncertain())?;
            rails.push(Rail {
                curve,
                map: [0.0, 1.0],
                pcurve: (Curve2::Nurbs2(Arc::new(pc)), [0.0, 1.0]),
                tolerance: tol.max(bound),
                on_blend: Some((Curve2::Line2(line), [0.0, 1.0])),
            });
        }
        (rails, Some([a, b]))
    };
    // Rails found on a flat spline's plane take their pcurves on the
    // spline itself.
    let mut rails = rails;
    for (side, r) in rails.iter_mut().enumerate() {
        let own = &body.surfaces[body.faces[faces[side]].surface];
        if own != &surfaces[side] && super::spine::twin(own).is_some() {
            *r = super::rail::onto(r.clone(), own, span).map_err(|_| uncertain())?;
        }
    }
    let (start, end) = if closed {
        ([Some(span[0]); 2], [Some(span[1]); 2])
    } else {
        ([None; 2], [None; 2])
    };
    Ok(Partial {
        edges,
        carrier,
        spine: None,
        faces,
        sides: [0, 1],
        rails: [rails[0].clone(), rails[1].clone()],
        ruled,
        span,
        start,
        end,
        closed,
        reach,
    })
}

/// The curve a chamfer's sections follow, running along the chain, and
/// the stretch of its parameter to search: the edge's own line, circle
/// or ellipse (a period round a circle or an ellipse, centred on the
/// edge, or the edge's own period round a closed one), or for a fitted
/// edge the branch of its faces' intersection through its middle.
fn carrier(
    body: &Body,
    spec: &Spec,
    surfaces: &[Surface; 2],
    forward: bool,
    closed: bool,
) -> Result<(Curve, [f64; 2]), Error> {
    let name = spec.name;
    let e = &body.edges[spec.edge];
    let curve = &body.curves[e.curve];
    let [t0, t1] = e.interval;
    let (curve, [a, b]) = match curve {
        Curve::Nurbs(_) => fitted_carrier(body, spec, surfaces)?,
        _ => (curve.clone(), [t0, t1]),
    };
    let (curve, [a, b]) = if forward {
        (curve, [a, b])
    } else {
        let (c, k) = curve.reversed();
        (c, [k - b, k - a])
    };
    let span = match curve.period() {
        Some(p) if !closed => {
            let m = (a + b) / 2.0;
            [m - p / 2.0, m + p / 2.0]
        }
        _ => [a, b],
    };
    let _ = name;
    Ok((curve, span))
}

/// The branch of the faces' intersection through a fitted edge's
/// middle, and its whole range.
fn fitted_carrier(
    body: &Body,
    spec: &Spec,
    surfaces: &[Surface; 2],
) -> Result<(Curve, [f64; 2]), Error> {
    let at = spec.name;
    let region =
        intersect::face_box(body, spec.faces[0]).union(&intersect::face_box(body, spec.faces[1]));
    let analytic = surfaces.iter().all(|s| s.frame().is_some());
    // Past the faces' ends, where their surfaces are carried on, so the
    // chamfer runs on past its end faces (the analytic surfaces' branches
    // are whole curves already).
    let region = if analytic {
        region
    } else {
        region.grow(0.25 * region.min().distance(region.max()))
    };
    let hit = super::meet(&surfaces[0], &surfaces[1], &region);
    let branches = match hit {
        Ok(SurfaceHit::Meet { branches, .. }) => branches,
        Ok(SurfaceHit::Coincident { .. }) | Err(Failure::Unsupported) => {
            return Err(BlendError::Unsupported { at }.into());
        }
        Err(Failure::Uncertain) => return Err(BlendError::Uncertain { at }.into()),
    };
    let e = &body.edges[spec.edge];
    let m = body.curves[e.curve].point((e.interval[0] + e.interval[1]) / 2.0);
    let mut best: Option<(f64, usize)> = None;
    for (k, b) in branches.iter().enumerate() {
        let (_, d) = nearest(&b.curve, b.range, m);
        if d <= 10.0 * (b.tolerance + e.tolerance.get()) && best.is_none_or(|x| d < x.0) {
            best = Some((d, k));
        }
    }
    let (_, k) = best.ok_or(BlendError::Uncertain { at })?;
    let b = &branches[k];
    // Off analytic faces an open edge's branch is cut to its own stretch and
    // a seventh of it each way: far enough to reach past the end faces,
    // and inside the quarter of their parameters the faces' surfaces are
    // carried by ([`super::spine::room`]), where setbacks still land.
    let range = if analytic || e.start == e.end {
        b.range
    } else {
        let ends = [e.start, e.end].map(|v| nearest(&b.curve, b.range, body.vertices[v].point).0);
        let (lo, hi) = (ends[0].min(ends[1]), ends[0].max(ends[1]));
        let m = 0.15 * (hi - lo);
        [(lo - m).max(b.range[0]), (hi + m).min(b.range[1])]
    };
    // The branch's own direction may run against the edge's.
    let (t, _) = nearest(&b.curve, b.range, m);
    let d = body.curves[e.curve]
        .eval((e.interval[0] + e.interval[1]) / 2.0)
        .d1;
    if b.curve.eval(t).d1.dot(d) < 0.0 {
        let (c, k) = b.curve.reversed();
        return Ok((c, [k - range[1], k - range[0]]));
    }
    Ok((b.curve.clone(), range))
}

/// The setbacks on the rail face and the other face.
fn setbacks(
    body: &Body,
    spec: &Spec,
    faces: [FaceId; 2],
    chamfer: &Chamfer,
    carrier: &Curve,
) -> Result<[f64; 2], Error> {
    let name = |f: FaceId| body.faces[f].name;
    Ok(match *chamfer {
        Chamfer::Equal { distance } => [distance; 2],
        Chamfer::Distances {
            first,
            second,
            first_face,
        } => {
            if name(faces[0]) == first_face {
                [first, second]
            } else {
                [second, first]
            }
        }
        Chamfer::DistanceAngle {
            distance,
            angle,
            face,
        } => {
            let alpha = wedge(body, spec, faces, carrier)?;
            if alpha + angle >= PI - crate::tol::ANGULAR_RESOLUTION {
                return Err(BlendError::BadSize { edge: spec.name }.into());
            }
            let other = distance * math::sin(angle) / math::sin(alpha + angle);
            if name(faces[0]) == face {
                [distance, other]
            } else {
                [other, distance]
            }
        }
    })
}

/// The angle of the wedge a chamfer cuts or fills, where it is the same
/// all along the edge: between two planes along a line, or between
/// faces coaxial with a circle. `Unsupported` elsewhere.
fn wedge(body: &Body, spec: &Spec, faces: [FaceId; 2], carrier: &Curve) -> Result<f64, Error> {
    let surfaces = faces.map(|f| &body.surfaces[body.faces[f].surface]);
    let constant = match carrier {
        Curve::Line(_) => surfaces.iter().all(|s| matches!(s, Surface::Plane(_))),
        Curve::Circle(c) => surfaces.iter().all(|s| coaxial(s, c.frame())),
        _ => false,
    };
    if !constant {
        return Err(BlendError::Unsupported { at: spec.name }.into());
    }
    let e = &body.edges[spec.edge];
    let tm = (e.interval[0] + e.interval[1]) / 2.0;
    let n = [0, 1].map(|k| {
        let face = &body.faces[faces[k]];
        let fin = spec.fins[usize::from(spec.faces[1] == faces[k])];
        let uv = body.fin_uv(fin, tm).expect("a pcurve");
        let n = surfaces[k].normal(uv.x, uv.y).expect("a normal").get();
        if face.sense == Sense::Forward { n } else { -n }
    });
    Ok(PI - math::atan2(n[0].cross(n[1]).length(), n[0].dot(n[1])))
}

/// Whether `s` is a surface of revolution about the axis of `frame` (a
/// plane square to it counts).
fn coaxial(s: &Surface, frame: &Frame) -> bool {
    let z = frame.z();
    match s {
        Surface::Plane(_) => coincide::parallel(s.frame().expect("a frame").z(), z),
        Surface::Cylinder(_) | Surface::Cone(_) | Surface::Torus(_) => {
            let f = s.frame().expect("a frame");
            coincide::parallel(f.z(), z)
                && coincide::off_axis(frame, f.origin()) <= LINEAR_RESOLUTION
        }
        Surface::Sphere(_) => {
            let f = s.frame().expect("a frame");
            coincide::off_axis(frame, f.origin()) <= LINEAR_RESOLUTION
        }
        _ => false,
    }
}

/// The point on `surface` (outward along its natural normal times
/// `sense`) at `distance` from the carrier's point at `t`, in the plane
/// normal to the carrier there, on the face's side: `along` is `1` where
/// the face's fin runs along the chain, so the face lies on its left.
/// Found by the secant method on the angle round the carrier from the
/// face's tangent direction.
fn setback(
    surface: &Surface,
    sense: f64,
    carrier: &Curve,
    t: f64,
    along: f64,
    distance: f64,
) -> Option<Point3> {
    let e = carrier.eval(t);
    let tangent = e.d1.normalize()?.get();
    let (u, v) = surface.project(e.point).param;
    let n = surface.normal(u, v)?.get() * sense;
    let into = n.cross(tangent * along).normalize()?.get();
    let side = tangent.cross(into);
    let at = |phi: f64| e.point + (into * math::cos(phi) + side * math::sin(phi)) * distance;
    let off = |phi: f64| -> Option<f64> {
        let q = at(phi);
        let p = surface.project(q);
        let (u, v) = p.param;
        let m = surface.normal(u, v)?.get() * sense;
        Some((q - p.point).dot(m))
    };
    let (mut x0, mut x1) = (0.0, 1e-3);
    let (mut f0, mut f1) = (off(x0)?, off(x1)?);
    for _ in 0..60 {
        if f0.abs() <= 1e-15 * distance.max(1.0) {
            return Some(at(x0));
        }
        if f1 == f0 {
            break;
        }
        let x2 = x1 - f1 * (x1 - x0) / (f1 - f0);
        (x0, f0) = (x1, f1);
        x1 = x2;
        f1 = off(x1)?;
        if f1.abs() <= 1e-15 * distance.max(1.0) {
            return Some(at(x1));
        }
    }
    (f1.abs() <= LINEAR_RESOLUTION).then(|| at(x1))
}

/// The chamfer surface between a segment's rails, and the sense of the
/// chamfer face on it: its normal out of the body, as the faces' normals
/// at the edge's middle point together.
pub(crate) fn face(
    body: &Body,
    specs: &[Spec],
    chain: &Chain,
    part: &Partial,
    name: Name,
) -> Result<(Surface, Sense), Error> {
    let unsupported = || -> Error { BlendError::Unsupported { at: name }.into() };
    let surface = match &part.ruled {
        Some([a, b]) => Surface::Nurbs(Arc::new(ruled(a, b, part.closed).ok_or_else(unsupported)?)),
        None => match (&part.rails[0].curve, &part.rails[1].curve) {
            (Curve::Line(a), Curve::Line(b)) => {
                let across = b.origin() - a.origin();
                let z = a.dir().get().cross(across);
                Surface::Plane(Plane::new(
                    Frame::new(a.origin(), z, a.dir().get()).map_err(|_| unsupported())?,
                ))
            }
            (Curve::Circle(a), Curve::Circle(b)) => {
                let (fa, fb) = (a.frame(), b.frame());
                let z = fa.z().get();
                let h = (fb.origin() - fa.origin()).dot(z);
                let (ra, rb) = (a.radius(), b.radius());
                if h.abs() <= LINEAR_RESOLUTION {
                    Surface::Plane(Plane::new(*fa))
                } else if (ra - rb).abs() <= LINEAR_RESOLUTION {
                    Surface::Cylinder(Cylinder::new(*fa, ra).map_err(|_| unsupported())?)
                } else {
                    // Widening towards +z from the smaller circle.
                    let (small, r0, r1) = if ra < rb { (fa, ra, rb) } else { (fb, rb, ra) };
                    let up = if (ra < rb) == (h > 0.0) { z } else { -z };
                    let frame = Frame::new(small.origin(), up, small.x().get())
                        .map_err(|_| unsupported())?;
                    let half = math::atan2(r1 - r0, h.abs());
                    Surface::Cone(Cone::new(frame, r0, half).map_err(|_| unsupported())?)
                }
            }
            _ => return Err(unsupported()),
        },
    };
    // Out of the body: the faces' outward normals at the edge's middle.
    let spec = &specs[chain.edges[part.edges[0]].0];
    let e = &body.edges[spec.edge];
    let tm = (e.interval[0] + e.interval[1]) / 2.0;
    let mut out = Vector3::ZERO;
    for k in 0..2 {
        let face = &body.faces[spec.faces[k]];
        let uv = body.fin_uv(spec.fins[k], tm).ok_or_else(unsupported)?;
        let n = body.surfaces[face.surface]
            .normal(uv.x, uv.y)
            .ok_or_else(unsupported)?
            .get();
        out += if face.sense == Sense::Forward { n } else { -n };
    }
    let (tc, _) = nearest(&part.carrier, part.span, body.curves[e.curve].point(tm));
    let (p0, p1) = (
        part.rails[0].curve.point(part.rails[0].param(tc)),
        part.rails[1].curve.point(part.rails[1].param(tc)),
    );
    let m = p0.lerp(p1, 0.5);
    let (u, v) = surface.project(m).param;
    let n = surface.normal(u, v).ok_or_else(unsupported)?.get();
    let sense = if n.dot(out) > 0.0 {
        Sense::Forward
    } else {
        Sense::Reversed
    };
    Ok((surface, sense))
}

/// The ruled NURBS from rail `a` (at `v = 0`) to rail `b` (`v = 1`), fitted
/// on shared knots; periodic along `u` when it goes all round.
fn ruled(a: &NurbsCurve, b: &NurbsCurve, closed: bool) -> Option<NurbsSurface> {
    if !closed {
        return NurbsSurface::ruled(a, b).ok();
    }
    let (pa, pb) = (a.bezier_periodic()?, b.bezier_periodic()?);
    let ku = pa.knots().clone();
    if &ku != pb.knots() {
        return None;
    }
    let kv = KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).ok()?;
    let mut points = Vec::new();
    let mut weights = Vec::new();
    for ((x, y), (wx, wy)) in pa
        .control_points()
        .zip(pb.control_points())
        .zip(pa.weights().iter().zip(pb.weights()))
    {
        points.extend([x, y]);
        weights.extend([*wx, *wy]);
    }
    NurbsSurface::new([ku, kv], points, Some(weights)).ok()
}

/// The joint of chamfers at chain vertex `vi`, a smooth join on `smooth`,
/// between the segment `a` ending there and `b` starting there: the
/// sections through the vertex, where the rails on the rail face meet and
/// the other faces' rails reach the smooth edge.
pub(crate) fn joint(
    body: &Body,
    chain: &Chain,
    a: &Partial,
    b: &Partial,
    vi: usize,
    smooth: EdgeId,
) -> Result<Joint2, Error> {
    let vertex = chain.vertices[vi];
    let at = body.vertices[vertex].name;
    let uncertain = || -> Error { BlendError::Uncertain { at }.into() };
    let v = body.vertices[vertex].point;
    let (ta, _) = nearest(&a.carrier, a.span, v);
    let (tb, _) = nearest(&b.carrier, b.span, v);
    let rail = |p: &Partial, side: usize, t: f64| p.rails[side].curve.point(p.rails[side].param(t));
    let corner = rail(a, 0, ta);
    let q = rail(a, 1, ta);
    let tol = 10.0 * (a.rails[1].tolerance + b.rails[1].tolerance);
    if rail(b, 1, tb).distance(q) > tol.max(1e-7) || rail(b, 0, tb).distance(corner) > tol.max(1e-7)
    {
        return Err(uncertain());
    }
    let le = &body.edges[smooth];
    let lc = &body.curves[le.curve];
    let reach = le.tolerance.get() + tol;
    let Some(param) = on_edge(lc, le.interval, q, reach.max(1e-7)) else {
        return Err(BlendError::Overflow {
            edge: body.edges[smooth].name,
            face: body.faces[a.faces[1]].name,
        }
        .into());
    };
    let point = lc.point(param);
    let line = Line::through(corner, point).map_err(|_| uncertain())?;
    Ok(Joint2 {
        ta,
        tb,
        at: SmoothAt {
            vertex: vi,
            smooth,
            point,
            at: param,
            section: Section {
                curve: Curve::Line(line),
                interval: [0.0, corner.distance(point)],
            },
        },
    })
}
