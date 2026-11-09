//! A feature's faces from one seed face: the smallest connected set
//! holding it that a cap closes.

use super::hole::is_hole;
use super::sets::{faces_of, sets};
use crate::name::Name;
use crate::topo::{Body, EntityId, FaceId};
use crate::{Error, TweakError};
use std::collections::BTreeSet;

/// The faces of the hole, boss or pocket `seed` is a face of, sorted by
/// name: grown from the seed across its edges, a neighbour joining unless
/// every loop of it the set touches is a hole of it (it is a host, which
/// the cap keeps). Read only. `NoFeature` where the set grows past half
/// the body's faces, or stops with a cap that doesn't close it;
/// `NoSuchFace` for a name no face has.
pub fn feature_faces(body: &Body, seed: Name) -> Result<Vec<Name>, Error> {
    let s = match body.by_name(seed) {
        Some(EntityId::Face(f)) => f,
        _ => return Err(TweakError::NoSuchFace { face: seed }.into()),
    };
    let none = || Error::from(TweakError::NoFeature { face: seed });
    let total = body.faces().count();
    let mut set: BTreeSet<FaceId> = BTreeSet::from([s]);
    loop {
        let around: BTreeSet<FaceId> = set
            .iter()
            .flat_map(|&f| body.face_loops(f).flat_map(|l| body.loop_fins(l)))
            .flat_map(|fin| faces_of(body, body.fins[fin].edge))
            .filter(|g| !set.contains(g))
            .collect();
        let joining: Vec<FaceId> = around
            .into_iter()
            .filter(|&g| {
                !body.face_loops(g).all(|l| {
                    let touches = body.loop_fins(l).any(|fin| {
                        faces_of(body, body.fins[fin].edge)
                            .iter()
                            .any(|f| set.contains(f))
                    });
                    !touches || is_hole(body, g, l)
                })
            })
            .collect();
        if joining.is_empty() {
            break;
        }
        set.extend(joining);
        if 2 * set.len() > total {
            return Err(none());
        }
    }
    match sets(body, &set).as_slice() {
        [one] if one.closable(body) => {
            let mut names: Vec<Name> = set.iter().map(|&f| body.faces[f].name).collect();
            names.sort();
            Ok(names)
        }
        _ => Err(none()),
    }
}
