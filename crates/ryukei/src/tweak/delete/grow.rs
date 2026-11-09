//! Growth: the deleted faces taken away inside the local re-trim, their
//! neighbours extended until the frontier walk closes the gap; and the
//! journal of what replaced them.

use super::least;
use super::sets::faces_of;
use crate::name::{Event, Journal, Name, OpKey};
use crate::topo::{Body, FaceId};
use crate::tweak::engine::Changes;
use crate::tweak::retrim::retrim;
use crate::work::Meter;
use crate::{Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// `body` with `deleted` grown over. A walk still reaching a working
/// face's side with the region at its largest is `GapNotHealed`: nothing
/// within reach closes the gap (a slab's side between parallel planes).
pub(crate) fn grow(
    key: OpKey,
    body: &Body,
    deleted: &BTreeSet<FaceId>,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    let changes = Changes {
        deleted: deleted.clone(),
        ..Changes::default()
    };
    let (out, j) = retrim(key, body, &changes, m).map_err(|e| match e {
        Error::InvalidTweak(TweakError::RegionTooSmall { .. }) => TweakError::GapNotHealed {
            face: least(body, deleted),
        }
        .into(),
        e => e,
    })?;
    let j = replaced(body, &out, deleted, &j);
    Ok((out, j))
}

/// The journal with each deleted face, and each of its edges that went,
/// `Replaced` by the new edges between the faces round its set where the
/// growth made any (a fillet by the sharp edge its supports meet in),
/// else `Deleted`.
pub(crate) fn replaced(
    input: &Body,
    out: &Body,
    deleted: &BTreeSet<FaceId>,
    j: &Journal,
) -> Journal {
    let created: BTreeSet<Name> = j
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::Created(n) => Some(*n),
            _ => None,
        })
        .collect();
    let mut by: BTreeMap<Name, Vec<Name>> = BTreeMap::new();
    for s in super::sets::sets(input, deleted) {
        let round: BTreeSet<Name> = s
            .boundary
            .iter()
            .flat_map(|&e| faces_of(input, e))
            .filter(|f| !deleted.contains(f))
            .map(|f| input.faces[f].name)
            .collect();
        // New edges between two faces round the set, with their faces.
        let new: Vec<(Name, [Name; 2])> = out
            .edges()
            .filter(|&e| created.contains(&out.edges[e].name))
            .filter_map(|e| match faces_of(out, e).as_slice() {
                [a, b] => {
                    let (a, b) = (out.faces[*a].name, out.faces[*b].name);
                    (round.contains(&a) && round.contains(&b))
                        .then_some((out.edges[e].name, [a, b]))
                }
                _ => None,
            })
            .collect();
        if new.is_empty() {
            continue;
        }
        for &f in &s.faces {
            by.insert(input.faces[f].name, new.iter().map(|x| x.0).collect());
        }
        for &e in &s.boundary {
            let n = input.edges[e].name;
            if out.by_name(n).is_some() {
                continue;
            }
            let keeps: Vec<Name> = faces_of(input, e)
                .into_iter()
                .filter(|f| !deleted.contains(f))
                .map(|f| input.faces[f].name)
                .collect();
            let on: Vec<Name> = new
                .iter()
                .filter(|(_, fs)| keeps.iter().any(|k| fs.contains(k)))
                .map(|x| x.0)
                .collect();
            if !on.is_empty() {
                by.insert(n, on);
            }
        }
    }
    let mut out_j = Journal::new();
    for e in j.events() {
        match e {
            Event::Deleted(n) if by.contains_key(n) => {}
            other => out_j.push(other.clone()),
        }
    }
    for &f in deleted {
        let n = input.faces[f].name;
        if !by.contains_key(&n) {
            out_j.push(Event::Deleted(n));
        }
    }
    for (old, mut new) in by {
        new.sort();
        out_j.push(Event::Replaced { old, new });
    }
    out_j
}
