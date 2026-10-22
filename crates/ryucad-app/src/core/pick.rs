//! Picking: which face, edge or vertex, or sketch point or curve, is
//! under the mouse. Faces are hit by a ray cast against their triangles;
//! edges, vertices and sketch entities by their distance on screen, so a
//! thin edge can be picked without pixel-perfect aim. Within reach, a
//! vertex or sketch point wins over an edge or sketch curve, and those
//! over a face; nothing hidden behind a face is picked. In a compare, a
//! face the working files no longer have is picked in the ghost, where
//! nothing of the model is nearer.

use crate::core::compare::Compare;
use crate::core::model::{BodyKey, Model};
use crate::core::nav::Projector;
use crate::core::sketch::Drawing;
use ryucad::entity::{EntityKind, Target};
use ryucad::mesh::{WireBody, WireMesh, name_hex};
use std::collections::HashMap;
use std::sync::Arc;

/// How close, in pixels, the mouse must be to an edge or vertex.
pub const EDGE_REACH: f64 = 5.0;
pub const VERTEX_REACH: f64 = 7.0;

/// What is under the mouse.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The entity reference: `<part>/<feature>/<name>`, or
    /// `<part>/<sketch>/<entity>`.
    pub entity: String,
    pub kind: Target,
    pub part: String,
    pub feature: String,
    /// The point picked, in meters: where an orbit turns and a zoom aims.
    pub point: [f64; 3],
    /// The revision it was picked at: a face gone from the working files,
    /// picked in a compare's ghost. `None` for the working model.
    pub rev: Option<String>,
}

/// A body ready for picking: its mesh and the box around it.
struct Body {
    key: BodyKey,
    mesh: Arc<WireBody>,
    lo: [f64; 3],
    hi: [f64; 3],
}

/// Where an entity is in the picker: a body's face, edge or vertex by its
/// index in the mesh, or a sketch's curve or point by its index in the
/// drawing.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Body(usize, EntityKind, usize),
    Curve(usize, usize),
    Dot(usize, usize),
}

/// The model's meshes and sketches prepared for picking. Rebuild it when
/// the model's generation changes.
pub struct Picker {
    pub generation: u64,
    bodies: Vec<Body>,
    sketches: Vec<(BodyKey, Arc<Drawing>)>,
    /// Every entity reference, and where it is.
    index: HashMap<String, Slot>,
    /// A compare's ghosts, each with the indices of its faces the working
    /// files no longer have: only those are picked.
    ghosts: Vec<(Body, Vec<usize>)>,
    /// The revision the ghosts are of.
    ghost_rev: Option<String>,
}

/// Where an entity is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// A face: its body and index among the mesh's faces.
    Face(BodyKey, usize),
    /// An edge's or sketch curve's polyline.
    Edge(Vec<[f64; 3]>),
    /// A vertex or sketch point.
    Vertex([f64; 3]),
}

fn mesh(b: &WireBody) -> Option<&WireMesh> {
    b.mesh.as_ref().ok()
}

