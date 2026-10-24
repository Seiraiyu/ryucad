//! Healing: what a file says about a solid, made into a body that keeps
//! ryukei's rules.
//!
//! An importer reads a solid into a [`Shape`]: plain data, not a
//! [`Body`], since what a file says may break the rules Euler operators
//! enforce. Faces may be cut open along seam edges, poles bounded by
//! vertex loops, edges repeated per face; geometry is the file's,
//! converted to ryukei's types and to meters. [`heal`] makes a body of
//! it, in steps that are each a pure function of the one before, so
//! nothing is built until every decision has been made and a refusal
//! leaves nothing half made:
//!
//! 1. **Names.** Every vertex, edge and face of the shape is named from
//!    the caller's key and a hash of its geometry as read (see
//!    [`ImportRole`]), before anything changes.
//! 2. **Sewing.** Edges the file repeats on two faces are joined, and the
//!    vertices at their ends (see `sew`).
//! 3. **Uses.** Every edge must then be used twice, once each way: a
//!    shell that leaves an edge used once is open, one that uses it more
//!    often is non-manifold.
//! 4. **Seams and poles.** Seam edges come out of periodic faces, vertex
//!    loops at poles go, and edges a seam split are joined again (see
//!    `unseam`).
//! 5. **Gaps.** Every edge's and vertex's tolerance grows to cover how
//!    far it lies from its curves and surfaces, up to the cap (see
//!    `gaps`).
//! 6. **Pcurves.** Every use of an edge gets its pcurve on the face:
//!    [`crate::pcurve::exact`] first, else the file's, if it fits the
//!    edge.
//! 7. **Build.** The shape is laid out and built through Euler operators
//!    ([`crate::layout`]), each solid's voids in its region, and renamed
//!    with the names of step 1. Tolerances cover the gaps between curves'
//!    ends and their vertices, and the bounds of approximated geometry.
//!    [`crate::check::check`] must then hold.
//!
//! Healing knows nothing of any file format: an importer of any kind
//! builds the same [`Shape`]. [`seamed`] goes the other way, a body as a
//! file must describe it.

mod build;
mod gaps;
mod names;
mod pcurves;
mod seamed;
mod sew;
mod unseam;

pub use seamed::seamed;

use crate::Error;
use crate::geom::{Curve, Curve2, Point3, Surface};
use crate::name::{Journal, Name, OpKey};
use crate::topo::{Body, Sense};

pub use crate::name::ImportRole;

/// A solid as a file describes it. Entities refer to each other by index
/// into these arrays; geometry is shared the same way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub surfaces: Vec<Surface>,
    pub curves: Vec<Curve>,
    /// Curves in surfaces' parameter planes: the file's pcurves.
    pub curves2: Vec<Curve2>,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub faces: Vec<Face>,
    pub shells: Vec<Shell>,
    pub solids: Vec<Solid>,
}

/// A vertex at `point`, with the tolerance its source gives it (0 where
/// it gives none). `seam` marks a vertex a writer added only to end a
/// seam where it crosses a loop inside an edge (see [`seamed`]): once the
/// seam is gone, healing removes it and joins the edge's two pieces. A
/// file's own vertices are never removed that way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub point: Point3,
    pub tolerance: f64,
    pub seam: bool,
}

/// An edge along curve `curve` over `interval` (increasing), from vertex
/// `start` to vertex `end` (the same vertex for a closed edge). Its
/// tolerance covers how far the curve may be from the true one: the
/// bound of an approximation, or 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub curve: usize,
    pub interval: [f64; 2],
    pub start: usize,
    pub end: usize,
    pub tolerance: f64,
}

/// A face on surface `surface`, its normal the surface's natural one
/// when `sense` is `Forward`; its loops run counterclockwise about that
/// normal. Its tolerance covers how far the surface may be from the true
/// one: the bound of an approximation, or 0.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub surface: usize,
    pub sense: Sense,
    pub loops: Vec<Loop>,
    pub tolerance: f64,
}

/// One loop of a face.
#[derive(Clone, Debug, PartialEq)]
pub enum Loop {
    /// Edges in loop order.
    Edges(Vec<Use>),
    /// A single vertex: a cone's apex, or a pole a file bounds.
    Vertex(usize),
}

/// One use of an edge by a loop: the sense the loop runs it, and the
/// file's pcurve for it on the face, if it has one: a `curves2` entry
/// whose parameter is an affine function of the edge's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Use {
    pub edge: usize,
    pub sense: Sense,
    pub pcurve: Option<usize>,
}

/// Faces joined by edges. `closed` is what the file says; healing finds
/// out for itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub faces: Vec<usize>,
    pub closed: bool,
}

/// A solid: its outer shell and its cavities' shells, which face into
/// the cavity.
#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    pub outer: usize,
    pub voids: Vec<usize>,
}

/// How far healing may go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HealOptions {
    /// The largest tolerance healing may give an entity, in meters.
    pub tolerance_cap: f64,
}

impl Default for HealOptions {
    fn default() -> Self {
        Self {
            tolerance_cap: 1e-4,
        }
    }
}

