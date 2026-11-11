//! Booleans: unite, subtract and intersect solid bodies; trim sheets by
//! solids; divide solids and sheets by sheets.
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
//! Faces on any surface take part: analytic ones, pipes, NURBS, swept
//! surfaces and sweeps along paths. Coincident and tangent contact are
//! handled by explicit rules; whatever stays uncertain is refused with
//! [`BooleanError::Uncertain`], never guessed, and a pair of surfaces no
//! intersector takes with [`BooleanError::Unsupported`].

pub(crate) mod build;
pub(crate) mod classify;
mod divide;
mod merge;
mod names;
mod place;
mod select;
#[cfg(test)]
mod sheet_tests;
pub(crate) mod table;

pub use crate::name::ToolKey;
pub(crate) use divide::divide_in;
pub use divide::{Divided, Sides, divide, divide_sides_with, divide_with};

use crate::geom::{Point3, Rigid};
use crate::name::{Journal, Name, OpKey};
use crate::topo::{Body, BodyKind, EdgeId, FaceId, VertexId};
use crate::work::{Meter, Work};
use crate::{BooleanError, Error, TopologyError};
use std::collections::{BTreeMap, BTreeSet};

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
/// The tools must be solids and the target a solid or a sheet, each
/// passing [`crate::check::check`] (debug builds assert it). A sheet
/// target is trimmed (plan 12.D): `Intersect` keeps its pieces inside
/// the tools' union, or lying on their faces, `Subtract` the rest, and
/// the result is a sheet whose cut edges are laminar. Fails with [`Error::InvalidBoolean`] for duplicate
/// tool keys, a tool placed outside the size box, faces on surfaces no
/// intersector takes, a result that would be non-manifold, a result whose
/// tolerances would pass the cap, or anything that stays uncertain. A
/// sheet as a tool is `NotASolid` (a sheet divides: see [`divide`]), as
/// is `Unite` of a sheet target, which would be a general body.
pub fn boolean(
    key: OpKey,
    op: BooleanOp,
    target: &Body,
    tools: &[Tool],
) -> Result<(Body, Journal), Error> {
    boolean_with(key, op, target, tools, &Work::unlimited())
}

/// [`boolean`] under `work`: refused with `Cancelled` or
/// `BudgetExceeded` when it stops, building nothing.
pub fn boolean_with(
    key: OpKey,
    op: BooleanOp,
    target: &Body,
    tools: &[Tool],
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |m| boolean_in(key, op, target, tools, m))
}

/// The union of a file's solids (ryustep's, plan 14.C) under `work`:
/// [`boolean_with`]'s `Unite`, each pair of faces met under the union
/// scope's cap. A pair whose trace can't settle (a coil's spline face
/// against a lead's cylinder, #550: 23.5M units) is refused `Uncertain`
/// in bounded units on every machine, where the trace's own cap would
/// let it run 25 s and more. A union no pair of which reaches the cap is
/// [`boolean_with`]'s, bit for bit.
pub fn unite_solids_with(
    key: OpKey,
    target: &Body,
    tools: &[Tool],
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |m| {
        boolean_scoped_in(
            key,
            BooleanOp::Unite,
            target,
            tools,
            Some(crate::work::Scope::Union),
            m,
        )
    })
}

/// [`boolean`] charging `m`, for operations made of booleans (shells).
pub(crate) fn boolean_in(
    key: OpKey,
    op: BooleanOp,
    target: &Body,
    tools: &[Tool],
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    boolean_scoped_in(key, op, target, tools, None, m)
}

/// [`boolean_in`], each face pair of the table met under `pair`'s cap
/// when given ([`unite_solids_with`]).
fn boolean_scoped_in(
    key: OpKey,
    op: BooleanOp,
    target: &Body,
    tools: &[Tool],
    pair: Option<crate::work::Scope>,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    // A sheet is trimmed by solids (plan 12.D); a sheet as a tool divides
    // ([`divide`]), and a sheet united with a solid would be a general
    // body.
    for t in tools {
        t.body.require_solid("boolean")?;
    }
    let sheet = target.kind() == BodyKind::Sheet;
    if sheet && op == BooleanOp::Unite {
        return Err(Error::NotASolid {
            op: "boolean unite",
        });
    }
    let mut keys = BTreeSet::new();
    for t in tools {
        if !keys.insert(t.key) {
            return Err(BooleanError::DuplicateToolKey { key: t.key }.into());
        }
    }
    if cfg!(debug_assertions) {
        for b in core::iter::once(target).chain(tools.iter().map(|t| t.body)) {
            // A cancel stops the checks too; `run` says it was one.
            if m.poll().is_err() {
                return Err(Error::Cancelled);
            }
            let faults = crate::check::check(b);
            assert!(
                faults.is_empty(),
                "a boolean's input is invalid: {faults:?}"
            );
        }
    }
    let ops = place::operands(target, tools)?;
    let named = |f: Fail| error(&ops, f);
    let table = table::build_capped(&ops, table::Scope::Operands, pair, m).map_err(named)?;
    let pieces = crate::split::split_all(&ops, &table, m).map_err(named)?;
    let (kept, built) = if sheet {
        // Only the sheet's pieces are kept, by their class against the
        // tools: no ray is cast into the sheet.
        let classes =
            classify::classify_with(&ops, &table, &pieces, &|op, _| op == 0, m).map_err(named)?;
        let kept = select::select_sheet(op, &pieces, &classes);
        let built = build::build_sheet(&ops, &table, &kept, key).map_err(named)?;
        (kept, built)
    } else {
        let classes = classify::classify(&ops, &table, &pieces, m).map_err(named)?;
        let kept = select::select(op, &pieces, &classes);
        let kept = build::enclosing_something(&ops, &table, kept);
        let built = build::build(&ops, &table, &kept, key, m).map_err(named)?;
        (kept, built)
    };
    let (mut body, sources) = merged(key, &ops, &table, &kept, built, m).map_err(named)?;
    let tool_keys: Vec<ToolKey> = tools.iter().map(|t| t.key).collect();
    let journal = names::name(
        key,
        &ops,
        &tool_keys,
        &table,
        (&pieces, &kept),
        &mut body,
        &sources,
        None,
    );
    checked(&body)?;
    Ok((body, journal))
}

