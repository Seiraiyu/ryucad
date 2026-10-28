//! The minimum distance between two entities, certified.
//!
//! Closed forms come first where both entities are analytic and the
//! surfaces' or lines' own distance (a lower bound for any pair of
//! points on them) is attained inside both: a vertex against a line,
//! circle or analytic face, parallel planes, two lines. Those are exact.
//!
//! Otherwise both entities are meshed by [`crate::mesh::tessellate`],
//! whose pieces lie within the chord (plus the edge tolerances) of the
//! exact geometry, so the nearest pair of pieces, less both slacks, is a
//! lower bound on the distance. That pair's points are lifted onto the
//! exact entities and refined by alternating Newton projections, whose
//! distance is an upper bound. The value is the upper bound and its bound
//! the gap to the lower; the chord is cut by four until the gap is
//! within the tolerance, or the mesh would grow too large.
//!
//! Where the lower bound reaches zero the entities may meet, and that is
//! decided by [`crate::intersect`]: faces by `face_face`, an edge against a
//! face by `curve_surface` and `point_in_face`, and a solid around a
//! point of the other by `point_in_body`. Meeting entities are exactly 0
//! apart; entities the intersectors leave uncertain are `Uncertain`.

use super::Entity;
use super::Value;
use super::prims::{Prim, point_segment, point_triangle};
use super::tree::{Piece, Tree};
use crate::Error;
use crate::MeasureError;
use crate::geom::{Curve, Point2, Point3, Rigid, Surface};
use crate::intersect::{
    CurveHit, FaceMeet, Failure, Inclusion, Place, curve_surface, face_box, face_face,
    point_in_body, point_in_face,
};
use crate::mesh::{BodyMesh, MeshParams, tessellate};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::topo::{Body, EdgeId, FaceId, VertexId};

/// How closely [`distance`] works.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistanceOptions {
    /// The bound to reach, relative to the size of the box holding both
    /// entities.
    pub tolerance: f64,
}

impl Default for DistanceOptions {
    fn default() -> Self {
        Self { tolerance: 1e-4 }
    }
}

/// The distance between two entities and the nearest points of each,
/// placed as the entities were.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distance {
    pub distance: Value,
    pub points: [Point3; 2],
}

/// Rounds of meshing at a finer chord before the bound is taken as it is.
const ROUNDS: usize = 4;

/// The most triangles a measuring mesh may have.
const MAX_TRIANGLES: u64 = 4_000_000;

/// One side of a measure: an entity of a body, placed.
#[derive(Clone, Copy)]
struct Side<'a> {
    body: &'a Body,
    entity: Entity,
    place: Rigid,
    inverse: Rigid,
}

/// Where a mesh piece came from, to lift its points onto the exact
/// entity.
#[derive(Clone, Copy, Debug)]
enum Hint {
    Vertex(VertexId),
    Edge(EdgeId, [f64; 2], [Point3; 2]),
    Face(FaceId, [Point2; 3], [Point3; 3]),
}

/// A point on an entity and where it is in its geometry's parameters:
/// what refinement moves.
#[derive(Clone, Copy, Debug)]
enum At {
    Fixed,
    Edge(EdgeId, f64),
    Face(FaceId, Point2),
}

