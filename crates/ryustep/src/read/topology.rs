//! A B-rep item as a [`heal::Shape`]: plain data, as the file says it.
//!
//! Shared instances stay shared: a `VERTEX_POINT`, an `EDGE_CURVE` or a
//! surface the file uses twice is one entry of the shape. An edge runs
//! along its ryukei curve's parameter, so an `EDGE_CURVE` whose sense
//! disagrees with its curve's has its vertices swapped, and each use's
//! sense says how the loop runs it: with the curve when the oriented
//! edge's orientation agrees with the edge's sense. A bound with
//! orientation false, an oriented face against its face and a shell used
//! against its orientation each run their loops the other way.
//!
//! An edge's interval comes from its vertices, projected onto its curve:
//! a closed edge on a periodic curve is one period from its vertex, on a
//! clamped curve whose ends meet its whole domain. A parabola or a
//! hyperbola is the exact rational arc between its edge's vertices.
//! A `POLY_LOOP` is line edges between its points, shared by the faces
//! that give the same two points.
//!
//! A `SURFACE_CURVE`'s pcurve on a face's surface goes with the use as
//! the file's pcurve, scaled from the file's units to the surface's
//! parameters where that is simple (a plane's lengths, a cylinder's,
//! cone's, sphere's or torus's angles, a spline's own); healing tries it
//! after the exact one.

use super::geometry::{Geo, invalid};
use crate::ImportError;
use crate::schema::{Brep, Curve as SCurve, Entity, Face as SFace, Id, Loop as SLoop};
use ryukei::geom::{Curve, KnotVector, NurbsCurve, Point3, Surface};
use ryukei::heal::{Edge, Face, Loop, Shape, Shell, Solid, Use, Vertex};
use ryukei::topo::Sense;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How deep oriented shells and faces may wrap each other before it is
/// taken for a cycle.
const DEPTH: usize = 64;

/// The instance each vertex, edge and face of a shape came from, by
/// index: a `VERTEX_POINT` (a poly loop's `CARTESIAN_POINT`), an
/// `EDGE_CURVE` (the `POLY_LOOP` a line edge was first made for) and a
/// face.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Sources {
    pub(crate) vertices: Vec<Id>,
    pub(crate) edges: Vec<Id>,
    pub(crate) faces: Vec<Id>,
    /// The B-rep item each face (by index) belongs to.
    pub(crate) face_items: Vec<Id>,
    /// Whether a face was split at a revolution's axis ([`Split`]): the
    /// body such a split makes crosses itself where the file's sweep
    /// folds, and is united with itself after healing.
    pub(crate) split: bool,
}

/// Builds one item's shape, keeping each instance's entry.
struct Builder<'a, 'g> {
    g: &'a Geo<'g>,
    s: Shape,
    sources: Sources,
    vertices: BTreeMap<Id, usize>,
    edges: BTreeMap<Id, usize>,
    surfaces: BTreeMap<Id, usize>,
    /// Poly loops' points and the line edges between them.
    points: BTreeMap<Id, usize>,
    lines: BTreeMap<(usize, usize), usize>,
    /// Faces on a revolution of a circle crossing its axis that use both
    /// its arcs, to be split once every face of the item is read.
    splits: Vec<Split>,
}

/// A face read on its surface's far half, to be split at the axis into
/// a lune on it and a lens on the near half ([`Builder::split`]); the
/// surface's own refusal stands where that can't be done.
struct Split {
    face: usize,
    crossing: super::geometry::Crossing,
    refusal: ImportError,
}

