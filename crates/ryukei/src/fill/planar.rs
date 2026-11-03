//! A boundary lying in one plane fills as one plane face bounded by its
//! sides, as [`crate::sweep::planar`] would make it. Splines on one plane
//! meeting at edges can't be told apart by the self-intersection check,
//! and a plane is what the boundary bounds.

use super::build::Namer;
use super::sides::Side;
use crate::error::Error;
use crate::geom::bound2::IVec3;
use crate::geom::{Curve, Frame, Plane, Point3, Surface, Vector3};
use crate::interval::Interval;
use crate::layout::{self, Boundary, Layout, Use};
use crate::name::{Event, FillRole, Journal, Name, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EntityId, Sense};
use crate::work::{Halt, Meter};
use crate::{TopologyError, check, math};
use std::sync::Arc;

/// How many stretches a tangent side's face normal is enclosed over.
const STRETCHES: usize = 64;

/// The plane the boundary lies in, its normal the side the loop runs
/// counterclockwise about: every side's control points (which enclose
/// it) within half of resolution of the plane through their mean along
/// Newell's normal of the corners and the sides' middles. `None` for a
/// boundary in no plane.
pub(crate) fn plane_of(sides: &[Side<'_>]) -> Option<Frame> {
    let mut ring = Vec::with_capacity(2 * sides.len());
    let mut all = Vec::new();
    for s in sides {
        let [a, b] = s.curve.domain();
        ring.push(s.curve.point(a));
        ring.push(s.curve.point(0.5 * (a + b)));
        all.extend(s.curve.control_points());
    }
    let mean = Point3::ORIGIN
        + all.iter().fold(Vector3::ZERO, |acc, p| acc + p.to_vector()) / all.len() as f64;
    let mut newell = Vector3::ZERO;
    for k in 0..ring.len() {
        newell += (ring[k] - mean).cross(ring[(k + 1) % ring.len()] - mean);
    }
    let z = newell.normalize()?;
    if all
        .iter()
        .any(|p| (*p - mean).dot(z.get()).abs() > 0.5 * LINEAR_RESOLUTION)
    {
        return None;
    }
    Frame::new(mean, z.get(), ring[1] - ring[0]).ok()
}

/// The angle between `plane` and each tangent side's face, certified over
/// [`STRETCHES`] stretches of the side; `None` where some side's can't be
/// had within `limit`.
pub(crate) fn angles(
    sides: &[Side<'_>],
    plane: &Frame,
    limit: f64,
    meter: &Meter,
) -> Result<Option<Vec<Option<f64>>>, Halt> {
    let z = plane.z().get();
    let mut out = Vec::with_capacity(sides.len());
    for s in sides {
        let Some(trace) = &s.trace else {
            out.push(None);
            continue;
        };
        let [a, b] = s.curve.domain();
        let mut worst: f64 = 0.0;
        for k in 0..STRETCHES {
            meter.charge(1)?;
            let r = [
                a + (b - a) * k as f64 / STRETCHES as f64,
                a + (b - a) * (k + 1) as f64 / STRETCHES as f64,
            ];
            let bx = s.curve.bounds(r);
            let (lo, hi) = (bx.min(), bx.max());
            let p: Option<IVec3> = (|| {
                Some([
                    Interval::new(lo.x, hi.x)?,
                    Interval::new(lo.y, hi.y)?,
                    Interval::new(lo.z, hi.z)?,
                ])
            })();
            let (e0, e1) = (trace.edge_param(r[0]), trace.edge_param(r[1]));
            let (t0, t1) = (e0.min(e1), e0.max(e1));
            let pad = 1e-9 * (t1 - t0) + 4.0 * f64::EPSILON * t0.abs().max(t1.abs());
            let Some(n) = p.and_then(|p| trace.face.normal_box([t0 - pad, t1 + pad], p)) else {
                return Ok(None);
            };
            // |n × z| with z exact.
            let zi = z.to_array().map(Interval::point);
            let x = [
                n[1] * zi[2] - n[2] * zi[1],
                n[2] * zi[0] - n[0] * zi[2],
                n[0] * zi[1] - n[1] * zi[0],
            ];
            let d = n[0] * zi[0] + n[1] * zi[1] + n[2] * zi[2];
            if d.lo() <= 0.0 && d.hi() >= 0.0 {
                return Ok(None);
            }
            let Some(sin) = (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt() else {
                return Ok(None);
            };
            let angle = math::asin(sin.hi().min(1.0));
            if angle > limit {
                return Ok(None);
            }
            worst = worst.max(angle);
        }
        out.push(Some(worst));
    }
    Ok(Some(out))
}

/// One plane face bounded by the sides, named from the stand-ins.
pub(crate) fn sheet(
    namer: &mut Namer,
    sides: &[Side<'_>],
    stand_ins: &[Name],
    plane: Frame,
) -> Result<(Body, Journal), Error> {
    let n = sides.len();
    let mut out = Layout::default();
    let mut vertex_names = Vec::with_capacity(n);
    for (k, s) in sides.iter().enumerate() {
        out.vertices.push(layout::Vertex {
            point: s.curve.point(s.curve.domain()[0]),
            tolerance: LINEAR_RESOLUTION,
        });
        vertex_names.push(namer.name(
            FillRole::Corner,
            &[stand_ins[(k + n - 1) % n], stand_ins[k]],
        ));
    }
    let mut arcs = Vec::with_capacity(n);
    for (k, s) in sides.iter().enumerate() {
        out.curves.push(Curve::Nurbs(Arc::new(s.curve.clone())));
        out.edges.push(layout::Edge {
            start: k,
            end: (k + 1) % n,
            curve: k,
            interval: s.curve.domain(),
            tolerance: LINEAR_RESOLUTION,
        });
        arcs.push(Use {
            edge: k,
            sense: Sense::Forward,
            pcurve: None,
        });
    }
    out.surfaces.push(Surface::Plane(Plane::new(plane)));
    out.faces.push(layout::Face {
        surface: 0,
        sense: Sense::Forward,
        loops: vec![Boundary::Arcs(arcs)],
    });
    let face = namer.name(FillRole::Patch, &[]);
    let key = namer.key();
    let built = layout::build_sheet(&out, key, Role::Merged)
        .map_err(|_| Error::Topology(TopologyError::Inconsistent))?;
    let mut b = built.body;
    b.set_recipes(true);
    namer.record(&mut b);
    b.rename(EntityId::Face(built.faces[0]), face);
    for (k, &name) in stand_ins.iter().enumerate() {
        b.rename(EntityId::Edge(built.edges[&k]), name);
    }
    for (&g, &v) in &built.vertices {
        b.rename(EntityId::Vertex(v), vertex_names[g]);
    }
    crate::pcurve::fill(&mut b)?;
    let mut journal = Journal::new();
    for f in b.faces() {
        journal.push(Event::Created(b.faces[f].name));
    }
    for e in b.edges() {
        journal.push(Event::Created(b.edges[e].name));
    }
    for v in b.vertices() {
        journal.push(Event::Created(b.vertices[v].name));
    }
    if cfg!(debug_assertions) {
        let faults = check::check(&b);
        assert!(faults.is_empty(), "a planar fill is invalid: {faults:?}");
    }
    Ok((b, journal))
}
