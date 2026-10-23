//! The healed shape into a body: laid out, built through Euler operators,
//! named, checked.

use super::names::Names;
use super::pcurves::Pcurves;
use super::{HealOptions, HealReport, Loop, Shape};
use crate::check::{self, Fault};
use crate::layout::{self, BuildError, Layout, Plan};
use crate::name::{Event, ImportRole, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EntityId, Sense};
use crate::{Error, HealError, TopologyError};
use std::collections::BTreeSet;

/// Every edge used twice, once each way.
pub(super) fn uses(s: &Shape, names: &Names) -> Result<(), Error> {
    let mut senses: Vec<Vec<Sense>> = vec![Vec::new(); s.edges.len()];
    for f in &s.faces {
        for l in &f.loops {
            if let Loop::Edges(list) = l {
                for u in list {
                    senses[u.edge].push(u.sense);
                }
            }
        }
    }
    let mut free: Vec<Name> = (0..s.edges.len())
        .filter(|&e| senses[e].len() == 1)
        .map(|e| names.edges[e])
        .collect();
    if !free.is_empty() {
        free.sort();
        return Err(HealError::OpenShell { free_edges: free }.into());
    }
    let mut first: Option<HealError> = None;
    for (e, list) in senses.iter().enumerate() {
        let fault = if list.len() > 2 {
            HealError::NonManifold { at: names.edges[e] }
        } else if list.len() == 2 && list[0] == list[1] {
            HealError::Misoriented { at: names.edges[e] }
        } else {
            continue;
        };
        if first.as_ref().is_none_or(|f| key(&fault) < key(f)) {
            first = Some(fault);
        }
    }
    first.map_or(Ok(()), |f| Err(f.into()))
}

/// The name a use fault is reported by, for choosing one by name.
fn key(f: &HealError) -> Name {
    match f {
        HealError::NonManifold { at } => *at,
        HealError::Misoriented { at } => *at,
        _ => Name::from_u128(0),
    }
}

/// The scaffold names' role: every entity is renamed afterwards.
const ROLE: Role = Role::Import(ImportRole::Face);

