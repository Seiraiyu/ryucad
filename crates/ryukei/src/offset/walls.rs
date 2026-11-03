//! The walls of a thickened sheet: along each laminar edge, the surface
//! its points sweep moving along the face's normal between the sheet's
//! two offsets.
//!
//! A line along which the face's normal keeps its direction sweeps a
//! plane; a circle whose normal keeps its angle to the circle's axis
//! sweeps a cylinder (normal along the axis), a plane (normal across it)
//! or a cone. Any other edge's wall is the ruled surface between its two
//! rails, made once the rails are known ([`ruled`]).

use super::classify::normal;
use crate::geom::{
    Cone, Curve, Cylinder, Frame, KnotVector, NurbsCurve, NurbsSurface, Plane, Surface, Vector3,
};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId};
use std::sync::Arc;

/// Samples along an edge at which a wall's normal is judged.
const SAMPLES: usize = 17;

/// How far apart (in radians, as a cross product's length) normals may be
/// and still count as one direction.
const SAME: f64 = 1e-9;

/// A laminar edge's wall, before the rails are known.
#[derive(Clone, Debug)]
pub(crate) enum Wall {
    /// A plane, cylinder or cone holding every normal line along the edge.
    Exact(Surface),
    /// The ruled surface between the rails.
    Ruled,
}

impl Wall {
    /// The wall's plane, if it is one.
    pub(crate) fn plane(&self) -> Option<&Plane> {
        match self {
            Self::Exact(Surface::Plane(p)) => Some(p),
            _ => None,
        }
    }
}

/// Whether two walls lie on one plane: both planes, their normals
/// parallel and each origin within resolution of the other plane.
pub(crate) fn same_plane(a: &Wall, b: &Wall) -> bool {
    let (Some(p), Some(q)) = (a.plane(), b.plane()) else {
        return false;
    };
    let (fp, fq) = (p.frame(), q.frame());
    let (zp, zq) = (fp.z().get(), fq.z().get());
    zp.cross(zq).length() <= SAME
        && (fq.origin() - fp.origin()).dot(zp).abs() <= LINEAR_RESOLUTION
        && (fp.origin() - fq.origin()).dot(zq).abs() <= LINEAR_RESOLUTION
}

/// The wall along laminar edge `e`.
pub(crate) fn wall(body: &Body, e: EdgeId) -> Wall {
    let edge = &body.edges[e];
    let fin = edge.fin;
    let [t0, t1] = edge.interval;
    let ts: Vec<f64> = (0..SAMPLES)
        .map(|i| t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64)
        .collect();
    let normals: Vec<Vector3> = ts.iter().map(|&t| normal(body, fin, t)).collect();
    if normals.iter().any(|n| n.length() < 0.5) {
        return Wall::Ruled;
    }
    match &body.curves[edge.curve] {
        Curve::Line(l) => {
            let n = normals[0];
            if normals.iter().any(|m| m.cross(n).length() > SAME) {
                return Wall::Ruled;
            }
            let z = l.dir().get().cross(n);
            Frame::new(l.origin(), z, l.dir().get())
                .ok()
                .map_or(Wall::Ruled, |f| Wall::Exact(Surface::Plane(Plane::new(f))))
        }
        Curve::Circle(c) => {
            let f = c.frame();
            let curve = &body.curves[edge.curve];
            // Each normal's components along the axis and outwards.
            let parts: Vec<(f64, f64)> = ts
                .iter()
                .zip(&normals)
                .map(|(&t, n)| {
                    let radial = (curve.point(t) - f.origin()) / c.radius();
                    (n.dot(f.z().get()), n.dot(radial))
                })
                .collect();
            let (a, b) = parts[0];
            if parts
                .iter()
                .any(|&(x, y)| (x - a).abs() > SAME || (y - b).abs() > SAME)
            {
                return Wall::Ruled;
            }
            if b.abs() <= SAME {
                // Along the axis: the cylinder through the circle.
                Cylinder::new(*f, c.radius())
                    .map_or(Wall::Ruled, |s| Wall::Exact(Surface::Cylinder(s)))
            } else if a.abs() <= SAME {
                // Across it: the circle's own plane.
                Wall::Exact(Surface::Plane(Plane::new(*f)))
            } else {
                // A cone through the circle, widening the way the normal
                // leans outwards.
                let z = if a * b > 0.0 {
                    f.z().get()
                } else {
                    -f.z().get()
                };
                let half = crate::math::atan2(b.abs(), a.abs());
                Frame::new(f.origin(), z, f.x().get())
                    .ok()
                    .and_then(|fr| Cone::new(fr, c.radius(), half).ok())
                    .map_or(Wall::Ruled, |s| Wall::Exact(Surface::Cone(s)))
            }
        }
        _ => Wall::Ruled,
    }
}

/// The curve over `range` as a NURBS over `[0, 1]`, exactly: an analytic
/// curve's NURBS form cut to the range, its knots mapped affinely.
fn unit(curve: &Curve, range: [f64; 2]) -> Option<NurbsCurve> {
    let (n, rp) = curve.to_nurbs(range).ok()?;
    let r = [rp.to_nurbs(range[0]), rp.to_nurbs(range[1])];
    let n = if r == n.domain() && n.period().is_none() {
        n
    } else {
        n.part(r).ok()?
    };
    let [a, b] = n.domain();
    let kv = n.knots();
    let knots: Vec<f64> = kv.knots().iter().map(|&k| (k - a) / (b - a)).collect();
    let knots = KnotVector::clamped(kv.degree(), &knots, kv.mults()).ok()?;
    let weights = n.weights().to_vec();
    let rational = weights.iter().any(|&w| w != 1.0);
    NurbsCurve::new(
        knots,
        n.control_points().collect(),
        rational.then_some(weights),
    )
    .ok()
}

/// The ruled surface from the front rail (at `v = 0`) to the back rail
/// (at `v = 1`), each over its range, both run over `[0, 1]`.
pub(crate) fn ruled(front: (&Curve, [f64; 2]), back: (&Curve, [f64; 2])) -> Option<Surface> {
    let a = unit(front.0, front.1)?;
    let b = unit(back.0, back.1)?;
    NurbsSurface::ruled(&a, &b)
        .ok()
        .map(|s| Surface::Nurbs(Arc::new(s)))
}