/// The shape of B-rep items `items`, one representation's: a solid
/// each, sharing what they share, and where its entities came from.
pub(crate) fn shape(g: &Geo, items: &[Id]) -> Result<(Shape, Sources), ImportError> {
    let mut b = Builder {
        g,
        s: Shape::default(),
        sources: Sources::default(),
        vertices: BTreeMap::new(),
        edges: BTreeMap::new(),
        surfaces: BTreeMap::new(),
        points: BTreeMap::new(),
        lines: BTreeMap::new(),
        splits: Vec::new(),
    };
    for &item in items {
        match g.e.get(item, item)? {
            Entity::Brep(Brep::Solid { outer, voids, .. }) => {
                let (outer, voids) = (*outer, voids.clone());
                let outer = b.shell(outer, false, 0)?;
                let voids = voids
                    .iter()
                    .map(|&v| b.shell(v, false, 0))
                    .collect::<Result<Vec<_>, _>>()?;
                b.s.solids.push(Solid { outer, voids });
            }
            // A surface model's shells are sheets: healed as solids'
            // shells unless the caller asks for sheets (see
            // `ryukei::heal::Open`).
            Entity::Brep(Brep::Shells { shells: list, .. } | Brep::Faces(list)) => {
                for &sh in &list.clone() {
                    let shell = b.shell(sh, false, 0)?;
                    b.s.sheets.push(shell);
                }
            }
            _ => return Err(crate::schema::wrong(item, "a B-rep item")),
        }
        for split in core::mem::take(&mut b.splits) {
            b.split(split)?;
        }
        let faces = b.sources.faces.len();
        b.sources.face_items.resize(faces, item);
    }
    Ok((b.s, b.sources))
}

impl Builder<'_, '_> {
    /// A shell, its faces turned over when `flip`. Oriented shells (and
    /// faces) wrapping each other more than [`DEPTH`] deep are a cycle.
    fn shell(&mut self, id: Id, flip: bool, depth: usize) -> Result<usize, ImportError> {
        if depth > DEPTH {
            return Err(ImportError::ReferenceCycle { instance: id });
        }
        match self.g.e.get(id, id)? {
            Entity::Shell { faces, closed } => {
                let (faces, closed) = (faces.clone(), *closed);
                let list = faces
                    .iter()
                    .map(|&f| self.face(f, flip, 0))
                    .collect::<Result<Vec<_>, _>>()?;
                self.s.shells.push(Shell {
                    faces: list,
                    closed,
                });
                Ok(self.s.shells.len() - 1)
            }
            Entity::FaceSet { faces } => {
                let faces = faces.clone();
                let list = faces
                    .iter()
                    .map(|&f| self.face(f, flip, 0))
                    .collect::<Result<Vec<_>, _>>()?;
                self.s.shells.push(Shell {
                    faces: list,
                    closed: false,
                });
                Ok(self.s.shells.len() - 1)
            }
            Entity::OrientedShell {
                shell, orientation, ..
            } => {
                let (shell, orientation) = (*shell, *orientation);
                self.shell(shell, flip ^ !orientation, depth + 1)
            }
            _ => Err(crate::schema::wrong(id, "a shell")),
        }
    }

    fn face(&mut self, id: Id, flip: bool, depth: usize) -> Result<usize, ImportError> {
        if depth > DEPTH {
            return Err(ImportError::ReferenceCycle { instance: id });
        }
        let face = match self.g.e.get(id, id)? {
            Entity::Face(f) => f.clone(),
            Entity::OrientedFace { face, orientation } => {
                let (face, orientation) = (*face, *orientation);
                return self.face(face, flip ^ !orientation, depth + 1);
            }
            _ => return Err(crate::schema::wrong(id, "a face")),
        };
        let SFace {
            bounds,
            surface: Some(surface_id),
            same_sense,
        } = face
        else {
            return Err(ImportError::UnsupportedEntity {
                instance: id,
                name: "FACE".into(),
            });
        };
        let read = self.surface(surface_id);
        let mut loops = Vec::new();
        for &bound in &bounds {
            let Entity::Bound {
                bound: lp,
                orientation,
                ..
            } = *self.g.e.get(id, bound)?
            else {
                return Err(crate::schema::wrong(bound, "a FACE_BOUND"));
            };
            let mut l = self.loop_(lp, surface_id)?;
            if !orientation ^ flip
                && let Loop::Edges(uses) = &mut l
            {
                uses.reverse();
                for u in uses {
                    u.sense = u.sense.flipped();
                }
            }
            loops.push(l);
        }
        let surface = match read {
            Ok(k) => k,
            Err(e) => {
                // A revolution of a circle crossing its axis: the arc this
                // face lies on (issue #316).
                let mut at = Vec::new();
                // Its vertices and points along its edges.
                for l in &loops {
                    match l {
                        Loop::Edges(uses) => {
                            for u in uses {
                                let edge = &self.s.edges[u.edge];
                                at.push(self.s.vertices[edge.start].point);
                                at.push(self.s.vertices[edge.end].point);
                                let [t0, t1] = edge.interval;
                                let curve = &self.s.curves[edge.curve];
                                for i in 1..4 {
                                    at.push(curve.point(t0 + (t1 - t0) * f64::from(i) / 4.0));
                                }
                            }
                        }
                        Loop::Vertex(v) => at.push(self.s.vertices[*v].point),
                    }
                }
                match self.g.crossing_revolution(surface_id, &at) {
                    Ok(Some(s)) => {
                        self.s.surfaces.push(s);
                        self.s.surfaces.len() - 1
                    }
                    // A face using both arcs: split once the item is read.
                    _ => match self.g.crossing_halves(surface_id) {
                        Ok(Some(c)) if loops.len() == 1 => {
                            self.s.surfaces.push(c.far.clone());
                            self.splits.push(Split {
                                face: self.s.faces.len(),
                                crossing: c,
                                refusal: e,
                            });
                            self.s.surfaces.len() - 1
                        }
                        _ => return Err(e),
                    },
                }
            }
        };
        let sense = if same_sense ^ flip {
            Sense::Forward
        } else {
            Sense::Reversed
        };
        self.s.faces.push(Face {
            surface,
            sense,
            loops,
            tolerance: 0.0,
        });
        self.sources.faces.push(id);
        Ok(self.s.faces.len() - 1)
    }

