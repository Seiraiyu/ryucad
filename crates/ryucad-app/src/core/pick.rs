//! Picking: which face, edge or vertex, or sketch point or curve, is
//! under the mouse. Faces are hit by a ray cast against their triangles,
//! through a box tree of the occurrences drawn and then each one's mesh
//! in its own frame;
//! edges, vertices and sketch entities by their distance on screen, so a
//! thin edge can be picked without pixel-perfect aim. Within reach, a
//! vertex or sketch point wins over an edge or sketch curve, and those
//! over a face; nothing hidden behind a face is picked. In a compare, a
//! face the working files no longer have is picked in the ghost, where
//! nothing of the model is nearer.

use crate::core::compare::Compare;
use crate::core::model::{BodyKey, Model, Pose};
use crate::core::nav::{Projector, Sight};
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

/// A body ready for picking: its mesh in its part's frame, where it is,
/// and the box around it there.
struct Body {
    key: BodyKey,
    /// The same key, for what a frame draws to share.
    shared: Arc<BodyKey>,
    mesh: Arc<WireBody>,
    /// The mesh the window fetched first ([`Model::fetch_level`]): a
    /// large view draws it far off.
    first: Arc<WireBody>,
    pose: Pose,
    lo: [f64; 3],
    hi: [f64; 3],
}

/// The box around a mesh in its part's frame.
fn local_box(m: &WireMesh) -> ([f64; 3], [f64; 3]) {
    bounds((0..m.positions.len()).map(|i| at(m, i as u32))).unwrap_or(([0.0; 3], [0.0; 3]))
}

impl Body {
    /// `mesh` at `pose`, with the box around it.
    fn new(key: BodyKey, mesh: Arc<WireBody>, pose: Pose) -> Option<Body> {
        let local = local_box(mesh_of(&mesh)?);
        let first = Arc::clone(&mesh);
        Body::placed(key, mesh, first, pose, local)
    }

    /// [`Body::new`], its mesh's own box `local` worked out already: the
    /// occurrences of one body share it.
    fn placed(
        key: BodyKey,
        mesh: Arc<WireBody>,
        first: Arc<WireBody>,
        pose: Pose,
        (lo, hi): ([f64; 3], [f64; 3]),
    ) -> Option<Body> {
        mesh_of(&mesh)?;
        // The box around the local box's corners, where they are.
        let corners = (0..8)
            .map(|c| pose.apply([0, 1, 2].map(|k| if c >> k & 1 == 0 { lo[k] } else { hi[k] })));
        let (lo, hi) = bounds(corners).expect("eight corners");
        Some(Body {
            shared: Arc::new(key.clone()),
            key,
            mesh,
            first,
            pose,
            lo,
            hi,
        })
    }

    fn mesh(&self) -> &WireMesh {
        mesh_of(&self.mesh).expect("only meshed bodies")
    }

    /// Position `i` of the mesh, where the body is.
    fn at(&self, i: u32) -> [f64; 3] {
        self.pose.apply(at(self.mesh(), i))
    }
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

/// A box tree over bodies: each node holds the box around its bodies,
/// split at the middle body along its box's longest side, down to a few
/// bodies a leaf. A ray visits only the bodies whose boxes it meets.
#[derive(Debug, Default)]
struct Tree {
    nodes: Vec<Node>,
}

#[derive(Debug)]
struct Node {
    lo: [f64; 3],
    hi: [f64; 3],
    /// The bodies of a leaf, or the two children of a split.
    kind: NodeKind,
}

#[derive(Debug)]
enum NodeKind {
    Leaf(Vec<usize>),
    Split(usize, usize),
}

/// Bodies per leaf at most.
const LEAF: usize = 4;

impl Tree {
    /// The tree over boxes `boxes`, by their indices.
    fn new(boxes: &[([f64; 3], [f64; 3])]) -> Tree {
        let mut t = Tree::default();
        if !boxes.is_empty() {
            t.build(boxes, (0..boxes.len()).collect());
        }
        t
    }

