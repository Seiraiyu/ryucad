//! A body's shape (cut open by [`ryukei::heal::seamed`]) as STEP's B-rep
//! entities.
//!
//! Each solid is a `MANIFOLD_SOLID_BREP`, or a `BREP_WITH_VOIDS` whose
//! voids are closed shells facing out of the cavity, used against their
//! orientation. Faces are `ADVANCED_FACE`s, each loop a `FACE_BOUND` of
//! `ORIENTED_EDGE`s (or a `VERTEX_LOOP`); edges are `EDGE_CURVE`s running
//! with their curve, a `SURFACE_CURVE` (a `SEAM_CURVE` for a seam) where
//! uses have pcurves, so readers needn't project. A vertex the writer
//! added only to end a seam is named [`crate::read::SEAM_VERTEX`].
//!
//! Geometry is the exact entity for each kind: lines (a unit vector),
//! circles, ellipses, planes, cylinders, cones, spheres and tori on
//! their frames; NURBS as B-splines with knots, rational ones as complex
//! instances, a periodic one unclamped with its first `degree` control
//! points again after its last; extrusions and revolutions as STEP's
//! swept surfaces. A pcurve is written where it keeps its kind under the
//! surface's units (any when both parameters scale alike, else a line
//! along a parameter or a spline) and runs counterclockwise if a circle
//! or an ellipse; otherwise the edge goes without it on that face.

use super::{Contexts, Out, b, direction, e, f, placement, point, r, refs, s};
use crate::part21::Param;
use crate::read::SEAM_VERTEX;
use ryukei::geom::{Curve, Curve2, KnotVector, Point2, Surface};
use ryukei::heal::{Loop, Shape};
use ryukei::topo::Sense;
use std::collections::BTreeMap;

/// Writes a shape's solids; returns their B-rep items.
pub(crate) fn solid_items(out: &mut Out, cx: &Contexts, sh: &Shape) -> Vec<u64> {
    let mut w = Writer {
        out,
        cx,
        sh,
        surfaces: BTreeMap::new(),
        curves: BTreeMap::new(),
        vertices: BTreeMap::new(),
        edges: BTreeMap::new(),
    };
    w.solids()
}

struct Writer<'a, 'b> {
    out: &'a mut Out,
    cx: &'b Contexts,
    sh: &'b Shape,
    surfaces: BTreeMap<usize, u64>,
    curves: BTreeMap<usize, u64>,
    vertices: BTreeMap<usize, u64>,
    edges: BTreeMap<usize, u64>,
}

impl Writer<'_, '_> {
    fn solids(&mut self) -> Vec<u64> {
        // Each edge's uses: face, loop, position.
        let mut uses: BTreeMap<usize, Vec<(usize, usize, usize)>> = BTreeMap::new();
        for (k, face) in self.sh.faces.iter().enumerate() {
            for (l, lp) in face.loops.iter().enumerate() {
                if let Loop::Edges(list) = lp {
                    for (i, u) in list.iter().enumerate() {
                        uses.entry(u.edge).or_default().push((k, l, i));
                    }
                }
            }
        }
        for k in 0..self.sh.faces.len() {
            self.surface(self.sh.faces[k].surface);
        }
        for (&edge, list) in &uses {
            self.edge(edge, list);
        }
        let solids = self.sh.solids.clone();
        let mut items = Vec::with_capacity(solids.len());
        for so in solids {
            let outer = self.shell(so.outer, false);
            if so.voids.is_empty() {
                items.push(self.out.add("MANIFOLD_SOLID_BREP", vec![s(""), r(outer)]));
                continue;
            }
            let voids: Vec<u64> = so
                .voids
                .iter()
                .map(|&v| {
                    let closed = self.shell(v, true);
                    self.out.add(
                        "ORIENTED_CLOSED_SHELL",
                        vec![s(""), Param::Derived, r(closed), b(false)],
                    )
                })
                .collect();
            items.push(
                self.out
                    .add("BREP_WITH_VOIDS", vec![s(""), r(outer), refs(voids)]),
            );
        }
        items
    }

