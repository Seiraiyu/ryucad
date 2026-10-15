//! Euler operators: the only way topology changes.
//!
//! Each operator makes or kills a fixed set of entities, so the
//! generalized Euler–Poincaré formula (see [`crate::check`]) still holds
//! afterwards, and each has an inverse. Topology and geometry change
//! together: an operator that makes an edge takes its curve. Operators
//! decide no geometry; the checker verifies it.
//!
//! Every operator checks all of its preconditions before writing
//! anything, so on `Err` the body is unchanged. Operators that make or
//! rename entities take the caller's [`OpKey`] and name what they make
//! (see [`crate::name`]); each returns a [`Journal`]. In debug builds every
//! operator asserts [`crate::check::check_topology`] afterwards.

mod shell;

pub use shell::{Mvfs, kvfs, mvfs};

use crate::geom::Point3;
use crate::name::{Name, OpKey, Role};
use crate::topo::{Arena, Body, EntityId, FaceId, FinId, Id, LoopId, RegionId, ShellId, VertexId};
use crate::{Error, TopologyError};

/// The longest parent recipe inlined into a child's recipe.
const RECIPE_INLINE: usize = 96;

/// Where a new edge attaches inside a loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// At the vertex where this fin starts, just before the fin.
    Before(FinId),
    /// At the vertex of this lone-vertex loop.
    Lone(LoopId),
}

pub(crate) fn dead() -> Error {
    Error::Topology(TopologyError::DeadEntity)
}

/// A live entity, or `DeadEntity`.
pub(crate) fn live<I: Id, T>(arena: &Arena<I, T>, id: I) -> Result<&T, Error> {
    arena.get(id).ok_or_else(dead)
}

/// The ID the next push into `arena` will get.
fn next_id<I: Id, T>(arena: &Arena<I, T>) -> I {
    I::from_index(arena.slot_count())
}

/// A point from outside the kernel: finite and inside the size box.
fn check_point(p: Point3) -> Result<Point3, Error> {
    Point3::try_new(p.x, p.y, p.z)
}

/// Asserts the structural checks in debug builds.
fn finish(body: &Body) {
    if cfg!(debug_assertions) {
        let faults = crate::check::check_topology(body);
        assert!(
            faults.is_empty(),
            "Euler operator broke topology: {faults:?}"
        );
    }
}

impl Body {
    /// Derives a name and, when recipes are on, records its recipe. `kind`
    /// is the operation, for the recipe only. A parent's own recipe is
    /// inlined while it is at most [`RECIPE_INLINE`] bytes; past that the
    /// parent's name stands in, so recipes stay small however long the
    /// history.
    pub(crate) fn derive_name(
        &mut self,
        kind: &str,
        key: OpKey,
        role: Role,
        parents: &[Name],
    ) -> Name {
        let name = Name::derive(key, role, parents);
        if let Some(recipes) = &mut self.recipes {
            let parts: Vec<String> = parents
                .iter()
                .map(|p| match recipes.get(p) {
                    Some(r) if r.len() <= RECIPE_INLINE => r.clone(),
                    _ => p.to_string(),
                })
                .collect();
            let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
            recipes.insert(name, crate::name::recipe(kind, key, role, &parts));
        }
        name
    }

    /// Adds a new entity's name to the index.
    pub(crate) fn index_name(&mut self, entity: EntityId) {
        let name = self.name_of(entity).expect("a live, named entity");
        self.names.insert(name, entity);
    }

    /// Drops a name from the index and its recipe.
    pub(crate) fn retire_name(&mut self, name: Name) {
        self.names.remove(&name);
        if let Some(r) = &mut self.recipes {
            r.remove(&name);
        }
    }

    fn link_shell(&mut self, region: RegionId, s: ShellId) {
        self.shells[s].region = region;
        match self.regions[region].first_shell {
            None => {
                (self.shells[s].next, self.shells[s].prev) = (s, s);
                self.regions[region].first_shell = Some(s);
            }
            Some(first) => {
                let last = self.shells[first].prev;
                (self.shells[s].next, self.shells[s].prev) = (first, last);
                self.shells[last].next = s;
                self.shells[first].prev = s;
            }
        }
    }

