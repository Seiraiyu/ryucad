//! The offset body built: its faces, edges and vertices laid out (see
//! [`crate::layout`]), named, given every pcurve, and checked.

use super::classify::Plan;
use super::edges::{EdgeGeo, NewCurve};
use super::vertices::{FillEnd, Part, VUse, VertexGeo};
use crate::check::{Fault, check, check_intersections};
use crate::geom::{Curve, Curve2, Surface};
use crate::layout::{self, Boundary, BuildError, Layout, Use};
use crate::name::{Event, Journal, Name, OffsetRole, OpKey, Role};
use crate::tol::{LINEAR_RESOLUTION, Tolerance};
use crate::topo::{Body, EdgeId, EntityId, FaceId, Pcurve, RegionKind, Sense, VertexId};
use crate::{Error, OffsetCase, OffsetError};
use std::collections::{BTreeMap, BTreeSet};

/// What a layout edge is, for naming.
#[derive(Clone, Copy, Debug)]
enum EdgeRole {
    /// The edge an original edge became, keeping its name.
    Kept(EdgeId),
    /// An opening edge's rail on its fin's face, by side.
    Rail(EdgeId, usize),
    /// A vertex's edge between two of the result's faces.
    Vertex(VertexId, [Part; 2]),
}

/// The layout under construction, and what each of its parts is.
struct Laid<'a> {
    plan: &'a Plan<'a>,
    layout: Layout,
    /// Each vertex's points' layout vertices.
    points: BTreeMap<(VertexId, usize), usize>,
    /// Each original edge's layout edges: one, or the two rails.
    edges: BTreeMap<EdgeId, Vec<usize>>,
    /// Each vertex's own edges' layout edges.
    vedges: BTreeMap<(VertexId, usize), usize>,
    roles: Vec<EdgeRole>,
    /// Each layout face's part.
    parts: Vec<Part>,
}

fn sense_of(forward: bool) -> Sense {
    if forward {
        Sense::Forward
    } else {
        Sense::Reversed
    }
}

fn flip(s: Sense) -> Sense {
    match s {
        Sense::Forward => Sense::Reversed,
        Sense::Reversed => Sense::Forward,
    }
}

impl<'a> Laid<'a> {
    fn curve2(&mut self, c: &Option<(Curve2, [f64; 2])>) -> Option<(usize, [f64; 2])> {
        let (c, map) = c.as_ref()?;
        self.layout.curves2.push(c.clone());
        Some((self.layout.curves2.len() - 1, *map))
    }