    /// A closed shell of the shape's faces, each turned over when `flip`
    /// (a void written as a solid's outside, used against its
    /// orientation).
    fn shell(&mut self, k: usize, flip: bool) -> u64 {
        let faces: Vec<u64> = self.sh.shells[k]
            .faces
            .clone()
            .into_iter()
            .map(|f| self.face(f, flip))
            .collect();
        self.out.add("CLOSED_SHELL", vec![s(""), refs(faces)])
    }

    fn face(&mut self, k: usize, flip: bool) -> u64 {
        let face = &self.sh.faces[k];
        let mut bounds = Vec::with_capacity(face.loops.len());
        for lp in face.loops.clone() {
            let lp = match lp {
                Loop::Vertex(v) => {
                    let v = self.vertex(v);
                    self.out.add("VERTEX_LOOP", vec![s(""), r(v)])
                }
                Loop::Edges(list) => {
                    let oriented: Vec<u64> = list
                        .iter()
                        .map(|u| {
                            let edge = self.edges[&u.edge];
                            self.out.add(
                                "ORIENTED_EDGE",
                                vec![
                                    s(""),
                                    Param::Derived,
                                    Param::Derived,
                                    r(edge),
                                    b(u.sense == Sense::Forward),
                                ],
                            )
                        })
                        .collect();
                    self.out.add("EDGE_LOOP", vec![s(""), refs(oriented)])
                }
            };
            // A face turned over runs its loops the other way: the bound
            // against its loop.
            bounds.push(self.out.add("FACE_BOUND", vec![s(""), r(lp), b(!flip)]));
        }
        let surface = self.surfaces[&face.surface];
        let forward = (face.sense == Sense::Forward) != flip;
        self.out.add(
            "ADVANCED_FACE",
            vec![s(""), refs(bounds), r(surface), b(forward)],
        )
    }

    fn vertex(&mut self, k: usize) -> u64 {
        if let Some(&id) = self.vertices.get(&k) {
            return id;
        }
        let v = self.sh.vertices[k];
        let p = point(self.out, self.cx, v.point);
        let name = if v.seam { SEAM_VERTEX } else { "" };
        let id = self.out.add("VERTEX_POINT", vec![s(name), r(p)]);
        self.vertices.insert(k, id);
        id
    }

    /// An edge, with its uses' pcurves.
    fn edge(&mut self, k: usize, uses: &[(usize, usize, usize)]) {
        let edge = self.sh.edges[k];
        let start = self.vertex(edge.start);
        let end = self.vertex(edge.end);
        let curve = self.curve(edge.curve);
        let mut pcurves = Vec::new();
        for &(face, l, i) in uses {
            let Loop::Edges(list) = &self.sh.faces[face].loops[l] else {
                continue;
            };
            let Some(c2) = list[i].pcurve else {
                continue;
            };
            let surface = self.sh.faces[face].surface;
            if let Some(pc) = self.pcurve(c2, surface) {
                pcurves.push(pc);
            }
        }
        let seam = uses.len() == 2 && uses[0].0 == uses[1].0;
        let geometry = match (seam, pcurves.len()) {
            (true, 2) => self.out.add(
                "SEAM_CURVE",
                vec![s(""), r(curve), refs(pcurves), e("PCURVE_S1")],
            ),
            (false, 1 | 2) => self.out.add(
                "SURFACE_CURVE",
                vec![s(""), r(curve), refs(pcurves), e("CURVE_3D")],
            ),
            _ => curve,
        };
        let id = self.out.add(
            "EDGE_CURVE",
            vec![s(""), r(start), r(end), r(geometry), b(true)],
        );
        self.edges.insert(k, id);
    }

