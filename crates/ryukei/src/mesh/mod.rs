//! Tessellation: a body's faces as triangles certified to lie within a
//! chord tolerance of the exact geometry.
//!
//! [`tessellate`] works in stages. Each edge is sampled once, finely
//! enough for its curve and for every face it bounds; its samples are
//! shared by those faces, so the mesh is watertight by index. Each face is
//! then triangulated in its parameter plane from its edges' samples and
//! refined until every triangle passes the certified chord and angle
//! tests. A face that needs a boundary segment split asks its edge for a
//! sample there, and the faces on that edge are meshed again (at most
//! three rounds). Last, the pieces are assembled in ID order.
//!
//! A face that can't be meshed within the guarantees is left out and
//! reported as a [`FaceFault`]; every other face is still meshed.
//!
//! Edges, and then faces, are meshed in parallel on the current rayon
//! pool (a caller picks the threads with `ThreadPool::install`). Each is a
//! pure function of the body, the parameters and the edges' samples, and
//! the results are collected in ID order, so the mesh is the same bits on
//! one thread or many.

mod assemble;
mod bound;
mod cdt;
mod check;
mod domain;
mod edge;
mod params;
mod refine;
pub(crate) mod uv;

pub use check::{MeshCheckFault, check_mesh};
pub use params::MeshParams;

use crate::Error;
use crate::geom::{Point2, Point3, UnitVector3};
use crate::topo::{Body, EdgeId, FaceId};
use bound::Targets;
use core::ops::Range;
use domain::{Anchor, Domain, gather};
use edge::{EdgeSamples, discretize};
use rayon::prelude::*;
use refine::{Chord, Stop, refine};
use uv::FaceGeom;

/// A body's mesh.
///
/// `positions` holds every point once: first the body's live vertices in
/// `VertexId` order, then each edge's inner samples in `EdgeId` order,
/// then the points inside faces. A point shared by several faces (a vertex,
/// an edge sample, a seam point) is one position, so the mesh is
/// watertight by index. What differs per face, its parameters and its
/// normal, is in `corners`.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyMesh {
    pub positions: Vec<Point3>,
    pub corners: Vec<Corner>,
    /// Indices into `corners`, counterclockwise seen from outside the
    /// solid.
    pub triangles: Vec<[u32; 3]>,
    /// The meshed faces in `FaceId` order: faulted faces are left out.
    pub faces: Vec<FaceMesh>,
    /// Every live edge in `EdgeId` order.
    pub edges: Vec<EdgePolyline>,
    /// Empty when every guarantee holds.
    pub faults: Vec<FaceFault>,
}

/// One face's use of a position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub position: u32,
    /// The surface parameters, unwrapped into one period: the two sides of
    /// a seam have different parameters at one position.
    pub uv: Point2,
    /// The face's outward unit normal. At a pole or an apex it is the
    /// surface's limit normal from this corner's side.
    pub normal: UnitVector3,
}

/// One face's corners and triangles.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceMesh {
    pub face: FaceId,
    pub corners: Range<u32>,
    pub triangles: Range<u32>,
}

/// The positions along an edge, from its start vertex to its end, and
/// their curve parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgePolyline {
    pub edge: EdgeId,
    pub positions: Vec<u32>,
    pub params: Vec<f64>,
}

/// A face left out of the mesh, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceFault {
    pub face: FaceId,
    pub kind: MeshFaultKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshFaultKind {
    /// An edge's samples lie farther from the face's surface than the
    /// edge's tolerance.
    EdgeOffSurface { edge: EdgeId },
    /// The face's loops don't make a region of its parameter plane.
    BadDomain,
    /// A triangle (or edge segment) fails its tests even at the smallest
    /// size refinement goes to.
    RefinementStalled,
    /// The face still needed a sample on this edge after the last round.
    BoundaryNotResolved { edge: EdgeId },
}

