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

mod assemble;
mod bound;
mod cdt;
mod check;
mod domain;
mod edge;
mod params;
mod refine;
mod uv;

pub use check::{MeshCheckFault, check_mesh};
pub use params::MeshParams;

use crate::Error;
use crate::geom::{Point2, Point3, UnitVector3};
use crate::topo::{Body, EdgeId, FaceId};
use bound::Targets;
use core::ops::Range;
use domain::{Anchor, Domain, gather};
use edge::{EdgeSamples, discretize};
use refine::{Stop, refine};
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

/// Rounds of boundary split requests before a face gives up.
const ROUNDS: usize = 3;

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
    let mut geoms: Vec<Option<FaceGeom>> = vec![None; body.faces.slot_count()];
    for f in body.faces() {
        geoms[f.index()] = Some(FaceGeom::new(body, f));
    }
    let mut samples: Vec<Option<EdgeSamples>> = vec![None; body.edges.slot_count()];
    for e in body.edges() {
        samples[e.index()] = Some(discretize(body, e, &geoms, &targets, &[]));
    }
    let mut extras: Vec<Vec<f64>> = vec![Vec::new(); body.edges.slot_count()];
    let mut work: Vec<Option<FaceWork>> = (0..body.faces.slot_count()).map(|_| None).collect();
    let mut todo: Vec<FaceId> = body.faces().collect();
    for round in 0..=ROUNDS {
        for &f in &todo {
            let g = geoms[f.index()].as_ref().expect("live face");
            work[f.index()] = Some(mesh_face(body, g, &samples, &targets, params.max_triangles));
        }
        let mut requests: Vec<(EdgeId, f64)> = todo
            .iter()
            .flat_map(|f| work[f.index()].as_ref().expect("meshed").requests.clone())
            .collect();
        if requests.is_empty() || round == ROUNDS {
            break;
        }
        requests.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        requests.dedup();
        let mut redo: Vec<FaceId> = Vec::new();
        for &(e, t) in &requests {
            extras[e.index()].push(t);
        }
        let mut split: Vec<EdgeId> = requests.iter().map(|r| r.0).collect();
        split.dedup();
        for &e in &split {
            let s = discretize(body, e, &geoms, &targets, &extras[e.index()]);
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

/// Meshes one face from its edges' samples.
fn mesh_face(
    b: &Body,
    g: &FaceGeom,
    samples: &[Option<EdgeSamples>],
    t: &Targets,
    cap: u64,
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
    let Ok(mut d) = Domain::new(g, &gather(b, g, samples)) else {
        w.fault = Some(MeshFaultKind::BadDomain);
        return w;
    };
    let fixed = |a: Anchor| match a {
        Anchor::Vertex(v) => b.vertices[v].point,
        Anchor::Sample(e, k) => samples[e.index()].as_ref().expect("sampled").points[k as usize],
        Anchor::Free => unreachable!("free points have no fixed position"),
    };
    let r = refine(&mut d, g, t, &fixed, cap);
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