    fn unlink_shell(&mut self, s: ShellId) {
        let (region, next, prev) = {
            let x = &self.shells[s];
            (x.region, x.next, x.prev)
        };
        let other = (next != s).then_some(next);
        self.shells[prev].next = next;
        self.shells[next].prev = prev;
        if self.regions[region].first_shell == Some(s) {
            self.regions[region].first_shell = other;
        }
    }

    fn link_face(&mut self, shell: ShellId, f: FaceId) {
        self.faces[f].shell = shell;
        match self.shells[shell].first_face {
            None => {
                (self.faces[f].next, self.faces[f].prev) = (f, f);
                self.shells[shell].first_face = Some(f);
            }
            Some(first) => {
                let last = self.faces[first].prev;
                (self.faces[f].next, self.faces[f].prev) = (first, last);
                self.faces[last].next = f;
                self.faces[first].prev = f;
            }
        }
    }

    fn unlink_face(&mut self, f: FaceId) {
        let (shell, next, prev) = {
            let x = &self.faces[f];
            (x.shell, x.next, x.prev)
        };
        let other = (next != f).then_some(next);
        self.faces[prev].next = next;
        self.faces[next].prev = prev;
        if self.shells[shell].first_face == Some(f) {
            self.shells[shell].first_face = other;
        }
    }

    fn link_loop(&mut self, face: FaceId, l: LoopId) {
        self.loops[l].face = face;
        match self.faces[face].first_loop {
            None => {
                (self.loops[l].next, self.loops[l].prev) = (l, l);
                self.faces[face].first_loop = Some(l);
            }
            Some(first) => {
                let last = self.loops[first].prev;
                (self.loops[l].next, self.loops[l].prev) = (first, last);
                self.loops[last].next = l;
                self.loops[first].prev = l;
            }
        }
    }

    fn unlink_loop(&mut self, l: LoopId) {
        let (face, next, prev) = {
            let x = &self.loops[l];
            (x.face, x.next, x.prev)
        };
        let other = (next != l).then_some(next);
        self.loops[prev].next = next;
        self.loops[next].prev = prev;
        if self.faces[face].first_loop == Some(l) {
            self.faces[face].first_loop = other;
        }
    }

    /// Kills a vertex and retires its name.
    fn kill_vertex(&mut self, v: VertexId) {
        let name = self.vertices[v].name;
        self.retire_name(name);
        self.vertices.kill(v);
    }

    /// Unlinks and kills a face, retiring its name.
    fn kill_face(&mut self, f: FaceId) {
        let name = self.faces[f].name;
        self.retire_name(name);
        self.unlink_face(f);
        self.faces.kill(f);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::geom::{Frame, Plane, Surface};

    pub(crate) const K: OpKey = OpKey(0x10);

    #[test]
    fn recipes_stay_small_over_long_histories() {
        let (mut b, _) = plane_body();
        let mut name = b.derive_name("a", OpKey(1), Role::NewFace, &[]);
        for i in 2..50 {
            name = b.derive_name("b", OpKey(i), Role::Part(0), &[name]);
            assert!(b.recipe(name).unwrap().len() <= 2 * RECIPE_INLINE);
        }
        let first = b.derive_name("a", OpKey(1), Role::NewFace, &[]);
        let child = b.derive_name("b", OpKey(2), Role::Part(0), &[first]);
        assert_eq!(b.recipe(child).unwrap(), "b#2(a#1/new-face)/part0");
        assert!(b.recipe(name).unwrap().starts_with("b#31("));
    }

    /// A body with one plane surface and nothing else.
    pub(crate) fn plane_body() -> (Body, crate::topo::SurfaceId) {
        let mut b = Body::new();
        let s = b.add_surface(Surface::Plane(Plane::new(Frame::WORLD)));
        (b, s)
    }

    /// The body after `compact`, without names: what an operator followed
    /// by its inverse must give back.
    pub(crate) fn shape(b: &Body) -> String {
        let mut b = b.clone();
        b.compact();
        b.dump_unnamed()
    }
}
