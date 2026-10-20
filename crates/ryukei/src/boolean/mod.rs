//! Booleans: unite, subtract and intersect solid bodies.
//!
//! The pipeline (design 32): the operands' faces are paired by a broad
//! phase and intersected; every place where they meet, and every edge
//! they cut, goes into one table ([`table`]), so neighbouring faces can't
//! disagree; each face is split in its parameter plane into pieces
//! ([`crate::split`]); pieces are classified against the other operands
//! by certified ray casts ([`classify`]).

// Used by the boolean, from the commit that adds it on.
#[allow(dead_code)]
pub(crate) mod classify;
// Used by the boolean, from the commit that adds it on.
#[allow(dead_code)]
pub(crate) mod table;

use crate::topo::{Body, EdgeId, FaceId, VertexId};

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