    /// Splits a face that wraps round the whole circle of a revolution
    /// crossing its axis (issue #316), read on the far half's surface,
    /// into a lune there and a lens on the near half's. Its loop must be
    /// two meridians (closed circles through both crossings) and a seam
    /// used once each way: each meridian is cut at the crossings, in
    /// every face using it, the seam goes (the lune doesn't close round
    /// the circle), and the pieces on each arc make each face's loop,
    /// running as the face's loop ran them, so the faces either side see
    /// the same edges. Both keep the face's sense: each half is the
    /// whole surface over its arc. Anything else refuses as the surface
    /// did.
    fn split(&mut self, split: Split) -> Result<(), ImportError> {
        let Split {
            face,
            crossing,
            refusal,
        } = split;
        let Some(done) = self.cut_crossing(face, &crossing) else {
            return Err(refusal);
        };
        self.sources.split = true;
        done
    }

    fn cut_crossing(
        &mut self,
        face: usize,
        c: &super::geometry::Crossing,
    ) -> Option<Result<(), ImportError>> {
        let tol = ryukei::tol::LINEAR_RESOLUTION;
        let [Loop::Edges(uses)] = &self.s.faces[face].loops[..] else {
            return None;
        };
        if uses.len() != 4 {
            return None;
        }
        let on = |k: &ryukei::geom::Circle, p: Point3| {
            let v = p - k.frame().origin();
            let h = v.dot(k.frame().z().get());
            let rad = (v - k.frame().z().get() * h).length();
            (h * h + (rad - k.radius()) * (rad - k.radius())).sqrt() <= 100.0 * tol
        };
        let meridian = |b: &Self, e: usize| {
            let edge = &b.s.edges[e];
            match &b.s.curves[edge.curve] {
                Curve::Circle(k) if edge.start == edge.end => {
                    c.ends.iter().all(|&p| on(k, p)).then_some(*k)
                }
                _ => None,
            }
        };
        let mut meridians = Vec::new();
        let mut seam = None;
        for u in uses {
            match meridian(self, u.edge) {
                Some(k) if !meridians.iter().any(|(e, _)| *e == u.edge) => {
                    meridians.push((u.edge, k));
                }
                Some(_) => return None,
                None => match seam {
                    None => seam = Some((u.edge, u.sense)),
                    Some((e, s)) if e == u.edge && s != u.sense => {}
                    _ => return None,
                },
            }
        }
        let (seam, _) = seam?;
        if meridians.len() != 2 || uses.iter().filter(|u| u.edge == seam).count() != 2 {
            return None;
        }
        // The crossings, one vertex each, shared by both meridians.
        let id = self.sources.faces[face];
        let ends: [usize; 2] = c.ends.map(|p| {
            self.s.vertices.push(Vertex {
                point: p,
                tolerance: 0.0,
                seam: false,
            });
            self.sources.vertices.push(id);
            self.s.vertices.len() - 1
        });
        // Each meridian's pieces, in its direction, and which are near.
        let mut pieces: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut near: BTreeMap<usize, bool> = BTreeMap::new();
        for (e, k) in &meridians {
            let edge = self.s.edges[*e];
            let [t0, t1] = edge.interval;
            if (t1 - t0 - core::f64::consts::TAU).abs() > 1e-9 {
                return None;
            }
            let param = |p: Point3| {
                let v = p - k.frame().origin();
                let t = ryukei::math::atan2(v.dot(k.frame().y().get()), v.dot(k.frame().x().get()));
                t0 + (t - t0).rem_euclid(core::f64::consts::TAU)
            };
            let mut cuts = [(param(c.ends[0]), ends[0]), (param(c.ends[1]), ends[1])];
            cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
            if cuts[0].0 - t0 <= 1e-9 || t1 - cuts[1].0 <= 1e-9 || cuts[1].0 - cuts[0].0 <= 1e-9 {
                return None;
            }
            let stops = [(t0, edge.start), cuts[0], cuts[1], (t1, edge.end)];
            let mut list = Vec::new();
            for (i, w) in stops.windows(2).enumerate() {
                let piece = Edge {
                    curve: edge.curve,
                    interval: [w[0].0, w[1].0],
                    start: w[0].1,
                    end: w[1].1,
                    tolerance: edge.tolerance,
                };
                let k = if i == 0 {
                    self.s.edges[*e] = piece;
                    *e
                } else {
                    self.s.edges.push(piece);
                    self.sources.edges.push(self.sources.edges[*e]);
                    self.s.edges.len() - 1
                };
                let mid = self.s.curves[edge.curve].point((w[0].0 + w[1].0) / 2.0);
                near.insert(k, c.near_at(mid)?);
                list.push(k);
            }
            // One near piece between two far ones.
            if [near[&list[0]], near[&list[1]], near[&list[2]]] != [false, true, false] {
                return None;
            }
            pieces.insert(*e, list);
        }
        // Every use of a meridian, in any face, takes its pieces.
        for f in &mut self.s.faces {
            for l in &mut f.loops {
                let Loop::Edges(uses) = l else { continue };
                let mut out = Vec::with_capacity(uses.len() + 4);
                for u in uses.iter() {
                    match pieces.get(&u.edge) {
                        Some(list) => {
                            let ordered: Vec<usize> = if u.sense == Sense::Forward {
                                list.clone()
                            } else {
                                list.iter().rev().copied().collect()
                            };
                            out.extend(ordered.into_iter().map(|edge| Use { edge, ..*u }));
                        }
                        None => out.push(*u),
                    }
                }
                *uses = out;
            }
        }
        // The lune's and the lens's loops: the pieces on each arc, chained
        // head to tail as the face's loop ran them.
        let Loop::Edges(uses) = self.s.faces[face].loops[0].clone() else {
            return None;
        };
        let ends_of = |b: &Self, u: &Use| {
            let e = &b.s.edges[u.edge];
            if u.sense == Sense::Forward {
                (e.start, e.end)
            } else {
                (e.end, e.start)
            }
        };
        let chain = |b: &Self, list: Vec<Use>| -> Option<Vec<Use>> {
            let mut left = list;
            let mut out = vec![left.remove(0)];
            while !left.is_empty() {
                let (_, end) = ends_of(b, out.last().expect("a use"));
                let i = left.iter().position(|u| ends_of(b, u).0 == end)?;
                out.push(left.remove(i));
            }
            (ends_of(b, &out[0]).0 == ends_of(b, out.last().expect("a use")).1).then_some(out)
        };
        let (lens, lune): (Vec<Use>, Vec<Use>) = uses
            .into_iter()
            .filter(|u| u.edge != seam)
            .partition(|u| near[&u.edge]);
        let lune = chain(self, lune)?;
        let lens = chain(self, lens)?;
        self.s.faces[face].loops = vec![Loop::Edges(lune)];
        self.s.surfaces.push(c.near.clone());
        let lens_face = Face {
            surface: self.s.surfaces.len() - 1,
            sense: self.s.faces[face].sense,
            loops: vec![Loop::Edges(lens)],
            tolerance: 0.0,
        };
        self.s.faces.push(lens_face);
        self.sources.faces.push(id);
        let lens_face = self.s.faces.len() - 1;
        for sh in &mut self.s.shells {
            if sh.faces.contains(&face) {
                sh.faces.push(lens_face);
            }
        }
        // The seam is used no more.
        self.s.edges.remove(seam);
        self.sources.edges.remove(seam);
        for f in &mut self.s.faces {
            for l in &mut f.loops {
                if let Loop::Edges(uses) = l {
                    for u in uses {
                        if u.edge > seam {
                            u.edge -= 1;
                        }
                    }
                }
            }
        }
        self.edges.retain(|_, k| *k != seam);
        for k in self.edges.values_mut() {
            if *k > seam {
                *k -= 1;
            }
        }
        for k in self.lines.values_mut() {
            if *k > seam {
                *k -= 1;
            }
        }
        Some(Ok(()))
    }