impl<'a> Side<'a> {
    fn new((body, entity, place): (&'a Body, Entity, &Rigid)) -> Result<Self, Error> {
        let live = match entity {
            Entity::Vertex(v) => body.vertex(v).is_some(),
            Entity::Edge(e) => body.edge(e).is_some(),
            Entity::Face(f) => body.face(f).is_some(),
            Entity::Body => body.faces().next().is_some(),
        };
        if !live {
            return Err(MeasureError::NotAnEntity.into());
        }
        Ok(Self {
            body,
            entity,
            place: *place,
            inverse: place.inverse(),
        })
    }

    fn world(&self, p: Point3) -> Point3 {
        self.place.apply_point(p)
    }

    fn local(&self, p: Point3) -> Point3 {
        self.inverse.apply_point(p)
    }

    /// The entity's faces and edges: what its box and tolerances cover.
    fn faces(&self) -> Vec<FaceId> {
        match self.entity {
            Entity::Face(f) => vec![f],
            Entity::Body => self.body.faces().collect(),
            _ => Vec::new(),
        }
    }

    /// A box holding the entity, placed.
    fn bbox(&self) -> crate::geom::Box3 {
        let b = self.body;
        let local = match self.entity {
            Entity::Vertex(v) => crate::geom::Box3::from_point(b.vertices[v].point),
            Entity::Edge(e) => {
                let edge = &b.edges[e];
                b.curves[edge.curve].bounds(edge.interval)
            }
            Entity::Face(f) => face_box(b, f),
            Entity::Body => b
                .faces()
                .map(|f| face_box(b, f))
                .reduce(|x, y| x.union(&y))
                .expect("a body with faces"),
        };
        local.transform_rigid(&self.place)
    }

    /// The most a mesh piece may lie from the entity beyond the chord:
    /// the largest tolerance of what it touches.
    fn tolerance(&self) -> f64 {
        let b = self.body;
        match self.entity {
            Entity::Vertex(v) => b.vertices[v].tolerance.get(),
            Entity::Edge(e) => {
                let edge = &b.edges[e];
                edge.tolerance
                    .get()
                    .max(b.vertices[edge.start].tolerance.get())
                    .max(b.vertices[edge.end].tolerance.get())
            }
            _ => {
                let edges = b.edges().map(|e| b.edges[e].tolerance.get());
                let vertices = b.vertices().map(|v| b.vertices[v].tolerance.get());
                edges.chain(vertices).fold(LINEAR_RESOLUTION, f64::max)
            }
        }
    }

    /// The mesh pieces of the entity, placed, and their hints.
    fn pieces(&self, mesh: Option<&BodyMesh>) -> (Vec<Piece>, Vec<Hint>) {
        let (mut pieces, mut hints) = (Vec::new(), Vec::new());
        let mut push = |prim: Prim, hint: Hint| {
            pieces.push(Piece {
                prim,
                tag: hints.len() as u32,
            });
            hints.push(hint);
        };
        match self.entity {
            Entity::Vertex(v) => push(
                Prim::Point(self.world(self.body.vertices[v].point)),
                Hint::Vertex(v),
            ),
            Entity::Edge(e) => {
                let m = mesh.expect("an edge is meshed");
                if let Some(p) = m.edges.iter().find(|p| p.edge == e) {
                    for k in 1..p.positions.len() {
                        let local =
                            [p.positions[k - 1], p.positions[k]].map(|i| m.positions[i as usize]);
                        push(
                            Prim::Segment(local.map(|q| self.world(q))),
                            Hint::Edge(e, [p.params[k - 1], p.params[k]], local),
                        );
                    }
                }
            }
            Entity::Face(_) | Entity::Body => {
                let m = mesh.expect("a face is meshed");
                for f in self.faces() {
                    let Some(fm) = m.faces.iter().find(|fm| fm.face == f) else {
                        continue;
                    };
                    for t in &m.triangles[fm.triangles.start as usize..fm.triangles.end as usize] {
                        let c = t.map(|k| m.corners[k as usize]);
                        let local = c.map(|c| m.positions[c.position as usize]);
                        push(
                            Prim::Triangle(local.map(|q| self.world(q))),
                            Hint::Face(f, c.map(|c| c.uv), local),
                        );
                    }
                }
            }
        }
        (pieces, hints)
    }

    /// A point of the entity near the mesh point `x` (placed) of the piece
    /// `hint` describes.
    fn lift(&self, x: Point3, hint: Hint) -> (Point3, At) {
        let b = self.body;
        let q = self.local(x);
        match hint {
            Hint::Vertex(v) => (self.world(b.vertices[v].point), At::Fixed),
            Hint::Edge(e, [t0, t1], seg) => {
                let near = point_segment(q, &seg);
                let len = seg[1].distance(seg[0]);
                let s = if len > 0.0 {
                    near.distance(seg[0]) / len
                } else {
                    0.0
                };
                let t = t0 + s * (t1 - t0);
                let edge = &b.edges[e];
                (self.world(b.curves[edge.curve].point(t)), At::Edge(e, t))
            }
            Hint::Face(f, uv, tri) => {
                let near = point_triangle(q, &tri);
                let w = barycentric(near, &tri);
                let p = Point2::new(
                    w[0] * uv[0].x + w[1] * uv[1].x + w[2] * uv[2].x,
                    w[0] * uv[0].y + w[1] * uv[1].y + w[2] * uv[2].y,
                );
                if matches!(
                    point_in_face(b, f, p),
                    Ok(Place::Inside | Place::OnEdge(_) | Place::OnVertex(_))
                ) {
                    let s = &b.surfaces[b.faces[f].surface];
                    (self.world(s.point(p.x, p.y)), At::Face(f, p))
                } else {
                    // Beside the boundary: the nearest corner, a point of
                    // the face's mesh within the tolerance of the face.
                    let k = (0..3)
                        .min_by(|&i, &j| tri[i].distance(q).total_cmp(&tri[j].distance(q)))
                        .expect("three corners");
                    (self.world(tri[k]), At::Fixed)
                }
            }
        }
    }

    /// The point of the entity nearest the placed point `x` that Newton's
    /// method finds from `at`, or `None` where it leaves the entity.
    fn nearer(&self, x: Point3, at: At) -> Option<(Point3, At)> {
        let b = self.body;
        let q = self.local(x);
        match at {
            At::Fixed => None,
            At::Edge(e, t) => {
                let edge = &b.edges[e];
                let c = &b.curves[edge.curve];
                let t = newton_curve(c, q, t, edge.interval);
                Some((self.world(c.point(t)), At::Edge(e, t)))
            }
            At::Face(f, uv) => {
                let s = &b.surfaces[b.faces[f].surface];
                let uv = newton_surface(s, q, uv)?;
                matches!(
                    point_in_face(b, f, uv),
                    Ok(Place::Inside | Place::OnEdge(_) | Place::OnVertex(_))
                )
                .then(|| (self.world(s.point(uv.x, uv.y)), At::Face(f, uv)))
            }
        }
    }

    /// A point of the entity, placed: for containment.
    fn some_point(&self, mesh: Option<&BodyMesh>) -> Option<Point3> {
        let b = self.body;
        match self.entity {
            Entity::Vertex(v) => Some(self.world(b.vertices[v].point)),
            Entity::Edge(e) => {
                let edge = &b.edges[e];
                let t = 0.5 * (edge.interval[0] + edge.interval[1]);
                Some(self.world(b.curves[edge.curve].point(t)))
            }
            // A corner of the face's own triangles: the body's first
            // position need not lie on this face.
            Entity::Face(f) => mesh.and_then(|m| face_point(m, f)).map(|p| self.world(p)),
            Entity::Body => mesh
                .and_then(|m| m.positions.first())
                .map(|&p| self.world(p)),
        }
    }
}

/// A point of face `f`: a corner of its own triangles in `mesh`.
fn face_point(mesh: &BodyMesh, f: FaceId) -> Option<Point3> {
    let fm = mesh.faces.iter().find(|fm| fm.face == f)?;
    let corner = mesh.corners.get(fm.corners.start as usize)?;
    mesh.positions.get(corner.position as usize).copied()
}

/// A point of the surface: the middle of its domain, a finite end where
/// a direction is open one way, else 0.
fn surface_point(s: &Surface) -> Point3 {
    let mid = |[lo, hi]: [f64; 2]| match (lo.is_finite(), hi.is_finite()) {
        (true, true) => 0.5 * (lo + hi),
        (true, false) => lo,
        (false, true) => hi,
        (false, false) => 0.0,
    };
    let [u, v] = s.domain();
    s.point(mid(u), mid(v))
}

fn barycentric(p: Point3, t: &[Point3; 3]) -> [f64; 3] {
    let (v0, v1, v2) = (t[1] - t[0], t[2] - t[0], p - t[0]);
    let (d00, d01, d11) = (v0.dot(v0), v0.dot(v1), v1.dot(v1));
    let (d20, d21) = (v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den == 0.0 || !den.is_finite() {
        return [1.0, 0.0, 0.0];
    }
    let v = (d11 * d20 - d01 * d21) / den;
    let w = (d00 * d21 - d01 * d20) / den;
    [1.0 - v - w, v, w]
}

/// Newton's method on `|C(t) - q|²` from `t`, kept inside `range`.
fn newton_curve(c: &Curve, q: Point3, mut t: f64, [lo, hi]: [f64; 2]) -> f64 {
    for _ in 0..32 {
        let e = c.eval(t);
        let r = e.point - q;
        let g = r.dot(e.d1);
        let mut h = e.d1.dot(e.d1) + r.dot(e.d2);
        if h <= 0.0 {
            h = e.d1.dot(e.d1);
        }
        if h <= 0.0 {
            break;
        }
        let next = (t - g / h).clamp(lo, hi);
        let done = (next - t).abs() <= 1e-15 * (1.0 + t.abs());
        t = next;
        if done {
            break;
        }
    }
    t
}

/// Newton's method on `|S(u, v) - q|²` from `uv`, kept inside the
/// surface's bounded directions; `None` where it doesn't settle.
fn newton_surface(s: &Surface, q: Point3, mut uv: Point2) -> Option<Point2> {
    let [[u0, u1], [v0, v1]] = s.domain();
    for _ in 0..32 {
        let e = s.eval(uv.x, uv.y);
        let r = e.point - q;
        let (gu, gv) = (r.dot(e.du), r.dot(e.dv));
        let (mut a, mut bb, mut c) = (
            e.du.dot(e.du) + r.dot(e.duu),
            e.du.dot(e.dv) + r.dot(e.duv),
            e.dv.dot(e.dv) + r.dot(e.dvv),
        );
        if a <= 0.0 || a * c - bb * bb <= 0.0 {
            (a, bb, c) = (e.du.dot(e.du), e.du.dot(e.dv), e.dv.dot(e.dv));
        }
        let det = a * c - bb * bb;
        if det <= 0.0 || !det.is_finite() {
            return None;
        }
        let du = (c * gu - bb * gv) / det;
        let dv = (a * gv - bb * gu) / det;
        let next = Point2::new((uv.x - du).clamp(u0, u1), (uv.y - dv).clamp(v0, v1));
        let done = (next - uv).length() <= 1e-15 * (1.0 + uv.to_vector().length());
        uv = next;
        if done {
            break;
        }
    }
    uv.x.is_finite().then_some(uv)
}

/// The minimum distance between `a`'s entity and `b`'s, each placed by
/// its `Rigid`. See the module documentation.
pub fn distance(
    a: (&Body, Entity, &Rigid),
    b: (&Body, Entity, &Rigid),
    opts: &DistanceOptions,
) -> Result<Distance, Error> {
    let (a, b) = (Side::new(a)?, Side::new(b)?);
    if let Some(d) = closed_form(&a, &b) {
        return Ok(d);
    }
    let (ba, bb) = (a.bbox(), b.bbox());
    let all = ba.union(&bb);
    let size = (all.max() - all.min()).length().max(LINEAR_RESOLUTION);
    let target = opts.tolerance * size;
    let mut chord = (size * 1e-3)
        .max(target / 2.0)
        .max(10.0 * LINEAR_RESOLUTION);
    let mut last: Option<Distance> = None;
    for round in 0..ROUNDS {
        let ma = mesh_of(&a, chord)?;
        let mb = mesh_of(&b, chord)?;
        if round == 0
            && let Some(d) = contained(&a, &b, ma.as_ref(), mb.as_ref())?
        {
            return Ok(d);
        }
        let (pa, ha) = a.pieces(ma.as_ref());
        let (pb, hb) = b.pieces(mb.as_ref());
        let near = Tree::new(pa)
            .nearest(&Tree::new(pb))
            .ok_or(MeasureError::Unsupported)?;
        let slack_a = if matches!(a.entity, Entity::Vertex(_)) {
            0.0
        } else {
            chord
        };
        let slack_b = if matches!(b.entity, Entity::Vertex(_)) {
            0.0
        } else {
            chord
        };
        let lower =
            near.distance - slack_a - slack_b - a.tolerance() - b.tolerance() - 1e-12 * size;
        let (mut xa, mut at_a) = a.lift(near.a.0, ha[near.a.1 as usize]);
        let (mut xb, mut at_b) = b.lift(near.b.0, hb[near.b.1 as usize]);
        let mut upper = xa.distance(xb);
        for _ in 0..40 {
            let mut moved = false;
            if let Some((p, at)) = b.nearer(xa, at_b)
                && p.distance(xa) < upper
            {
                (xb, at_b, upper, moved) = (p, at, p.distance(xa), true);
            }
            if let Some((p, at)) = a.nearer(xb, at_a)
                && p.distance(xb) < upper
            {
                let d = p.distance(xb);
                moved |= upper - d > 1e-15 * size;
                (xa, at_a, upper) = (p, at, d);
            }
            if !moved {
                break;
            }
        }
        if lower <= 0.0
            && let Some(p) = meets(&a, &b, upper, xa.lerp(xb, 0.5))?
        {
            return Ok(Distance {
                distance: Value::Exact(0.0),
                points: [p, p],
            });
        }
        let d = Distance {
            distance: Value::Bounded {
                value: upper,
                bound: upper - lower.max(0.0),
            },
            points: [xa, xb],
        };
        let next = chord / 4.0;
        let enough = upper - lower.max(0.0) <= target;
        last = Some(d);
        if enough || next < 10.0 * LINEAR_RESOLUTION {
            break;
        }
        chord = next;
    }
    last.ok_or_else(|| MeasureError::Unsupported.into())
}

/// The entity's body meshed at `chord`, unless it is a vertex. A mesh
/// that would be too large or leaves faces out is `Unsupported`.
fn mesh_of(s: &Side<'_>, chord: f64) -> Result<Option<BodyMesh>, Error> {
    if matches!(s.entity, Entity::Vertex(_)) {
        return Ok(None);
    }
    let params = MeshParams {
        max_triangles: MAX_TRIANGLES,
        ..MeshParams::new(chord, 0.5)
    };
    let m = tessellate(s.body, &params).map_err(|_| MeasureError::Unsupported)?;
    let faces = s.faces();
    if m.faults
        .iter()
        .any(|f| faces.contains(&f.face) || faces.is_empty())
    {
        return Err(MeasureError::Unsupported.into());
    }
    Ok(Some(m))
}

/// Zero where one side is a solid holding a point of the other.
fn contained(
    a: &Side<'_>,
    b: &Side<'_>,
    ma: Option<&BodyMesh>,
    mb: Option<&BodyMesh>,
) -> Result<Option<Distance>, Error> {
    for (solid, other, m) in [(a, b, mb), (b, a, ma)] {
        if solid.entity != Entity::Body {
            continue;
        }
        let Some(p) = other.some_point(m) else {
            continue;
        };
        match point_in_body(solid.body, solid.local(p)) {
            Ok(Inclusion::Inside | Inclusion::Boundary) => {
                return Ok(Some(Distance {
                    distance: Value::Exact(0.0),
                    points: [p, p],
                }));
            }
            Ok(Inclusion::Outside) => {}
            Err(_) => return Err(MeasureError::Uncertain.into()),
        }
    }
    Ok(None)
}

fn fail(f: Failure) -> Error {
    match f {
        Failure::Unsupported => MeasureError::Unsupported.into(),
        Failure::Uncertain => MeasureError::Uncertain.into(),
    }
}

/// Whether the entities meet, and a point where they do (placed as `a`
/// is). Vertices and pairs of edges meet where refinement brought them
/// within their tolerances (`upper`), at the refined pair's middle `mid`.
fn meets(a: &Side<'_>, b: &Side<'_>, upper: f64, mid: Point3) -> Result<Option<Point3>, Error> {
    let faces_a = a.faces();
    let faces_b = b.faces();
    let point_like = |s: &Side<'_>| matches!(s.entity, Entity::Vertex(_) | Entity::Edge(_));
    let vertex = |s: &Side<'_>| matches!(s.entity, Entity::Vertex(_));
    if (point_like(a) && point_like(b)) || vertex(a) || vertex(b) {
        let reach = a.tolerance() + b.tolerance() + LINEAR_RESOLUTION;
        return Ok((upper <= reach).then_some(mid));
    }
    // Bring b into a's frame.
    let rel = a.inverse.then_after(&b.place);
    let moved = b
        .body
        .transformed(&rel)
        .map_err(|_| MeasureError::Unsupported)?;
    let near_a = |f: FaceId| face_box(a.body, f);
    let bb = b.bbox().transform_rigid(&a.inverse);
    let grow = a.tolerance() + b.tolerance() + LINEAR_RESOLUTION;
    let edge_face =
        |body: &Body, e: EdgeId, other: &Body, f: FaceId| -> Result<Option<Point3>, Error> {
            let edge = &body.edges[e];
            let curve = &body.curves[edge.curve];
            let surface = &other.surfaces[other.faces[f].surface];
            let hits = curve_surface(curve, edge.interval, surface, grow).map_err(fail)?;
            for h in hits {
                let p = match h {
                    CurveHit::Point { point, .. } => point,
                    CurveHit::On { t } => curve.point(0.5 * (t[0] + t[1])),
                };
                let (u, v) = surface.project(p).param;
                match point_in_face(other, f, Point2::new(u, v)).map_err(fail)? {
                    Place::Outside => {}
                    _ => return Ok(Some(p)),
                }
            }
            Ok(None)
        };
    match (a.entity, b.entity) {
        (Entity::Edge(e), _) => {
            for f in faces_b {
                if let Some(p) = edge_face(a.body, e, &moved, f)? {
                    return Ok(Some(a.world(p)));
                }
            }
            Ok(None)
        }
        (_, Entity::Edge(e)) => {
            for f in faces_a {
                if let Some(p) = edge_face(&moved, e, a.body, f)? {
                    return Ok(Some(a.world(p)));
                }
            }
            Ok(None)
        }
        _ => {
            for &fa in &faces_a {
                let box_a = near_a(fa).grow(grow);
                if !box_a.intersects(&bb.grow(grow)) {
                    continue;
                }
                for &fb in &faces_b {
                    if !box_a.intersects(&face_box(&moved, fb).grow(grow)) {
                        continue;
                    }
                    let meet = face_face((a.body, fa), (&moved, fb)).map_err(fail)?;
                    let p = match meet {
                        FaceMeet::Coincident { rims, whole, .. } => {
                            if whole {
                                // Neither face has a boundary, so every
                                // point of `fa`'s surface is on both.
                                Some(surface_point(&a.body.surfaces[a.body.faces[fa].surface]))
                            } else {
                                rims.first().map(|r| {
                                    let body = if r.of == 0 { a.body } else { &moved };
                                    let edge = &body.edges[r.edge];
                                    body.curves[edge.curve].point(0.5 * (r.range[0] + r.range[1]))
                                })
                            }
                        }
                        FaceMeet::Meet {
                            stretches,
                            contacts,
                        } => stretches
                            .first()
                            .map(|s| s.curve.point(0.5 * (s.range[0] + s.range[1])))
                            .or(contacts.first().map(|c| c.point)),
                    };
                    if let Some(p) = p {
                        return Ok(Some(a.world(p)));
                    }
                }
            }
            Ok(None)
        }
    }
}

/// The exact distance where both entities are analytic and their
/// geometry's own distance is attained inside both.
fn closed_form(a: &Side<'_>, b: &Side<'_>) -> Option<Distance> {
    use Entity::*;
    match (a.entity, b.entity) {
        (Vertex(v), Vertex(w)) => {
            let (p, q) = (
                a.world(a.body.vertices[v].point),
                b.world(b.body.vertices[w].point),
            );
            Some(exact(p, q))
        }
        (Vertex(v), Edge(_)) => {
            let p = a.world(a.body.vertices[v].point);
            point_edge(p, b).map(|q| exact(p, q))
        }
        (Edge(_), Vertex(_)) => closed_form(b, a).map(swap),
        (Vertex(v), Face(f)) => {
            let p = a.world(a.body.vertices[v].point);
            point_face(p, b, f).map(|q| exact(p, q))
        }
        (Face(_), Vertex(_)) => closed_form(b, a).map(swap),
        (Edge(e), Edge(g)) => lines(a, e, b, g),
        (Face(f), Face(g)) => planes(a, f, b, g),
        _ => None,
    }
}

fn exact(p: Point3, q: Point3) -> Distance {
    Distance {
        distance: Value::Exact(p.distance(q)),
        points: [p, q],
    }
}

fn swap(d: Distance) -> Distance {
    Distance {
        points: [d.points[1], d.points[0]],
        ..d
    }
}

/// The nearest point of a line or circle edge to `p`, where it lies
/// inside the edge: the curve's own projection is then the answer (a
/// circle's distance has one minimum round it).
fn point_edge(p: Point3, s: &Side<'_>) -> Option<Point3> {
    let Entity::Edge(e) = s.entity else {
        return None;
    };
    let edge = &s.body.edges[e];
    let c = &s.body.curves[edge.curve];
    let [t0, t1] = edge.interval;
    let q = s.local(p);
    let t = match c {
        Curve::Line(_) => c.project(q).param,
        Curve::Circle(_) => {
            let t = c.project(q).param;
            if c.distance(q) <= LINEAR_RESOLUTION {
                return None;
            }
            // Into the edge's period.
            let k = ((t0 - t) / core::f64::consts::TAU).ceil();
            t + k * core::f64::consts::TAU
        }
        _ => return None,
    };
    (t >= t0 && t <= t1).then(|| s.world(c.point(t)))
}

/// The nearest point of an analytic face to `p`, where the surface's own
/// nearest point lies inside the face.
fn point_face(p: Point3, s: &Side<'_>, f: FaceId) -> Option<Point3> {
    let surface = &s.body.surfaces[s.body.faces[f].surface];
    surface.frame()?;
    let q = s.local(p);
    let proj = surface.project(q);
    let (u, v) = proj.param;
    match point_in_face(s.body, f, Point2::new(u, v)) {
        Ok(Place::Inside | Place::OnEdge(_) | Place::OnVertex(_)) => Some(s.world(proj.point)),
        _ => None,
    }
}

/// Two straight edges: the nearest points of their lines where both lie
/// inside the edges.
fn lines(a: &Side<'_>, e: EdgeId, b: &Side<'_>, g: EdgeId) -> Option<Distance> {
    let ends = |s: &Side<'_>, e: EdgeId| {
        let edge = &s.body.edges[e];
        match &s.body.curves[edge.curve] {
            Curve::Line(l) => Some((
                s.world(l.origin()),
                s.place.apply_unit(l.dir()).get(),
                edge.interval,
            )),
            _ => None,
        }
    };
    let ((o1, d1, [s0, s1]), (o2, d2, [t0, t1])) = (ends(a, e)?, ends(b, g)?);
    let w = o1 - o2;
    let bdot = d1.dot(d2);
    let den = 1.0 - bdot * bdot;
    if den <= ANGULAR_RESOLUTION {
        return None;
    }
    let (dd, ee) = (d1.dot(w), d2.dot(w));
    let s = (bdot * ee - dd) / den;
    let t = (ee - bdot * dd) / den;
    (s >= s0 && s <= s1 && t >= t0 && t <= t1).then(|| exact(o1 + d1 * s, o2 + d2 * t))
}

/// Two planar faces facing each other on parallel planes: their planes'
/// separation, where a vertex of one projects inside the other.
fn planes(a: &Side<'_>, f: FaceId, b: &Side<'_>, g: FaceId) -> Option<Distance> {
    let plane = |s: &Side<'_>, f: FaceId| match &s.body.surfaces[s.body.faces[f].surface] {
        Surface::Plane(p) => Some((
            s.world(p.frame().origin()),
            s.place.apply_unit(p.frame().z()),
        )),
        _ => None,
    };
    let ((_, na), (_, nb)) = (plane(a, f)?, plane(b, g)?);
    if na.get().cross(nb.get()).length() > ANGULAR_RESOLUTION {
        return None;
    }
    for (from, ff, to, gg, flip) in [(a, f, b, g, false), (b, g, a, f, true)] {
        let mut vs: Vec<VertexId> = Vec::new();
        for l in from.body.face_loops(ff) {
            for fin in from.body.loop_fins(l) {
                if let Some(v) = from.body.fin_start(fin) {
                    vs.push(v);
                }
            }
        }
        vs.sort_unstable();
        for v in vs {
            let p = from.world(from.body.vertices[v].point);
            if let Some(q) = point_face(p, to, gg) {
                let d = exact(p, q);
                return Some(if flip { swap(d) } else { d });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vector3;
    use crate::name::OpKey;
    use crate::primitive;

    fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid::translation(Vector3::new(x, y, z))
    }

    fn block() -> Body {
        primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0
    }

    fn ball(r: f64) -> Body {
        primitive::sphere(OpKey(2), r, &Rigid::IDENTITY).unwrap().0
    }

    fn opts() -> DistanceOptions {
        DistanceOptions::default()
    }

    #[test]
    fn vertices_and_a_ball_are_exact() {
        let b = block();
        let s = ball(0.5);
        let v = b.vertices().next().unwrap();
        let p = b.vertices[v].point;
        let face = s.faces().next().unwrap();
        let place = at(p.x + 3.0, p.y + 4.0, p.z);
        let d = distance(
            (&b, Entity::Vertex(v), &Rigid::IDENTITY),
            (&s, Entity::Face(face), &place),
            &opts(),
        )
        .unwrap();
        assert_eq!(d.distance, Value::Exact(4.5));
        let d = distance(
            (&b, Entity::Vertex(v), &Rigid::IDENTITY),
            (&b, Entity::Vertex(v), &at(0.0, 0.0, 2.0)),
            &opts(),
        )
        .unwrap();
        assert_eq!(d.distance, Value::Exact(2.0));
    }

    #[test]
    fn facing_planes_are_exact_and_bodies_bounded() {
        let b = block();
        let place = at(1.25, 0.5, 0.0);
        let pick = |x: f64| {
            b.faces()
                .find(|&f| {
                    let Surface::Plane(p) = &b.surfaces[b.faces[f].surface] else {
                        return false;
                    };
                    (p.frame().origin().x - x).abs() < 1e-12 && p.frame().z().get().x.abs() > 0.999
                })
                .unwrap()
        };
        let d = distance(
            (&b, Entity::Face(pick(1.0)), &Rigid::IDENTITY),
            (&b, Entity::Face(pick(0.0)), &place),
            &opts(),
        )
        .unwrap();
        assert_eq!(d.distance, Value::Exact(0.25));
        let d = distance(
            (&b, Entity::Body, &Rigid::IDENTITY),
            (&b, Entity::Body, &place),
            &opts(),
        )
        .unwrap();
        assert!(d.distance.holds(0.25, 0.0), "{d:?}");
        assert!((d.distance.value() - 0.25).abs() < 1e-12, "{d:?}");
    }

    #[test]
    fn two_balls_converge_on_the_gap() {
        let s = ball(0.5);
        let d = distance(
            (&s, Entity::Body, &Rigid::IDENTITY),
            (&s, Entity::Body, &at(1.2, 0.9, 0.0)),
            &opts(),
        )
        .unwrap();
        let want = 1.5 - 1.0;
        assert!(d.distance.holds(want, 1e-12), "{d:?}");
        assert!((d.distance.value() - want).abs() < 1e-9, "{d:?}");
        assert!(d.distance.bound() <= 1e-4 * 3.0, "{d:?}");
        assert!((d.points[0].distance(Point3::ORIGIN) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn overlapping_touching_and_held_bodies_are_zero() {
        let b = block();
        let zero = |place: Rigid, ea: Entity, eb: Entity| {
            distance((&b, ea, &Rigid::IDENTITY), (&b, eb, &place), &opts())
                .unwrap()
                .distance
        };
        assert_eq!(
            zero(at(0.5, 0.5, 0.5), Entity::Body, Entity::Body),
            Value::Exact(0.0)
        );
        assert_eq!(
            zero(at(1.0, 0.2, 0.3), Entity::Body, Entity::Body),
            Value::Exact(0.0)
        );
        let small = primitive::block(OpKey(3), 0.2, 0.2, 0.2, &at(0.4, 0.4, 0.4))
            .unwrap()
            .0;
        let d = distance(
            (&b, Entity::Body, &Rigid::IDENTITY),
            (&small, Entity::Body, &Rigid::IDENTITY),
            &opts(),
        )
        .unwrap();
        assert_eq!(d.distance, Value::Exact(0.0));
        // Two faces of one box share an edge.
        let mut faces = b.faces();
        let (f, g) = (faces.next().unwrap(), faces.next().unwrap());
        let shared = b.face_loops(f).any(|l| {
            b.loop_fins(l).any(|x| {
                b.edge_fins(b.fins[x].edge)
                    .any(|y| b.loops[b.fins[y].loop_].face == g)
            })
        });
        if shared {
            assert_eq!(
                zero(Rigid::IDENTITY, Entity::Face(f), Entity::Face(g)),
                Value::Exact(0.0)
            );
        }
    }

    #[test]
    fn the_same_bits_on_one_thread_and_many() {
        let s = ball(0.5);
        let b = block();
        let run = || {
            distance(
                (&s, Entity::Body, &Rigid::IDENTITY),
                (&b, Entity::Body, &at(0.7, -0.2, 0.1)),
                &opts(),
            )
            .unwrap()
        };
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(run);
        let four = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap()
            .install(run);
        assert_eq!(one, four);
        assert!(one.distance.holds(0.5f64.sqrt() - 0.5, 1e-12), "{one:?}");
    }

    #[test]
    fn dead_entities_are_refused() {
        let b = block();
        let mut c = b.clone();
        let none = Body::new();
        assert_eq!(
            distance(
                (&none, Entity::Body, &Rigid::IDENTITY),
                (&c, Entity::Body, &Rigid::IDENTITY),
                &opts()
            ),
            Err(MeasureError::NotAnEntity.into())
        );
        c.compact();
    }
}