    fn add_edge(
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

    /// A new curve cut between its ends at the edge's vertices, as a
    /// layout edge running with the curve. `EdgeVanishes` where its ends
    /// come in the wrong order.
    fn cut(
        &mut self,
        e: EdgeId,
        c: &NewCurve,
        ends: [(usize, f64); 2],
        role: EdgeRole,
    ) -> Result<usize, Error> {
        let edge = &self.plan.body.edges[e];
        let at = edge.name;
        let a = (self.points[&(edge.start, ends[0].0)], ends[0].1);
        let b = (self.points[&(edge.end, ends[1].0)], ends[1].1);
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

    fn vuse(&mut self, v: VertexId, g: &VertexGeo, u: VUse, part: Part) -> Use {
        let e = &g.edges[u.edge];
        let pc = e
            .parts
            .iter()
            .find(|p| p.0 == part)
            .and_then(|p| p.1.clone());
        Use {
            edge: self.vedges[&(v, u.edge)],
            sense: sense_of(u.from == e.from),
            pcurve: self.curve2(&pc),
        }
    }
}

/// Builds the offset body from its plan, edges and vertices.
pub(crate) fn build(
    key: OpKey,
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    vgeo: &BTreeMap<VertexId, VertexGeo>,
) -> Result<(Body, Journal), Error> {
    let body = plan.body;
    let mut laid = Laid {
        plan,
        layout: Layout::default(),
        points: BTreeMap::new(),
        edges: BTreeMap::new(),
        vedges: BTreeMap::new(),
        roles: Vec::new(),
        parts: Vec::new(),
    };
    // Vertices, in the order of the vertices they come from.
    for (&v, g) in vgeo {
        for (k, &p) in g.points.iter().enumerate() {
            laid.layout.vertices.push(layout::Vertex {
                point: p,
                tolerance: LINEAR_RESOLUTION,
            });
            laid.points.insert((v, k), laid.layout.vertices.len() - 1);
        }
    }
    // Edges: each original edge's, then each vertex's.
    for &e in &plan.edges {
        let edge = &body.edges[e];
        let (s, t) = ((e, false), (e, true));
        let ids = match &geo[&e] {
            EdgeGeo::One(c) => {
                let ends = [vgeo[&edge.start].one[&s], vgeo[&edge.end].one[&t]];
                vec![laid.cut(e, c, ends, EdgeRole::Kept(e))?]
            }
            EdgeGeo::Open { rails, .. } => {
                let (a, b) = (vgeo[&edge.start].open[&s], vgeo[&edge.end].open[&t]);
                let mut ids = Vec::new();
                for side in 0..2 {
                    ids.push(laid.cut(
                        e,
                        &rails[side],
                        [a[side], b[side]],
                        EdgeRole::Rail(e, side),
                    )?);
                }
                ids
            }
        };
        laid.edges.insert(e, ids);
    }
    for (&v, g) in vgeo {
        for (k, ve) in g.edges.iter().enumerate() {
            let (a, b) = (laid.points[&(v, ve.from)], laid.points[&(v, ve.to)]);
            let id = laid.add_edge(
                &ve.curve,
                ve.range,
                (a, b),
                ve.tolerance,
                EdgeRole::Vertex(v, [ve.parts[0].0, ve.parts[1].0]),
            );
            laid.vedges.insert((v, k), id);
        }
    }
    // Faces: each face's offset, in name order.
    let mut faces: Vec<FaceId> = body.faces().collect();
    faces.sort_by_key(|&f| body.faces[f].name);
    for &f in &faces {
        let face = &body.faces[f];
        laid.layout.surfaces.push(plan.surface[&f].clone());
        let surface = laid.layout.surfaces.len() - 1;
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
                let side = usize::from(plan.fins(e)[0] != fin);
                match &geo[&e] {
                    EdgeGeo::One(c) => {
                        let sense = if c.reversed { flip(fi.sense) } else { fi.sense };
                        let pc = laid.curve2(&c.pcurves[side]);
                        arcs.push(Use {
                            edge: laid.edges[&e][0],
                            sense,
                            pcurve: pc,
                        });
                    }
                    EdgeGeo::Open { rails, .. } => {
                        let pc = laid.curve2(&rails[side].pcurves[0]);
                        arcs.push(Use {
                            edge: laid.edges[&e][side],
                            sense: fi.sense,
                            pcurve: pc,
                        });
                    }
                }
                let v = body.end_of(fin);
                if let Some(ins) = vgeo[&v].inserts.get(&fin) {
                    for &u in ins {
                        let x = laid.vuse(v, &vgeo[&v], u, Part::Face(f));
                        arcs.push(x);
                    }
                }
            }
            loops.push(Boundary::Arcs(arcs));
        }
        laid.layout.faces.push(layout::Face {
            surface,
            sense: face.sense,
            loops,
        });
        laid.parts.push(Part::Face(f));
    }
    // Fills, in the order of their edges' names.
    for &e in &plan.edges {
        let EdgeGeo::Open { fill, sense, .. } = &geo[&e] else {
            continue;
        };
        let edge = &body.edges[e];
        laid.layout.surfaces.push(fill.clone());
        let surface = laid.layout.surfaces.len() - 1;
        let rails = laid.edges[&e].clone();
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
                    arcs.push(laid.vuse(edge.start, &vgeo[&edge.start], u, Part::Fill(e)));
                }
                arcs.push(r1);
                // At the end, from the second rail back to the first.
                for &u in b.iter().rev() {
                    let ve = &vgeo[&edge.end].edges[u.edge];
                    let back = VUse {
                        edge: u.edge,
                        from: if u.from == ve.from { ve.to } else { ve.from },
                    };
                    arcs.push(laid.vuse(edge.end, &vgeo[&edge.end], back, Part::Fill(e)));
                }
                vec![Boundary::Arcs(arcs)]
            }
            _ => return Err(crate::OffsetError::Uncertain { at: edge.name }.into()),
        };
        laid.layout.faces.push(layout::Face {
            surface,
            sense: *sense,
            loops,
        });
        laid.parts.push(Part::Fill(e));
    }
    // Spheres, in the order of their vertices.
    for (&v, g) in vgeo {
        let Some((s, sense, lp)) = &g.sphere else {
            continue;
        };
        laid.layout.surfaces.push(s.clone());
        let surface = laid.layout.surfaces.len() - 1;
        let arcs = lp
            .iter()
            .map(|&u| laid.vuse(v, g, u, Part::Sphere(v)))
            .collect();
        laid.layout.faces.push(layout::Face {
            surface,
            sense: *sense,
            loops: vec![Boundary::Arcs(arcs)],
        });
        laid.parts.push(Part::Sphere(v));
    }
    finish(key, plan, vgeo, laid)
}

