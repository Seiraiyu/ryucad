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

/// An edge by its name.
pub(crate) fn edge_id(b: &Body, n: Name) -> crate::topo::EdgeId {
    match b.by_name(n) {
        Some(EntityId::Edge(e)) => e,
        other => panic!("no edge {n}: {other:?}"),
    }
}

/// A boss: a cylinder of radius 1 standing 1 high on a 4 × 4 × 1 plate,
/// united; its foot is a concave circle at `z = 1`.
pub(crate) fn boss() -> Body {
    use crate::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use crate::geom::Vector3;
    let plate = primitive::block(OpKey(2), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let cyl = primitive::cylinder(OpKey(1), 1.0, 1.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &cyl,
        place: Rigid::translation(Vector3::new(2.0, 2.0, 0.5)),
    };
    boolean(OpKey(3), BooleanOp::Unite, &plate, &[tool])
        .unwrap()
        .0
}

/// A cylinder of radius 1 from `z = 0` to `2`, cut by a plane through
/// `(0, 0, 1.5)` tilted 0.3 rad about `x`: its top rim is an ellipse.
pub(crate) fn slanted() -> Body {
    use crate::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use crate::geom::{UnitVector3, Vector3};
    let cyl = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let cutter = primitive::block(OpKey(2), 6.0, 6.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let x = UnitVector3::new_unchecked(Vector3::new(1.0, 0.0, 0.0));
    let place = Rigid::translation(Vector3::new(0.0, 0.0, 1.5))
        .then_after(&Rigid::rotation(x, 0.3))
        .then_after(&Rigid::translation(Vector3::new(-3.0, -3.0, 0.0)));
    let tool = Tool {
        key: ToolKey(1),
        body: &cutter,
        place,
    };
    boolean(OpKey(3), BooleanOp::Subtract, &cyl, &[tool])
        .unwrap()
        .0
}

/// A shaft of radius 1 along `x` from `-2` to `2`, and a boss of radius
/// 0.5 along `z` standing on it to `z = 2`, united: the foot is a closed
/// curve where two cylinders cross, fitted.
pub(crate) fn shaft() -> Body {
    use crate::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use crate::geom::{UnitVector3, Vector3};
    let y = UnitVector3::new_unchecked(Vector3::new(0.0, 1.0, 0.0));
    let along_x = Rigid::rotation(y, core::f64::consts::FRAC_PI_2)
        .then_after(&Rigid::translation(Vector3::new(0.0, 0.0, -2.0)));
    let shaft = primitive::cylinder(OpKey(1), 1.0, 4.0, &along_x).unwrap().0;
    let boss = primitive::cylinder(OpKey(2), 0.5, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &boss,
        place: Rigid::IDENTITY,
    };
    boolean(OpKey(3), BooleanOp::Unite, &shaft, &[tool])
        .unwrap()
        .0
}
