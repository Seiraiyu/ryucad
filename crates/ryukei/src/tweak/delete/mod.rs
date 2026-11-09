//! Deleting faces (design 399, plan 13.B): each connected set of deleted
//! faces is healed by a cap, where every edge round it lies on a hole of
//! one surviving face (that face loses the hole, nothing is constructed),
//! or by growth, where the neighbours round the gap are extended until
//! they meet (the local re-trim with the faces taken away).

// Until delete_faces wires the modules up (plan 13.B, Task 7).
#![allow(dead_code, unused_imports)]

pub(crate) mod cap;
pub(crate) mod hole;
pub(crate) mod sets;

#[cfg(test)]
mod tests;

use crate::name::{Event, Name};
use crate::topo::{Body, FaceId};
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
