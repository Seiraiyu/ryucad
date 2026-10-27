//! Names for a sweep's entities, from the caller's keys.

use crate::check;
use crate::name::{Event, Journal, Name, OpKey, Role, SweepRole};
use crate::topo::{Body, EntityId};
use crate::{Error, TopologyError};

/// Names a sweep's entities from its key and the profile's keys.
pub(crate) struct Namer {
    kind: &'static str,
    key: OpKey,
}

impl Namer {
    pub(crate) fn new(kind: &'static str, key: OpKey) -> Self {
        Self { kind, key }
    }

    /// The stand-in for a segment's key: a parent, never on an entity.
    pub(crate) fn segment(&self, b: &mut Body, key: u128) -> Name {
        let caller = [Name::from_u128(key)];
        b.derive_name(
            self.kind,
            self.key,
            Role::Sweep(SweepRole::Segment),
            &caller,
        )
    }

    /// The stand-in for a path segment's key.
    pub(crate) fn path_segment(&self, b: &mut Body, key: u128) -> Name {
        let caller = [Name::from_u128(key)];
        b.derive_name(
            self.kind,
            self.key,
            Role::Sweep(SweepRole::PathSegment),
            &caller,
        )
    }

    /// The stand-in for a loop's key.
    pub(crate) fn loop_(&self, b: &mut Body, key: u128) -> Name {
        let caller = [Name::from_u128(key)];
        b.derive_name(self.kind, self.key, Role::Sweep(SweepRole::Loop), &caller)
    }

    /// Names `entity` by its role and parents, and returns the name.
    pub(crate) fn name(
        &self,
        b: &mut Body,
        entity: EntityId,
        role: SweepRole,
        parents: &[Name],
    ) -> Name {
        let name = b.derive_name(self.kind, self.key, Role::Sweep(role), parents);
        b.rename(entity, name);
        name
    }

    /// Gives every fin its exact pcurve, then returns the journal of the
    /// finished sweep: every face, then every edge, then
    /// every vertex, created, in array order. Debug builds assert the whole
    /// check; release builds still refuse a body whose topology fails it,
    /// a kernel bug reported rather than handed on.
    pub(crate) fn finish(&self, b: &mut Body) -> Result<Journal, Error> {
        crate::pcurve::fill(b)?;
        let mut journal = Journal::new();
        for f in b.faces() {
            journal.push(Event::Created(b.faces[f].name));
        }
        for e in b.edges() {
            journal.push(Event::Created(b.edges[e].name));
        }
        for v in b.vertices() {
            journal.push(Event::Created(b.vertices[v].name));
        }
        if cfg!(debug_assertions) {
            let faults = check::check(b);
            assert!(faults.is_empty(), "{} is invalid: {faults:?}", self.kind);
        } else if !check::check_topology(b).is_empty() {
            return Err(TopologyError::Inconsistent.into());
        }
        Ok(journal)
    }
}