fn at(m: &WireMesh, i: u32) -> [f64; 3] {
    let p = m.positions[i as usize];
    [0, 1, 2].map(|k| m.origin[k] + f64::from(p[k]))
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Where the ray `o + t d` meets the triangle, as `t` (Möller-Trumbore).
fn ray_triangle(o: [f64; 3], d: [f64; 3], t: [[f64; 3]; 3]) -> Option<f64> {
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-30 {
        return None;
    }
    let s = sub(o, t[0]);
    let u = dot(s, p) / det;
    let q = cross(s, e1);
    let v = dot(d, q) / det;
    if !(0.0..=1.0).contains(&u) || v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(dot(e2, q) / det).filter(|t| *t >= 0.0)
}

/// Whether the ray meets the box (slab test).
fn ray_box(o: [f64; 3], d: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> bool {
    let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
    for k in 0..3 {
        if d[k] == 0.0 {
            if o[k] < lo[k] || o[k] > hi[k] {
                return false;
            }
            continue;
        }
        let (a, b) = ((lo[k] - o[k]) / d[k], (hi[k] - o[k]) / d[k]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    t0 <= t1 && t1 >= 0.0
}

/// The distance from `p` to the segment `a`..`b` on screen, and how far
/// along it the nearest point is.
fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let s = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    };
    let q = [a[0] + ab[0] * s - p[0], a[1] + ab[1] * s - p[1]];
    ((q[0] * q[0] + q[1] * q[1]).sqrt(), s)
}

/// A candidate point or curve: how far from the mouse on screen, its
/// point, and what it is.
struct Near {
    px: f64,
    point: [f64; 3],
    entity: String,
    key: BodyKey,
    kind: Target,
}

/// The box around `points`, if there are any.
fn bounds(points: impl IntoIterator<Item = [f64; 3]>) -> Option<([f64; 3], [f64; 3])> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

impl Picker {
    /// The model's meshes and sketches.
    pub fn new(model: &Model) -> Picker {
        Picker::of(model, true)
    }

    /// The model's meshes, and its sketches if `sketches` (they are
    /// drawn).
    pub fn of(model: &Model, sketches: bool) -> Picker {
        let bodies: Vec<Body> = model
            .meshes
            .iter()
            .filter_map(|(key, b)| {
                let m = mesh(b)?;
                let (lo, hi) = bounds((0..m.positions.len()).map(|i| at(m, i as u32)))
                    .unwrap_or(([0.0; 3], [0.0; 3]));
                Some(Body {
                    key: key.clone(),
                    mesh: Arc::clone(b),
                    lo,
                    hi,
                })
            })
            .collect();
        let sketches: Vec<(BodyKey, Arc<Drawing>)> = if sketches {
            model
                .sketches
                .iter()
                .map(|(k, d)| (k.clone(), Arc::clone(d)))
                .collect()
        } else {
            Vec::new()
        };
        let mut index = HashMap::new();
        for (i, b) in bodies.iter().enumerate() {
            let m = mesh(&b.mesh).expect("only meshed bodies");
            let named = |n: &[u8; 16]| format!("{}/{}/{}", b.key.0, b.key.1, name_hex(n));
            for (j, f) in m.faces.iter().enumerate() {
                index.insert(named(&f.name), Slot::Body(i, EntityKind::Face, j));
            }
            for (j, (n, _)) in m.edges.iter().enumerate() {
                index.insert(named(n), Slot::Body(i, EntityKind::Edge, j));
            }
            for (j, (n, _)) in m.vertices.iter().enumerate() {
                index.insert(named(n), Slot::Body(i, EntityKind::Vertex, j));
            }
        }
        for (i, (key, d)) in sketches.iter().enumerate() {
            let named = |e: &str| format!("{}/{}/{e}", key.0, key.1);
            for (j, c) in d.curves.iter().enumerate() {
                index.insert(named(&c.entity), Slot::Curve(i, j));
            }
            for (j, p) in d.dots.iter().enumerate() {
                index.insert(named(&p.entity), Slot::Dot(i, j));
            }
        }
        Picker {
            generation: model.generation,
            bodies,
            sketches,
            index,
            ghosts: Vec::new(),
            ghost_rev: None,
        }
    }

    /// The same, with a compare's ghosts: the faces they have that the
    /// working files don't can be picked.
    pub fn with_compare(mut self, compare: Option<&Compare>) -> Picker {
        let Some(c) = compare else { return self };
        for (key, b) in &c.ghosts {
            let Some(m) = mesh(b) else { continue };
            let gone: Vec<usize> = (0..m.faces.len())
                .filter(|i| c.is_gone(key, &name_hex(&m.faces[*i].name)))
                .collect();
            if gone.is_empty() {
                continue;
            }
            let (lo, hi) = bounds((0..m.positions.len()).map(|i| at(m, i as u32)))
                .unwrap_or(([0.0; 3], [0.0; 3]));
            let body = Body {
                key: key.clone(),
                mesh: Arc::clone(b),
                lo,
                hi,
            };
            self.ghosts.push((body, gone));
        }
        self.ghost_rev = Some(c.rev.clone());
        self
    }

    /// The nearest ghost face along the ray `o + t d`, of those that can
    /// be picked: its `t`, name and body.
    fn ghost_along(&self, o: [f64; 3], d: [f64; 3]) -> Option<(f64, String, BodyKey)> {
        let mut face: Option<(f64, String, BodyKey)> = None;
        for (b, gone) in &self.ghosts {
            if !ray_box(o, d, b.lo, b.hi) {
                continue;
            }
            let m = mesh(&b.mesh).expect("only meshed ghosts");
            for f in gone.iter().map(|i| &m.faces[*i]) {
                for t in &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize] {
                    let tri = t.map(|c| at(m, m.corners[c as usize].0));
                    if let Some(t) = ray_triangle(o, d, tri)
                        && face.as_ref().is_none_or(|(best, ..)| t < *best)
                    {
                        face = Some((t, name_hex(&f.name), b.key.clone()));
                    }
                }
            }
        }
        face
    }

    /// The points that show where a slot is: a face's corners, an edge's
    /// or curve's polyline, a vertex or point.
    fn slot_points(&self, slot: Slot) -> Vec<[f64; 3]> {
        match slot {
            Slot::Body(i, kind, j) => {
                let m = mesh(&self.bodies[i].mesh).expect("only meshed bodies");
                match kind {
                    EntityKind::Face => {
                        let f = &m.faces[j];
                        m.corners[f.corners.0 as usize..f.corners.1 as usize]
                            .iter()
                            .map(|c| at(m, c.0))
                            .collect()
                    }
                    EntityKind::Edge => m.edges[j].1.iter().map(|p| at(m, *p)).collect(),
                    EntityKind::Vertex => vec![at(m, m.vertices[j].1)],
                }
            }
            Slot::Curve(i, j) => self.sketches[i].1.curves[j].points.clone(),
            Slot::Dot(i, j) => vec![self.sketches[i].1.dots[j].at],
        }
    }

    /// Where the entity `reference` is drawn, if it is in the meshes or
    /// the sketches.
    pub fn place(&self, reference: &str) -> Option<Place> {
        let slot = *self.index.get(reference)?;
        Some(match slot {
            Slot::Body(i, EntityKind::Face, j) => Place::Face(self.bodies[i].key.clone(), j),
            Slot::Body(_, EntityKind::Edge, _) | Slot::Curve(..) => {
                Place::Edge(self.slot_points(slot))
            }
            Slot::Body(_, EntityKind::Vertex, _) | Slot::Dot(..) => {
                Place::Vertex(self.slot_points(slot)[0])
            }
        })
    }

    /// The points of the given entities, those in the meshes and
    /// sketches: what a camera frames.
    pub fn points_of(&self, references: &[String]) -> Vec<[f64; 3]> {
        references
            .iter()
            .filter_map(|r| self.index.get(r))
            .flat_map(|s| self.slot_points(*s))
            .collect()
    }

    /// The mesh points of one body, or the points of one sketch, or of
    /// them all.
    pub fn points(&self, body: Option<&BodyKey>) -> Vec<[f64; 3]> {
        let mut out = Vec::new();
        for b in self
            .bodies
            .iter()
            .filter(|b| body.is_none_or(|k| &b.key == k))
        {
            let m = mesh(&b.mesh).expect("only meshed bodies");
            out.extend((0..m.positions.len()).map(|i| at(m, i as u32)));
        }
        for (_, d) in self
            .sketches
            .iter()
            .filter(|(k, _)| body.is_none_or(|b| k == b))
        {
            out.extend(d.points());
        }
        out
    }

    /// The box around the given entities, those in the meshes and
    /// sketches.
    pub fn bounds_of(&self, references: &[String]) -> Option<([f64; 3], [f64; 3])> {
        bounds(self.points_of(references))
    }

    /// What `reference` is, if it is in the meshes or the sketches.
    pub fn kind(&self, reference: &str) -> Option<Target> {
        Some(match *self.index.get(reference)? {
            Slot::Body(_, k, _) => Target::Body(k),
            Slot::Curve(i, j) => Target::Sketch(self.sketches[i].1.curves[j].ty),
            Slot::Dot(..) => Target::Sketch("point"),
        })
    }

    /// The faces of a body, by index: to paint it whole.
    pub fn faces(&self, key: &BodyKey) -> usize {
        self.bodies
            .iter()
            .find(|b| &b.key == key)
            .and_then(|b| mesh(&b.mesh))
            .map_or(0, |m| m.faces.len())
    }

    /// The box around one body, or one sketch.
    pub fn body_bounds(&self, key: &BodyKey) -> Option<([f64; 3], [f64; 3])> {
        match self.bodies.iter().find(|b| &b.key == key) {
            Some(b) => Some((b.lo, b.hi)),
            None => bounds(self.points(Some(key))),
        }
    }

    /// The box around every body and sketch, if there is any.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let corners = self.bodies.iter().flat_map(|b| [b.lo, b.hi]);
        let sketched = self.sketches.iter().flat_map(|(_, d)| d.points());
        bounds(corners.chain(sketched))
    }

    /// The nearest face along the ray `o + t d`: its `t`, name and body.
    fn face_along(&self, o: [f64; 3], d: [f64; 3]) -> Option<(f64, String, BodyKey)> {
        let mut face: Option<(f64, String, BodyKey)> = None;
        for b in &self.bodies {
            if !ray_box(o, d, b.lo, b.hi) {
                continue;
            }
            let m = mesh(&b.mesh).expect("only meshed bodies");
            for f in &m.faces {
                for t in &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize] {
                    let tri = t.map(|c| at(m, m.corners[c as usize].0));
                    if let Some(t) = ray_triangle(o, d, tri)
                        && face.as_ref().is_none_or(|(best, ..)| t < *best)
                    {
                        face = Some((t, name_hex(&f.name), b.key.clone()));
                    }
                }
            }
        }
        face
    }

    /// Whether a face hides `p` from the eye (or it is behind the eye):
    /// judged along its own sight line, as picking judges points.
    pub fn hidden(&self, pr: &Projector, p: [f64; 3]) -> bool {
        let Some(n) = pr.ndc(p) else {
            return true;
        };
        let (o, d) = pr.ray(n);
        let (_, _, forward) = pr.camera.basis();
        let depth = |q: [f64; 3]| dot(sub(q, pr.camera.eye), forward);
        self.face_along(o, d).is_some_and(|(t, ..)| {
            let f = depth([0, 1, 2].map(|k| o[k] + d[k] * t));
            depth(p) > f + 1e-3 * f.abs().max(pr.camera.distance())
        })
    }

    /// What is under the pixel `px` of a viewport `size` pixels big.
    pub fn pick(&self, pr: &Projector, size: [f64; 2], px: [f64; 2]) -> Option<Hit> {
        let to_ndc = |q: [f64; 2]| [2.0 * q[0] / size[0] - 1.0, 1.0 - 2.0 * q[1] / size[1]];
        let (o, d) = pr.ray(to_ndc(px));
        let (_, _, forward) = pr.camera.basis();
        let screen = |p: [f64; 3]| {
            pr.ndc(p)
                .map(|n| [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]])
        };
        let depth = |p: [f64; 3]| dot(sub(p, pr.camera.eye), forward);
        // The nearest face along the ray.
        let face = self.face_along(o, d);
        // Is `p`, at screen position `s`, in front of whatever face is
        // there? Judged at its own pixel, not the mouse's: beside an edge
        // the mouse is over a face that may slope well away from it.
        let visible = |p: [f64; 3], s: [f64; 2]| {
            let (o, d) = pr.ray(to_ndc(s));
            self.face_along(o, d).is_none_or(|(t, ..)| {
                let f = depth([0, 1, 2].map(|k| o[k] + d[k] * t));
                depth(p) <= f + 1e-3 * f.abs().max(pr.camera.distance())
            })
        };
        // A point within reach of the mouse.
        let point_near = |p: [f64; 3], entity: String, key: &BodyKey, kind: Target| {
            let s = screen(p)?;
            let (dx, dy) = (s[0] - px[0], s[1] - px[1]);
            let dist = (dx * dx + dy * dy).sqrt();
            (dist <= VERTEX_REACH).then(|| {
                let n = Near {
                    px: dist,
                    point: p,
                    entity,
                    key: key.clone(),
                    kind,
                };
                (n, s)
            })
        };
        // A polyline's nearest segment within reach of the mouse.
        let curve_near = |ps: &[[f64; 3]], entity: &str, key: &BodyKey, kind: Target| {
            let mut best: Option<(Near, [f64; 2])> = None;
            for w in ps.windows(2) {
                let (a, c) = (w[0], w[1]);
                let (Some(sa), Some(sc)) = (screen(a), screen(c)) else {
                    continue;
                };
                let (dist, s) = to_segment(px, sa, sc);
                if dist > EDGE_REACH || best.as_ref().is_some_and(|(n, _)| n.px <= dist) {
                    continue;
                }
                // The point along the curve under that pixel. In
                // perspective, a fraction across the screen is not the
                // same fraction along the curve: weigh by depth.
                let t = match pr.camera.projection {
                    ryucad::view::Projection::Perspective { .. } => {
                        let (za, zc) = (depth(a), depth(c));
                        s * za / (s * za + (1.0 - s) * zc)
                    }
                    ryucad::view::Projection::Orthographic { .. } => s,
                };
                let p = [0, 1, 2].map(|k| a[k] + (c[k] - a[k]) * t);
                let at_s = [sa[0] + (sc[0] - sa[0]) * s, sa[1] + (sc[1] - sa[1]) * s];
                let n = Near {
                    px: dist,
                    point: p,
                    entity: entity.to_owned(),
                    key: key.clone(),
                    kind,
                };
                best = Some((n, at_s));
            }
            best
        };
        // Points and curves within reach, nearest first; the first visible
        // one wins.
        let mut points: Vec<(Near, [f64; 2])> = Vec::new();
        let mut curves: Vec<(Near, [f64; 2])> = Vec::new();
        for b in &self.bodies {
            let m = mesh(&b.mesh).expect("only meshed bodies");
            let vertex = Target::Body(EntityKind::Vertex);
            for (name, i) in &m.vertices {
                points.extend(point_near(at(m, *i), name_hex(name), &b.key, vertex));
            }
            let edge = Target::Body(EntityKind::Edge);
            for (name, ps) in &m.edges {
                let ps: Vec<[f64; 3]> = ps.iter().map(|p| at(m, *p)).collect();
                curves.extend(curve_near(&ps, &name_hex(name), &b.key, edge));
            }
        }
        for (key, d) in &self.sketches {
            for p in &d.dots {
                points.extend(point_near(
                    p.at,
                    p.entity.clone(),
                    key,
                    Target::Sketch("point"),
                ));
            }
            for c in &d.curves {
                curves.extend(curve_near(&c.points, &c.entity, key, Target::Sketch(c.ty)));
            }
        }
        let first_visible = |mut v: Vec<(Near, [f64; 2])>| {
            v.sort_by(|a, b| a.0.px.total_cmp(&b.0.px));
            v.into_iter()
                .find(|(n, s)| visible(n.point, *s))
                .map(|(n, _)| n)
        };
        let hit = |n: Near| Hit {
            entity: format!("{}/{}/{}", n.key.0, n.key.1, n.entity),
            kind: n.kind,
            part: n.key.0,
            feature: n.key.1,
            point: n.point,
            rev: None,
        };
        if let Some(p) = first_visible(points) {
            return Some(hit(p));
        }
        if let Some(c) = first_visible(curves) {
            return Some(hit(c));
        }
        // A gone face in the ghost, where the model has nothing nearer.
        if let Some((t, name, key)) = self.ghost_along(o, d)
            && face.as_ref().is_none_or(|(best, ..)| t < *best)
        {
            let point = [0, 1, 2].map(|k| o[k] + d[k] * t);
            let mut h = hit(Near {
                px: 0.0,
                point,
                entity: name,
                key,
                kind: Target::Body(EntityKind::Face),
            });
            h.rev.clone_from(&self.ghost_rev);
            return Some(h);
        }
        let (t, name, key) = face?;
        let point = [0, 1, 2].map(|k| o[k] + d[k] * t);
        Some(hit(Near {
            px: 0.0,
            point,
            entity: name,
            key,
            kind: Target::Body(EntityKind::Face),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryucad::mesh::WireFace;
    use ryucad::view::{self, StandardView};

    /// A unit square face in the plane z = 0 (two triangles), its four
    /// edges, and its corners, as one body of part `p`, feature `f`.
    fn square_model() -> Model {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let corners = (0..4).map(|i| (i, [0.0, 0.0, 1.0])).collect();
        let name = |n: u8| [n; 16];
        let mesh = WireMesh {
            origin: [0.0; 3],
            chord: 0.0,
            angle: 0.0,
            positions,
            corners,
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            faces: vec![WireFace {
                name: name(1),
                corners: (0, 4),
                triangles: (0, 2),
            }],
            edges: (0..4u32)
                .map(|i| (name(10 + i as u8), vec![i, (i + 1) % 4]))
                .collect(),
            vertices: (0..4u32).map(|i| (name(20 + i as u8), i)).collect(),
            faults: Vec::new(),
        };
        let mut model = Model::default();
        let body = WireBody {
            feature: "f".into(),
            hash: "h".into(),
            mesh: Ok(mesh),
        };
        model
            .meshes
            .insert(("p".into(), "f".into()), Arc::new(body));
        model
    }

    #[test]
    fn vertices_beat_edges_beat_faces() {
        let model = square_model();
        let picker = Picker::new(&model);
        assert_eq!(picker.bounds(), Some(([0.0; 3], [1.0, 1.0, 0.0])));
        let camera = view::frame(
            [0.0; 3],
            [1.0, 1.0, 0.0],
            StandardView::Top,
            view::DEFAULT_PROJECTION,
        );
        let pr = Projector::new(&camera, 1.0, [0.5, 0.5, 0.0], 1.0);
        let size = [400.0, 400.0];
        let px = |p: [f64; 3]| {
            let n = pr.ndc(p).unwrap();
            [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]]
        };
        let pick = |p: [f64; 3], nudge: [f64; 2]| {
            let s = px(p);
            picker.pick(&pr, size, [s[0] + nudge[0], s[1] + nudge[1]])
        };
        let hex = |n: u8| name_hex(&[n; 16]);
        // The middle of the square is the face, at the point picked.
        let h = pick([0.5, 0.5, 0.0], [0.0, 0.0]).unwrap();
        assert_eq!(
            (h.kind, h.entity.clone()),
            (Target::Body(EntityKind::Face), format!("p/f/{}", hex(1)))
        );
        assert!(
            h.point
                .iter()
                .zip([0.5, 0.5, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-9)
        );
        // Near the middle of the bottom edge, the edge.
        let h = pick([0.5, 0.0, 0.0], [0.0, -3.0]).unwrap();
        assert_eq!(
            (h.kind, h.entity),
            (Target::Body(EntityKind::Edge), format!("p/f/{}", hex(10)))
        );
        // Near a corner, the vertex, even though an edge is nearer still.
        let h = pick([1.0, 1.0, 0.0], [-4.0, 0.0]).unwrap();
        assert_eq!(
            (h.kind, h.entity),
            (Target::Body(EntityKind::Vertex), format!("p/f/{}", hex(22)))
        );
        // Off the model, nothing.
        assert_eq!(pick([2.0, 2.0, 0.0], [0.0, 0.0]), None);
        // Every entity can be found by reference.
        let key = ("p".to_owned(), "f".to_owned());
        assert_eq!(
            picker.place(&format!("p/f/{}", hex(1))),
            Some(Place::Face(key.clone(), 0))
        );
        assert_eq!(
            picker.place(&format!("p/f/{}", hex(11))),
            Some(Place::Edge(vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]))
        );
        assert_eq!(
            picker.place(&format!("p/f/{}", hex(23))),
            Some(Place::Vertex([0.0, 1.0, 0.0]))
        );
        assert_eq!(
            picker.kind(&format!("p/f/{}", hex(23))),
            Some(Target::Body(EntityKind::Vertex))
        );
        assert_eq!(picker.place("p/f/00"), None);
        let edge = format!("p/f/{}", hex(11));
        assert_eq!(
            picker.bounds_of(&[edge]),
            Some(([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]))
        );
        assert_eq!(picker.bounds_of(&["p/f/00".into()]), None);
        assert_eq!(picker.faces(&key), 1);
        assert_eq!(picker.body_bounds(&key), Some(([0.0; 3], [1.0, 1.0, 0.0])));
    }

    #[test]
    fn sketch_points_and_curves_pick_like_vertices_and_edges() {
        use crate::core::sketch::{Curve, Dot, Drawing};
        use ryucad::sketch_view::EntityState;
        let mut model = square_model();
        // A sketch just above the square: a line across its middle and a
        // point on it; and one below it, hidden.
        let sketch = |z: f64| Drawing {
            revision: "r".into(),
            curves: vec![Curve {
                entity: "l1".into(),
                ty: "line",
                construction: false,
                state: EntityState::Under,
                points: vec![[0.1, 0.5, z], [0.9, 0.5, z]],
            }],
            dots: vec![Dot {
                entity: "p1".into(),
                state: EntityState::Under,
                at: [0.3, 0.5, z],
            }],
            labels: Vec::new(),
        };
        model
            .sketches
            .insert(("p".into(), "s".into()), Arc::new(sketch(1e-4)));
        model
            .sketches
            .insert(("p".into(), "u".into()), Arc::new(sketch(-0.5)));
        let picker = Picker::new(&model);
        let camera = view::frame(
            [0.0, 0.0, -0.5],
            [1.0, 1.0, 0.0],
            StandardView::Top,
            view::DEFAULT_PROJECTION,
        );
        let pr = Projector::new(&camera, 1.0, [0.5, 0.5, -0.25], 1.0);
        let size = [400.0, 400.0];
        let pick = |picker: &Picker, p: [f64; 3], nudge: f64| {
            let n = pr.ndc(p).unwrap();
            let s = [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]];
            picker.pick(&pr, size, [s[0], s[1] + nudge])
        };
        let h = pick(&picker, [0.3, 0.5, 0.0], 2.0).unwrap();
        assert_eq!(
            (h.kind, h.entity.as_str()),
            (Target::Sketch("point"), "p/s/p1")
        );
        let h = pick(&picker, [0.7, 0.5, 0.0], 2.0).unwrap();
        assert_eq!(
            (h.kind, h.entity.as_str()),
            (Target::Sketch("line"), "p/s/l1")
        );
        assert!((h.point[0] - 0.7).abs() < 0.01, "{:?}", h.point);
        // Placed and framed by reference; the hidden sketch is never picked.
        assert_eq!(
            picker.place("p/s/p1"),
            Some(Place::Vertex([0.3, 0.5, 1e-4]))
        );
        assert_eq!(picker.kind("p/u/l1"), Some(Target::Sketch("line")));
        assert_eq!(picker.points(Some(&("p".into(), "s".into()))).len(), 3);
        assert_eq!(picker.bounds().unwrap().0[2], -0.5);
        // Not drawn, not picked.
        let bare = Picker::of(&model, false);
        let h = pick(&bare, [0.3, 0.5, 0.0], 2.0).unwrap();
        assert_eq!(h.kind, Target::Body(EntityKind::Face));
        assert_eq!(bare.place("p/s/p1"), None);
    }

    #[test]
    fn gone_faces_are_picked_in_the_ghost() {
        let model = square_model();
        let key = ("p".to_owned(), "f".to_owned());
        // The square as the revision had it, a little above: its face is
        // gone from the working files, so it is picked over the model's.
        let mut above = model.meshes[&key].as_ref().clone();
        above.hash = "old".into();
        above.mesh.as_mut().unwrap().origin = [0.0, 0.0, 0.25];
        let mut c = Compare::new("HEAD~1");
        c.wanted.insert(key.clone(), "old".into());
        c.take_ghost("p", above);
        let camera = view::frame(
            [0.0; 3],
            [1.0, 1.0, 0.25],
            StandardView::Top,
            view::DEFAULT_PROJECTION,
        );
        let pr = Projector::new(&camera, 1.0, [0.5, 0.5, 0.1], 1.0);
        let size = [400.0, 400.0];
        let middle = |p: &Picker| p.pick(&pr, size, [200.0, 200.0]).unwrap();
        let plain = Picker::new(&model).with_compare(Some(&c));
        assert_eq!(middle(&plain).rev, None);
        c.gone_bodies.insert(key.clone());
        let ghosted = Picker::new(&model).with_compare(Some(&c));
        let h = middle(&ghosted);
        assert_eq!(h.rev.as_deref(), Some("HEAD~1"));
        assert_eq!(h.kind, Target::Body(EntityKind::Face));
        assert!((h.point[2] - 0.25).abs() < 1e-9);
        // Its edges and corners are never picked: the model's are.
        let n = pr.ndc([1.0, 1.0, 0.0]).unwrap();
        let s = [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]];
        let h = ghosted.pick(&pr, size, s).unwrap();
        assert_eq!((h.kind, h.rev), (Target::Body(EntityKind::Vertex), None));
    }

    #[test]
    fn hidden_edges_cant_be_picked() {
        let mut model = square_model();
        // A second square, a little below the first: its edges are hidden
        // behind the first square's face from above.
        let mut lower = model.meshes[&("p".to_owned(), "f".to_owned())]
            .as_ref()
            .clone();
        let m = lower.mesh.as_mut().unwrap();
        m.origin = [0.0, 0.0, -0.5];
        m.positions = vec![
            [0.2, 0.2, 0.0],
            [0.8, 0.2, 0.0],
            [0.8, 0.8, 0.0],
            [0.2, 0.8, 0.0],
        ];
        lower.feature = "g".into();
        model
            .meshes
            .insert(("p".into(), "g".into()), Arc::new(lower));
        let picker = Picker::new(&model);
        let camera = view::frame(
            [0.0, 0.0, -0.5],
            [1.0, 1.0, 0.0],
            StandardView::Top,
            view::DEFAULT_PROJECTION,
        );
        let pr = Projector::new(&camera, 1.0, [0.5, 0.5, -0.25], 1.0);
        let size = [400.0, 400.0];
        let n = pr.ndc([0.5, 0.2, -0.5]).unwrap();
        let s = [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]];
        let h = picker.pick(&pr, size, s).unwrap();
        assert_eq!(
            (h.kind, h.feature.as_str()),
            (Target::Body(EntityKind::Face), "f")
        );
    }
}