    /// A `PCURVE` on surface `surface`, if the curve keeps its kind there.
    fn pcurve(&mut self, c2: usize, surface: usize) -> Option<u64> {
        let scale = match &self.sh.surfaces[surface] {
            Surface::Plane(_) => [1.0 / self.cx.unit; 2],
            Surface::Cylinder(_) | Surface::Cone(_) => [1.0, 1.0 / self.cx.unit],
            Surface::Sphere(_) | Surface::Torus(_) | Surface::Nurbs(_) => [1.0, 1.0],
            _ => return None,
        };
        let curve = self.curve2(&self.sh.curves2[c2].clone(), scale)?;
        let rep = self.out.add(
            "DEFINITIONAL_REPRESENTATION",
            vec![s(""), refs([curve]), r(self.cx.parametric)],
        );
        let on = self.surfaces[&surface];
        Some(self.out.add("PCURVE", vec![s(""), r(on), r(rep)]))
    }

    fn point2(&mut self, p: Point2, scale: [f64; 2]) -> u64 {
        self.out.add(
            "CARTESIAN_POINT",
            vec![
                s(""),
                Param::List(vec![f(p.x * scale[0]), f(p.y * scale[1])]),
            ],
        )
    }

    fn curve2(&mut self, c: &Curve2, scale: [f64; 2]) -> Option<u64> {
        let even = scale[0] == scale[1];
        match c {
            Curve2::Line2(l) => {
                let d = l.dir();
                if !even && d.x != 0.0 && d.y != 0.0 {
                    return None;
                }
                let o = self.point2(l.origin(), scale);
                let (dx, dy) = (d.x * scale[0], d.y * scale[1]);
                let len = ryukei::math::sqrt(dx * dx + dy * dy);
                let dir = self.out.add(
                    "DIRECTION",
                    vec![s(""), Param::List(vec![f(dx / len), f(dy / len)])],
                );
                let v = self.out.add("VECTOR", vec![s(""), r(dir), f(len)]);
                Some(self.out.add("LINE", vec![s(""), r(o), r(v)]))
            }
            Curve2::Circle2(c) if even && c.frame().is_ccw() => {
                let a = self.axis2(c.frame().origin(), c.frame().x(), scale);
                Some(
                    self.out
                        .add("CIRCLE", vec![s(""), r(a), f(c.radius() * scale[0])]),
                )
            }
            Curve2::Ellipse2(el) if even && el.frame().is_ccw() => {
                let a = self.axis2(el.frame().origin(), el.frame().x(), scale);
                Some(self.out.add(
                    "ELLIPSE",
                    vec![
                        s(""),
                        r(a),
                        f(el.major() * scale[0]),
                        f(el.minor() * scale[0]),
                    ],
                ))
            }
            Curve2::Nurbs2(n) => {
                let pts: Vec<u64> = n.control_points().map(|p| self.point2(p, scale)).collect();
                Some(spline_curve(self.out, n.knots(), pts, n.weights()))
            }
            _ => None,
        }
    }

    fn axis2(&mut self, o: Point2, x: ryukei::geom::Vector2, scale: [f64; 2]) -> u64 {
        let p = self.point2(o, scale);
        let d = self
            .out
            .add("DIRECTION", vec![s(""), Param::List(vec![f(x.x), f(x.y)])]);
        self.out.add("AXIS2_PLACEMENT_2D", vec![s(""), r(p), r(d)])
    }

