//! The flat arrays and the records they hold.

use super::id::{
    Curve2Id, CurveId, EdgeId, FaceId, FinId, Id, LoopId, RegionId, ShellId, SurfaceId, VertexId,
};
use crate::geom::Point3;
use crate::name::Name;
use crate::tol::Tolerance;
use core::marker::PhantomData;
use core::ops::{Index, IndexMut};

/// A flat array of entity slots indexed by `I`. Deleting an entity leaves
/// a tombstone (`None`); slots are never reused, so a stale ID can never
/// alias a new entity. `Body::compact` drops the tombstones.
#[derive(Clone, Debug)]
pub(crate) struct Arena<I, T> {
    slots: Vec<Option<T>>,
    _id: PhantomData<I>,
}

impl<I, T> Default for Arena<I, T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            _id: PhantomData,
        }
    }
}

impl<I: Id, T> Arena<I, T> {
    pub(crate) fn push(&mut self, value: T) -> I {
        self.slots.push(Some(value));
        I::from_index(self.slots.len() - 1)
    }

    /// The live entity at `id`, or `None` for a tombstone or an index past
    /// the end.
    pub(crate) fn get(&self, id: I) -> Option<&T> {
        self.slots.get(id.index()).and_then(Option::as_ref)
    }

    /// Leaves a tombstone; returns the dead entity.
    pub(crate) fn kill(&mut self, id: I) -> Option<T> {
        self.slots.get_mut(id.index()).and_then(Option::take)
    }

    /// The number of slots, live or dead.
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Live entities in slot order.
    pub(crate) fn live(&self) -> impl Iterator<Item = (I, &T)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|t| (I::from_index(i), t)))
    }

    /// Drops the tombstones, keeping live entities in order. Returns each
    /// old slot's new ID, or `None` for the dropped ones.
    pub(crate) fn compact(&mut self) -> Vec<Option<I>> {
        let mut next = 0;
        let map = self
            .slots
            .iter()
            .map(|s| {
                s.as_ref().map(|_| {
                    next += 1;
                    I::from_index(next - 1)
                })
            })
            .collect();
        self.slots.retain(Option::is_some);
        map
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.slots.iter_mut().flatten()
    }
}

/// Kernel code indexes only IDs it knows are live; a dead one is a kernel
/// bug, so this panics.
impl<I: Id, T> Index<I> for Arena<I, T> {
    type Output = T;
    fn index(&self, id: I) -> &T {
        self.get(id).expect("live entity")
    }
}

impl<I: Id, T> IndexMut<I> for Arena<I, T> {
    fn index_mut(&mut self, id: I) -> &mut T {
        self.slots
            .get_mut(id.index())
            .and_then(Option::as_mut)
            .expect("live entity")
    }
}

/// Whether something runs with or against its reference: a face's normal
/// against its surface's natural normal, or a fin against its edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sense {
    Forward,
    Reversed,
}

impl Sense {
    pub fn flipped(self) -> Self {
        match self {
            Self::Forward => Self::Reversed,
            Self::Reversed => Self::Forward,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegionKind {
    /// Outside every solid. Every body has exactly one void region, its
    /// first.
    Void,
    Solid,
}

/// Rings (`next`/`prev`) are circular: a lone member points at itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub kind: RegionKind,
    pub first_shell: Option<ShellId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub region: RegionId,
    pub next: ShellId,
    pub prev: ShellId,
    pub first_face: Option<FaceId>,
}

/// A face's normal is its surface's natural normal, reversed when `sense`
/// is `Reversed`; it points out of the solid. Its loops run
/// counterclockwise seen from that side: the face is on their left.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub shell: ShellId,
    pub next: FaceId,
    pub prev: FaceId,
    pub first_loop: Option<LoopId>,
    pub surface: SurfaceId,
    pub sense: Sense,
    pub name: Name,
}

/// Exactly one of `first_fin` and `vertex` is set: a cycle of fins, or a
/// lone vertex.
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub face: FaceId,
    pub next: LoopId,
    pub prev: LoopId,
    pub first_fin: Option<FinId>,
    pub vertex: Option<VertexId>,
}

/// One use of an edge by a loop. `Forward` runs from the edge's start to
/// its end. `around` is the next fin in the ring of fins on the same edge.
/// `pcurve` is the edge in the parameter plane of the loop's face; a
/// finished body has one on every fin.
#[derive(Clone, Debug, PartialEq)]
pub struct Fin {
    pub loop_: LoopId,
    pub edge: EdgeId,
    pub sense: Sense,
    pub next: FinId,
    pub prev: FinId,
    pub around: FinId,
    pub pcurve: Option<Pcurve>,
}

/// An edge in a face's `(u, v)` plane: at the edge's parameter `t` it is
/// at `curve(map[0] + map[1] t)`, which the face's surface takes to within
/// the edge's tolerance of the edge's curve at `t`. The affine map keeps
/// pcurves exact where the plane's parameter runs at another speed than
/// the edge's (a cone's ruling) or the other way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pcurve {
    pub curve: Curve2Id,
    pub map: [f64; 2],
}

impl Pcurve {
    /// The pcurve's own parameter at the edge's parameter `t`.
    pub fn param(&self, t: f64) -> f64 {
        self.map[0] + self.map[1] * t
    }
}

/// An edge runs along its curve from `interval[0]` to `interval[1]`
/// (`interval[0] < interval[1]`). A closed edge has `start == end`.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub fin: FinId,
    pub start: VertexId,
    pub end: VertexId,
    pub curve: CurveId,
    pub interval: [f64; 2],
    pub tolerance: Tolerance,
    pub name: Name,
}

/// `edge` is one incident edge, or `None` for the vertex of a lone-vertex
/// loop, which `lone` then names.
#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub point: Point3,
    pub tolerance: Tolerance,
    pub edge: Option<EdgeId>,
    pub lone: Option<LoopId>,
    pub name: Name,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_slots() {
        let mut a: Arena<FaceId, &str> = Arena::default();
        let x = a.push("x");
        let y = a.push("y");
        assert_eq!(a[y], "y");
        a.slots[y.index()] = None;
        assert_eq!(a.get(y), None);
        assert_eq!(a.get(<FaceId as Id>::from_index(9)), None);
        assert_eq!(a.slot_count(), 2);
        a[x] = "w";
        assert_eq!(a.live().map(|(_, v)| *v).collect::<Vec<_>>(), ["w"]);
    }

    #[test]
    fn compaction_keeps_order() {
        let mut a: Arena<FaceId, &str> = Arena::default();
        let x = a.push("x");
        let y = a.push("y");
        a.push("z");
        assert_eq!(a.kill(y), Some("y"));
        assert_eq!(a.kill(y), None);
        assert_eq!(a.compact(), [Some(x), None, Some(y)]);
        assert_eq!(a.slot_count(), 2);
        assert_eq!(a[y], "z");
        for v in a.values_mut() {
            *v = "w";
        }
        assert_eq!(a.live().map(|(_, v)| *v).collect::<Vec<_>>(), ["w", "w"]);
    }

    #[test]
    #[should_panic(expected = "live entity")]
    fn indexing_a_tombstone_panics() {
        let mut a: Arena<EdgeId, u8> = Arena::default();
        let e = a.push(1);
        a.slots[0] = None;
        let _ = a[e];
    }

    #[test]
    fn senses_flip() {
        assert_eq!(Sense::Forward.flipped(), Sense::Reversed);
        assert_eq!(Sense::Reversed.flipped(), Sense::Forward);
    }
}