/// The layout built as solids shaped as the body's, named, given every
/// pcurve, journalled and checked.
fn finish(
    key: OpKey,
    plan: &Plan,
    vgeo: &BTreeMap<VertexId, VertexGeo>,
    laid: Laid,
) -> Result<(Body, Journal), Error> {
    let body = plan.body;
    let first = |parts: &[usize]| -> Name {
        parts
            .iter()
            .find_map(|&k| match laid.parts[k] {
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
    let lp = layout::Plan::new(&laid.layout).map_err(fail)?;
    // Each new shell is the body's shell its faces' offsets come from:
    // outer shells first, each void in the region of its outer shell.
    let shells = lp.shells();
    let shell_of = |s: &[usize]| {
        s.iter().find_map(|&k| match laid.parts[k] {
            Part::Face(f) => Some(body.faces[f].shell),
            _ => None,
        })
    };
    let mut outer_of: Vec<Option<usize>> = vec![None; shells.len()];
    for (i, s) in shells.iter().enumerate() {
        let Some(sh) = shell_of(s) else {
            return Err(crate::OffsetError::Uncertain { at: first(&all) }.into());
        };
        let region = body.shells[sh].region;
        let outer = body.regions[region].first_shell;
        if body.regions[region].kind == RegionKind::Solid && outer != Some(sh) {
            outer_of[i] = shells.iter().position(|x| shell_of(x) == outer);
            if outer_of[i].is_none() {
                return Err(crate::OffsetError::Uncertain { at: first(s) }.into());
            }
        }
    }
    let role = Role::Offset(OffsetRole::Point);
    let built = lp.build(key, role, &shells, &outer_of).map_err(fail)?;
    let mut out = built.body;
    out.set_recipes(true);
    let mut journal = Journal::new();
    // Faces.
    let mut face_names: BTreeMap<usize, Name> = BTreeMap::new();
    for (k, &f) in built.faces.iter().enumerate() {
        let name = match laid.parts[k] {
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
        }
    };
    // Edges.
    let mut seen: BTreeSet<Name> = BTreeSet::new();
    for (&g, &e) in &built.edges {
        let name = match laid.roles[g] {
            EdgeRole::Kept(o) => {
                let n = body.edges[o].name;
                journal.push(Event::Modified(n));
                n
            }
            EdgeRole::Rail(o, side) => {
                let fill = part_name(Part::Fill(o));
                let face = body.faces[plan.face_of(plan.fins(o)[side])].name;
                let n =
                    out.derive_name("offset", key, Role::Offset(OffsetRole::Rail), &[fill, face]);
                journal.push(Event::Created(n));
                n
            }
            EdgeRole::Vertex(v, parts) => {
                let names = parts.map(part_name);
                let vn = body.vertices[v].name;
                let n = match parts {
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
        .filter(|((v, _), _)| vgeo[v].kept)
        .map(|(&(v, _), &lv)| (lv, v))
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
    for (k, &p) in laid.parts.iter().enumerate() {
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
            Part::Sphere(v) => fills_at.entry(v).or_default().push(face_names[&k]),
            Part::Face(_) => {}
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
    pcurves(&mut out).map_err(|_| crate::OffsetError::Uncertain { at: first(&all) })?;
    judge(&out)?;
    Ok((out, journal))
}

/// Gives every fin without a pcurve one: exact where
/// [`crate::pcurve::exact`] finds it, else fitted, the edge's tolerance
/// covering the fit.
fn pcurves(body: &mut Body) -> Result<(), Error> {
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
                let (n, bound) = crate::intersect::fit_pcurve(&surface, &curve, edge.interval)
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
fn judge(body: &Body) -> Result<(), Error> {
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
    let mut faults = check_intersections(body);
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
