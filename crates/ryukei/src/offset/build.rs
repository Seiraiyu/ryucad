//! The offset body built: its faces, edges and vertices laid out (see
//! [`crate::layout`]), named, given every pcurve, and checked.

use super::classify::Plan;
use super::edges::{EdgeGeo, NewCurve};
use super::vertices::{FillEnd, Part, VUse, VertexGeo};
use crate::check::{Fault, check, check_intersections_in};
use crate::geom::{Curve, Curve2, Surface};
use crate::layout::{self, Boundary, BuildError, Layout, Use};
use crate::name::{Event, Journal, Name, OffsetRole, OpKey, Role};
use crate::tol::{LINEAR_RESOLUTION, Tolerance};
use crate::topo::{Body, BodyKind, EdgeId, EntityId, FaceId, Pcurve, RegionKind, Sense, VertexId};
use crate::{Error, OffsetCase, OffsetError};
use std::collections::{BTreeMap, BTreeSet};

/// What a layout edge is, for naming. The first field is the side it
/// lies on (see [`Side`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum EdgeRole {
    /// The edge an original edge became, keeping its name.
    Kept(usize, EdgeId),
    /// An opening edge's rail on its fin's face, by side.
    Rail(usize, EdgeId, usize),
    /// A vertex's edge between two of the result's faces.
    Vertex(usize, VertexId, [Part; 2]),
    /// A laminar edge's rail.
    Laminar(usize, EdgeId),
    /// A thickened sheet's wall edge along a vertex's normal.
    Normal(VertexId),
}

/// One side of what is built: an offset body has one; a thickened sheet
/// two, its back built the other way round.
pub(crate) struct Side<'a> {
    pub(crate) plan: &'a Plan<'a>,
    pub(crate) geo: &'a BTreeMap<EdgeId, EdgeGeo>,
    /// Every vertex's geometry, the boundary's too.
    pub(crate) vgeo: &'a BTreeMap<VertexId, VertexGeo>,
    /// Every laminar edge's rail.
    pub(crate) rails: &'a BTreeMap<EdgeId, NewCurve>,
    /// Faces face the other way: senses and loops reversed.
    pub(crate) flip: bool,
}

/// The layout under construction, and what each of its parts is.
pub(crate) struct Laid<'a> {
    pub(crate) body: &'a Body,
    pub(crate) layout: Layout,
    /// Each side's vertices' points' layout vertices.
    pub(crate) points: BTreeMap<(usize, VertexId, usize), usize>,
    /// Each side's original edges' layout edges: one, or the two rails.
    pub(crate) edges: BTreeMap<(usize, EdgeId), Vec<usize>>,
    /// Each side's vertices' own edges' layout edges.
    pub(crate) vedges: BTreeMap<(usize, VertexId, usize), usize>,
    /// Each side's laminar edges' rails' layout edges.
    pub(crate) rails: BTreeMap<(usize, EdgeId), usize>,
    pub(crate) roles: Vec<EdgeRole>,
    /// Each layout face's side and part.
    pub(crate) parts: Vec<(usize, Part)>,
}

fn sense_of(forward: bool) -> Sense {
    if forward {
        Sense::Forward
    } else {
        Sense::Reversed
    }
}

pub(crate) fn flip(s: Sense) -> Sense {
    match s {
        Sense::Forward => Sense::Reversed,
        Sense::Reversed => Sense::Forward,
    }
}

/// A face's loops run the other way: each loop's uses in reverse order,
/// each the other way.
fn turned(loops: Vec<Boundary>) -> Vec<Boundary> {
    loops
        .into_iter()
        .map(|b| match b {
            Boundary::Arcs(mut arcs) => {
                arcs.reverse();
                for a in &mut arcs {
                    a.sense = flip(a.sense);
                }
                Boundary::Arcs(arcs)
            }
            lone => lone,
        })
        .collect()
}

