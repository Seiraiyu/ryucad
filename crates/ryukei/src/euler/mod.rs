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

mod edge;
mod face;
mod ring;
mod shell;

pub use edge::{Mev, kev, mev};
pub use face::{Kef, Mef, kef, mef};
pub use ring::{Kemr, Mekr, kemr, mekr};
pub use shell::{Mvfs, kvfs, mvfs};

use crate::geom::Point3;
use crate::name::{Name, OpKey, Role};
use crate::topo::{
    Arena, Body, EdgeId, EntityId, FaceId, FinId, Id, LoopId, RegionId, ShellId, VertexId,
};
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

/// A parameter interval from outside the kernel: finite and increasing.
fn check_interval(i: [f64; 2]) -> Result<[f64; 2], Error> {
    if !(i[0].is_finite() && i[1].is_finite()) {
        Err(Error::NonFinite)
    } else if i[0] >= i[1] {
        Err(Error::Degenerate)
    } else {
        Ok(i)
    }
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

    /// Gives a live face, edge or vertex a new name; the old one retires.
    pub(crate) fn rename(&mut self, entity: EntityId, name: Name) {
        let slot = match entity {
            EntityId::Face(f) => &mut self.faces[f].name,
            EntityId::Edge(e) => &mut self.edges[e].name,
            EntityId::Vertex(v) => &mut self.vertices[v].name,
            _ => unreachable!("only faces, edges and vertices have names"),
        };
        let old = core::mem::replace(slot, name);
        if self.names.get(&old) == Some(&entity) {
            self.retire_name(old);
        }
        self.names.insert(name, entity);
    }

    /// Drops a name from the index and its recipe.
    pub(crate) fn retire_name(&mut self, name: Name) {
        self.names.remove(&name);
        if let Some(r) = &mut self.recipes {
            r.remove(&name);
        }
    }

    /// The loop and vertex of a corner.
    fn corner(&self, at: Corner) -> Result<(LoopId, VertexId), Error> {
        match at {
            Corner::Before(f) => {
                live(&self.fins, f)?;
                Ok((self.fins[f].loop_, self.start_of(f)))
            }
            Corner::Lone(l) => {
                let vertex = live(&self.loops, l)?.vertex;
                Ok((l, vertex.ok_or(TopologyError::NotLone)?))
            }
        }
    }

    /// The two fins of an edge, or `NotManifold`.
    fn two_fins(&self, e: EdgeId) -> Result<[FinId; 2], Error> {
        let first = live(&self.edges, e)?.fin;
        let second = self.fins[first].around;
        if second == first || self.fins[second].around != first {
            return Err(TopologyError::NotManifold.into());
        }
        Ok([first, second])
    }

    /// Makes `fins` the whole ring of loop `lp`, in order.
    fn ring_fins(&mut self, lp: LoopId, fins: &[FinId]) {
        let n = fins.len();
        for (i, &f) in fins.iter().enumerate() {
            let fin = &mut self.fins[f];
            fin.loop_ = lp;
            fin.next = fins[(i + 1) % n];
            fin.prev = fins[(i + n - 1) % n];
        }
    }

    /// The fins from `from` up to, not including, `to`, following `next`.
    /// The whole ring when `from == to`.
    fn fins_between(&self, from: FinId, to: FinId) -> Vec<FinId> {
        let mut out = vec![from];
        let mut f = self.fins[from].next;
        while f != to {
            out.push(f);
            f = self.fins[f].next;
        }
        out
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

    /// Kills an edge, its fins and its name. Its loops must already be
    /// spliced around the fins.
    fn kill_edge(&mut self, e: EdgeId, fins: [FinId; 2]) {
        let name = self.edges[e].name;
        self.retire_name(name);
        self.edges.kill(e);
        for f in fins {
            self.fins.kill(f);
        }
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
    use crate::geom::{Curve, Frame, Line, Plane, Surface};
    use crate::topo::{Counts, CurveId};

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

    pub(crate) fn counts(v: usize, e: usize, f: usize, l: usize) -> Counts {
        Counts {
            regions: 2,
            shells: 1,
            faces: f,
            loops: l,
            fins: 2 * e,
            edges: e,
            vertices: v,
        }
    }

    /// A line from `p` to `q` and its interval.
    pub(crate) fn line(b: &mut Body, p: Point3, q: Point3) -> (CurveId, [f64; 2]) {
        let c = b.add_curve(Curve::Line(Line::through(p, q).unwrap()));
        (c, [0.0, p.distance(q)])
    }

    #[test]
    fn rename_retires_the_old_name() {
        let (mut b, s) = plane_body();
        let (m, _) = mvfs(&mut b, K, Point3::ORIGIN, s).unwrap();
        let old = b.name_of(EntityId::Face(m.face)).unwrap();
        let new = b.derive_name("test", OpKey(9), Role::NewFace, &[old]);
        b.rename(EntityId::Face(m.face), new);
        assert_eq!(b.by_name(old), None);
        assert_eq!(b.recipe(old), None);
        assert_eq!(b.by_name(new), Some(EntityId::Face(m.face)));
        assert_eq!(b.recipe(new), Some("test#9(mvfs#10/new-face)/new-face"));
    }
}
