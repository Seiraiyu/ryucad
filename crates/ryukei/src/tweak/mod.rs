//! Face tweaks (design 384): chosen faces take new surfaces (moved,
//! offset or replaced), and their neighbours' edges and vertices are
//! found again on them, keeping the body's topology and every name.
//!
//! - **Edges** (see `classify`): kept where nothing at them moves;
//!   carried along their own curve where only a vertex moves; moved with
//!   their faces where both move by one motion that carries them;
//!   otherwise the branch of their faces' new surfaces' intersection
//!   nearest where their middle goes. Faces tangent along an edge that the
//!   tweak would part are refused (`TangencyBroken`) unless `tangent`
//!   brings the neighbour into the set.
//! - **Vertices** (see `vertices`): one point, on every new curve there;
//!   one that would split is refused (`VertexSplits`).
//! - Unchanged spline and swept faces are carried past their domains
//!   where their edges move beyond them (`ExtensionUnsupported` where
//!   they can't be).
//!
//! The result is laid out as the offset's local build lays out an
//! offset, and must pass [`crate::check::check`] and
//! [`crate::check::check_intersections`]. Nothing is repaired. Every
//! face, edge and vertex keeps its name; the journal is `Modified` for
//! each that changed. The key names nothing.

pub(crate) mod chain;
pub(crate) mod classify;
pub(crate) mod curves;
pub(crate) mod engine;
pub(crate) mod extend;
pub(crate) mod motion;
pub(crate) mod vertices;

#[cfg(test)]
mod tests;

pub(crate) use motion::Motion;

use crate::geom::Rigid;
use crate::name::{Event, Journal, Name, OpKey};
use crate::topo::{Body, EntityId, FaceId};
use crate::work::{Meter, Work};
use crate::{Error, TweakError};
use engine::Changes;
use std::collections::BTreeMap;

/// Faces moved by one rigid motion. With `tangent`, faces tangent to a
/// moved face across a smooth edge move too, repeatedly.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveSpec {
    pub faces: Vec<Name>,
    pub motion: Rigid,
    pub tangent: bool,
}

/// `body` with `spec.faces` moved by `spec.motion`: a new body and the
/// journal. A motion moving every face is the body moved; one moving no
/// face (a plane slid along itself) changes nothing. Refusals are
/// `Error::InvalidTweak`, made before anything is built.
pub fn move_faces(key: OpKey, body: &Body, spec: &MoveSpec) -> Result<(Body, Journal), Error> {
    move_faces_with(key, body, spec, &Work::unlimited())
}

/// [`move_faces`] under `work`: refused with `Cancelled` or
/// `BudgetExceeded` when it stops, building nothing.
pub fn move_faces_with(
    key: OpKey,
    body: &Body,
    spec: &MoveSpec,
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |meter| move_faces_in(key, body, spec, meter))
}

fn move_faces_in(
    key: OpKey,
    body: &Body,
    spec: &MoveSpec,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    let faces = resolve(body, spec.faces.iter().copied())?;
    if !finite(&spec.motion) {
        return Err(TweakError::NotFinite.into());
    }
    let motion: BTreeMap<FaceId, Motion> = faces
        .iter()
        .map(|&f| (f, Motion::Rigid(spec.motion)))
        .collect();
    let motion = if spec.tangent {
        chain::grow(body, motion)?
    } else {
        motion
    };
    rigidly(key, body, motion, meter)
}

/// Faces moved along their outward normals, each by its own distance
/// (positive outwards); with `tangent`, tangent neighbours by the same.
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetFacesSpec {
    pub faces: Vec<(Name, f64)>,
    pub tangent: bool,
}

/// `body` with `spec`'s faces offset, the rest staying: their edges and
/// vertices found again on the offsets. Distances are finite and zero or
/// at least `10 × LINEAR_RESOLUTION` either way.
pub fn offset_faces(
    key: OpKey,
    body: &Body,
    spec: &OffsetFacesSpec,
) -> Result<(Body, Journal), Error> {
    offset_faces_with(key, body, spec, &Work::unlimited())
}

