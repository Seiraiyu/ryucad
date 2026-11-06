//! How each edge of a tweaked body changes (design 384's Edges).

use super::Motion;
use crate::blend::convex::{Bend, BendFail, bend};
use crate::offset::classify::{fins, normal};
use crate::topo::{Body, EdgeId, FaceId, FinId, VertexId};
use crate::{Error, TweakCase, TweakError};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// Samples along an edge at which tangency is judged, ends included.
const SAMPLES: usize = 17;

/// The largest angle between an edge's faces' normals, at every sample,
/// for the faces to count as tangent along it (decision 3): blends'
/// faces meet within `1e-9` of tangent; a sharp edge's never come within
/// this at all 17 samples.
pub(crate) const TANGENT: f64 = 1e-6;

/// How an edge changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// Nothing at it moves: the edge as it is, bit for bit.
    Kept,
    /// Its faces stay but a vertex moves: its curve carried past its
    /// interval to the new vertex.
    Carried,
    /// Both faces move by one motion that carries the edge exactly.
    Moved,
    /// The branch of its faces' new surfaces' intersection nearest where
    /// its middle goes.
    Closing,
    /// A parting line's edge: it lies on its faces and their drafts, and
    /// stays, its curve carried past its ends for a vertex sliding along
    /// it.
    Hinge,
}

/// The face a fin lies on.
fn face_of(body: &Body, fin: FinId) -> FaceId {
    body.loops[body.fins[fin].loop_].face
}

/// Whether the vertex moves: some face round it changes.
fn moves(body: &Body, motion: &BTreeMap<FaceId, Motion>, v: VertexId) -> bool {
    body.vertex_edges(v).into_iter().any(|e| {
        body.edge_fins(e)
            .any(|f| motion.contains_key(&face_of(body, f)))
    })
}

/// Whether the edge's faces are tangent all along it: `bend` calls it
/// smooth and every sample's normals are within [`TANGENT`]. Smooth in
/// places only is `Unsupported` (`PartlySmooth`).
pub(crate) fn tangent(body: &Body, e: EdgeId) -> Result<bool, Error> {
    let at = body.edges[e].name;
    match bend(body, e) {
        Ok(Bend::Convex | Bend::Concave) => return Ok(false),
        Ok(Bend::Smooth) | Err(BendFail::Changes) => {}
        Err(BendFail::Uncertain) => return Err(TweakError::Uncertain { at }.into()),
    }
    let [fa, fb] = fins(body, e);
    let [t0, t1] = body.edges[e].interval;
    let all = (0..SAMPLES).all(|i| {
        let t = t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64;
        let (a, b) = (normal(body, fa, t), normal(body, fb, t));
        crate::math::atan2(a.cross(b).length(), a.dot(b)) <= TANGENT
    });
    if all {
        Ok(true)
    } else {
        Err(TweakError::Unsupported {
            at,
            case: TweakCase::PartlySmooth,
        }
        .into())
    }
}

/// Every inner edge's class with no parting line.
#[cfg(test)]
pub(crate) fn classes(
    body: &Body,
    motion: &BTreeMap<FaceId, Motion>,
) -> Result<BTreeMap<EdgeId, Class>, Error> {
    classes_with(body, motion, &BTreeSet::new())
}