impl<'a> Laid<'a> {
    pub(crate) fn new(body: &'a Body) -> Self {
        Self {
            body,
            layout: Layout::default(),
            points: BTreeMap::new(),
            edges: BTreeMap::new(),
            vedges: BTreeMap::new(),
            rails: BTreeMap::new(),
            roles: Vec::new(),
            parts: Vec::new(),
        }
    }

    pub(crate) fn curve2(&mut self, c: &Option<(Curve2, [f64; 2])>) -> Option<(usize, [f64; 2])> {
        let (c, map) = c.as_ref()?;
        self.layout.curves2.push(c.clone());
        Some((self.layout.curves2.len() - 1, *map))
    }

    pub(crate) fn add_edge(
        &mut self,
        curve: &Curve,
        interval: [f64; 2],
        (start, end): (usize, usize),
        tolerance: f64,
        role: EdgeRole,
    ) -> usize {
        self.layout.curves.push(curve.clone());
        self.layout.edges.push(layout::Edge {
            start,
            end,
            curve: self.layout.curves.len() - 1,
            interval,
            tolerance: tolerance.max(LINEAR_RESOLUTION),
        });
        self.roles.push(role);
        self.layout.edges.len() - 1
    }

    /// A new curve cut between its ends at the edge's vertices on side
    /// `s`, as a layout edge running with the curve. `EdgeVanishes` where
    /// its ends come in the wrong order.
    fn cut(
        &mut self,
        s: usize,
        e: EdgeId,
        c: &NewCurve,
        ends: [(usize, f64); 2],
        role: EdgeRole,
    ) -> Result<usize, Error> {
        let edge = &self.body.edges[e];
        let at = edge.name;
        let a = (self.points[&(s, edge.start, ends[0].0)], ends[0].1);
        let b = (self.points[&(s, edge.end, ends[1].0)], ends[1].1);
        let ((v0, t0), (v1, t1)) = if c.reversed { (b, a) } else { (a, b) };
        // A closed edge on a closed clamped curve runs over all of it,
        // from its vertex at the curve's start.
        let ends_meet = c
            .curve
            .point(c.range[0])
            .distance(c.curve.point(c.range[1]))
            <= c.tolerance.max(LINEAR_RESOLUTION);
        let (mut t0, mut t1) = (t0, t1);
        if edge.start == edge.end && c.period.is_none() && ends_meet && t0 >= t1 {
            (t0, t1) = (c.range[0], c.range[1]);
        }
        if let Some(p) = c.period {
            while t1 <= t0 {
                t1 += p;
            }
            if t1 - t0 > p {
                t1 -= p;
            }
        }
        if t0.partial_cmp(&t1) != Some(core::cmp::Ordering::Less) {
            return Err(OffsetError::EdgeVanishes { edge: at }.into());
        }
        Ok(self.add_edge(&c.curve, [t0, t1], (v0, v1), c.tolerance, role))
    }

    pub(crate) fn vuse(
        &mut self,
        s: usize,
        v: VertexId,
        g: &VertexGeo,
        u: VUse,
        part: Part,
    ) -> Use {
        let e = &g.edges[u.edge];
        let pc = e
            .parts
            .iter()
            .find(|p| p.0 == part)
            .and_then(|p| p.1.clone());
        Use {
            edge: self.vedges[&(s, v, u.edge)],
            sense: sense_of(u.from == e.from),
            pcurve: self.curve2(&pc),
        }
    }

    fn face(
        &mut self,
        surface: Surface,
        sense: Sense,
        loops: Vec<Boundary>,
        flip_: bool,
        part: (usize, Part),
    ) {
        self.layout.surfaces.push(surface);
        let surface = self.layout.surfaces.len() - 1;
        let (sense, loops) = if flip_ {
            (flip(sense), turned(loops))
        } else {
            (sense, loops)
        };
        self.layout.faces.push(layout::Face {
            surface,
            sense,
            loops,
        });
        self.parts.push(part);
    }