/// [`offset_faces`] under `work`.
pub fn offset_faces_with(
    key: OpKey,
    body: &Body,
    spec: &OffsetFacesSpec,
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |meter| offset_faces_in(key, body, spec, meter))
}

fn offset_faces_in(
    key: OpKey,
    body: &Body,
    spec: &OffsetFacesSpec,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    let faces = resolve(body, spec.faces.iter().map(|&(n, _)| n))?;
    let mut motion: BTreeMap<FaceId, Motion> = BTreeMap::new();
    for (&f, &(_, d)) in faces.iter().zip(&spec.faces) {
        if !d.is_finite() {
            return Err(TweakError::NotFinite.into());
        }
        if !crate::offset::classify::usable(d) {
            return Err(TweakError::BadDistance.into());
        }
        if d != 0.0 {
            motion.insert(f, Motion::Offset(d));
        }
    }
    if motion.is_empty() {
        return Ok((body.clone(), Journal::new()));
    }
    let motion = if spec.tangent {
        chain::grow(body, motion)?
    } else {
        motion
    };
    let mut surface = BTreeMap::new();
    for (&f, m) in &motion {
        let Motion::Offset(d) = *m else {
            unreachable!("faces offset")
        };
        let s = crate::offset::classify::offset_surface(body, f, d).map_err(engine::translate)?;
        surface.insert(f, s);
    }
    engine::run(
        key,
        body,
        &Changes {
            motion,
            surface,
            senses: BTreeMap::new(),
        },
        meter,
    )
}

/// The faces `names` names, in order: `NoFaces` for none, `NoSuchFace`
/// for a name no face has, `DuplicateFace` for one named twice.
pub(crate) fn resolve(
    body: &Body,
    names: impl Iterator<Item = Name>,
) -> Result<Vec<FaceId>, Error> {
    let mut out = Vec::new();
    for face in names {
        let f = match body.by_name(face) {
            Some(EntityId::Face(f)) => f,
            _ => return Err(TweakError::NoSuchFace { face }.into()),
        };
        if out.contains(&f) {
            return Err(TweakError::DuplicateFace { face }.into());
        }
        out.push(f);
    }
    if out.is_empty() {
        return Err(TweakError::NoFaces.into());
    }
    Ok(out)
}

/// A rigid motion whose every number is finite.
fn finite(r: &Rigid) -> bool {
    let t = r.translation_part();
    r.rotation_matrix().iter().flatten().all(|x| x.is_finite())
        && [t.x, t.y, t.z].iter().all(|x| x.is_finite())
}

/// Faces moved rigidly: the body moved when every face moves by one
/// motion, nothing when no face's surface changes, else the engine.
fn rigidly(
    key: OpKey,
    body: &Body,
    motion: BTreeMap<FaceId, Motion>,
    meter: &Meter,
) -> Result<(Body, Journal), Error> {
    let first = *motion.values().next().expect("a face");
    if let Motion::Rigid(r) = first
        && motion.len() == body.faces().count()
        && motion.values().all(|m| *m == first)
    {
        let out = body.transformed(&r)?;
        let mut journal = Journal::new();
        for n in body
            .faces()
            .map(|f| body.faces[f].name)
            .chain(body.edges().map(|e| body.edges[e].name))
            .chain(body.vertices().map(|v| body.vertices[v].name))
        {
            journal.push(Event::Modified(n));
        }
        return Ok((out, journal));
    }
    let surface: BTreeMap<FaceId, crate::geom::Surface> = motion
        .iter()
        .map(|(&f, m)| {
            let s = &body.surfaces[body.faces[f].surface];
            let Motion::Rigid(r) = m else {
                unreachable!("faces moved rigidly")
            };
            (f, s.transformed(r))
        })
        .collect();
    if surface.iter().all(|(&f, s)| {
        crate::intersect::coincident(&body.surfaces[body.faces[f].surface], s) == Some(true)
    }) {
        return Ok((body.clone(), Journal::new()));
    }
    engine::run(
        key,
        body,
        &Changes {
            motion,
            surface,
            senses: BTreeMap::new(),
        },
        meter,
    )
}
