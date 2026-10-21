//! Bodies and names shared by the blend's tests.

use crate::geom::Rigid;
use crate::name::{FaceRole, Name, OpKey, Role};
use crate::primitive;
use crate::topo::{Body, EntityId, FaceId};

pub(crate) const K: OpKey = OpKey(0xb1e);

/// A unit cube from the origin, keyed by [`K`].
pub(crate) fn block() -> Body {
    primitive::block(K, 1.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0
}

/// The name of a primitive's face.
pub(crate) fn face_name(role: FaceRole) -> Name {
    Name::derive(K, Role::Face(role), &[])
}

/// A primitive's face by its role.
pub(crate) fn face_of(b: &Body, role: FaceRole) -> FaceId {
    match b.by_name(face_name(role)) {
        Some(EntityId::Face(f)) => f,
        other => panic!("no face {role:?}: {other:?}"),
    }
}

/// The name of a primitive's edge between two faces.
pub(crate) fn edge_of(_: &Body, a: FaceRole, b: FaceRole) -> Name {
    let mut faces = [face_name(a), face_name(b)];
    faces.sort();
    Name::derive(K, Role::Edge, &faces)
}
