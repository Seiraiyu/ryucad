//! What recognition reads of faces and edges: planes and cylinders with
//! their outward normals, the faces either side of an edge, and the
//! tolerances a comparison adds.

use crate::geom::{Curve, Frame, Point3, Surface, UnitVector3, polar_angle};
use crate::topo::{Body, EdgeId, FaceId, FinId, Sense};
use core::f64::consts::{PI, TAU};

/// A planar face's plane: a point on it and its outward normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Flat {
    pub(super) origin: Point3,
    pub(super) normal: UnitVector3,
}

impl Flat {
    /// The signed distance of `p` along the outward normal.
    pub(super) fn height(&self, p: Point3) -> f64 {
        (p - self.origin).dot(self.normal.get())
    }
}

/// A cylindrical face's cylinder: its frame, radius and the face's sense
/// (`Reversed` faces its axis: a bend's inner side).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Round {
    pub(super) frame: Frame,
    pub(super) radius: f64,
    pub(super) sense: Sense,
}

pub(super) fn flat(b: &Body, f: FaceId) -> Option<Flat> {
    let face = &b.faces[f];
    let Surface::Plane(p) = &b.surfaces[face.surface] else {
        return None;
    };
    let z = p.frame().z();
    let normal = match face.sense {
        Sense::Forward => z,
        Sense::Reversed => -z,
    };
    Some(Flat {
        origin: p.frame().origin(),
        normal,
    })
}

pub(super) fn round(b: &Body, f: FaceId) -> Option<Round> {
    let face = &b.faces[f];
    let s = &b.surfaces[face.surface];
    let Surface::Cylinder(c) = s else {
        return None;
    };
    Some(Round {
        frame: *s.frame()?,
        radius: c.radius(),
        sense: face.sense,
    })
}

/// The face of a fin's loop.
pub(super) fn face_of(b: &Body, fin: FinId) -> FaceId {
    b.loops[b.fins[fin].loop_].face
}

/// The faces using an edge, in fin order (two for a manifold body).
pub(super) fn faces_of(b: &Body, e: EdgeId) -> Vec<FaceId> {
    b.edge_fins(e).map(|fin| face_of(b, fin)).collect()
}

/// The other face along an edge from `f`, for a manifold body.
pub(super) fn across(b: &Body, e: EdgeId, f: FaceId) -> Option<FaceId> {
    let faces = faces_of(b, e);
    match faces.as_slice() {
        [x, y] if *x == f => Some(*y),
        [x, y] if *y == f => Some(*x),
        _ => None,
    }
}

/// The fins of a face, loop by loop.
pub(super) fn fins_of(b: &Body, f: FaceId) -> Vec<FinId> {
    b.face_loops(f).flat_map(|l| b.loop_fins(l)).collect()
}

