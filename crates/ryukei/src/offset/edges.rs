//! What each edge becomes: the curve where its faces' offsets meet, or,
//! where they move apart, two rails and the fill rolled between them.

use super::classify::{Class, Plan};
use crate::geom::{
    Circle, Curve, Curve2, Cylinder, Frame, Line, Pipe, Point3, Sphere, Surface, Torus,
    Vector3,
};
use crate::intersect::{Failure, SurfaceHit, fit_along, surface_surface};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{EdgeId, FinId, Sense};
use crate::{Error, OffsetCase, OffsetError};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Samples at which an exact moved curve must hold.
const SAMPLES: usize = 17;

/// A new curve: where faces' offsets meet, or a rail.
#[derive(Clone, Debug)]
pub(crate) struct NewCurve {
    pub(crate) curve: Curve,
    /// The stretch of its parameter it is found over: the edge's interval
    /// for a curve moved with it, a branch's range for a closing edge's.
    pub(crate) range: [f64; 2],
    /// Its period where it is closed and may be cut anywhere.
    pub(crate) period: Option<f64>,
    pub(crate) tolerance: f64,
    /// Its pcurves on the offsets of the edge's faces, in the order of its
    /// fins (along the edge first), by its own parameter; a rail has one
    /// on its own face only.
    pub(crate) pcurves: [Option<(Curve2, [f64; 2])>; 2],
    /// It runs against the edge.
    pub(crate) reversed: bool,
}

/// What an edge becomes.
#[derive(Clone, Debug)]
pub(crate) enum EdgeGeo {
    /// One curve, where the offsets meet (or the edge itself, kept).
    One(NewCurve),
    /// Where the offsets move apart: the rail on each face's offset (in
    /// the order of the edge's fins) and the fill between them, of radius
    /// `|d|`, with its sense.
    Open {
        rails: [NewCurve; 2],
        fill: Surface,
        sense: Sense,
    },
}

/// Every edge's new geometry, in parallel.
pub(crate) fn curves(plan: &Plan) -> Result<BTreeMap<EdgeId, EdgeGeo>, Error> {
    plan.edges
        .par_iter()
        .map(|&e| edge(plan, e).map(|g| (e, g)))
        .collect()
}

fn edge(plan: &Plan, e: EdgeId) -> Result<EdgeGeo, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let at = edge.name;
    let fins = plan.fins(e);
    let (class, gap) = plan.class[&e];
    match class {
        Class::Kept => {
            let pcurve = |f: FinId| {
                body.fins[f]
                    .pcurve
                    .map(|p| (body.curves2[p.curve].clone(), p.map))
            };
            Ok(EdgeGeo::One(NewCurve {
                curve: body.curves[edge.curve].clone(),
                range: edge.interval,
                period: None,
                tolerance: edge.tolerance.get(),
                pcurves: [pcurve(fins[0]), pcurve(fins[1])],
                reversed: false,
            }))
        }
        Class::Smooth => {
            let mut c = moved(plan, e, fins[0])?;
            let other = moved(plan, e, fins[1])?;
            c.tolerance = c.tolerance.max(other.tolerance) + gap;
            c.pcurves[1] = other.pcurves[0].clone();
            Ok(EdgeGeo::One(c))
        }
        Class::Opening => {
            let rails = [moved(plan, e, fins[0])?, moved(plan, e, fins[1])?];
            let d = plan.d(plan.face_of(fins[0]));
            let fill = fill(plan, e, fins, d).ok_or(OffsetError::Unsupported {
                at,
                case: OffsetCase::Fill,
            })?;
            let sense = if d > 0.0 {
                Sense::Forward
            } else {
                Sense::Reversed
            };
            Ok(EdgeGeo::Open { rails, fill, sense })
        }
        Class::Closing => closing(plan, e, fins).map(EdgeGeo::One),
    }
}