impl BodyMesh {
    /// The triangles as indices into `positions`: welded, for export and
    /// clash detection.
    pub fn welded_triangles(&self) -> impl Iterator<Item = [u32; 3]> + '_ {
        self.triangles
            .iter()
            .map(|t| t.map(|c| self.corners[c as usize].position))
    }
}

/// The round after which an edge still asked for samples is halved all
/// along rather than split where asked.
const HALVE_AFTER: usize = 3;

/// Rounds of boundary split requests before a face gives up. Most faces
/// need none or one; a thin tube's band (a slot turned whole) asks for its
/// boundary circles several times finer than the edge pass gives, and each
/// round only halves the segments it asks about, so it takes about ten.
const ROUNDS: usize = 12;

/// One face's state between rounds.
struct FaceWork {
    domain: Option<Domain>,
    fault: Option<MeshFaultKind>,
    requests: Vec<(EdgeId, f64)>,
    too_large: bool,
    triangles: u64,
}

/// Meshes a body. See the module documentation.
///
/// Fails with `InvalidMeshParams`, or `MeshTooLarge` when the mesh would
/// pass `params.max_triangles`.
pub fn tessellate(body: &Body, params: &MeshParams) -> Result<BodyMesh, Error> {
    let targets = params.targets()?;
    debug_assert!(crate::check::check_topology(body).is_empty());
    let faces: Vec<FaceId> = body.faces().collect();
    let mut geoms: Vec<Option<FaceGeom>> = vec![None; body.faces.slot_count()];
    for (f, g) in faces
        .par_iter()
        .map(|&f| (f, FaceGeom::new(body, f)))
        .collect::<Vec<_>>()
    {
        geoms[f.index()] = Some(g);
    }
    let edges: Vec<EdgeId> = body.edges().collect();
    let mut samples: Vec<Option<EdgeSamples>> = vec![None; body.edges.slot_count()];
    for (e, s) in edges
        .par_iter()
        .map(|&e| (e, discretize(body, e, &geoms, &targets, &[])))
        .collect::<Vec<_>>()
    {
        samples[e.index()] = Some(s);
    }
    let triangles = estimate(body, &geoms, &samples, params);
    if triangles > params.max_triangles {
        return Err(Error::MeshTooLarge { triangles });
    }
    let mut extras: Vec<Vec<f64>> = vec![Vec::new(); body.edges.slot_count()];
    let mut work: Vec<Option<FaceWork>> = (0..body.faces.slot_count()).map(|_| None).collect();
    // Chords between boundary points that faces split (see
    // `refine::chords`).
    let mut split: Vec<Chord> = Vec::new();
    let mut todo: Vec<FaceId> = faces;
    for round in 0..=ROUNDS {
        let done: Vec<(FaceId, FaceWork)> = todo
            .par_iter()
            .map(|&f| {
                let g = geoms[f.index()].as_ref().expect("live face");
                (
                    f,
                    mesh_face(body, g, &samples, &targets, params.max_triangles, &split),
                )
            })
            .collect();
        for (f, w) in done {
            work[f.index()] = Some(w);
        }
        let mut requests: Vec<(EdgeId, f64)> = todo
            .iter()
            .flat_map(|f| work[f.index()].as_ref().expect("meshed").requests.clone())
            .collect();
        if requests.is_empty() && round < ROUNDS {
            // Chords between boundary points drawn by two faces.
            let shared = shared_chords(body, &work);
            if !shared.is_empty() {
                todo = shared
                    .iter()
                    .flat_map(|(_, faces)| faces.iter().copied())
                    .collect();
                todo.sort();
                todo.dedup();
                split.extend(shared.into_iter().map(|(c, _)| c));
                split.sort();
                split.dedup();
                continue;
            }
        }
        if requests.is_empty() || round == ROUNDS {
            break;
        }
        requests.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        requests.dedup();
        let mut redo: Vec<FaceId> = Vec::new();
        for &(e, t) in &requests {
            extras[e.index()].push(t);
        }
        // Remeshing a face moves its triangles, and now and then they ask
        // for boundary samples somewhere new every round. Past the first
        // few rounds, an edge still asked about is halved all along, so
        // its samples close in geometrically and the asking stops.
        if round + 1 >= HALVE_AFTER {
            let mut asked: Vec<EdgeId> = requests.iter().map(|r| r.0).collect();
            asked.dedup();
            for e in asked {
                let params = &samples[e.index()].as_ref().expect("sampled").params;
                let mids = params.windows(2).map(|w| 0.5 * (w[0] + w[1]));
                extras[e.index()].extend(mids);
            }
        }
        let mut split: Vec<EdgeId> = requests.iter().map(|r| r.0).collect();
        split.dedup();
        let resampled: Vec<(EdgeId, EdgeSamples)> = split
            .par_iter()
            .map(|&e| (e, discretize(body, e, &geoms, &targets, &extras[e.index()])))
            .collect();
        for (e, s) in resampled {
            redo.extend(s.faces.iter().map(|(f, _)| *f));
            samples[e.index()] = Some(s);
        }
        redo.sort();
        redo.dedup();
        todo = redo;
    }
    let mut total: u64 = 0;
    let mut too_large = false;
    for f in body.faces() {
        let w = work[f.index()].as_mut().expect("meshed");
        total += w.triangles;
        too_large |= w.too_large;
        if w.fault.is_none()
            && let Some(&(edge, _)) = w.requests.first()
        {
            w.fault = Some(MeshFaultKind::BoundaryNotResolved { edge });
        }
    }
    if too_large || total > params.max_triangles {
        return Err(Error::MeshTooLarge { triangles: total });
    }
    Ok(assemble::assemble(body, &geoms, &samples, &work))
}