    /// Lays out side `s`: its vertices, then its edges (each original
    /// inner edge's, each laminar edge's rail, then each vertex's), then
    /// its faces (each face's offset in name order, fills in the order of
    /// their edges' names, spheres in the order of their vertices).
    pub(crate) fn lay(&mut self, s: usize, side: &Side) -> Result<(), Error> {
        let (plan, geo, vgeo) = (side.plan, side.geo, side.vgeo);
        let body = plan.body;
        // A spindle fill's pole is one vertex for both its ends.
        let mut poles: BTreeMap<EdgeId, usize> = BTreeMap::new();
        for (&v, g) in vgeo {
            for (k, &p) in g.points.iter().enumerate() {
                let pole_of = g.poles.iter().find(|x| *x.1 == k).map(|x| *x.0);
                if let Some(&lv) = pole_of.and_then(|e| poles.get(&e)) {
                    self.points.insert((s, v, k), lv);
                    continue;
                }
                self.layout.vertices.push(layout::Vertex {
                    point: p,
                    tolerance: LINEAR_RESOLUTION,
                });
                let lv = self.layout.vertices.len() - 1;
                if let Some(e) = pole_of {
                    poles.insert(e, lv);
                }
                self.points.insert((s, v, k), lv);
            }
        }
        for &e in &plan.edges {
            let edge = &body.edges[e];
            let (a, b) = ((e, false), (e, true));
            let ids = match &geo[&e] {
                EdgeGeo::One(c) => {
                    let ends = [vgeo[&edge.start].one[&a], vgeo[&edge.end].one[&b]];
                    vec![self.cut(s, e, c, ends, EdgeRole::Kept(s, e))?]
                }
                EdgeGeo::Open { rails, .. } => {
                    let (x, y) = (vgeo[&edge.start].open[&a], vgeo[&edge.end].open[&b]);
                    let mut ids = Vec::new();
                    for k in 0..2 {
                        ids.push(self.cut(
                            s,
                            e,
                            &rails[k],
                            [x[k], y[k]],
                            EdgeRole::Rail(s, e, k),
                        )?);
                    }
                    ids
                }
            };
            self.edges.insert((s, e), ids);
        }
        for &e in &plan.laminar {
            let edge = &body.edges[e];
            let ends = [
                vgeo[&edge.start].laminar[&(e, false)],
                vgeo[&edge.end].laminar[&(e, true)],
            ];
            let id = self.cut(s, e, &side.rails[&e], ends, EdgeRole::Laminar(s, e))?;
            self.rails.insert((s, e), id);
        }
        for (&v, g) in vgeo {
            for (k, ve) in g.edges.iter().enumerate() {
                let (a, b) = (self.points[&(s, v, ve.from)], self.points[&(s, v, ve.to)]);
                let id = self.add_edge(
                    &ve.curve,
                    ve.range,
                    (a, b),
                    ve.tolerance,
                    EdgeRole::Vertex(s, v, [ve.parts[0].0, ve.parts[1].0]),
                );
                self.vedges.insert((s, v, k), id);
            }
        }
        // Faces: each face's offset, in name order.
        let mut faces: Vec<FaceId> = body.faces().collect();
        faces.sort_by_key(|&f| body.faces[f].name);
        for &f in &faces {
            let face = &body.faces[f];
            let mut loops = Vec::new();
            for l in body.face_loops(f) {
                if body.loops[l].vertex.is_some() {
                    return Err(OffsetError::Unsupported {
                        at: face.name,
                        case: OffsetCase::LoneVertex,
                    }
                    .into());
                }
                let mut arcs = Vec::new();
                for fin in body.loop_fins(l) {
                    let fi = &body.fins[fin];
                    let e = fi.edge;
                    if body.is_laminar(e) {
                        let pc = self.curve2(&side.rails[&e].pcurves[0]);
                        arcs.push(Use {
                            edge: self.rails[&(s, e)],
                            sense: fi.sense,
                            pcurve: pc,
                        });
                        continue;
                    }
                    let k = usize::from(plan.fins(e)[0] != fin);
                    match &geo[&e] {
                        EdgeGeo::One(c) => {
                            let sense = if c.reversed { flip(fi.sense) } else { fi.sense };
                            let pc = self.curve2(&c.pcurves[k]);
                            arcs.push(Use {
                                edge: self.edges[&(s, e)][0],
                                sense,
                                pcurve: pc,
                            });
                        }
                        EdgeGeo::Open { rails, .. } => {
                            let pc = self.curve2(&rails[k].pcurves[0]);
                            arcs.push(Use {
                                edge: self.edges[&(s, e)][k],
                                sense: fi.sense,
                                pcurve: pc,
                            });
                        }
                    }
                    let v = body.end_of(fin);
                    if let Some(ins) = vgeo[&v].inserts.get(&fin) {
                        for &u in ins {
                            let x = self.vuse(s, v, &vgeo[&v], u, Part::Face(f));
                            arcs.push(x);
                        }
                    }
                }
                loops.push(Boundary::Arcs(arcs));
            }
            // An inverted sphere's parametrization turns its normal over.
            let sense = match plan.inversions.get(&f) {
                Some(i) if i.flip => flip(face.sense),
                _ => face.sense,
            };
            self.face(
                plan.surface[&f].clone(),
                sense,
                loops,
                side.flip,
                (s, Part::Face(f)),
            );
        }
        // Fills, in the order of their edges' names.
        for &e in &plan.edges {
            let EdgeGeo::Open {
                fill,
                sense,
                spindle,
                ..
            } = &geo[&e]
            else {
                continue;
            };
            if let Some(sp) = spindle {
                self.spindle_faces(s, side, e, (fill, *sense), sp)?;
                continue;
            }
            let edge = &body.edges[e];
            let rails = self.edges[&(s, e)].clone();
            let r0 = Use {
                edge: rails[0],
                sense: Sense::Reversed,
                pcurve: None,
            };
            let r1 = Use {
                edge: rails[1],
                sense: Sense::Forward,
                pcurve: None,
            };
            let start = &vgeo[&edge.start].fill_ends[&(e, false)];
            let end = &vgeo[&edge.end].fill_ends[&(e, true)];
            let loops = match (start, end) {
                (FillEnd::Wraps, FillEnd::Wraps) => {
                    vec![Boundary::Arcs(vec![r0]), Boundary::Arcs(vec![r1])]
                }
                (FillEnd::Edges(a), FillEnd::Edges(b)) => {
                    let mut arcs = vec![r0];
                    for &u in a {
                        arcs.push(self.vuse(s, edge.start, &vgeo[&edge.start], u, Part::Fill(e)));
                    }
                    arcs.push(r1);
                    // At the end, from the second rail back to the first.
                    for &u in b.iter().rev() {
                        let ve = &vgeo[&edge.end].edges[u.edge];
                        let back = VUse {
                            edge: u.edge,
                            from: if u.from == ve.from { ve.to } else { ve.from },
                        };
                        arcs.push(self.vuse(s, edge.end, &vgeo[&edge.end], back, Part::Fill(e)));
                    }
                    vec![Boundary::Arcs(arcs)]
                }
                _ => return Err(crate::OffsetError::Uncertain { at: edge.name }.into()),
            };
            self.face(fill.clone(), *sense, loops, side.flip, (s, Part::Fill(e)));
        }
        // Spheres, in the order of their vertices.
        for (&v, g) in vgeo {
            let Some((sphere, sense, lp)) = &g.sphere else {
                continue;
            };
            let arcs = lp
                .iter()
                .map(|&u| self.vuse(s, v, g, u, Part::Sphere(v)))
                .collect();
            self.face(
                sphere.clone(),
                *sense,
                vec![Boundary::Arcs(arcs)],
                side.flip,
                (s, Part::Sphere(v)),
            );
        }
        // An envelope's fans, in the order of their vertices.
        for (&v, g) in vgeo {
            let Some((sphere, triangles)) = &g.fan else {
                continue;
            };
            for (m, (sense, lp)) in triangles.iter().enumerate() {
                let arcs = lp
                    .iter()
                    .map(|&u| self.vuse(s, v, g, u, Part::Fan(v, m)))
                    .collect();
                self.face(
                    sphere.clone(),
                    *sense,
                    vec![Boundary::Arcs(arcs)],
                    side.flip,
                    (s, Part::Fan(v, m)),
                );
            }
        }
        Ok(())
    }