    fn surface(&mut self, id: Id) -> Result<usize, ImportError> {
        if let Some(&k) = self.surfaces.get(&id) {
            return Ok(k);
        }
        let s = self.g.surface(id)?;
        self.s.surfaces.push(s);
        self.surfaces.insert(id, self.s.surfaces.len() - 1);
        Ok(self.s.surfaces.len() - 1)
    }

    fn vertex(&mut self, id: Id) -> Result<usize, ImportError> {
        if let Some(&k) = self.vertices.get(&id) {
            return Ok(k);
        }
        let Entity::Vertex { name, point } = self.g.e.get(id, id)? else {
            return Err(crate::schema::wrong(id, "a VERTEX_POINT"));
        };
        let seam = &**name == super::SEAM_VERTEX;
        let p = self.g.point(*point)?;
        self.s.vertices.push(Vertex {
            point: p,
            tolerance: 0.0,
            seam,
        });
        self.sources.vertices.push(id);
        self.vertices.insert(id, self.s.vertices.len() - 1);
        Ok(self.s.vertices.len() - 1)
    }

    /// A loop as the file runs it, on the face whose surface is
    /// `surface`.
    fn loop_(&mut self, id: Id, surface: Id) -> Result<Loop, ImportError> {
        match self.g.e.get(id, id)? {
            Entity::Loop(SLoop::Vertex(v)) => {
                let v = *v;
                Ok(Loop::Vertex(self.vertex(v)?))
            }
            Entity::Loop(SLoop::Edges(list)) => {
                let list = list.clone();
                let mut uses = Vec::with_capacity(list.len());
                for oe in list {
                    let Entity::OrientedEdge { edge, orientation } = *self.g.e.get(id, oe)? else {
                        return Err(crate::schema::wrong(oe, "an ORIENTED_EDGE"));
                    };
                    let (k, same) = self.edge(edge)?;
                    let sense = if orientation == same {
                        Sense::Forward
                    } else {
                        Sense::Reversed
                    };
                    let pcurve = self.pcurve(edge, surface)?;
                    uses.push(Use {
                        edge: k,
                        sense,
                        pcurve,
                    });
                }
                Ok(Loop::Edges(uses))
            }
            Entity::Loop(SLoop::Poly(points)) => {
                let points = points.clone();
                self.poly(id, &points)
            }
            _ => Err(crate::schema::wrong(id, "a loop")),
        }
    }