pub(super) fn build(
    key: OpKey,
    s: &Shape,
    names: &Names,
    pcurves: &Pcurves,
    options: &HealOptions,
) -> Result<(Body, Journal, HealReport), Error> {
    let mut report = HealReport::default();
    for (k, f) in s.faces.iter().enumerate() {
        if f.tolerance > options.tolerance_cap {
            return Err(HealError::GapTooLarge {
                entity: names.faces[k],
                gap: f.tolerance,
            }
            .into());
        }
        if f.tolerance > 0.0 {
            report.approximated.push((names.faces[k], f.tolerance));
        }
    }
    report.approximated.sort_by_key(|x| x.0);
    let layout = lay_out(s, pcurves);
    let fail = |e: BuildError| -> Error {
        match e {
            BuildError::NonManifold { at, faces } => {
                // The layout's vertex there, else the first face.
                let v = s.vertices.iter().position(|v| v.point == at);
                HealError::NonManifold {
                    at: v.map_or(names.faces[faces[0]], |v| names.vertices[v]),
                }
                .into()
            }
            BuildError::Inconsistent => Error::Topology(TopologyError::Inconsistent),
        }
    };
    let plan = Plan::new(&layout).map_err(fail)?;
    // The shells as the shape lists them, outer shells first: each must
    // be one connected piece.
    let found: BTreeSet<Vec<usize>> = plan.shells().into_iter().collect();
    let mut shells: Vec<Vec<usize>> = Vec::new();
    let mut outer_of: Vec<Option<usize>> = Vec::new();
    for so in &s.solids {
        let o = shells.len();
        for (i, &sh) in core::iter::once(&so.outer).chain(&so.voids).enumerate() {
            let mut faces = s.shells[sh].faces.clone();
            faces.sort_unstable();
            if !found.contains(&faces) {
                return Err(super::malformed("a shell's faces are not connected"));
            }
            shells.push(faces);
            outer_of.push((i > 0).then_some(o));
        }
    }
    let built = plan.build(key, ROLE, &shells, &outer_of).map_err(fail)?;
    let mut body = built.body;
    renumber_geometry(&mut body);
    body.set_recipes(true);
    for (k, &f) in built.faces.iter().enumerate() {
        body.rename(EntityId::Face(f), names.faces[k]);
    }
    for (&g, &e) in &built.edges {
        body.rename(EntityId::Edge(e), names.edges[g]);
    }
    for (&g, &v) in &built.vertices {
        body.rename(EntityId::Vertex(v), names.vertices[g]);
    }
    let faults = check::check(&body);
    if let Some(f) = faults.first() {
        return Err(fault(&body, f));
    }
    // Gaps the tolerances had to grow to cover, past the cap.
    let mut over: Vec<(Name, f64)> = Vec::new();
    for e in body.edges() {
        over.push((body.edges[e].name, body.edges[e].tolerance.get()));
    }
    for v in body.vertices() {
        over.push((body.vertices[v].name, body.vertices[v].tolerance.get()));
    }
    over.retain(|x| x.1 > options.tolerance_cap);
    if let Some(&(entity, gap)) = over.iter().min_by_key(|x| x.0) {
        return Err(HealError::GapTooLarge { entity, gap }.into());
    }
    let mut journal = Journal::new();
    for f in body.faces() {
        journal.push(Event::Created(body.faces[f].name));
    }
    for e in body.edges() {
        journal.push(Event::Created(body.edges[e].name));
    }
    for v in body.vertices() {
        journal.push(Event::Created(body.vertices[v].name));
    }
    for e in body.edges() {
        let t = body.edges[e].tolerance.get();
        if t > LINEAR_RESOLUTION {
            report.tolerances.push((body.edges[e].name, t));
        }
    }
    for v in body.vertices() {
        let t = body.vertices[v].tolerance.get();
        if t > LINEAR_RESOLUTION {
            report.tolerances.push((body.vertices[v].name, t));
        }
    }
    report.tolerances.sort_by_key(|x| x.0);
    Ok((body, journal, report))
}

/// Puts the body's geometry in the order of what uses it: surfaces by
/// face, curves by edge, pcurves by fin. The body then doesn't depend on
/// the order of the shape's arrays or where its loops started.
fn renumber_geometry(b: &mut Body) {
    use crate::topo::{Arena, Curve2Id, CurveId, SurfaceId};
    use std::collections::BTreeMap;
    let mut surfaces: Arena<SurfaceId, _> = Arena::default();
    let mut map: BTreeMap<SurfaceId, SurfaceId> = BTreeMap::new();
    for f in b.faces().collect::<Vec<_>>() {
        let old = b.faces[f].surface;
        let new = *map
            .entry(old)
            .or_insert_with(|| surfaces.push(b.surfaces[old].clone()));
        b.faces[f].surface = new;
    }
    b.surfaces = surfaces;
    let mut curves: Arena<CurveId, _> = Arena::default();
    let mut map: BTreeMap<CurveId, CurveId> = BTreeMap::new();
    for e in b.edges().collect::<Vec<_>>() {
        let old = b.edges[e].curve;
        let new = *map
            .entry(old)
            .or_insert_with(|| curves.push(b.curves[old].clone()));
        b.edges[e].curve = new;
    }
    b.curves = curves;
    let mut curves2: Arena<Curve2Id, _> = Arena::default();
    let mut map: BTreeMap<Curve2Id, Curve2Id> = BTreeMap::new();
    for f in b.fins().collect::<Vec<_>>() {
        if let Some(mut p) = b.fins[f].pcurve {
            let old = p.curve;
            p.curve = *map
                .entry(old)
                .or_insert_with(|| curves2.push(b.curves2[old].clone()));
            b.fins[f].pcurve = Some(p);
        }
    }
    b.curves2 = curves2;
}