    /// A spindle fill's two faces: on the outer rail's side `fill`, on the
    /// other side the spindle's inner surface, each bounded by its rail and
    /// the halves of the sections at the fill's ends that reach the pole
    /// (or, round a closed edge, by its rail alone).
    fn spindle_faces(
        &mut self,
        s: usize,
        side: &Side,
        e: EdgeId,
        (fill, sense): (&Surface, Sense),
        sp: &super::edges::Spindle,
    ) -> Result<(), Error> {
        let vgeo = side.vgeo;
        let body = self.body;
        let edge = &body.edges[e];
        let rails = self.edges[&(s, e)].clone();
        let rail = |k: usize| Use {
            edge: rails[k],
            sense: if k == 0 {
                Sense::Reversed
            } else {
                Sense::Forward
            },
            pcurve: None,
        };
        let start = &vgeo[&edge.start].fill_ends[&(e, false)];
        let end = &vgeo[&edge.end].fill_ends[&(e, true)];
        let mut loops: [Vec<Use>; 2] = [vec![rail(0)], vec![rail(1)]];
        match (start, end) {
            (FillEnd::Wraps, FillEnd::Wraps) => {}
            (FillEnd::Edges(a), FillEnd::Edges(b)) if a.len() == 2 && b.len() == 2 => {
                let back = |v: VertexId, u: VUse| {
                    let ve = &vgeo[&v].edges[u.edge];
                    VUse {
                        edge: u.edge,
                        from: if u.from == ve.from { ve.to } else { ve.from },
                    }
                };
                let part = |k: usize| {
                    if k == sp.outer {
                        Part::Fill(e)
                    } else {
                        Part::Spindle(e)
                    }
                };
                // Side 0: its rail back to the start, out to the pole, back
                // to the end. Side 1: from the pole to the start, its rail,
                // back to the pole.
                let s0 = self.vuse(s, edge.start, &vgeo[&edge.start], a[0], part(0));
                let e0 = self.vuse(s, edge.end, &vgeo[&edge.end], back(edge.end, b[0]), part(0));
                loops[0].extend([s0, e0]);
                let s1 = self.vuse(s, edge.start, &vgeo[&edge.start], a[1], part(1));
                let e1 = self.vuse(s, edge.end, &vgeo[&edge.end], back(edge.end, b[1]), part(1));
                loops[1] = vec![s1, rail(1), e1];
            }
            _ => return Err(crate::OffsetError::Uncertain { at: edge.name }.into()),
        }
        for k in [sp.outer, 1 - sp.outer] {
            let (surface, sense, part) = if k == sp.outer {
                (fill.clone(), sense, Part::Fill(e))
            } else {
                (sp.inner.clone(), sp.inner_sense, Part::Spindle(e))
            };
            self.face(
                surface,
                sense,
                vec![Boundary::Arcs(loops[k].clone())],
                side.flip,
                (s, part),
            );
        }
        Ok(())
    }
}