    /// A `POLY_LOOP`'s line edges.
    fn poly(&mut self, id: Id, points: &[Id]) -> Result<Loop, ImportError> {
        let mut at = Vec::with_capacity(points.len());
        for &p in points {
            let k = match self.points.get(&p) {
                Some(&k) => k,
                None => {
                    self.s.vertices.push(Vertex {
                        point: self.g.point(p)?,
                        tolerance: 0.0,
                        seam: false,
                    });
                    self.sources.vertices.push(p);
                    self.points.insert(p, self.s.vertices.len() - 1);
                    self.s.vertices.len() - 1
                }
            };
            at.push(k);
        }
        let n = at.len();
        let mut uses = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = (at[i], at[(i + 1) % n]);
            let key = (a.min(b), a.max(b));
            let e = match self.lines.get(&key) {
                Some(&e) => e,
                None => {
                    let (p, q) = (self.s.vertices[key.0].point, self.s.vertices[key.1].point);
                    let line = ryukei::geom::Line::through(p, q).map_err(|e| invalid(id, e))?;
                    self.s.curves.push(Curve::Line(line));
                    self.s.edges.push(Edge {
                        curve: self.s.curves.len() - 1,
                        interval: [0.0, p.distance(q)],
                        start: key.0,
                        end: key.1,
                        tolerance: 0.0,
                    });
                    self.sources.edges.push(id);
                    self.lines.insert(key, self.s.edges.len() - 1);
                    self.s.edges.len() - 1
                }
            };
            uses.push(Use {
                edge: e,
                sense: if a == key.0 {
                    Sense::Forward
                } else {
                    Sense::Reversed
                },
                pcurve: None,
            });
        }
        Ok(Loop::Edges(uses))
    }

    /// An `EDGE_CURVE`'s shape edge, and whether it runs the way the
    /// file's edge does.
    fn edge(&mut self, id: Id) -> Result<(usize, bool), ImportError> {
        let Entity::Edge(e) = *self.g.e.get(id, id)? else {
            return Err(crate::schema::wrong(id, "an EDGE_CURVE"));
        };
        if let Some(&k) = self.edges.get(&id) {
            return Ok((k, e.same_sense));
        }
        let (a, b) = (self.vertex(e.start)?, self.vertex(e.end)?);
        let (start, end) = if e.same_sense { (a, b) } else { (b, a) };
        let (p, q) = (self.s.vertices[start].point, self.s.vertices[end].point);
        let curve = match self.conic(e.curve, p, q)? {
            Some(c) => c,
            None => self.g.curve(e.curve)?,
        };
        let interval = interval(&curve, p, q, start == end).ok_or(ImportError::Malformed {
            instance: id,
            attribute: 3,
            what: "the edge's vertices don't bound a stretch of its curve",
        })?;
        self.s.curves.push(curve);
        self.s.edges.push(Edge {
            curve: self.s.curves.len() - 1,
            interval,
            start,
            end,
            tolerance: 0.0,
        });
        self.sources.edges.push(id);
        self.edges.insert(id, self.s.edges.len() - 1);
        Ok((self.s.edges.len() - 1, e.same_sense))
    }

    /// The curve beneath wrappers (trims, curves on surfaces).
    fn base(&self, mut id: Id) -> Result<Id, ImportError> {
        for _ in 0..DEPTH {
            match self.g.e.curve(id, id)? {
                SCurve::Trimmed { basis, .. } => id = *basis,
                SCurve::OnSurface { curve, .. } => id = *curve,
                _ => return Ok(id),
            }
        }
        Err(ImportError::ReferenceCycle { instance: id })
    }

    /// A parabola's or hyperbola's arc from `p` to `q`, exact.
    fn conic(&self, id: Id, p: Point3, q: Point3) -> Result<Option<Curve>, ImportError> {
        let base = self.base(id)?;
        let (position, kind) = match self.g.e.curve(base, base)? {
            SCurve::Parabola {
                position,
                focal_dist,
            } => (*position, Conic::Parabola(self.g.length * focal_dist)),
            SCurve::Hyperbola {
                position,
                semi_axis,
                semi_imag_axis,
            } => (
                *position,
                Conic::Hyperbola(self.g.length * semi_axis, self.g.length * semi_imag_axis),
            ),
            _ => return Ok(None),
        };
        let f = self.g.frame(position)?;
        let (c, x, y) = (f.origin(), f.x().get(), f.y().get());
        let fail = |e| invalid(base, e);
        let (b0, b1, b2, w) = match kind {
            Conic::Parabola(fd) => {
                // P(t) = C + f (t² x + 2t y).
                let t = |r: Point3| (r - c).dot(y) / (2.0 * fd);
                let (t0, t1) = (t(p), t(q));
                let at = |u: f64| c + x * (fd * u * u) + y * (2.0 * fd * u);
                (
                    at(t0),
                    c + x * (fd * t0 * t1) + y * (fd * (t0 + t1)),
                    at(t1),
                    1.0,
                )
            }
            Conic::Hyperbola(a, bb) => {
                // P(t) = C + a cosh t x + b sinh t y.
                let asinh = |s: f64| ryukei::math::ln(s + ryukei::math::sqrt(s * s + 1.0));
                let cosh = |u: f64| (ryukei::math::exp(u) + ryukei::math::exp(-u)) / 2.0;
                let sinh = |u: f64| (ryukei::math::exp(u) - ryukei::math::exp(-u)) / 2.0;
                let t = |r: Point3| asinh((r - c).dot(y) / bb);
                let (t0, t1) = (t(p), t(q));
                let at = |u: f64| c + x * (a * cosh(u)) + y * (bb * sinh(u));
                let (m, h) = ((t0 + t1) / 2.0, (t1 - t0) / 2.0);
                let mid = c + x * (a * cosh(m) / cosh(h)) + y * (bb * sinh(m) / cosh(h));
                (at(t0), mid, at(t1), cosh(h))
            }
        };
        let knots = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).map_err(fail)?;
        let n = NurbsCurve::new(knots, vec![b0, b1, b2], Some(vec![1.0, w, 1.0])).map_err(fail)?;
        Ok(Some(Curve::Nurbs(Arc::new(n))))
    }

    /// The file's pcurve for edge `id` on the surface `surface`, if its
    /// geometry is a curve on that surface with one.
    fn pcurve(&mut self, id: Id, surface: Id) -> Result<Option<usize>, ImportError> {
        let Entity::Edge(e) = *self.g.e.get(id, id)? else {
            return Ok(None);
        };
        let SCurve::OnSurface {
            kind, associated, ..
        } = self.g.e.curve(id, e.curve)?
        else {
            return Ok(None);
        };
        // A seam's two pcurves are on one surface: healing takes the seam
        // away, so neither is needed.
        if *kind == crate::schema::OnSurface::Seam {
            return Ok(None);
        }
        for &a in associated {
            let Ok(SCurve::Pcurve {
                surface: on,
                representation,
            }) = self.g.e.curve(e.curve, a)
            else {
                continue;
            };
            if *on != surface {
                continue;
            }
            let Entity::Representation { items, .. } = self.g.e.get(a, *representation)? else {
                continue;
            };
            let Some(&item) = items.first() else {
                continue;
            };
            let Some(scale) = self.scale(surface)? else {
                return Ok(None);
            };
            return Ok(match self.g.curve2(item, scale) {
                Ok(Some(c)) => {
                    self.s.curves2.push(c);
                    Some(self.s.curves2.len() - 1)
                }
                // A pcurve that can't be read is no reason to refuse the
                // body: healing finds one another way.
                _ => None,
            });
        }
        Ok(None)
    }

    /// How a surface's parameters scale from the file's units: lengths
    /// by the length unit, angles by the angle unit, a spline's as they
    /// are; `None` for surfaces whose parameters differ from ryukei's.
    fn scale(&mut self, surface: Id) -> Result<Option<[f64; 2]>, ImportError> {
        let k = self.surface(surface)?;
        let (l, a) = (self.g.length, self.g.angle);
        Ok(match &self.s.surfaces[k] {
            Surface::Plane(_) => Some([l, l]),
            Surface::Cylinder(_) | Surface::Cone(_) => Some([a, l]),
            Surface::Sphere(_) | Surface::Torus(_) => Some([a, a]),
            Surface::Nurbs(_) => Some([1.0, 1.0]),
            _ => None,
        })
    }
}

