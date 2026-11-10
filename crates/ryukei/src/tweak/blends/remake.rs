//! Deleting blends and making them again: the blends a request picks,
//! closed over their chains and corners, deleted by growth (13.B's
//! `delete_faces`); then, for a resize or a rebuilt tweak, each made
//! again on the sharp edges between its supports by one `blend::blend`,
//! and its faces, edges and vertices given back the names of those they
//! stand in place of.

// Taken out in Task 11, once every function here is used.
#![allow(dead_code, unused_imports)]

use super::recognise::{self, size};
use super::{DeleteBlends, RecogniseOptions, Recognised, Recognition};
use crate::blend::{BlendKind, Chamfer, EdgeBlend};
use crate::name::{Event, Journal, Name, OpKey};
use crate::topo::{Body, EntityId, FaceId};
use crate::tweak::delete::{DeleteSpec, Heal, delete_faces_in};
use crate::work::Meter;
use crate::{BlendError, BlendLoss, Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// Picked blends closed over their chains and the corners they reach:
/// the blends' indices and every face to delete.
pub(crate) fn close(
    rec: &Recognition,
    picked: BTreeSet<usize>,
) -> (BTreeSet<usize>, BTreeSet<Name>) {
    let mut blends = picked;
    loop {
        let before = blends.len();
        let chains: BTreeSet<usize> = blends.iter().map(|&i| rec.blends[i].chain).collect();
        for (i, b) in rec.blends.iter().enumerate() {
            if chains.contains(&b.chain) {
                blends.insert(i);
            }
        }
        for c in &rec.corners {
            if c.blends.iter().any(|i| blends.contains(i)) {
                blends.extend(c.blends.iter().copied());
            }
        }
        if blends.len() == before {
            break;
        }
    }
    let mut faces: BTreeSet<Name> = blends
        .iter()
        .flat_map(|&i| rec.blends[i].faces.iter().copied())
        .collect();
    for c in &rec.corners {
        if c.blends.iter().any(|i| blends.contains(i)) {
            faces.insert(c.face);
        }
    }
    (blends, faces)
}

/// The blends holding `faces`: `NotABlend` for a face that is neither a
/// blend's nor a corner's.
fn holding(rec: &Recognition, faces: &[Name]) -> Result<BTreeSet<usize>, Error> {
    let mut out = BTreeSet::new();
    for &f in faces {
        match rec.holding(f) {
            Some(Ok(i)) => {
                out.insert(i);
            }
            Some(Err(j)) => out.extend(rec.corners[j].blends.iter().copied()),
            None => return Err(TweakError::NotABlend { face: f }.into()),
        }
    }
    Ok(out)
}

/// `faces` deleted by growth.
fn grow(
    key: OpKey,
    body: &Body,
    faces: &BTreeSet<Name>,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    delete_faces_in(
        key,
        body,
        &DeleteSpec {
            faces: faces.iter().copied().collect(),
            heal: Heal::Grow,
        },
        m,
    )
}

pub(crate) fn delete_blends(
    key: OpKey,
    body: &Body,
    spec: &DeleteBlends,
    m: &Meter,
) -> Result<(Body, Journal), Error> {
    if let Some(s) = spec.max_size
        && !(s.is_finite() && s >= 0.0)
    {
        return Err(TweakError::BadDistance.into());
    }
    let rec = recognise::recognise(body, &spec.options, m)?;
    let mut picked = holding(&rec, &spec.faces)?;
    if let Some(s) = spec.max_size {
        for (i, b) in rec.blends.iter().enumerate() {
            if size(&b.kind) <= s {
                picked.insert(i);
            }
        }
    }
    if picked.is_empty() {
        return Ok((body.clone(), Journal::new()));
    }
    let (_, faces) = close(&rec, picked);
    grow(key, body, &faces, m)
}
