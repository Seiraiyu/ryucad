//! Typed indices into a body's flat arrays. Mixing them up is a type
//! error, and none of them means anything outside its own body.

/// An index type for an [`Arena`](super::store::Arena).
pub(crate) trait Id: Copy + Eq {
    fn index(self) -> usize;
    fn from_index(i: usize) -> Self;
}

macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            /// The slot index in the body's array.
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl Id for $name {
            fn index(self) -> usize {
                self.0 as usize
            }

            fn from_index(i: usize) -> Self {
                Self(u32::try_from(i).expect("a body holds fewer than 2^32 entities of a kind"))
            }
        }
    )*};
}

ids!(
    /// A region: the space inside a solid, or the void around it.
    RegionId,
    /// A connected set of faces bounding a region.
    ShellId,
    FaceId,
    /// A boundary of a face: a cycle of fins, or a lone vertex.
    LoopId,
    /// One use of an edge by a loop (Parasolid's fin, a coedge).
    FinId,
    EdgeId,
    VertexId,
    CurveId,
    SurfaceId,
    /// A curve in a face's parameter plane: a fin's pcurve.
    Curve2Id,
);

/// Any topological entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityId {
    Region(RegionId),
    Shell(ShellId),
    Face(FaceId),
    Loop(LoopId),
    Fin(FinId),
    Edge(EdgeId),
    Vertex(VertexId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_their_index() {
        let f = <FaceId as Id>::from_index(7);
        assert_eq!(f.index(), 7);
        assert_eq!(Id::index(f), 7);
        assert!(<EdgeId as Id>::from_index(1) < <EdgeId as Id>::from_index(2));
        assert_ne!(EntityId::Face(f), EntityId::Loop(Id::from_index(7)));
    }
}