/// What healing did, by the shape's import names. Lists are sorted by
/// name.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct HealReport {
    /// Edges made by sewing two of the file's that were one.
    pub sewn: Vec<Name>,
    /// Vertices made by joining the file's at the ends of sewn edges.
    pub merged: Vec<Name>,
    /// Seam edges removed.
    pub seams: Vec<Name>,
    /// Pole vertices dropped with their loops or seams.
    pub poles: Vec<Name>,
    /// Vertices removed between two edges on one curve, which were joined.
    pub joined: Vec<Name>,
    /// Entities whose tolerance is above [`crate::tol::LINEAR_RESOLUTION`],
    /// with it.
    pub tolerances: Vec<(Name, f64)>,
    /// Faces whose surface is an approximation, with its bound.
    pub approximated: Vec<(Name, f64)>,
}

/// Heals `shape` into a body named from `key`: the body, the journal of
/// what happened to the shape's import names, and the report. Fails with
/// [`Error::InvalidHeal`] for a shape that can't be made into a valid
/// body, naming its entities by import name; geometry outside the size
/// box fails with [`Error::OutsideSizeBox`].
pub fn heal(
    key: OpKey,
    shape: &Shape,
    options: &HealOptions,
) -> Result<(Body, Journal, HealReport), Error> {
    validate(shape)?;
    let mut names = names::names(key, shape);
    let mut shape = shape.clone();
    let sewn = sew::sew(key, &mut shape, &mut names, options.tolerance_cap)?;
    build::uses(&shape, &names)?;
    let unseamed = unseam::unseam(key, &mut shape, &mut names)?;
    gaps::tolerate(&mut shape, &names, options.tolerance_cap)?;
    let pcurves = pcurves::pcurves(&shape, &names)?;
    let (body, created, mut report) = build::build(key, &shape, &names, &pcurves, options)?;
    let mut journal = sewn.journal;
    journal.append(unseamed.journal);
    journal.append(created);
    report.sewn = sewn.edges;
    report.merged = sewn.vertices;
    report.seams = unseamed.seams;
    report.poles = unseamed.poles;
    report.joined = unseamed.joined;
    Ok((body, journal, report))
}

fn malformed(what: &'static str) -> Error {
    crate::HealError::Malformed { what }.into()
}

/// Indices in range, intervals increasing, every face in exactly one
/// shell and every shell in exactly one solid.
fn validate(s: &Shape) -> Result<(), Error> {
    let finite = |x: f64| x.is_finite() && x >= 0.0;
    for v in &s.vertices {
        if !finite(v.tolerance) {
            return Err(malformed("a tolerance that is negative or not finite"));
        }
    }
    for e in &s.edges {
        if e.curve >= s.curves.len() || e.start >= s.vertices.len() || e.end >= s.vertices.len() {
            return Err(malformed(
                "an edge refers to a curve or vertex the shape lacks",
            ));
        }
        let [t0, t1] = e.interval;
        if !(t0.is_finite() && t1.is_finite() && t0 < t1) {
            return Err(malformed("an edge's interval is not finite and increasing"));
        }
        if !finite(e.tolerance) {
            return Err(malformed("a tolerance that is negative or not finite"));
        }
    }
    for f in &s.faces {
        if f.surface >= s.surfaces.len() {
            return Err(malformed("a face refers to a surface the shape lacks"));
        }
        if !finite(f.tolerance) {
            return Err(malformed("a tolerance that is negative or not finite"));
        }
        for l in &f.loops {
            match l {
                Loop::Vertex(v) if *v >= s.vertices.len() => {
                    return Err(malformed("a loop refers to a vertex the shape lacks"));
                }
                Loop::Vertex(_) => {}
                Loop::Edges(uses) => {
                    if uses.is_empty() {
                        return Err(malformed("a loop has no edges"));
                    }
                    for u in uses {
                        if u.edge >= s.edges.len() || u.pcurve.is_some_and(|p| p >= s.curves2.len())
                        {
                            return Err(malformed("a loop refers to an edge the shape lacks"));
                        }
                    }
                }
            }
        }
    }
    let mut face_shell = vec![None; s.faces.len()];
    for (k, sh) in s.shells.iter().enumerate() {
        if sh.faces.is_empty() {
            return Err(malformed("a shell has no faces"));
        }
        for &f in &sh.faces {
            match face_shell.get_mut(f) {
                Some(slot @ None) => *slot = Some(k),
                Some(Some(_)) => return Err(malformed("a face is in two shells")),
                None => return Err(malformed("a shell refers to a face the shape lacks")),
            }
        }
    }
    if face_shell.iter().any(Option::is_none) {
        return Err(malformed("a face is in no shell"));
    }
    let mut shell_solid = vec![false; s.shells.len()];
    for so in &s.solids {
        for &sh in core::iter::once(&so.outer).chain(&so.voids) {
            match shell_solid.get_mut(sh) {
                Some(slot @ false) => *slot = true,
                Some(true) => return Err(malformed("a shell is in two solids")),
                None => return Err(malformed("a solid refers to a shell the shape lacks")),
            }
        }
    }
    if shell_solid.iter().any(|&x| !x) {
        return Err(malformed("a shell is in no solid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
