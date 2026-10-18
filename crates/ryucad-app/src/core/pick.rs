//! Picking: which face, edge or vertex is under the mouse. Faces are hit
//! by a ray cast against their triangles; edges and vertices by their
//! distance on screen, so a thin edge can be picked without pixel-perfect
//! aim. Within reach, a vertex wins over an edge and an edge over a face,
//! and nothing hidden behind a face is picked.

use crate::core::model::{BodyKey, Model};
use crate::core::nav::Projector;
use ryucad::entity::EntityKind;
use ryucad::mesh::{WireBody, WireMesh, name_hex};
use std::collections::HashMap;
use std::sync::Arc;

/// How close, in pixels, the mouse must be to an edge or vertex.
pub const EDGE_REACH: f64 = 5.0;
pub const VERTEX_REACH: f64 = 7.0;

/// What is under the mouse.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The entity reference: `<part>/<feature>/<name>`.
    pub entity: String,
    pub kind: EntityKind,
    pub part: String,
    pub feature: String,
    /// The point picked, in meters: where an orbit turns and a zoom aims.
    pub point: [f64; 3],
}

/// A body ready for picking: its mesh and the box around it.
struct Body {
    key: BodyKey,
    mesh: Arc<WireBody>,
    lo: [f64; 3],
    hi: [f64; 3],
}

/// The model's meshes prepared for picking. Rebuild it when the model's
/// generation changes.
pub struct Picker {
    pub generation: u64,
    bodies: Vec<Body>,
    /// Every entity reference: its body (an index into `bodies`), its kind
    /// and its index in the body's mesh.
    index: HashMap<String, (usize, EntityKind, usize)>,
}

/// Where an entity is drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// A face: its body and index among the mesh's faces.
    Face(BodyKey, usize),
    /// An edge's polyline.
    Edge(Vec<[f64; 3]>),
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

/// A candidate edge or vertex: how far from the mouse on screen, and its
/// point.
struct Near {
    px: f64,
    point: [f64; 3],
    entity: String,
    key: BodyKey,
}