/// Every inner edge's class, in parallel, in the order of the edges'
/// names, `hinges` (a parting line's edges) `Hinge` first;
/// `TangencyBroken` (the least-named edge first) where a tangent pair's
/// faces would part.
pub(crate) fn classes_with(
    body: &Body,
    motion: &BTreeMap<FaceId, Motion>,
    hinges: &BTreeSet<EdgeId>,
) -> Result<BTreeMap<EdgeId, Class>, Error> {
    let mut edges: Vec<EdgeId> = body.edges().filter(|&e| !body.is_laminar(e)).collect();
    edges.sort_by_key(|&e| body.edges[e].name);
    edges
        .par_iter()
        .map(|&e| {
            if hinges.contains(&e) {
                Ok((e, Class::Hinge))
            } else {
                class(body, motion, e).map(|c| (e, c))
            }
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect()
}

fn class(body: &Body, motion: &BTreeMap<FaceId, Motion>, e: EdgeId) -> Result<Class, Error> {
    let edge = &body.edges[e];
    let [fa, fb] = fins(body, e).map(|f| face_of(body, f));
    match (motion.get(&fa), motion.get(&fb)) {
        (None, None) => Ok(
            if moves(body, motion, edge.start) || moves(body, motion, edge.end) {
                Class::Carried
            } else {
                Class::Kept
            },
        ),
        (Some(Motion::Rigid(a)), Some(Motion::Rigid(b))) if a == b => Ok(Class::Moved),
        (Some(Motion::Offset(a)), Some(Motion::Offset(b))) if a == b => Ok(if tangent(body, e)? {
            Class::Moved
        } else {
            Class::Closing
        }),
        (Some(a @ Motion::Draft { .. }), Some(b @ Motion::Draft { .. })) if a == b => {
            Ok(if tangent(body, e)? {
                Class::Moved
            } else {
                Class::Closing
            })
        }
        _ => {
            if tangent(body, e)? {
                Err(TweakError::TangencyBroken { edge: edge.name }.into())
            } else {
                Ok(Class::Closing)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Vector3};
    use crate::name::{FaceRole, Name, OpKey, Role};
    use crate::primitive;

    const K: OpKey = OpKey(3);

    fn face(b: &Body, r: FaceRole) -> FaceId {
        let n = Name::derive(K, Role::Face(r), &[]);
        b.faces().find(|&f| b.faces[f].name == n).unwrap()
    }

    #[test]
    fn a_raised_top_moves_its_edges_carries_the_sides_and_keeps_the_bottom() {
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let top = face(&b, FaceRole::PlusZ);
        let up = Motion::Rigid(Rigid::translation(Vector3::new(0.0, 0.0, 0.1)));
        let motion = BTreeMap::from([(top, up)]);
        let c = classes(&b, &motion).unwrap();
        let count = |k: Class| c.values().filter(|&&x| x == k).count();
        // 4 round the top close (top moved, side kept), 4 sides carried,
        // 4 round the bottom kept.
        assert_eq!(
            (
                count(Class::Closing),
                count(Class::Carried),
                count(Class::Kept)
            ),
            (4, 4, 4)
        );
    }

    #[test]
    fn a_whole_body_moved_moves_every_edge() {
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let m = Motion::Rigid(Rigid::translation(Vector3::new(0.3, 0.0, 0.0)));
        let motion = b.faces().map(|f| (f, m)).collect();
        let c = classes(&b, &motion).unwrap();
        assert!(c.values().all(|&x| x == Class::Moved));
    }

    #[test]
    fn drafted_walls_close_at_their_corners_and_hinge_edges_stay() {
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let pull = crate::geom::UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let d = Motion::Draft { pull, angle: 0.05 };
        let walls = [
            FaceRole::PlusX,
            FaceRole::MinusX,
            FaceRole::PlusY,
            FaceRole::MinusY,
        ];
        let motion: BTreeMap<_, _> = walls.iter().map(|&r| (face(&b, r), d)).collect();
        let c = classes(&b, &motion).unwrap();
        let count = |k: Class| c.values().filter(|&&x| x == k).count();
        // 4 corners between walls, 4 round the top, 4 round the bottom.
        assert_eq!(count(Class::Closing), 12);
        // The bottom's edges as a parting line: hinges.
        let bottom = face(&b, FaceRole::MinusZ);
        let hinges: BTreeSet<_> = b
            .loop_fins(b.face_loops(bottom).next().unwrap())
            .map(|f| b.fins[f].edge)
            .collect();
        let c = classes_with(&b, &motion, &hinges).unwrap();
        assert_eq!(c.values().filter(|&&x| x == Class::Hinge).count(), 4);
    }

    #[test]
    fn a_top_beside_its_fillet_breaks_tangency() {
        let (bx, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let mut f = [
            Name::derive(K, Role::Face(FaceRole::PlusZ), &[]),
            Name::derive(K, Role::Face(FaceRole::PlusX), &[]),
        ];
        f.sort();
        let edge = Name::derive(K, Role::Edge, &f);
        let (b, _) = crate::blend::blend(
            OpKey(4),
            &bx,
            &[crate::blend::EdgeBlend {
                edge,
                kind: crate::blend::BlendKind::Fillet { radius: 0.1 },
            }],
        )
        .unwrap();
        let top = face(&b, FaceRole::PlusZ);
        let up = Motion::Rigid(Rigid::translation(Vector3::new(0.0, 0.0, 0.05)));
        let err = classes(&b, &BTreeMap::from([(top, up)])).unwrap_err();
        assert!(
            matches!(err, Error::InvalidTweak(TweakError::TangencyBroken { .. })),
            "{err:?}"
        );
    }
}