/// The edges of a face, sorted, each once.
pub(super) fn edges_of(b: &Body, f: FaceId) -> Vec<EdgeId> {
    let mut out: Vec<EdgeId> = fins_of(b, f).iter().map(|&x| b.fins[x].edge).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The faces sharing an edge with `f`, sorted, each once.
pub(super) fn neighbours(b: &Body, f: FaceId) -> Vec<FaceId> {
    let mut out: Vec<FaceId> = edges_of(b, f)
        .into_iter()
        .filter_map(|e| across(b, e, f))
        .filter(|&g| g != f)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The largest tolerance of a face's edges and vertices: what any
/// comparison about the face adds to the caller's.
pub(super) fn face_tolerance(b: &Body, f: FaceId) -> f64 {
    let mut t: f64 = 0.0;
    for e in edges_of(b, f) {
        let edge = &b.edges[e];
        t = t
            .max(edge.tolerance.get())
            .max(b.vertices[edge.start].tolerance.get())
            .max(b.vertices[edge.end].tolerance.get());
    }
    t
}

pub(super) fn edge_tolerance(b: &Body, e: EdgeId) -> f64 {
    let edge = &b.edges[e];
    edge.tolerance
        .get()
        .max(b.vertices[edge.start].tolerance.get())
        .max(b.vertices[edge.end].tolerance.get())
}

/// The edge's end points.
pub(super) fn ends(b: &Body, e: EdgeId) -> [Point3; 2] {
    let edge = &b.edges[e];
    [b.vertices[edge.start].point, b.vertices[edge.end].point]
}

/// The point at the middle of an edge's interval.
pub(super) fn middle(b: &Body, e: EdgeId) -> Point3 {
    let edge = &b.edges[e];
    let [t0, t1] = edge.interval;
    b.curves[edge.curve].point(0.5 * (t0 + t1))
}

/// Whether two unit directions are parallel or opposite within `angle`.
pub(super) fn parallel(a: UnitVector3, b: UnitVector3, angle: f64) -> bool {
    a.get().cross(b.get()).length() <= angle
}

/// The angle of a point about a frame's axis, from its `x`, in `[0, 2π)`.
pub(super) fn angle_of(frame: &Frame, p: Point3) -> f64 {
    let l = frame.to_local(p);
    polar_angle(l.y, l.x)
}

/// The distance of a point from a frame's axis.
pub(super) fn axis_distance(frame: &Frame, p: Point3) -> f64 {
    let l = frame.to_local(p);
    crate::math::hypot(l.x, l.y)
}

/// `a` moved by whole turns to lie as near `to` as it can.
pub(super) fn unwrap(a: f64, to: f64) -> f64 {
    a + TAU * ((to - a) / TAU).round()
}

/// The range of `u` a face's pcurves cover on a surface periodic in `u`,
/// each moved by whole periods next to the first: `None` for a fin with
/// no pcurve.
pub(super) fn u_range(b: &Body, f: FaceId) -> Option<[f64; 2]> {
    let mut out: Option<[f64; 2]> = None;
    for fin in fins_of(b, f) {
        let fi = &b.fins[fin];
        let pc = fi.pcurve?;
        let [s0, s1] = b.edges[fi.edge].interval.map(|t| pc.param(t));
        let bx = b.curves2[pc.curve].bounds([s0.min(s1), s0.max(s1)]);
        let [lo, hi] = [bx.min().x, bx.max().x];
        let shift = match out {
            None => 0.0,
            Some([a, z]) => unwrap(0.5 * (lo + hi), 0.5 * (a + z)) - 0.5 * (lo + hi),
        };
        let (lo, hi) = (lo + shift, hi + shift);
        out = Some(match out {
            None => [lo, hi],
            Some([a, z]) => [a.min(lo), z.max(hi)],
        });
    }
    out
}

/// Whether an edge is a straight line parallel to a frame's axis at
/// `radius` from it, within `eps`: the angle about the axis it lies at.
pub(super) fn axial_line(b: &Body, e: EdgeId, frame: &Frame, radius: f64, eps: f64) -> Option<f64> {
    let edge = &b.edges[e];
    if !matches!(b.curves[edge.curve], Curve::Line(_)) {
        return None;
    }
    let [p, q] = ends(b, e);
    for x in [p, q] {
        if (axis_distance(frame, x) - radius).abs() > eps {
            return None;
        }
    }
    let (a, c) = (angle_of(frame, p), angle_of(frame, q));
    let c = unwrap(c, a);
    ((a - c).abs() * radius <= eps).then_some(0.5 * (a + c))
}

/// Whether two faces on parallel planes overlap seen along `normal`: their
/// boxes, projected onto the plane square to it, share more than `eps`
/// both ways. Boxes hold their faces, so faces that don't overlap this way
/// certainly don't face each other across the sheet.
pub(super) fn overlap_along(b: &Body, f: FaceId, g: FaceId, normal: UnitVector3, eps: f64) -> bool {
    let Ok(frame) = Frame::new(Point3::ORIGIN, normal.get(), any_square(normal)) else {
        return true;
    };
    let span = |face: FaceId| {
        let mut out = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
        for c in crate::intersect::face_box(b, face).corners() {
            let l = frame.to_local(c);
            for (k, x) in [l.x, l.y].into_iter().enumerate() {
                out[k] = [out[k][0].min(x), out[k][1].max(x)];
            }
        }
        out
    };
    let (p, q) = (span(f), span(g));
    (0..2).all(|k| p[k][1].min(q[k][1]) - p[k][0].max(q[k][0]) > eps)
}

/// A direction square to `n`.
fn any_square(n: UnitVector3) -> crate::geom::Vector3 {
    let v = n.get();
    if v.x.abs() < 0.9 {
        crate::geom::Vector3::new(1.0, 0.0, 0.0)
    } else {
        crate::geom::Vector3::new(0.0, 1.0, 0.0)
    }
}

/// Half a turn: the largest bend.
pub(super) const HALF_TURN: f64 = PI;