impl Picker {
    pub fn new(model: &Model) -> Picker {
        let bodies: Vec<Body> = model
            .meshes
            .iter()
            .filter_map(|(key, b)| {
                let m = mesh(b)?;
                let mut lo = [f64::INFINITY; 3];
                let mut hi = [f64::NEG_INFINITY; 3];
                for i in 0..m.positions.len() {
                    let p = at(m, i as u32);
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
                Some(Body {
                    key: key.clone(),
                    mesh: Arc::clone(b),
                    lo,
                    hi,
                })
            })
            .collect();
        let mut index = HashMap::new();
        for (i, b) in bodies.iter().enumerate() {
            let m = mesh(&b.mesh).expect("only meshed bodies");
            let named = |n: &[u8; 16]| format!("{}/{}/{}", b.key.0, b.key.1, name_hex(n));
            for (j, f) in m.faces.iter().enumerate() {
                index.insert(named(&f.name), (i, EntityKind::Face, j));
            }
            for (j, (n, _)) in m.edges.iter().enumerate() {
                index.insert(named(n), (i, EntityKind::Edge, j));
            }
            for (j, (n, _)) in m.vertices.iter().enumerate() {
                index.insert(named(n), (i, EntityKind::Vertex, j));
            }
        }
        Picker {
            generation: model.generation,
            bodies,
            index,
        }
    }

    /// Where the entity `reference` is drawn, if it is in the meshes.
    pub fn place(&self, reference: &str) -> Option<Place> {
        let (i, kind, j) = *self.index.get(reference)?;
        let b = &self.bodies[i];
        let m = mesh(&b.mesh)?;
        Some(match kind {
            EntityKind::Face => Place::Face(b.key.clone(), j),
            EntityKind::Edge => Place::Edge(m.edges[j].1.iter().map(|p| at(m, *p)).collect()),
            EntityKind::Vertex => Place::Vertex(at(m, m.vertices[j].1)),
        })
    }

    /// The box around the given entities, those in the meshes.
    pub fn bounds_of(&self, references: &[String]) -> Option<([f64; 3], [f64; 3])> {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        let mut grow = |p: [f64; 3]| {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        };
        for r in references {
            let Some(&(i, kind, j)) = self.index.get(r) else {
                continue;
            };
            let m = mesh(&self.bodies[i].mesh).expect("only meshed bodies");
            match kind {
                EntityKind::Face => {
                    let f = &m.faces[j];
                    for c in &m.corners[f.corners.0 as usize..f.corners.1 as usize] {
                        grow(at(m, c.0));
                    }
                }
                EntityKind::Edge => m.edges[j].1.iter().for_each(|p| grow(at(m, *p))),
                EntityKind::Vertex => grow(at(m, m.vertices[j].1)),
            }
        }
        (lo[0] <= hi[0]).then_some((lo, hi))
    }

    /// The kind of entity `reference` is, if it is in the meshes.
    pub fn kind(&self, reference: &str) -> Option<EntityKind> {
        self.index.get(reference).map(|e| e.1)
    }

    /// The faces of a body, by index: to paint it whole.
    pub fn faces(&self, key: &BodyKey) -> usize {
        self.bodies
            .iter()
            .find(|b| &b.key == key)
            .and_then(|b| mesh(&b.mesh))
            .map_or(0, |m| m.faces.len())
    }

    /// The box around one body.
    pub fn body_bounds(&self, key: &BodyKey) -> Option<([f64; 3], [f64; 3])> {
        self.bodies
            .iter()
            .find(|b| &b.key == key)
            .map(|b| (b.lo, b.hi))
    }

    /// The box around every body, if there is any.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for b in &self.bodies {
            for k in 0..3 {
                lo[k] = lo[k].min(b.lo[k]);
                hi[k] = hi[k].max(b.hi[k]);
            }
        }
        (lo[0] <= hi[0]).then_some((lo, hi))
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
        // Vertices and edges within reach, nearest first; the first visible
        // one wins.
        let mut vertices: Vec<(Near, [f64; 2])> = Vec::new();
        let mut edges: Vec<(Near, [f64; 2])> = Vec::new();
        for b in &self.bodies {
            let m = mesh(&b.mesh).expect("only meshed bodies");
            for (name, i) in &m.vertices {
                let p = at(m, *i);
                let Some(s) = screen(p) else { continue };
                let (dx, dy) = (s[0] - px[0], s[1] - px[1]);
                let dist = (dx * dx + dy * dy).sqrt();
                if dist <= VERTEX_REACH {
                    let n = Near {
                        px: dist,
                        point: p,
                        entity: name_hex(name),
                        key: b.key.clone(),
                    };
                    vertices.push((n, s));
                }
            }
            for (name, ps) in &m.edges {
                // This edge's nearest segment within reach.
                let mut best: Option<(Near, [f64; 2])> = None;
                for w in ps.windows(2) {
                    let (a, c) = (at(m, w[0]), at(m, w[1]));
                    let (Some(sa), Some(sc)) = (screen(a), screen(c)) else {
                        continue;
                    };
                    let (dist, s) = to_segment(px, sa, sc);
                    if dist > EDGE_REACH || best.as_ref().is_some_and(|(n, _)| n.px <= dist) {
                        continue;
                    }
                    // The point along the edge under that pixel. In
                    // perspective, a fraction across the screen is not the
                    // same fraction along the edge: weigh by depth.
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
                        entity: name_hex(name),
                        key: b.key.clone(),
                    };
                    best = Some((n, at_s));
                }
                edges.extend(best);
            }
        }
        let first_visible = |mut v: Vec<(Near, [f64; 2])>| {
            v.sort_by(|a, b| a.0.px.total_cmp(&b.0.px));
            v.into_iter()
                .find(|(n, s)| visible(n.point, *s))
                .map(|(n, _)| n)
        };
        let vertex = first_visible(vertices);
        let edge = first_visible(edges);
        let hit = |kind, n: Near| Hit {
            entity: format!("{}/{}/{}", n.key.0, n.key.1, n.entity),
            kind,
            part: n.key.0,
            feature: n.key.1,
            point: n.point,
        };
        if let Some(v) = vertex {
            return Some(hit(EntityKind::Vertex, v));
        }
        if let Some(e) = edge {
            return Some(hit(EntityKind::Edge, e));
        }
        let (t, name, key) = face?;
        let point = [0, 1, 2].map(|k| o[k] + d[k] * t);
        Some(hit(
            EntityKind::Face,
            Near {
                px: 0.0,
                point,
                entity: name,
                key,
            },
        ))
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
            (EntityKind::Face, format!("p/f/{}", hex(1)))
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
            (EntityKind::Edge, format!("p/f/{}", hex(10)))
        );
        // Near a corner, the vertex, even though an edge is nearer still.
        let h = pick([1.0, 1.0, 0.0], [-4.0, 0.0]).unwrap();
        assert_eq!(
            (h.kind, h.entity),
            (EntityKind::Vertex, format!("p/f/{}", hex(22)))
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
            Some(EntityKind::Vertex)
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
        assert_eq!((h.kind, h.feature.as_str()), (EntityKind::Face, "f"));
    }
}