/// Cells per direction a face's box is cut into for the estimate.
const CELLS: usize = 8;

/// A rough count of the triangles a body's mesh will have, from each
/// face's parameter box (its edges' samples, or its domain when it has
/// none) and the bounds over it: equilateral triangles at the size the
/// chord allows (`M R² / 2 = chord`, area `1.3 R²`), the turn the angle
/// allows, and the edge limit, plus two per boundary sample. It bounds the
/// box cell by cell and errs high; it is there to refuse
/// a runaway request before meshing, not to predict exactly.
fn estimate(
    b: &Body,
    geoms: &[Option<FaceGeom>],
    samples: &[Option<EdgeSamples>],
    params: &MeshParams,
) -> u64 {
    let mut total: f64 = 0.0;
    for f in b.faces() {
        let g = geoms[f.index()].as_ref().expect("live face");
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        let mut boundary = 0.0;
        for s in samples.iter().flatten() {
            if let Some(uv) = s.uv_on(f) {
                boundary += uv.len() as f64;
                for p in uv {
                    for (k, x) in [p.x, p.y].into_iter().enumerate() {
                        lo[k] = lo[k].min(x);
                        hi[k] = hi[k].max(x);
                    }
                }
            }
        }
        let range: [[f64; 2]; 2] = core::array::from_fn(|k| {
            let [d0, d1] = g.domain[k];
            if g.periods[k].is_some() || lo[k] > hi[k] {
                [d0, d1]
            } else {
                [lo[k], hi[k]]
            }
        });
        if range
            .iter()
            .any(|r| !(r[0].is_finite() && r[1].is_finite()))
        {
            continue;
        }
        // Cell by cell: one bound over a whole NURBS face can let a
        // derivative's interval reach zero and its turning rate blow up.
        let mut face = 0.0;
        for i in 0..CELLS {
            for j in 0..CELLS {
                let cut = |k: usize, c: usize| {
                    let [a, b] = range[k];
                    let at = |c: usize| a + (b - a) * c as f64 / CELLS as f64;
                    [at(c), at(c + 1)]
                };
                let cell = [cut(0, i), cut(1, j)];
                let (su, sv) = (cell[0][1] - cell[0][0], cell[1][1] - cell[1][0]);
                let area = su * g.metric.a * sv * g.metric.b;
                let bound = g.surface.derivative_bound(cell);
                let m = bound::second_derivative(&bound, g.metric);
                let chord = area * m / (2.6 * params.chord);
                let turn = |rate: f64, span: f64| {
                    if rate.is_finite() {
                        (rate * span / params.angle).ceil()
                    } else {
                        1.0
                    }
                };
                let angle = 2.0 * turn(bound.turn_u, su) * turn(bound.turn_v, sv);
                let edge = params.max_edge.map_or(0.0, |e| area / (0.43 * e * e));
                face += chord.max(angle).max(edge);
            }
        }
        total += face + 2.0 * boundary;
    }
    if total.is_finite() && total < u64::MAX as f64 {
        total as u64
    } else {
        u64::MAX
    }
}