    /// Adds the node over `ids`; returns its index.
    fn build(&mut self, boxes: &[([f64; 3], [f64; 3])], mut ids: Vec<usize>) -> usize {
        let (lo, hi) = bounds(ids.iter().flat_map(|&i| [boxes[i].0, boxes[i].1]))
            .expect("a node holds a body");
        let at = self.nodes.len();
        if ids.len() <= LEAF {
            self.nodes.push(Node {
                lo,
                hi,
                kind: NodeKind::Leaf(ids),
            });
            return at;
        }
        let axis = (0..3)
            .max_by(|a, b| (hi[*a] - lo[*a]).total_cmp(&(hi[*b] - lo[*b])))
            .expect("three axes");
        let mid = |i: &usize| boxes[*i].0[axis] + boxes[*i].1[axis];
        ids.sort_by(|a, b| mid(a).total_cmp(&mid(b)).then(a.cmp(b)));
        let right = ids.split_off(ids.len() / 2);
        self.nodes.push(Node {
            lo,
            hi,
            kind: NodeKind::Leaf(Vec::new()),
        });
        let l = self.build(boxes, ids);
        let r = self.build(boxes, right);
        self.nodes[at].kind = NodeKind::Split(l, r);
        at
    }

    /// The bodies whose boxes may be in view, in index order: a node's
    /// whole subtree left out when its box is out of view, and taken
    /// without judging its bodies when its box is all in view.
    fn seen(
        &self,
        boxes: impl Fn(usize) -> ([f64; 3], [f64; 3]),
        sight: impl Fn([f64; 3], [f64; 3]) -> Sight,
    ) -> Vec<usize> {
        let mut out = Vec::new();
        // Nodes, and whether their boxes are known to be all in view.
        let mut stack = if self.nodes.is_empty() {
            vec![]
        } else {
            vec![(0, false)]
        };
        while let Some((n, all)) = stack.pop() {
            let node = &self.nodes[n];
            let all = all
                || match sight(node.lo, node.hi) {
                    Sight::Out => continue,
                    Sight::Partly => false,
                    Sight::In => true,
                };
            match &node.kind {
                NodeKind::Leaf(ids) if all => out.extend(ids),
                NodeKind::Leaf(ids) => out.extend(ids.iter().filter(|&&i| {
                    let (lo, hi) = boxes(i);
                    sight(lo, hi) != Sight::Out
                })),
                NodeKind::Split(l, r) => stack.extend([(*r, all), (*l, all)]),
            }
        }
        out.sort_unstable();
        out
    }

    /// The bodies of every leaf whose box the ray `o + t d` (`t >= 0`)
    /// meets, each once: every body whose own box it meets is among them.
    fn along(&self, o: [f64; 3], d: [f64; 3], mut visit: impl FnMut(usize)) {
        let mut stack = if self.nodes.is_empty() {
            vec![]
        } else {
            vec![0]
        };
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if !ray_box(o, d, node.lo, node.hi) {
                continue;
            }
            match &node.kind {
                NodeKind::Leaf(ids) => ids.iter().for_each(|&i| visit(i)),
                NodeKind::Split(l, r) => stack.extend([*r, *l]),
            }
        }
    }
}

/// The model's meshes and sketches prepared for picking. Rebuild it when
/// the model's generation changes.
pub struct Picker {
    pub generation: u64,
    bodies: Vec<Body>,
    /// A box tree over `bodies`.
    tree: Tree,
    sketches: Vec<(BodyKey, Arc<Drawing>)>,
    /// Each body's place in `bodies`, by key: a body's entities are found
    /// in its own mesh ([`Picker::slot`]), since naming every entity of
    /// 100,000 occurrences took seconds.
    by_key: HashMap<BodyKey, usize>,
    /// Every sketch entity's reference, and where it is.
    index: HashMap<String, Slot>,
    /// A compare's ghosts, each with the indices of its faces the working
    /// files no longer have: only those are picked.
    ghosts: Vec<(Body, Vec<usize>)>,
    /// The revision the ghosts are of.
    ghost_rev: Option<String>,
}