/// Builds the offset body from its one side.
pub(crate) fn build(
    key: OpKey,
    side: &Side,
    meter: &crate::work::Meter,
) -> Result<(Body, Journal), Error> {
    let mut laid = Laid::new(side.plan.body);
    laid.lay(0, side)?;
    finish(key, side.plan, side.vgeo, laid, meter)
}

/// The layout built as solids shaped as the body's, named, given every
/// pcurve, journalled and checked.
fn finish(
    key: OpKey,
    plan: &Plan,
    vgeo: &BTreeMap<VertexId, VertexGeo>,
    laid: Laid,
    meter: &crate::work::Meter,
) -> Result<(Body, Journal), Error> {
    let body = plan.body;
    let first = |parts: &[usize]| -> Name {
        parts
            .iter()
            .find_map(|&k| match laid.parts[k].1 {
                Part::Face(f) => Some(body.faces[f].name),
                _ => None,
            })
            .unwrap_or(Name::from_u128(0))
    };
    let all: Vec<usize> = (0..laid.parts.len()).collect();
    let fail = |e: BuildError| -> Error {
        match e {
            BuildError::NonManifold { faces, .. } => {
                crate::OffsetError::Uncertain { at: first(&faces) }
            }
            BuildError::Inconsistent => crate::OffsetError::Uncertain { at: first(&all) },
        }
        .into()
    };
    let role = Role::Offset(OffsetRole::Point);
    let built = if body.kind() == BodyKind::Sheet {
        layout::build_sheet(&laid.layout, key, role).map_err(fail)?
    } else {
        solids(key, body, &laid, role).map_err(|e| match e {
            Some(e) => fail(e),
            None => crate::OffsetError::Uncertain { at: first(&all) }.into(),
        })?
    };
    let mut out = built.body;
    out.set_recipes(true);
    let mut journal = Journal::new();
    // Faces.
    let mut face_names: BTreeMap<usize, Name> = BTreeMap::new();
    for (k, &f) in built.faces.iter().enumerate() {
        let name = match laid.parts[k].1 {
            Part::Face(o) => {
                let n = body.faces[o].name;
                journal.push(Event::Modified(n));
                n
            }
            Part::Fill(e) => out.derive_name(
                "offset",
                key,
                Role::Offset(OffsetRole::Edge),
                &[body.edges[e].name],
            ),
            Part::Sphere(v) => out.derive_name(
                "offset",
                key,
                Role::Offset(OffsetRole::Vertex),
                &[body.vertices[v].name],
            ),
            Part::Fan(v, m) => out.derive_name(
                "offset",
                key,
                Role::Offset(OffsetRole::Vertex),
                &[body.vertices[v].name, Name::from_u128(m as u128 + 1)],
            ),
            Part::Spindle(e) => out.derive_name(
                "offset",
                key,
                Role::Offset(OffsetRole::Edge),
                &[body.edges[e].name, Name::from_u128(1)],
            ),
            Part::Wall(_) => return Err(crate::OffsetError::Uncertain { at: first(&all) }.into()),
        };
        face_names.insert(k, name);
        out.rename(EntityId::Face(f), name);
    }
    let part_name = |p: Part| -> Name {
        match p {
            Part::Face(f) => body.faces[f].name,
            Part::Fill(e) => {
                Name::derive(key, Role::Offset(OffsetRole::Edge), &[body.edges[e].name])
            }
            Part::Sphere(v) => Name::derive(
                key,
                Role::Offset(OffsetRole::Vertex),
                &[body.vertices[v].name],
            ),
            Part::Fan(v, m) => Name::derive(
                key,
                Role::Offset(OffsetRole::Vertex),
                &[body.vertices[v].name, Name::from_u128(m as u128 + 1)],
            ),
            Part::Spindle(e) => Name::derive(
                key,
                Role::Offset(OffsetRole::Edge),
                &[body.edges[e].name, Name::from_u128(1)],
            ),
            Part::Wall(e) => body.edges[e].name,
        }
    };
    // Edges.
    let mut seen: BTreeSet<Name> = BTreeSet::new();
    for (&g, &e) in &built.edges {
        let name = match laid.roles[g] {
            EdgeRole::Kept(_, o) | EdgeRole::Laminar(_, o) => {
                let n = body.edges[o].name;
                journal.push(Event::Modified(n));
                n
            }
            EdgeRole::Rail(_, o, side) => {
                let fill = part_name(Part::Fill(o));
                let face = body.faces[plan.face_of(plan.fins(o)[side])].name;
                let n =
                    out.derive_name("offset", key, Role::Offset(OffsetRole::Rail), &[fill, face]);
                journal.push(Event::Created(n));
                n
            }
            EdgeRole::Normal(_) => {
                return Err(crate::OffsetError::Uncertain { at: first(&all) }.into());
            }
            EdgeRole::Vertex(_, v, parts) => {
                let names = parts.map(part_name);
                let vn = body.vertices[v].name;
                let n = match parts {
                    [Part::Fill(_), Part::Wall(_)] => out.derive_name(
                        "offset",
                        key,
                        Role::Offset(OffsetRole::End),
                        &[names[0], vn],
                    ),
                    [Part::Fill(_), Part::Face(_)] => out.derive_name(
                        "offset",
                        key,
                        Role::Offset(OffsetRole::End),
                        &[names[0], names[1], vn],
                    ),
                    _ => {
                        let mut s = names;
                        s.sort();
                        out.derive_name(
                            "offset",
                            key,
                            Role::Offset(OffsetRole::Section),
                            &[s[0], s[1], vn],
                        )
                    }
                };
                journal.push(Event::Created(n));
                n
            }
        };
        if !seen.insert(name) {
            return Err(crate::OffsetError::Uncertain { at: name }.into());
        }
        out.rename(EntityId::Edge(e), name);
    }
    // Vertices: a vertex that stays one keeps its name; any other is
    // named by the edges meeting there.
    let kept_points: BTreeMap<usize, VertexId> = laid
        .points
        .iter()
        .filter(|((_, v, _), _)| vgeo[v].kept)
        .map(|(&(_, v, _), &lv)| (lv, v))
        .collect();
    for (&g, &v) in &built.vertices {
        let name = match kept_points.get(&g) {
            Some(&o) => {
                let n = body.vertices[o].name;
                journal.push(Event::Modified(n));
                n
            }
            None => {
                let mut edges: Vec<Name> = out
                    .vertex_edges(v)
                    .into_iter()
                    .map(|e| out.edges[e].name)
                    .collect();
                edges.sort();
                edges.dedup();
                let n = out.derive_name("offset", key, Role::Offset(OffsetRole::Point), &edges);
                journal.push(Event::Created(n));
                n
            }
        };
        if !seen.insert(name) {
            return Err(crate::OffsetError::Uncertain { at: name }.into());
        }
        out.rename(EntityId::Vertex(v), name);
    }
    // What fills replace: each opening edge, and each vertex that is no
    // longer one vertex, by the fills there.
    let mut fills_at: BTreeMap<VertexId, Vec<Name>> = BTreeMap::new();
    for (k, &(_, p)) in laid.parts.iter().enumerate() {
        match p {
            Part::Fill(e) => {
                journal.push(Event::Replaced {
                    old: body.edges[e].name,
                    new: vec![face_names[&k]],
                });
                let edge = &body.edges[e];
                for v in [edge.start, edge.end] {
                    fills_at.entry(v).or_default().push(face_names[&k]);
                }
            }
            Part::Sphere(v) | Part::Fan(v, _) => {
                fills_at.entry(v).or_default().push(face_names[&k])
            }
            Part::Face(_) | Part::Wall(_) | Part::Spindle(_) => {}
        }
    }
    for (v, mut new) in fills_at {
        if vgeo[&v].kept {
            continue;
        }
        new.sort();
        new.dedup();
        journal.push(Event::Replaced {
            old: body.vertices[v].name,
            new,
        });
    }
    pcurves(&mut out, meter).map_err(|_| crate::OffsetError::Uncertain { at: first(&all) })?;
    if plan.mode == super::classify::Mode::Local {
        judge(&out, meter)?;
    }
    Ok((out, journal))
}