    fn surface(&mut self, k: usize) -> u64 {
        if let Some(&id) = self.surfaces.get(&k) {
            return id;
        }
        let surface = self.sh.surfaces[k].clone();
        let id = match &surface {
            Surface::Plane(_) => {
                let a = self.frame(&surface);
                self.out.add("PLANE", vec![s(""), r(a)])
            }
            Surface::Cylinder(c) => {
                let a = self.frame(&surface);
                let radius = c.radius() / self.cx.unit;
                self.out
                    .add("CYLINDRICAL_SURFACE", vec![s(""), r(a), f(radius)])
            }
            Surface::Cone(c) => {
                let a = self.frame(&surface);
                let radius = c.radius() / self.cx.unit;
                self.out.add(
                    "CONICAL_SURFACE",
                    vec![s(""), r(a), f(radius), f(c.half_angle())],
                )
            }
            Surface::Sphere(c) => {
                let a = self.frame(&surface);
                let radius = c.radius() / self.cx.unit;
                self.out
                    .add("SPHERICAL_SURFACE", vec![s(""), r(a), f(radius)])
            }
            Surface::Torus(t) => {
                let a = self.frame(&surface);
                let u = self.cx.unit;
                self.out.add(
                    "TOROIDAL_SURFACE",
                    vec![s(""), r(a), f(t.major() / u), f(t.minor() / u)],
                )
            }
            Surface::Nurbs(n) => {
                let [nu, nv] = n.counts();
                let [ku, kv] = n.knots();
                let (fu, fv) = (flat_count(ku), flat_count(kv));
                let mut rows = Vec::with_capacity(fu);
                for i in 0..fu {
                    let mut row = Vec::with_capacity(fv);
                    for j in 0..fv {
                        let p = n.control_point(i % nu, j % nv);
                        row.push(point(self.out, self.cx, p));
                    }
                    rows.push(row);
                }
                let w = n.weights();
                let weights: Vec<Vec<f64>> = (0..fu)
                    .map(|i| (0..fv).map(|j| w[(i % nu) * nv + j % nv]).collect())
                    .collect();
                spline_surface(self.out, n.degrees(), [ku, kv], rows, weights)
            }
            Surface::Extrusion(x) => {
                let c = self.curve_entity(x.curve());
                let d = direction(self.out, x.dir().get());
                let v = self.out.add("VECTOR", vec![s(""), r(d), f(1.0)]);
                self.out
                    .add("SURFACE_OF_LINEAR_EXTRUSION", vec![s(""), r(c), r(v)])
            }
            Surface::Revolution(rv) => {
                let c = self.curve_entity(rv.curve());
                let p = point(self.out, self.cx, rv.frame().origin());
                let z = direction(self.out, rv.frame().z().get());
                let a = self.out.add("AXIS1_PLACEMENT", vec![s(""), r(p), r(z)]);
                self.out
                    .add("SURFACE_OF_REVOLUTION", vec![s(""), r(c), r(a)])
            }
            _ => unreachable!("pipes are NURBS by now"),
        };
        self.surfaces.insert(k, id);
        id
    }

    /// An analytic surface's frame as an `AXIS2_PLACEMENT_3D`.
    fn frame(&mut self, surface: &Surface) -> u64 {
        let fr = surface.frame().expect("an analytic surface");
        placement(self.out, self.cx, fr.origin(), fr.z().get(), fr.x().get())
    }

    fn curve(&mut self, k: usize) -> u64 {
        if let Some(&id) = self.curves.get(&k) {
            return id;
        }
        let c = self.sh.curves[k].clone();
        let id = self.curve_entity(&c);
        self.curves.insert(k, id);
        id
    }

    fn curve_entity(&mut self, c: &Curve) -> u64 {
        let u = self.cx.unit;
        match c {
            Curve::Line(l) => {
                let p = point(self.out, self.cx, l.origin());
                let d = direction(self.out, l.dir().get());
                let v = self.out.add("VECTOR", vec![s(""), r(d), f(1.0)]);
                self.out.add("LINE", vec![s(""), r(p), r(v)])
            }
            Curve::Circle(c) => {
                let fr = c.frame();
                let a = placement(self.out, self.cx, fr.origin(), fr.z().get(), fr.x().get());
                self.out.add("CIRCLE", vec![s(""), r(a), f(c.radius() / u)])
            }
            Curve::Ellipse(el) => {
                let fr = el.frame();
                let a = placement(self.out, self.cx, fr.origin(), fr.z().get(), fr.x().get());
                self.out.add(
                    "ELLIPSE",
                    vec![s(""), r(a), f(el.major() / u), f(el.minor() / u)],
                )
            }
            Curve::Nurbs(n) => {
                let k = n.knots();
                let m = n.control_points().len();
                let all: Vec<_> = n.control_points().collect();
                let pts: Vec<u64> = (0..flat_count(k))
                    .map(|i| point(self.out, self.cx, all[i % m]))
                    .collect();
                let w = n.weights();
                let weights: Vec<f64> = (0..flat_count(k)).map(|i| w[i % m]).collect();
                spline_curve(self.out, k, pts, &weights)
            }
            _ => unreachable!("every curve kind is written"),
        }
    }
}