/// The edge moved along its fin's face's outward normal by the face's
/// distance, on the edge's own parameter: a line or a circle whose
/// normal keeps its place against it moves exactly (to a line, or a
/// circle about the same axis); any other curve is fitted along its
/// pcurve on the face's offset ([`fit_along`]), certified. Its pcurve on
/// the offset is the fin's own, which the offset keeps.
fn moved(plan: &Plan, e: EdgeId, fin: FinId) -> Result<NewCurve, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let at = edge.name;
    let face = plan.face_of(fin);
    let d = plan.d(face);
    let offset = &plan.surface[&face];
    let pc = body.fins[fin]
        .pcurve
        .ok_or(OffsetError::Uncertain { at })?;
    let pcurve = (body.curves2[pc.curve].clone(), pc.map);
    let curve = &body.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let ts: Vec<f64> = (0..SAMPLES)
        .map(|i| t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64)
        .collect();
    let target = |t: f64| curve.point(t) + plan.normal(fin, t) * d;
    let exact = match curve {
        Curve::Line(l) => {
            let n = plan.normal(fin, (t0 + t1) / 2.0);
            Line::from_unit(l.origin() + n * d, l.dir())
                .ok()
                .map(Curve::Line)
        }
        Curve::Circle(c) => {
            let f = c.frame();
            let t = (t0 + t1) / 2.0;
            let n = plan.normal(fin, t);
            let radial = (curve.point(t) - f.origin()) / c.radius();
            let (a, b) = (n.dot(f.z().get()), n.dot(radial));
            let frame = Frame::from_axes(f.origin() + f.z().get() * (a * d), f.z(), f.x()).ok();
            frame
                .and_then(|fr| Circle::new(fr, c.radius() + b * d).ok())
                .map(Curve::Circle)
        }
        _ => None,
    };
    if let Some(c) = exact {
        let worst = ts
            .iter()
            .map(|&t| c.point(t).distance(target(t)))
            .fold(0.0, f64::max);
        if worst <= LINEAR_RESOLUTION / 4.0 {
            return Ok(NewCurve {
                curve: c,
                range: edge.interval,
                period: None,
                tolerance: LINEAR_RESOLUTION.max(2.0 * worst),
                pcurves: [Some(pcurve), None],
                reversed: false,
            });
        }
    }
    let lifted = fit_along(offset, &pcurve.0, pcurve.1, edge.interval)
        .map_err(|_| OffsetError::Uncertain { at })?;
    Ok(NewCurve {
        curve: Curve::Nurbs(Arc::new(lifted.curve)),
        range: edge.interval,
        period: None,
        tolerance: lifted.tolerance,
        pcurves: [Some(pcurve), None],
        reversed: false,
    })
}

/// Where a ball of radius `|d|` rolled about the edge goes: a cylinder
/// about a line (its seam opposite the middle of the faces' normals), a
/// torus about a circle (`None` unless the circle is larger than `|d|`),
/// otherwise a pipe about the edge's curve over its interval, anchored on
/// the first face's offset (through the fin's pcurve where that isn't
/// analytic).
fn fill(plan: &Plan, e: EdgeId, fins: [FinId; 2], d: f64) -> Option<Surface> {
    let body = plan.body;
    let edge = &body.edges[e];
    let curve = &body.curves[edge.curve];
    let r = d.abs();
    let [t0, t1] = edge.interval;
    match curve {
        Curve::Line(l) => {
            let t = (t0 + t1) / 2.0;
            let middle = plan.normal(fins[0], t) + plan.normal(fins[1], t);
            let x = if middle.length() > 1e-9 {
                -middle
            } else {
                plan.normal(fins[0], t)
            };
            let frame = Frame::new(l.origin(), l.dir().get(), x).ok()?;
            Some(Surface::Cylinder(Cylinder::new(frame, r).ok()?))
        }
        Curve::Circle(c) => {
            if r >= c.radius() {
                return None;
            }
            Some(Surface::Torus(Torus::new(*c.frame(), c.radius(), r).ok()?))
        }
        _ => {
            let spine = match curve {
                Curve::Nurbs(n) => {
                    let [d0, d1] = n.domain();
                    if t0 <= d0 && t1 >= d1 && curve.is_closed() {
                        n.bezier_periodic()
                            .or_else(|| (n.period().is_some()).then(|| (**n).clone()))?
                    } else {
                        n.part([t0.max(d0), t1.min(d1)]).ok()?
                    }
                }
                _ => return None,
            };
            let spine = Curve::Nurbs(Arc::new(spine));
            let face = plan.face_of(fins[0]);
            let anchor = plan.surface[&face].clone();
            let pipe = if anchor.frame().is_some() {
                Pipe::new(spine, r, anchor)
            } else {
                let pc = body.fins[fins[0]].pcurve?;
                Pipe::on_rail(spine, r, anchor, body.curves2[pc.curve].clone(), pc.map)
            };
            Some(Surface::Pipe(Arc::new(pipe.ok()?)))
        }
    }
}

