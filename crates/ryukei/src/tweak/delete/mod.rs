//! Deleting faces (design 399, plan 13.B): each connected set of deleted
//! faces is healed by a cap, where every edge round it lies on a hole of
//! one surviving face (that face loses the hole, nothing is constructed),
//! or by growth, where the neighbours round the gap are extended until
//! they meet (the local re-trim with the faces taken away).

use crate::name::Name;

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