/// How many control points a knot vector's flat form takes: a periodic
/// one's first `degree` again after its last.
fn flat_count(k: &KnotVector) -> usize {
    k.flat_count()
}

/// A knot vector's distinct flat knots and their multiplicities, as
/// STEP's lists.
fn knot_lists(k: &KnotVector) -> (Param, Param) {
    let mut values: Vec<f64> = Vec::new();
    let mut mults: Vec<i64> = Vec::new();
    for &x in k.flat() {
        if values.last() == Some(&x) {
            *mults.last_mut().expect("a knot") += 1;
        } else {
            values.push(x);
            mults.push(1);
        }
    }
    (
        Param::List(mults.into_iter().map(Param::Int).collect()),
        Param::List(values.into_iter().map(f).collect()),
    )
}

fn rational(weights: &[f64]) -> bool {
    weights.iter().any(|&w| w != 1.0)
}

/// A B-spline curve: a simple record, or a complex one with weights.
fn spline_curve(out: &mut Out, k: &KnotVector, points: Vec<u64>, weights: &[f64]) -> u64 {
    let degree = Param::Int(k.degree() as i64);
    let closed = b(k.is_periodic());
    let (mults, values) = knot_lists(k);
    if !rational(weights) {
        return out.add(
            "B_SPLINE_CURVE_WITH_KNOTS",
            vec![
                s(""),
                degree,
                refs(points),
                e("UNSPECIFIED"),
                closed,
                b(false),
                mults,
                values,
                e("UNSPECIFIED"),
            ],
        );
    }
    out.complex(vec![
        ("BOUNDED_CURVE", vec![]),
        (
            "B_SPLINE_CURVE",
            vec![degree, refs(points), e("UNSPECIFIED"), closed, b(false)],
        ),
        (
            "B_SPLINE_CURVE_WITH_KNOTS",
            vec![mults, values, e("UNSPECIFIED")],
        ),
        ("CURVE", vec![]),
        ("GEOMETRIC_REPRESENTATION_ITEM", vec![]),
        (
            "RATIONAL_B_SPLINE_CURVE",
            vec![Param::List(weights.iter().map(|&w| f(w)).collect())],
        ),
        ("REPRESENTATION_ITEM", vec![s("")]),
    ])
}

/// A B-spline surface: rows of control points along `v`, one per `u`.
fn spline_surface(
    out: &mut Out,
    [p, q]: [usize; 2],
    [ku, kv]: [&KnotVector; 2],
    rows: Vec<Vec<u64>>,
    weights: Vec<Vec<f64>>,
) -> u64 {
    let net = Param::List(rows.into_iter().map(refs).collect());
    let (mu, vu) = knot_lists(ku);
    let (mv, vv) = knot_lists(kv);
    let head = vec![
        Param::Int(p as i64),
        Param::Int(q as i64),
        net,
        e("UNSPECIFIED"),
        b(ku.is_periodic()),
        b(kv.is_periodic()),
        b(false),
    ];
    if !weights.iter().any(|row| rational(row)) {
        let mut params = vec![s("")];
        params.extend(head);
        params.extend([mu, mv, vu, vv, e("UNSPECIFIED")]);
        return out.add("B_SPLINE_SURFACE_WITH_KNOTS", params);
    }
    out.complex(vec![
        ("BOUNDED_SURFACE", vec![]),
        ("B_SPLINE_SURFACE", head),
        (
            "B_SPLINE_SURFACE_WITH_KNOTS",
            vec![mu, mv, vu, vv, e("UNSPECIFIED")],
        ),
        ("GEOMETRIC_REPRESENTATION_ITEM", vec![]),
        (
            "RATIONAL_B_SPLINE_SURFACE",
            vec![Param::List(
                weights
                    .into_iter()
                    .map(|row| Param::List(row.into_iter().map(f).collect()))
                    .collect(),
            )],
        ),
        ("REPRESENTATION_ITEM", vec![s("")]),
        ("SURFACE", vec![]),
    ])
}