/// Where a closing edge's faces' offsets meet: the branch of their
/// intersection near the edge, nearest to where the edge's middle goes
/// (the point on both faces' tangent planes there, moved), within the
/// larger distance of it. `EdgeVanishes` where no branch comes that near.
fn closing(plan: &Plan, e: EdgeId, fins: [FinId; 2]) -> Result<NewCurve, Error> {
    let body = plan.body;
    let edge = &body.edges[e];
    let at = edge.name;
    let faces = fins.map(|f| plan.face_of(f));
    let ds = faces.map(|f| plan.d(f));
    let surfaces = faces.map(|f| &plan.surface[&f]);
    let curve = &body.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let tm = (t0 + t1) / 2.0;
    let reach = ds[0].abs().max(ds[1].abs());
    let estimate = moved_point(
        curve.point(tm),
        [plan.normal(fins[0], tm), plan.normal(fins[1], tm)],
        ds,
    );
    let grow = 2.0 * reach + 1e-3 * plan.size;
    let region = curve.bounds(edge.interval).grow(grow);
    let hit = surface_surface(surfaces[0], surfaces[1], &region).map_err(|f| match f {
        Failure::Uncertain => Error::from(OffsetError::Uncertain { at }),
        Failure::Unsupported => OffsetError::Unsupported {
            at,
            case: OffsetCase::PartlySmooth,
        }
        .into(),
    })?;
    let SurfaceHit::Meet { branches, .. } = hit else {
        return Err(OffsetError::Uncertain { at }.into());
    };
    let best = branches
        .into_iter()
        .map(|b| {
            let q = project_on(&b.curve, b.range, estimate);
            (q.1.distance(estimate), q.0, b)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((dist, t, b)) = best else {
        return Err(OffsetError::EdgeVanishes { edge: at }.into());
    };
    if dist > reach.max(10.0 * LINEAR_RESOLUTION) {
        return Err(OffsetError::EdgeVanishes { edge: at }.into());
    }
    if b.tangent {
        return Err(OffsetError::Uncertain { at }.into());
    }
    let along = curve.eval(tm).d1;
    let reversed = b.curve.eval(t).d1.dot(along) < 0.0;
    let period = (b.curve.period() == Some(b.range[1] - b.range[0])).then(|| b.range[1] - b.range[0]);
    let [pa, pb] = b.pcurves;
    Ok(NewCurve {
        curve: b.curve,
        range: b.range,
        period,
        tolerance: b.tolerance,
        pcurves: [Some(pa), Some(pb)],
        reversed,
    })
}

/// The point the faces' tangent planes at `p`, with outward normals `n`,
/// meet at once each is moved by its distance: on both moved planes,
/// along the normals' span. Where the normals are nearly parallel, the
/// mean of the moved points.
pub(crate) fn moved_point(p: Point3, n: [Vector3; 2], d: [f64; 2]) -> Point3 {
    let c = n[0].dot(n[1]);
    let det = 1.0 - c * c;
    if det < 1e-12 {
        return p + (n[0] * d[0] + n[1] * d[1]) * 0.5;
    }
    let alpha = (d[0] - c * d[1]) / det;
    let beta = (d[1] - c * d[0]) / det;
    p + n[0] * alpha + n[1] * beta
}

/// The parameter in `range` of the point of `curve` nearest `p`, and that
/// point: the curve's projection, else the nearer end of the range.
pub(crate) fn project_on(curve: &Curve, range: [f64; 2], p: Point3) -> (f64, Point3) {
    let q = curve.project(p);
    let inside = |t: f64| range[0] <= t && t <= range[1];
    let t = match curve.period() {
        Some(per) if !inside(q.param) => {
            let shifted = range[0] + (q.param - range[0]).rem_euclid(per);
            if inside(shifted) { shifted } else { q.param }
        }
        _ => q.param,
    };
    if inside(t) {
        return (t, curve.point(t));
    }
    let ends = range.map(|t| (t, curve.point(t)));
    if ends[0].1.distance(p) <= ends[1].1.distance(p) {
        ends[0]
    } else {
        ends[1]
    }
}

/// The sphere about `centre` of `radius` through the points `feet`, with
/// its seam opposite them and its poles square to the direction they lie
/// in (as blends make ball corners).
pub(crate) fn sphere(centre: Point3, radius: f64, feet: &[Point3]) -> Option<Surface> {
    let m = feet
        .iter()
        .fold(Vector3::ZERO, |acc, &f| acc + (f - centre))
        .normalize()?
        .get();
    let z = (feet[0] - centre).cross(m);
    let z = if z.length() > 1e-6 * radius {
        z
    } else {
        let a = m.cross(Vector3::new(1.0, 0.0, 0.0));
        if a.length() > 1e-3 {
            a
        } else {
            m.cross(Vector3::new(0.0, 1.0, 0.0))
        }
    };
    let frame = Frame::new(centre, z, -m).ok()?;
    Some(Surface::Sphere(Sphere::new(frame, radius).ok()?))
}