/// The layout built as solids shaped as the body's: each new shell is the
/// body's shell its faces' offsets come from, outer shells first, each
/// void in the region of its outer shell. `None` where a shell has no
/// face's offset, or a void's outer shell isn't there.
fn solids(
    key: OpKey,
    body: &Body,
    laid: &Laid,
    role: Role,
) -> Result<layout::Built, Option<BuildError>> {
    let lp = layout::Plan::new(&laid.layout).map_err(Some)?;
    let shells = lp.shells();
    let shell_of = |s: &[usize]| {
        s.iter().find_map(|&k| match laid.parts[k].1 {
            Part::Face(f) => Some(body.faces[f].shell),
            _ => None,
        })
    };
    let mut outer_of: Vec<Option<usize>> = vec![None; shells.len()];
    for (i, s) in shells.iter().enumerate() {
        let sh = shell_of(s).ok_or(None)?;
        let region = body.shells[sh].region;
        let outer = body.regions[region].first_shell;
        if body.regions[region].kind == RegionKind::Solid && outer != Some(sh) {
            outer_of[i] = shells.iter().position(|x| shell_of(x) == outer);
            if outer_of[i].is_none() {
                return Err(None);
            }
        }
    }
    lp.build(key, role, &shells, &outer_of).map_err(Some)
}

