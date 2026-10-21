//! Blends: fillets and chamfers on the edges of solid bodies (design 43).
//!
//! A blend takes any set of edges, each with its own fillet radius or
//! chamfer, and rounds or bevels each where it is convex and fills it
//! where it is concave. The pipeline makes every decision before it
//! changes anything: the edges are resolved and checked ([`spec`]), which
//! way each bends is certified ([`convex`]), they are grouped into chains
//! joined tangentially and every vertex is classed ([`chain`]); each
//! chain's geometry is made in parallel ([`plan`]): for fillets, spines
//! from the faces' offsets ([`spine`]), rails ([`rail`]) and the surfaces
//! the balls sweep ([`fillet`]); for chamfers, rails set back along the
//! faces and the planes, cones or ruled surfaces between them
//! ([`chamfer`]); and where the blends end ([`end`]), meet in a ball
//! ([`corner`]), trim each other ([`trim`]) or are mitred ([`mitre`]).
//! Every new curve is checked to stay on its face ([`overflow`]). Only
//! then is a copy of the body rebuilt through Euler operators ([`build`])
//! and its new entities named from the blend's key and the names around
//! them ([`names`]). Where one ball can't close a corner, the blend is
//! built in stages, the smaller blends rolling over the larger
//! ([`stage`]).
//!
//! Only faces on the analytic surfaces (planes, cylinders, cones, spheres
//! and tori) and on earlier fillets' pipes may be blended; the rest is
//! refused with [`BlendError::Unsupported`], never guessed.

mod build;
mod chain;
mod chamfer;
mod convex;
mod corner;
mod end;
mod fillet;
mod mitre;
mod names;
mod overflow;
mod plan;
mod rail;
mod seam;
mod spec;
mod spine;
mod stage;
mod trim;

use crate::name::{BlendRole, Journal, Name, OpKey, Role};
use crate::topo::{Body, EdgeId, EntityId};
use crate::{BlendError, Error, TopologyError};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// What a blend does to one edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendKind {
    /// A rolling-ball fillet of this radius.
    Fillet { radius: f64 },
    /// A bevel set back from the edge along each face.
    Chamfer(Chamfer),
}

/// How far a chamfer is set back from its edge on each face: a sphere of
/// that radius swept along the edge meets the face there, so on a planar
/// face it is the distance from the edge along the face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Chamfer {
    /// The same distance on both faces.
    Equal { distance: f64 },
    /// `first` on `first_face`, one of the edge's faces, and `second` on
    /// the other.
    Distances {
        first: f64,
        second: f64,
        first_face: Name,
    },
    /// `distance` on `face`, one of the edge's faces, and the chamfer at
    /// `angle` (radians, in `(0, π/2]`) to that face.
    DistanceAngle {
        distance: f64,
        angle: f64,
        face: Name,
    },
}

/// One edge to blend, named by its stable name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeBlend {
    pub edge: Name,
    pub kind: BlendKind,
}

/// Blends the edges of `body` named in `blends`: a new body (the input is
/// untouched) and the journal of what happened to names.
///
/// Each edge is rounded by a rolling-ball fillet, or bevelled by a
/// chamfer, where it is convex and filled where it is concave. Edges
/// meeting tangentially must be blended alike and form chains. At a
/// vertex a blend ends where a third face cuts it off, runs on into a
/// tangent blended edge, or meets the blends of the other edges
/// there: fillets of one size bending one way in a ball, a larger one or
/// the only one bending its way rolled over by the other two, two of
/// three edges trimming each other, three chamfers mitred. Refusals are
/// [`Error::InvalidBlend`], before anything is changed.
///
/// The input must pass [`crate::check::check`] (debug builds assert it).
pub fn blend(key: OpKey, body: &Body, blends: &[EdgeBlend]) -> Result<(Body, Journal), Error> {
    if cfg!(debug_assertions) {
        let faults = crate::check::check(body);
        assert!(faults.is_empty(), "a blend's input is invalid: {faults:?}");
    }
    let specs = spec::resolve(body, blends)?;
    let chains = chain::chains(body, &specs)?;
    let (stages, rolled) = stage::stages(body, &specs, &chains)?;
    if rolled.is_empty() {
        let (out, journal, _) = once(key, body, &specs, &chains)?;
        return Ok((out, journal));
    }
    // Stage by stage, each blending the body the one before left, with
    // the end curves at rolled corners joining the edges rolling over
    // them.
    let before: BTreeSet<Name> = names_of(body);
    let mut out = body.clone();
    let mut journals = Vec::new();
    let mut aliases: BTreeMap<Name, Name> = BTreeMap::new();
    let mut extra: Vec<(usize, EdgeBlend)> = Vec::new();
    let last = stages.iter().copied().max().unwrap_or(0);
    for s in 0..=last {
        let mut now: Vec<EdgeBlend> = specs
            .iter()
            .zip(&stages)
            .filter(|&(_, &t)| t == s)
            .map(|(x, _)| EdgeBlend {
                edge: x.name,
                kind: x.kind,
            })
            .collect();
        now.extend(extra.iter().filter(|(t, _)| *t == s).map(|(_, b)| *b));
        let k = if s == 0 {
            key
        } else {
            OpKey(
                Name::derive(
                    key,
                    Role::Blend(BlendRole::Face),
                    &[Name::from_u128(s as u128)],
                )
                .to_u128(),
            )
        };
        let sp = spec::resolve(&out, &now)?;
        let ch = chain::chains(&out, &sp)?;
        let (next, journal, ends) = once(k, &out, &sp, &ch)?;
        for r in rolled.iter().filter(|r| stages[r.first] == s) {
            let v = body.vertices[r.vertex].name;
            let Some(&(_, end)) = ends.iter().find(|(x, _)| *x == v) else {
                return Err(BlendError::Unsupported { at: v }.into());
            };
            aliases.insert(end, v);
            let kind = specs[r.later[0]].kind;
            extra.push((stages[r.later[0]], EdgeBlend { edge: end, kind }));
        }
        journals.push(journal);
        out = next;
    }
    Ok((out, stage::compose(&before, &journals, &aliases)))
}