enum Conic {
    Parabola(f64),
    Hyperbola(f64, f64),
}

/// The interval along `curve` from `p` to `q`: one period from `p` for a
/// closed edge on a periodic curve, the whole domain on a clamped curve
/// whose ends meet; else the projections, the end unwrapped after the
/// start or taken at the domain's ends where they meet a vertex.
pub(crate) fn interval(curve: &Curve, p: Point3, q: Point3, closed: bool) -> Option<[f64; 2]> {
    let t0 = curve.project(p).param;
    let near = |a: Point3, b: Point3| a.distance(b) <= 1e3 * ryukei::tol::LINEAR_RESOLUTION;
    match curve.period() {
        Some(per) => {
            if closed {
                return Some([t0, t0 + per]);
            }
            let t1 = curve.project(q).param;
            let t1 = if t1 <= t0 { t1 + per } else { t1 };
            (t1 > t0).then_some([t0, t1])
        }
        None => {
            let [d0, d1] = curve.domain();
            if closed {
                return (d0.is_finite()
                    && d1.is_finite()
                    && near(curve.point(d0), curve.point(d1)))
                .then_some([d0, d1]);
            }
            let mut t1 = curve.project(q).param;
            let mut t0 = t0;
            if t1 <= t0 {
                // A clamped curve whose ends meet: the vertex there is at
                // the start for the edge's start, at the end for its end.
                if d0.is_finite() && near(curve.point(d0), p) {
                    t0 = d0;
                }
                if d1.is_finite() && near(curve.point(d1), q) {
                    t1 = d1;
                }
            }
            (t1 > t0).then_some([t0, t1])
        }
    }
}