/// The shape as a layout: its geometry as it is, the pcurves found, and
/// tolerances at least the resolution, an edge's and its vertices'
/// covering the faces they bound.
fn lay_out(s: &Shape, pcurves: &Pcurves) -> Layout {
    let mut out = Layout {
        surfaces: s.surfaces.clone(),
        curves: s.curves.clone(),
        ..Layout::default()
    };
    let mut edge_tol: Vec<f64> = s.edges.iter().map(|e| e.tolerance).collect();
    let mut vertex_tol: Vec<f64> = s.vertices.iter().map(|v| v.tolerance).collect();
    for (k, f) in s.faces.iter().enumerate() {
        let mut loops = Vec::new();
        for (l, lp) in f.loops.iter().enumerate() {
            match lp {
                Loop::Vertex(v) => {
                    vertex_tol[*v] = vertex_tol[*v].max(f.tolerance);
                    loops.push(layout::Boundary::Lone(*v));
                }
                Loop::Edges(list) => {
                    let mut arcs = Vec::new();
                    for (i, u) in list.iter().enumerate() {
                        let e = &s.edges[u.edge];
                        edge_tol[u.edge] = edge_tol[u.edge].max(f.tolerance);
                        for v in [e.start, e.end] {
                            vertex_tol[v] = vertex_tol[v].max(f.tolerance);
                        }
                        let (c2, map, bound) = &pcurves[k][l][i];
                        edge_tol[u.edge] = edge_tol[u.edge].max(*bound);
                        out.curves2.push(c2.clone());
                        arcs.push(layout::Use {
                            edge: u.edge,
                            sense: u.sense,
                            pcurve: Some((out.curves2.len() - 1, *map)),
                        });
                    }
                    loops.push(layout::Boundary::Arcs(arcs));
                }
            }
        }
        out.faces.push(layout::Face {
            surface: f.surface,
            sense: f.sense,
            loops,
        });
    }
    out.vertices = s
        .vertices
        .iter()
        .zip(&vertex_tol)
        .map(|(v, &t)| layout::Vertex {
            point: v.point,
            tolerance: t.max(LINEAR_RESOLUTION),
        })
        .collect();
    out.edges = s
        .edges
        .iter()
        .zip(&edge_tol)
        .map(|(e, &t)| layout::Edge {
            start: e.start,
            end: e.end,
            curve: e.curve,
            interval: e.interval,
            tolerance: t.max(LINEAR_RESOLUTION),
        })
        .collect();
    out
}

/// The refusal for the first fault the built body has.
fn fault(b: &Body, f: &Fault) -> Error {
    let face = |f: crate::topo::FaceId| b.faces[f].name;
    let gap = |entity: Name, gap: f64| HealError::GapTooLarge { entity, gap }.into();
    match *f {
        Fault::VertexOffCurve {
            vertex, distance, ..
        }
        | Fault::VertexOffSurface {
            vertex, distance, ..
        } => gap(b.vertices[vertex].name, distance),
        Fault::EdgeOffSurface { edge, distance, .. } => gap(b.edges[edge].name, distance),
        Fault::PcurveOffEdge { fin, .. } => {
            let fin = &b.fins[fin];
            HealError::NoPcurve {
                edge: b.edges[fin.edge].name,
                face: face(b.loops[fin.loop_].face),
            }
            .into()
        }
        Fault::Loopless { face: f } => HealError::BadDomain { face: face(f) }.into(),
        Fault::FaceInsideOut { face: f } => HealError::Misoriented { at: face(f) }.into(),
        Fault::NotOutward { region } => {
            let at = b
                .region_shells(region)
                .flat_map(|s| b.shell_faces(s))
                .map(face)
                .min()
                .unwrap_or(Name::from_u128(0));
            HealError::Misoriented { at }.into()
        }
        Fault::OutsideSizeBox { .. } => Error::OutsideSizeBox,
        _ => Error::Topology(TopologyError::Inconsistent),
    }
}
