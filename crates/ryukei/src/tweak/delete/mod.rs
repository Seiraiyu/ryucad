//! Deleting faces (design 399, plan 13.B): each connected set of deleted
//! faces is healed by a cap, where every edge round it lies on a hole of
//! one surviving face (that face loses the hole, nothing is constructed),
//! or by growth, where the neighbours round the gap are extended until
//! they meet (the local re-trim with the faces taken away).

pub(crate) mod cap;
pub(crate) mod feature;
pub(crate) mod grow;
pub(crate) mod hole;
pub(crate) mod sets;

#[cfg(test)]
mod tests;

pub use feature::feature_faces;

use crate::name::{Event, Journal, Name, OpKey};
use crate::topo::{Body, BodyKind, FaceId};
use crate::work::{Meter, Work};
use crate::{Error, TweakError};
use std::collections::BTreeSet;

/// How a deleted set's gap is closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Heal {
    /// A cap where one closes the set, else growth.
    #[default]
    Auto,
    /// Only a cap: each edge round the set on a hole of one face.
    Cap,
    /// Only growth: the neighbours extended until they meet.
    Grow,
}

/// The faces to delete and how their gaps heal.
#[derive(Clone, Debug, PartialEq)]
pub struct DeleteSpec {
    pub faces: Vec<Name>,
    pub heal: Heal,
}

/// The least name among `faces`.
pub(crate) fn least(body: &Body, faces: &BTreeSet<FaceId>) -> Name {
    faces
        .iter()
        .map(|&f| body.faces[f].name)
        .min()
        .expect("a set holds a face")
}

/// `body` with `spec.faces` taken away and each gap they leave closed:
/// a new body and the journal. Refusals are `Error::InvalidTweak`, made
/// before anything is built.
pub fn delete_faces(key: OpKey, body: &Body, spec: &DeleteSpec) -> Result<(Body, Journal), Error> {
    delete_faces_with(key, body, spec, &Work::unlimited())
}

/// [`delete_faces`] under `work`.
pub fn delete_faces_with(
    key: OpKey,
    body: &Body,
    spec: &DeleteSpec,
    work: &Work,
) -> Result<(Body, Journal), Error> {
    crate::work::run(work, |meter| delete_faces_in(key, body, spec, meter))
}

pub(crate) fn delete_faces_in(
    key: OpKey,
    body: &Body,
    spec: &DeleteSpec,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    let deleted: BTreeSet<FaceId> = super::resolve(body, spec.faces.iter().copied())?
        .into_iter()
        .collect();
    let all = sets::sets(body, &deleted);
    // A set nothing surrounds (every face of a shell) leaves no gap to heal.
    if let Some(s) = all.iter().find(|s| s.boundary.is_empty()) {
        return Err(TweakError::GapNotHealed {
            face: least(body, &s.faces),
        }
        .into());
    }
    m.charge(all.len() as u64).map_err(|_| {
        Error::from(TweakError::Uncertain {
            at: least(body, &deleted),
        })
    })?;
    let (caps, grows): (Vec<&sets::Set>, Vec<&sets::Set>) =
        all.iter().partition(|s| s.closable(body));
    match spec.heal {
        Heal::Cap if !grows.is_empty() => {
            return Err(TweakError::CapNotOnFace {
                face: least(body, &grows[0].faces),
            }
            .into());
        }
        Heal::Grow if !caps.is_empty() => {
            return Err(TweakError::GrowWouldCap {
                face: least(body, &caps[0].faces),
            }
            .into());
        }
        _ => {}
    }
    // A cap keeps the body's kind, so growth on a sheet is refused before
    // anything is built.
    if !grows.is_empty() && body.kind() != BodyKind::Solid {
        return Err(Error::NotASolid {
            op: "delete faces by growth",
        });
    }
    let (mut out, mut journal) = (body.clone(), Journal::new());
    if !caps.is_empty() {
        (out, journal) = cap::cap(key, body, &caps, m)?;
    }
    if !grows.is_empty() {
        let names: BTreeSet<Name> = grows
            .iter()
            .flat_map(|s| s.faces.iter().map(|&f| body.faces[f].name))
            .collect();
        let ids: BTreeSet<FaceId> = out
            .faces()
            .filter(|&f| names.contains(&out.faces[f].name))
            .collect();
        let (grown, j) = grow::grow(key, &out, &ids, m)?;
        out = grown;
        for e in j.events() {
            if !journal.events().contains(e) {
                journal.push(e.clone());
            }
        }
    }
    // A name Modified by the cap that the growth took away is gone.
    let kept: Vec<Event> = journal
        .events()
        .iter()
        .filter(|e| !matches!(e, Event::Modified(n) if out.by_name(*n).is_none()))
        .cloned()
        .collect();
    let mut j = Journal::new();
    for e in kept {
        j.push(e);
    }
    Ok((out, j))
}