/// Meshes one face from its edges' samples.
/// The chords between boundary points (see [`refine::chords`]) drawn
/// inside one face and also in another (inside it or as a loop segment),
/// and the faces they are drawn inside.
fn shared_chords(body: &Body, work: &[Option<FaceWork>]) -> Vec<(Chord, Vec<FaceId>)> {
    let mut all: Vec<(Chord, FaceId, bool)> = body
        .faces()
        .filter_map(|f| work[f.index()].as_ref()?.domain.as_ref().map(|d| (f, d)))
        .flat_map(|(f, d)| {
            refine::chords(d)
                .into_iter()
                .map(move |(c, fixed)| (c, f, fixed))
        })
        .collect();
    all.sort();
    let mut out: Vec<(Chord, Vec<FaceId>)> = Vec::new();
    for group in all.chunk_by(|a, b| a.0 == b.0) {
        let mut faces: Vec<FaceId> = group.iter().map(|x| x.1).collect();
        faces.dedup();
        let inside: Vec<FaceId> = group.iter().filter(|x| !x.2).map(|x| x.1).collect();
        if !inside.is_empty() && faces.len() > 1 {
            out.push((group[0].0, inside));
        }
    }
    out
}

fn mesh_face(
    b: &Body,
    g: &FaceGeom,
    samples: &[Option<EdgeSamples>],
    t: &Targets,
    cap: u64,
    split: &[Chord],
) -> FaceWork {
    let mut w = FaceWork {
        domain: None,
        fault: None,
        requests: Vec::new(),
        too_large: false,
        triangles: 0,
    };
    let mut edges: Vec<EdgeId> = b
        .face_loops(g.face)
        .flat_map(|l| b.loop_fins(l).map(|f| b.fins[f].edge))
        .collect();
    edges.sort();
    edges.dedup();
    for &e in &edges {
        let s = samples[e.index()].as_ref().expect("sampled");
        if s.off.contains(&g.face) {
            w.fault = Some(MeshFaultKind::EdgeOffSurface { edge: e });
            return w;
        }
        if s.stalled {
            w.fault = Some(MeshFaultKind::RefinementStalled);
            return w;
        }
    }
    let Ok(mut d) = Domain::with_requests(g, &gather(b, g, samples), &mut w.requests) else {
        w.fault = Some(MeshFaultKind::BadDomain);
        return w;
    };
    let fixed = |a: Anchor| match a {
        Anchor::Vertex(v) => b.vertices[v].point,
        Anchor::Sample(e, k) => samples[e.index()].as_ref().expect("sampled").points[k as usize],
        Anchor::Free => unreachable!("free points have no fixed position"),
    };
    let r = refine(&mut d, g, t, &fixed, cap, split);
    w.triangles = d.inside.iter().filter(|&&x| x).count() as u64;
    match r.stop {
        Some(Stop::TooLarge) => w.too_large = true,
        Some(Stop::Stalled) => w.fault = Some(MeshFaultKind::RefinementStalled),
        None => {}
    }
    w.requests = r.requests;
    w.domain = Some(d);
    w
}

#[cfg(test)]
mod tests;