/// A body in view ([`Picker::seen`]): its key, the mesh picking reads
/// and the one fetched first, where it is and the box around it there.
pub struct Seen<'a> {
    pub key: &'a Arc<BodyKey>,
    pub mesh: &'a Arc<WireBody>,
    pub first: &'a Arc<WireBody>,
    pub pose: Pose,
    pub lo: [f64; 3],
    pub hi: [f64; 3],
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

fn mesh_of(b: &WireBody) -> Option<&WireMesh> {
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
        Picker::of_bodies(model, model.drawn(), sketches)
    }

    /// What the window hides of the parts it shows, to pick what is
    /// hidden under the mouse (Shift+Tab shows it again).
    pub fn of_hidden(model: &Model) -> Picker {
        Picker::of_bodies(model, model.hidden_drawn(), false)
    }

    fn of_bodies<'a>(
        model: &'a Model,
        drawn: impl Iterator<Item = (&'a BodyKey, &'a Arc<WireBody>, Pose)>,
        sketches: bool,
    ) -> Picker {
        // Each mesh's own box once, however many occurrences share it.
        let mut boxes: HashMap<*const WireBody, ([f64; 3], [f64; 3])> = HashMap::new();
        let bodies: Vec<Body> = drawn
            .filter_map(|(key, b, pose)| {
                let local = match boxes.get(&Arc::as_ptr(b)) {
                    Some(l) => *l,
                    None => {
                        let l = local_box(mesh_of(b)?);
                        boxes.insert(Arc::as_ptr(b), l);
                        l
                    }
                };
                let first = model.meshes.get(key).unwrap_or(b);
                Body::placed(key.clone(), Arc::clone(b), Arc::clone(first), pose, local)
            })
            .collect();
        drop(boxes);
        let sketches: Vec<(BodyKey, Arc<Drawing>)> = if sketches {
            model
                .drawn_sketches()
                .map(|(k, d)| (k.clone(), Arc::clone(d)))
                .collect()
        } else {
            Vec::new()
        };
        let by_key = bodies
            .iter()
            .enumerate()
            .map(|(i, b)| (b.key.clone(), i))
            .collect();
        let mut index = HashMap::new();
        for (i, (key, d)) in sketches.iter().enumerate() {
            let named = |e: &str| format!("{}/{}/{e}", key.0, key.1);
            for (j, c) in d.curves.iter().enumerate() {
                index.insert(named(&c.entity), Slot::Curve(i, j));
            }
            for (j, p) in d.dots.iter().enumerate() {
                index.insert(named(&p.entity), Slot::Dot(i, j));
            }
        }
        let boxes: Vec<([f64; 3], [f64; 3])> = bodies.iter().map(|b| (b.lo, b.hi)).collect();
        Picker {
            generation: model.generation,
            tree: Tree::new(&boxes),
            bodies,
            sketches,
            by_key,
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
            let Some(m) = mesh_of(b) else { continue };
            let gone: Vec<usize> = (0..m.faces.len())
                .filter(|i| c.is_gone(key, &name_hex(&m.faces[*i].name)))
                .collect();
            if gone.is_empty() {
                continue;
            }
            let body = Body::new(key.clone(), Arc::clone(b), c.ghost_pose(key)).expect("meshed");
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
            let m = b.mesh();
            for f in gone.iter().map(|i| &m.faces[*i]) {
                for t in &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize] {
                    let tri = t.map(|c| b.at(m.corners[c as usize].0));
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
                let b = &self.bodies[i];
                let m = b.mesh();
                match kind {
                    EntityKind::Face => {
                        let f = &m.faces[j];
                        m.corners[f.corners.0 as usize..f.corners.1 as usize]
                            .iter()
                            .map(|c| b.at(c.0))
                            .collect()
                    }
                    EntityKind::Edge => m.edges[j].1.iter().map(|p| b.at(*p)).collect(),
                    EntityKind::Vertex => vec![b.at(m.vertices[j].1)],
                }
            }
            Slot::Curve(i, j) => self.sketches[i].1.curves[j].points.clone(),
            Slot::Dot(i, j) => vec![self.sketches[i].1.dots[j].at],
        }
    }

    /// Where `reference` is: a sketch entity by its reference, else a
    /// body's face, edge or vertex, found by name in the mesh of the body
    /// the rest of the reference names (`<part>/<solid>/<name>`, or an
    /// occurrence's `<asm>/<path>/<solid>/<name>`).
    fn slot(&self, reference: &str) -> Option<Slot> {
        if let Some(s) = self.index.get(reference) {
            return Some(*s);
        }
        let (key, name) = reference.rsplit_once('/')?;
        let (part, solid) = key.split_once('/')?;
        if name.len() != 32 {
            return None;
        }
        let name = u128::from_str_radix(name, 16).ok()?.to_be_bytes();
        let i = *self.by_key.get(&(part.to_owned(), solid.to_owned()))?;
        let m = self.bodies[i].mesh();
        let found = |kind, j: Option<usize>| j.map(|j| Slot::Body(i, kind, j));
        found(
            EntityKind::Face,
            m.faces.iter().position(|f| f.name == name),
        )
        .or_else(|| {
            found(
                EntityKind::Edge,
                m.edges.iter().position(|(n, _)| *n == name),
            )
        })
        .or_else(|| {
            found(
                EntityKind::Vertex,
                m.vertices.iter().position(|(n, _)| *n == name),
            )
        })
    }

    /// Where the entity `reference` is drawn, if it is in the meshes or
    /// the sketches.
    pub fn place(&self, reference: &str) -> Option<Place> {
        let slot = self.slot(reference)?;
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
            .filter_map(|r| self.slot(r))
            .flat_map(|s| self.slot_points(s))
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
            out.extend((0..b.mesh().positions.len()).map(|i| b.at(i as u32)));
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
        Some(match self.slot(reference)? {
            Slot::Body(_, k, _) => Target::Body(k),
            Slot::Curve(i, j) => Target::Sketch(self.sketches[i].1.curves[j].ty),
            Slot::Dot(..) => Target::Sketch("point"),
        })
    }

    /// The faces of a body, by index: to paint it whole.
    pub fn faces(&self, key: &BodyKey) -> usize {
        self.by_key
            .get(key)
            .map_or(0, |i| self.bodies[*i].mesh().faces.len())
    }

    /// The box around one body, or one sketch.
    pub fn body_bounds(&self, key: &BodyKey) -> Option<([f64; 3], [f64; 3])> {
        match self.by_key.get(key).map(|i| &self.bodies[*i]) {
            Some(b) => Some((b.lo, b.hi)),
            None => bounds(self.points(Some(key))),
        }
    }

    /// Every body whose box `sees` keeps, culled through the box tree, in
    /// the order the model draws them: what levels of detail choose from,
    /// each with what they need, so a frame of 100,000 occurrences looks
    /// nothing up.
    pub fn seen(&self, sight: impl Fn([f64; 3], [f64; 3]) -> Sight) -> Vec<Seen<'_>> {
        self.tree
            .seen(|i| (self.bodies[i].lo, self.bodies[i].hi), sight)
            .into_iter()
            .map(|i| {
                let b = &self.bodies[i];
                Seen {
                    key: &b.shared,
                    mesh: &b.mesh,
                    first: &b.first,
                    pose: b.pose,
                    lo: b.lo,
                    hi: b.hi,
                }
            })
            .collect()
    }

    /// The box around every body and sketch, if there is any.
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let corners = self.bodies.iter().flat_map(|b| [b.lo, b.hi]);
        let sketched = self.sketches.iter().flat_map(|(_, d)| d.points());
        bounds(corners.chain(sketched))
    }

    /// Every body a face of which is under the pixel `px` of a viewport
    /// `size` pixels big, in body order: what Shift+Tab shows again.
    pub fn bodies_under(&self, pr: &Projector, size: [f64; 2], px: [f64; 2]) -> Vec<BodyKey> {
        let ndc = [2.0 * px[0] / size[0] - 1.0, 1.0 - 2.0 * px[1] / size[1]];
        let (o, d) = pr.ray(ndc);
        let mut hit = Vec::new();
        self.tree.along(o, d, |i| {
            let b = &self.bodies[i];
            let (o, d) = (b.pose.unapply(o), b.pose.unturn(d));
            let m = b.mesh();
            let any = m.triangles.iter().any(|t| {
                let tri = t.map(|c| at(m, m.corners[c as usize].0));
                ray_triangle(o, d, tri).is_some()
            });
            if any {
                hit.push(i);
            }
        });
        hit.sort_unstable();
        hit.into_iter()
            .map(|i| self.bodies[i].key.clone())
            .collect()
    }

    /// The nearest face along the ray `o + t d`: its `t`, name and body.
    /// Each body is hit in its own frame, the ray taken there: a rigid
    /// move keeps `t`.
    fn face_along(&self, o: [f64; 3], d: [f64; 3]) -> Option<(f64, String, BodyKey)> {
        let mut face: Option<(f64, String, BodyKey, usize)> = None;
        self.tree.along(o, d, |i| {
            let b = &self.bodies[i];
            if !ray_box(o, d, b.lo, b.hi) {
                return;
            }
            let (o, d) = (b.pose.unapply(o), b.pose.unturn(d));
            let m = b.mesh();
            for f in &m.faces {
                for t in &m.triangles[f.triangles.0 as usize..f.triangles.1 as usize] {
                    let tri = t.map(|c| at(m, m.corners[c as usize].0));
                    // The nearest, and among equals the first body: the
                    // tree's order mustn't choose.
                    if let Some(t) = ray_triangle(o, d, tri)
                        && face
                            .as_ref()
                            .is_none_or(|(best, .., j)| t < *best || (t == *best && i < *j))
                    {
                        face = Some((t, name_hex(&f.name), b.key.clone(), i));
                    }
                }
            }
        });
        face.map(|(t, n, k, _)| (t, n, k))
    }

    /// Whether the mouse at `px` is within reach of what body `b` may
    /// show on screen: its box's corners, projected, grown by the reach.
    /// A box partly behind the eye is always.
    fn within_reach(b: &Body, screen: impl Fn([f64; 3]) -> Option<[f64; 2]>, px: [f64; 2]) -> bool {
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for c in 0..8 {
            let p = [0, 1, 2].map(|k| if c >> k & 1 == 0 { b.lo[k] } else { b.hi[k] });
            let Some(s) = screen(p) else {
                return true;
            };
            for k in 0..2 {
                lo[k] = lo[k].min(s[k]);
                hi[k] = hi[k].max(s[k]);
            }
        }
        let r = VERTEX_REACH.max(EDGE_REACH);
        (0..2).all(|k| px[k] >= lo[k] - r && px[k] <= hi[k] + r)
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
            if !Picker::within_reach(b, screen, px) {
                continue;
            }
            let m = b.mesh();
            let vertex = Target::Body(EntityKind::Vertex);
            for (name, i) in &m.vertices {
                points.extend(point_near(b.at(*i), name_hex(name), &b.key, vertex));
            }
            let edge = Target::Body(EntityKind::Edge);
            for (name, ps) in &m.edges {
                let ps: Vec<[f64; 3]> = ps.iter().map(|p| b.at(*p)).collect();
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

    /// A 1 cm cube at the origin: six faces of two triangles.
    fn cube_mesh() -> WireMesh {
        let p = |i: u32| [0, 1, 2].map(|k| if i >> k & 1 == 1 { 0.01f32 } else { 0.0 });
        let faces: [([f32; 3], [u32; 4]); 6] = [
            ([-1.0, 0.0, 0.0], [0, 4, 6, 2]),
            ([1.0, 0.0, 0.0], [1, 3, 7, 5]),
            ([0.0, -1.0, 0.0], [0, 1, 5, 4]),
            ([0.0, 1.0, 0.0], [2, 6, 7, 3]),
            ([0.0, 0.0, -1.0], [0, 2, 3, 1]),
            ([0.0, 0.0, 1.0], [4, 5, 7, 6]),
        ];
        let mut m = WireMesh {
            origin: [0.0; 3],
            chord: 0.0,
            angle: 0.0,
            positions: (0..8).map(p).collect(),
            corners: Vec::new(),
            triangles: Vec::new(),
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
            faults: Vec::new(),
        };
        for (i, (n, c)) in faces.iter().enumerate() {
            let (base, t) = (m.corners.len() as u32, m.triangles.len() as u32);
            m.corners.extend(c.iter().map(|&k| (k, *n)));
            m.triangles.push([base, base + 1, base + 2]);
            m.triangles.push([base, base + 2, base + 3]);
            m.faces.push(WireFace {
                name: [i as u8; 16],
                corners: (base, base + 4),
                triangles: (t, t + 2),
            });
        }
        m
    }

    #[test]
    fn what_is_hidden_under_the_mouse_is_found() {
        use serde_json::json;
        let occurrences: Vec<serde_json::Value> = [0.0, 0.04]
            .iter()
            .enumerate()
            .map(|(i, x)| {
                json!({"path": format!("c{i}"), "part": "cube",
                       "solids": [{"solid": "s", "hash": "h"}],
                       "pose": {"origin_m": [x, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
            })
            .collect();
        let mut model = Model::default();
        let want = model.apply_part(&json!({"part": "row", "revision": "r", "kind": "assembly",
            "features": [], "solids": [], "occurrences": occurrences}));
        model.ask(want);
        let body = WireBody {
            feature: "c0/s".into(),
            hash: "h".into(),
            mesh: Ok(cube_mesh()),
        };
        assert!(model.take_mesh("row", body));
        let camera = view::frame(
            [0.0; 3],
            [0.05, 0.01, 0.01],
            StandardView::Top,
            view::DEFAULT_PROJECTION,
        );
        let pr = Projector::new(&camera, 1.0, [0.025, 0.005, 0.005], 0.03);
        let size = [400.0, 400.0];
        let n = pr.ndc([0.045, 0.005, 0.01]).unwrap();
        let px = [(n[0] + 1.0) / 2.0 * size[0], (1.0 - n[1]) / 2.0 * size[1]];
        assert_eq!(Picker::new(&model).bodies_under(&pr, size, px).len(), 1);
        model.hidden.insert("row/c1".into());
        assert_eq!(
            Picker::new(&model).bodies_under(&pr, size, px),
            Vec::<BodyKey>::new()
        );
        let under = Picker::of_hidden(&model).bodies_under(&pr, size, px);
        assert_eq!(under, [("row".to_owned(), "c1/s".to_owned())]);
    }

    #[test]
    fn the_tree_visits_what_every_box_test_would() {
        // A 20 × 20 × 3 grid of unit boxes, every other one along x.
        let boxes: Vec<([f64; 3], [f64; 3])> = (0..1200)
            .map(|i| {
                let p = [
                    f64::from(i % 20) * 2.0,
                    f64::from(i / 20 % 20),
                    f64::from(i / 400),
                ];
                (p, [p[0] + 1.0, p[1] + 1.0, p[2] + 1.0])
            })
            .collect();
        let tree = Tree::new(&boxes);
        // A fixed sequence (Knuth's MMIX constants).
        let mut state = 7u64;
        let mut r = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..200 {
            let o = [r() * 50.0 - 5.0, r() * 30.0 - 5.0, r() * 10.0 - 3.0];
            let d = [r() - 0.5, r() - 0.5, r() - 0.5];
            let mut seen = Vec::new();
            tree.along(o, d, |i| seen.push(i));
            seen.sort_unstable();
            let visited = seen.len();
            seen.dedup();
            assert_eq!(seen.len(), visited, "each once");
            seen.retain(|i| ray_box(o, d, boxes[*i].0, boxes[*i].1));
            let all: Vec<usize> = (0..boxes.len())
                .filter(|i| ray_box(o, d, boxes[*i].0, boxes[*i].1))
                .collect();
            assert_eq!(seen, all);
            assert!(
                visited <= all.len() + 4 * LEAF * 40,
                "{visited} for {}",
                all.len()
            );
        }
        assert!(Tree::new(&[]).nodes.is_empty());
    }
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
        // The part is in the model, so the window shows it.
        model.parts.insert(
            "p".into(),
            crate::core::model::Part::from_report(&serde_json::json!({"part": "p"})),
        );
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
