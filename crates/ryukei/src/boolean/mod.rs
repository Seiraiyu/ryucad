//! Booleans: unite, subtract and intersect solid bodies.
//!
//! [`boolean`] combines a target body with any number of tool bodies, each
//! placed by a rigid motion, in one pass (design 32): every tool is moved
//! into place ([`place`]); the operands' faces are paired by a broad phase
//! and intersected, and every place where they meet, and every edge they
//! cut, goes into one table ([`table`]), so neighbouring faces can't
//! disagree; each face is split in its parameter plane into pieces
//! ([`crate::split`]); pieces are classified against the other operands
//! by certified ray casts ([`classify`]); the operation keeps some of
//! them, some reversed ([`select`]); the kept pieces are built into a
//! new body through Euler operators ([`build`]); and what is left
//! redundant (faces either side of an edge on one surface, vertices
//! between two edges on one curve) is merged ([`merge`]). The result's
//! entities are named from the caller's keys and from which faces cut
//! which, and the journal says what happened to every operand's
//! ([`names`]).
//!
//! Only faces on analytic surfaces (planes, cylinders, cones, spheres and
//! tori) may meet; a face on any other surface takes part only where it
//! is certified apart from every face it might meet. Coincident and
//! tangent contact are handled by explicit rules; whatever stays uncertain
//! is refused with [`BooleanError::Uncertain`], never guessed.

pub(crate) mod build;
pub(crate) mod classify;
mod merge;
mod names;
mod place;
mod select;
pub(crate) mod table;

pub use crate::name::ToolKey;

use crate::geom::{Point3, Rigid};
use crate::name::{Journal, Name, OpKey};
use crate::topo::{Body, EdgeId, FaceId, VertexId};
use crate::{BooleanError, Error, TopologyError};
use std::collections::BTreeSet;

/// What a boolean does with its target `A` and the union `T` of its tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BooleanOp {
    /// `A ∪ T`.
    Unite,
    /// `A − T`.
    Subtract,
    /// `A ∩ T`.
    Intersect,
}

/// A tool of a boolean: a body moved into place by `place`, keyed by the
/// caller. One body may be placed as several tools.
#[derive(Clone, Copy, Debug)]
pub struct Tool<'a> {
    pub key: ToolKey,
    pub body: &'a Body,
    pub place: Rigid,
}

/// Combines `target` with `tools` by `op`: a new body (the inputs are
/// untouched) and the journal of what happened to names. Results are
/// regularized, so leftovers of no thickness vanish; they may have
/// several lumps, cavities, or nothing at all.
///
/// The inputs must be solids that pass [`crate::check::check`] (debug
/// builds assert it). Fails with [`Error::InvalidBoolean`] for duplicate
/// tool keys, a tool placed outside the size box, faces that may meet on
/// a surface that isn't analytic, a result that would be non-manifold, or
/// anything that stays uncertain.
pub fn boolean(
    key: OpKey,
    op: BooleanOp,
    target: &Body,
    tools: &[Tool],
) -> Result<(Body, Journal), Error> {
    let mut keys = BTreeSet::new();
    for t in tools {
        if !keys.insert(t.key) {
            return Err(BooleanError::DuplicateToolKey { key: t.key }.into());
        }
    }
    if cfg!(debug_assertions) {
        for b in core::iter::once(target).chain(tools.iter().map(|t| t.body)) {
            let faults = crate::check::check(b);
            assert!(
                faults.is_empty(),
                "a boolean's input is invalid: {faults:?}"
            );
        }
    }
    let ops = place::operands(target, tools)?;
    let named = |f: Fail| error(&ops, f);
    let table = table::build(&ops).map_err(named)?;
    let pieces = crate::split::split_all(&ops, &table).map_err(named)?;
    let classes = classify::classify(&ops, &table, &pieces).map_err(named)?;
    let kept = select::select(op, &pieces, &classes);
    let built = build::build(&ops, &table, &kept, key).map_err(named)?;
    let mut body = built.body;
    let mut sources = merge::Sources {
        faces: built
            .faces
            .iter()
            .enumerate()
            .map(|(k, &f)| (f, vec![k]))
            .collect(),
        edges: built.edges.iter().map(|(&g, &e)| (e, vec![g])).collect(),
        vertices: built.vertices.iter().map(|(&g, &v)| (v, g)).collect(),
    };
    // Lone vertices an operand had (a cone's apex) stay.
    let keep_lone: BTreeSet<VertexId> = built
        .vertices
        .iter()
        .filter(|(g, _)| {
            table.vertices[**g]
                .originals
                .iter()
                .any(|o| ops[o.op].vertices[o.vertex].lone.is_some())
        })
        .map(|(_, &v)| v)
        .collect();
    let mut count = 0u128;
    let mut keys = || {
        count += 1;
        OpKey(
            Name::derive(
                key,
                crate::name::Role::Boolean(crate::name::BooleanRole::Merged),
                &[Name::from_u128(u128::MAX - count)],
            )
            .to_u128(),
        )
    };
    merge::merge(&mut body, &mut sources, &keep_lone, &mut keys).map_err(named)?;
    let tool_keys: Vec<ToolKey> = tools.iter().map(|t| t.key).collect();
    let journal = names::name(key, &ops, &tool_keys, &table, &kept, &mut body, &sources);
    if cfg!(debug_assertions) {
        let faults = crate::check::check(&body);
        assert!(
            faults.is_empty(),
            "a boolean's result is invalid: {faults:?}"
        );
    } else if !crate::check::check_topology(&body).is_empty() {
        return Err(Error::Topology(TopologyError::Inconsistent));
    }
    Ok((body, journal))
}