/// Gives every fin without a pcurve one: exact where
/// [`crate::pcurve::exact`] finds it, else fitted, the edge's tolerance
/// covering the fit.
pub(crate) fn pcurves(body: &mut Body, meter: &crate::work::Meter) -> Result<(), Error> {
    let fins: Vec<_> = body
        .fins()
        .filter(|&f| body.fins[f].pcurve.is_none())
        .collect();
    for fin in fins {
        let e = body.fins[fin].edge;
        let face = body.loops[body.fins[fin].loop_].face;
        let surface: Surface = body.surfaces[body.faces[face].surface].clone();
        let edge = body.edges[e].clone();
        let curve = body.curves[edge.curve].clone();
        let tol = edge.tolerance.get();
        let (c, map, bound) = match crate::pcurve::exact(&surface, &curve, edge.interval, tol) {
            Some((c, m)) => (c, m, tol),
            None => {
                let (n, bound) =
                    crate::intersect::fit_pcurve(&surface, &curve, edge.interval, meter)
                        .map_err(|_| Error::NoPcurve { edge: e, face })?;
                (Curve2::Nurbs2(std::sync::Arc::new(n)), [0.0, 1.0], bound)
            }
        };
        let id = body.add_curve2(c);
        crate::euler::set_pcurve(body, fin, Some(Pcurve { curve: id, map }))?;
        if bound > tol {
            let t = Tolerance::new(bound).map_err(|_| Error::Degenerate)?;
            crate::euler::set_edge_tolerance(body, e, t)?;
            for v in [edge.start, edge.end] {
                if body.vertices[v].tolerance.get() < bound {
                    crate::euler::set_vertex_tolerance(body, v, t)?;
                }
            }
        }
    }
    Ok(())
}

