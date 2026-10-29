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
            Entity::Brep(Brep::Shells(list)) => {
                for &sh in &list.clone() {
                    let outer = b.shell(sh, false, 0)?;
                    b.s.solids.push(Solid {
                        outer,
                        voids: Vec::new(),
                    });
                }
            }
            _ => return Err(crate::schema::wrong(item, "a B-rep item")),
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
        let surface = self.surface(surface_id)?;
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