/// The end edges a stage made, by the names of the vertices taken away
/// there.
type Ends = Vec<(Name, Name)>;

/// One stage of a blend: every refusal before anything is built, then
/// the rebuild, its names and the checks. Returns the body, the journal,
/// and the end edges by the names of the vertices taken away there.
fn once(
    key: OpKey,
    body: &Body,
    specs: &[spec::Spec],
    chains: &[chain::Chain],
) -> Result<(Body, Journal, Ends), Error> {
    if let Some(at) = chains.iter().find_map(|c| {
        c.vertices
            .iter()
            .zip(&c.classes)
            .find(|(_, class)| matches!(class, chain::Class::Rolled { .. }))
            .map(|(&v, _)| body.vertices[v].name)
    }) {
        return Err(BlendError::Unsupported { at }.into());
    }
    let partials: Vec<Result<Vec<plan::Partial>, Error>> = chains
        .par_iter()
        .map(|c| plan::partials(body, specs, c))
        .collect();
    let partials = partials.into_iter().collect::<Result<Vec<_>, _>>()?;
    let corners = corner::corners(body, specs, chains)?;
    let trims = trim::trims(body, specs, chains, &partials)?;
    let mitres = mitre::mitres(body, specs, chains, &partials)?;
    let near = Near {
        corners: &corners,
        trims: &trims,
        mitres: &mitres,
    };
    let plans: Vec<Result<plan::Plan, Error>> = chains
        .par_iter()
        .zip(partials)
        .enumerate()
        .map(|(ci, (c, parts))| plan::finish(body, specs, c, ci, parts, &near))
        .collect();
    let plans = plans.into_iter().collect::<Result<Vec<_>, _>>()?;
    overflow::check(body, specs, chains, &plans, &near)?;
    let mut out = body.clone();
    let made = build::build(&mut out, key, specs, chains, &plans, &near)?;
    let journal = names::name(&mut out, key, &made);
    if cfg!(debug_assertions) {
        let faults = crate::check::check(&out);
        assert!(faults.is_empty(), "a blend's result is invalid: {faults:?}");
    } else if !crate::check::check_topology(&out).is_empty() {
        return Err(Error::Topology(TopologyError::Inconsistent));
    }
    let ends = made
        .ends
        .iter()
        .map(|&(e, _, _, v)| (v, out.edges[e].name))
        .collect();
    Ok((out, journal, ends))
}

/// The places where chains meet: ball corners, trims and mitres.
pub(crate) struct Near<'a> {
    pub(crate) corners: &'a [corner::BallCorner],
    pub(crate) trims: &'a [trim::Trim],
    pub(crate) mitres: &'a [mitre::Mitre],
}

/// Every face, edge and vertex name of a body.
fn names_of(body: &Body) -> BTreeSet<Name> {
    body.faces()
        .map(EntityId::Face)
        .chain(body.edges().map(EntityId::Edge))
        .chain(body.vertices().map(EntityId::Vertex))
        .filter_map(|x| body.name_of(x))
        .collect()
}

/// The edges reachable from `edge` through vertices where an edge meets
/// exactly one other tangentially, in order along the chain: from one end
/// to the other for an open chain, from `edge` along its direction for a
/// closed one. A blend refuses a blended edge that meets an unblended
/// one tangentially ([`BlendError::OpenTangentChain`]); ryucad offers to
/// blend the whole chain instead. Fails with `DeadEntity` for an edge
/// that is not live.
pub fn tangent_chain(body: &Body, edge: EdgeId) -> Result<Vec<EdgeId>, Error> {
    chain::tangent_chain(body, edge)
}

#[cfg(test)]
mod tests;
