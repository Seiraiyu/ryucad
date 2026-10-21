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

/// A slot: a stadium of length 2 and width 1 in the
/// `xy` plane from the origin, extruded 1 along `z`: four sides (two
/// planes and two half cylinders) meeting tangentially at four smooth
/// lateral edges, and tangent chains of four edges round each cap.
pub(crate) fn slot() -> Body {
    use crate::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2, Vector3};
    use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    use core::f64::consts::FRAC_PI_2;
    let line = |key: u128, a: (f64, f64), b: (f64, f64)| {
        let (a, b) = (Point2::new(a.0, a.1), Point2::new(b.0, b.1));
        Segment {
            key: SegmentKey(key),
            curve: Curve2::Line2(Line2::through(a, b).unwrap()),
            t0: 0.0,
            t1: a.distance(b),
        }
    };
    let arc = |key: u128, x: f64, t0: f64| {
        let f = Frame2::new(Point2::new(x, 0.5), Vector2::new(1.0, 0.0), true).unwrap();
        Segment {
            key: SegmentKey(key),
            curve: Curve2::Circle2(Circle2::new(f, 0.5).unwrap()),
            t0,
            t1: t0 + 2.0 * FRAC_PI_2,
        }
    };
    let profile = Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![
                line(11, (0.0, 0.0), (2.0, 0.0)),
                arc(12, 2.0, -FRAC_PI_2),
                line(13, (2.0, 1.0), (0.0, 1.0)),
                arc(14, 0.0, FRAC_PI_2),
            ],
        }],
    };
    let profile = ValidProfile::new(profile).unwrap();
    extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
        .unwrap()
        .0
}