/// The result must check: sound, and its faces meeting only where they
/// share edges.
pub(crate) fn judge(body: &Body, meter: &crate::work::Meter) -> Result<(), Error> {
    let faults = check(body);
    if let Some(f) = faults.first() {
        let at = body
            .faces()
            .next()
            .map_or(Name::from_u128(0), |x| body.faces[x].name);
        let _ = f;
        return Err(crate::OffsetError::Uncertain { at }.into());
    }
    let name = |f: FaceId| body.faces[f].name;
    let mut faults = check_intersections_in(body, meter);
    faults.sort_by_key(|f| match f {
        Fault::FacesCross { a, b } | Fault::FacesOverlap { a, b } | Fault::CannotCheck { a, b } => {
            let (x, y) = (name(*a), name(*b));
            (x.min(y), x.max(y))
        }
        _ => (Name::from_u128(0), Name::from_u128(0)),
    });
    match faults.first() {
        None => Ok(()),
        Some(Fault::FacesCross { a, b } | Fault::FacesOverlap { a, b }) => {
            let mut faces = [name(*a), name(*b)];
            faces.sort();
            Err(OffsetError::SelfIntersecting { faces }.into())
        }
        Some(Fault::CannotCheck { a, .. }) => {
            Err(crate::OffsetError::Uncertain { at: name(*a) }.into())
        }
        Some(_) => Err(OffsetError::Uncertain {
            at: name(body.faces().next().expect("a face")),
        }
        .into()),
    }
}