/// A result's last checks: no tolerance past the cap, and sound
/// (asserted in debug builds; topology alone in release builds).
fn checked(body: &Body) -> Result<(), Error> {
    if let Some((at, needed)) = past_cap(body) {
        return Err(BooleanError::ToleranceTooLarge { at, needed }.into());
    }
    if cfg!(debug_assertions) {
        let faults = crate::check::check(body);
        assert!(
            faults.is_empty(),
            "a boolean's result is invalid: {faults:?}"
        );
    } else if !crate::check::check_topology(body).is_empty() {
        return Err(Error::Topology(TopologyError::Inconsistent));
    }
    Ok(())
}

/// The kept pieces built into a body, merged and named: what a boolean
/// and a self-union (design 299) do once they have selected.
pub(crate) fn assemble(
    key: OpKey,
    ops: &[Body],
    tools: &[ToolKey],
    table: &table::Table,
    (pieces, kept): (&[crate::split::Piece], &[build::Kept]),
    m: &Meter,
) -> Result<(Body, Journal), Fail> {
    let built = build::build(ops, table, kept, key, m)?;
    let (mut body, sources) = merged(key, ops, table, kept, built, m)?;
    let journal = names::name(
        key,
        ops,
        tools,
        table,
        (pieces, kept),
        &mut body,
        &sources,
        None,
    );
    Ok((body, journal))
}

/// What `build` made, with what is left redundant merged (see
/// [`merge`]): the body and where each of its entities came from.
fn merged(
    key: OpKey,
    ops: &[Body],
    table: &table::Table,
    kept: &[build::Kept],
    built: crate::layout::Built,
    m: &Meter,
) -> Result<(Body, merge::Sources), Fail> {
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
    let (inherited, settled) = inherited(
        ops,
        table,
        kept,
        (&built.faces, &built.edges, &built.vertices),
        &body,
    );
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
    merge::merge(
        &mut body,
        &mut sources,
        merge::Keep {
            lone: &keep_lone,
            edges: &inherited,
            vertices: &settled,
        },
        &mut keys,
        m,
    )?;
    Ok((body, sources))
}

/// The entity whose tolerance is furthest past the cap, if any is
/// (the least name among equals).
pub(crate) fn past_cap(body: &Body) -> Option<(Name, f64)> {
    let cap = crate::tol::TOLERANCE_CAP;
    body.edges()
        .map(|e| (body.edges[e].name, body.edges[e].tolerance.get()))
        .chain(
            body.vertices()
                .map(|v| (body.vertices[v].name, body.vertices[v].tolerance.get())),
        )
        .filter(|&(_, t)| t > cap)
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
}

/// What an operand had and the boolean left as it was, which merging
/// must leave too (issue #204): the result's edges that are (pieces of)
/// an operand's edge between two of its faces, with pieces of those very
/// faces either side, and the result's vertices that are an operand's
/// vertex between exactly two of its edges. A body read from a file may
/// hold faces on one surface either side of an edge, or edges on one
/// curve either side of a vertex; joining them would rename faces and
/// edges far from where the operands meet. What the boolean made (an
/// edge where faces of different operands meet, a stretch, a face
/// split by a tool) still merges.
fn inherited(
    ops: &[Body],
    table: &table::Table,
    kept: &[build::Kept],
    (faces, edges, vertices): (
        &[FaceId],
        &BTreeMap<table::GeId, EdgeId>,
        &BTreeMap<table::GvId, VertexId>,
    ),
    body: &Body,
) -> (BTreeSet<EdgeId>, BTreeSet<VertexId>) {
    let piece_of: BTreeMap<FaceId, FaceRef> = faces
        .iter()
        .enumerate()
        .map(|(k, &f)| (f, kept[k].piece.face))
        .collect();
    let faces_of = |b: &Body, e: EdgeId| -> Vec<FaceId> {
        b.edge_fins(e)
            .map(|fin| b.loops[b.fins[fin].loop_].face)
            .collect()
    };
    let edges = edges
        .iter()
        .filter(|&(&g, &e)| {
            let sides: Vec<FaceRef> = faces_of(body, e).iter().map(|f| piece_of[f]).collect();
            let [a, b] = sides[..] else { return false };
            a.op == b.op
                && a.face != b.face
                && table.edges[g].members.iter().any(|m| match *m {
                    table::Member::Edge(r) if r.op == a.op => {
                        let had: BTreeSet<FaceId> =
                            faces_of(&ops[r.op], r.edge).into_iter().collect();
                        had == BTreeSet::from([a.face, b.face])
                    }
                    _ => false,
                })
        })
        .map(|(_, &e)| e)
        .collect();
    let vertices = vertices
        .iter()
        .filter(|&(&g, _)| {
            table.vertices[g]
                .originals
                .iter()
                .any(|o| ops[o.op].vertex_edges(o.vertex).len() == 2)
        })
        .map(|(_, &v)| v)
        .collect();
    (edges, vertices)
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
    /// Two faces that may meet on surfaces no intersector takes.
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