/// The error for a failure, naming the operands' faces.
fn error(ops: &[Body], f: Fail) -> Error {
    let name = |r: FaceRef| ops[r.op].faces[r.face].name;
    match f {
        Fail::Unsupported(a, b) => BooleanError::Unsupported {
            a: name(a),
            b: name(b),
        }
        .into(),
        Fail::Uncertain(a, b) => BooleanError::Uncertain {
            a: name(a),
            b: name(b),
        }
        .into(),
        Fail::NonManifold { at, faces } => BooleanError::NonManifold {
            at,
            faces: faces.into_iter().map(name).collect::<Vec<Name>>(),
        }
        .into(),
        Fail::Inconsistent => Error::Topology(TopologyError::Inconsistent),
    }
}

/// A face of one of a boolean's operands: operand 0 is the target, then
/// the tools in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FaceRef {
    pub(crate) op: usize,
    pub(crate) face: FaceId,
}

/// An edge of an operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct EdgeRef {
    pub(crate) op: usize,
    pub(crate) edge: EdgeId,
}

/// A vertex of an operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct VertexRef {
    pub(crate) op: usize,
    pub(crate) vertex: VertexId,
}

/// Why the pipeline stopped, by the operands' faces; the boolean names
/// them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fail {
    /// Two faces that may meet, one not on an analytic surface.
    Unsupported(FaceRef, FaceRef),
    /// An intersection, ordering or classification that stayed uncertain.
    Uncertain(FaceRef, FaceRef),
    /// Kept faces that would meet at an edge or a point only.
    NonManifold { at: Point3, faces: Vec<FaceRef> },
    /// The kept pieces could not be built into a body: a kernel bug.
    Inconsistent,
}

/// The largest tolerance on a face's boundary, at least resolution.
pub(crate) fn face_tolerance(body: &Body, f: FaceId) -> f64 {
    let mut t = crate::tol::LINEAR_RESOLUTION;
    for l in body.face_loops(f) {
        if let Some(v) = body.loops[l].vertex {
            t = t.max(body.vertices[v].tolerance.get());
        }
        for fin in body.loop_fins(l) {
            let e = &body.edges[body.fins[fin].edge];
            t = t
                .max(e.tolerance.get())
                .max(body.vertices[e.start].tolerance.get())
                .max(body.vertices[e.end].tolerance.get());
        }
    }
    t
}
