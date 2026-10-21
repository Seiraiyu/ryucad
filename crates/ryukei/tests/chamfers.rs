//! Chamfers against exact volumes: every result passes the checker and
//! the self-intersection check, meshes without faults, and encloses the
//! volume it should.

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Curve, Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::{BlendError, Error};
use std::f64::consts::PI;

const K: OpKey = OpKey(0xc0);

fn volume(b: &Body, chord: f64) -> f64 {
    assert_eq!(check(b), []);
    let m = tessellate(b, &MeshParams::new(chord, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn near(got: f64, want: f64, tol: f64) {
    assert!((got - want).abs() <= tol, "{got} against {want}");
}

fn face(key: OpKey, r: FaceRole) -> Name {
    Name::derive(key, Role::Face(r), &[])
}

fn edge(key: OpKey, a: FaceRole, b: FaceRole) -> Name {
    let mut f = [face(key, a), face(key, b)];
    f.sort();
    Name::derive(key, Role::Edge, &f)
}

fn chamfer(edge: Name, c: Chamfer) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Chamfer(c),
    }
}

fn equal(edge: Name, distance: f64) -> EdgeBlend {
    chamfer(edge, Chamfer::Equal { distance })
}

fn faces_on(b: &Body, f: impl Fn(&Surface) -> bool) -> usize {
    b.faces()
        .filter(|&x| f(b.surface(b.face(x).unwrap().surface).unwrap()))
        .count()
}

#[test]
fn a_box_edge_three_ways() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let e = edge(k, FaceRole::PlusX, FaceRole::PlusZ);
    let (r, _) = blend(K, &b, &[equal(e, 0.2)]).unwrap();
    assert_eq!(check_intersections(&r), []);
    assert_eq!(faces_on(&r, |s| matches!(s, Surface::Plane(_))), 7);
    near(volume(&r, 1e-3), 6.0 - 0.5 * 0.2 * 0.2 * 2.0, 1e-9);
    // 0.1 on +X, 0.3 on +Z.
    let two = Chamfer::Distances {
        first: 0.1,
        second: 0.3,
        first_face: face(k, FaceRole::PlusX),
    };
    let (r, _) = blend(K, &b, &[chamfer(e, two)]).unwrap();
    near(volume(&r, 1e-3), 6.0 - 0.5 * 0.1 * 0.3 * 2.0, 1e-9);
    // The box's top is at z = 3: 0.1 down +X and 0.3 in along +Z.
    let at = |x: f64, z: f64| {
        r.vertices().any(|v| {
            let p = r.vertex(v).unwrap().point;
            (p.x - x).abs() < 1e-12 && (p.z - z).abs() < 1e-12
        })
    };
    assert!(at(1.0, 2.9) && at(0.7, 3.0));
    // 0.2 on +Z at 30° to it: 0.2 sin 30° / sin 120° on +X.
    let angle = Chamfer::DistanceAngle {
        distance: 0.2,
        angle: PI / 6.0,
        face: face(k, FaceRole::PlusZ),
    };
    let (r, _) = blend(K, &b, &[chamfer(e, angle)]).unwrap();
    let other = 0.2 * 0.5 / ryukei::math::sin(PI * 2.0 / 3.0);
    near(volume(&r, 1e-3), 6.0 - 0.5 * 0.2 * other * 2.0, 1e-9);
}

#[test]
fn rims_are_cones() {
    let b = primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rim = b.edges().next().unwrap();
    let (r, _) = blend(K, &b, &[equal(b.edge(rim).unwrap().name, 0.2)]).unwrap();
    assert_eq!(check_intersections(&r), []);
    assert_eq!(faces_on(&r, |s| matches!(s, Surface::Cone(_))), 1);
    // The ring of triangles removed has its centroid a third of the way
    // in from the rim.
    let ring = 2.0 * PI * (1.0 - 0.2 / 3.0) * 0.5 * 0.2 * 0.2;
    near(volume(&r, 1e-5), PI - ring, 2e-4);
}

#[test]
fn a_bosss_foot_is_filled_by_a_cone() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let cyl = primitive::cylinder(OpKey(2), 1.0, 1.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &cyl,
        place: Rigid::translation(Vector3::new(2.0, 2.0, 0.5)),
    };
    let b = boolean(OpKey(3), BooleanOp::Unite, &plate, &[tool])
        .unwrap()
        .0;
    let foot = b
        .edges()
        .find(|&e| {
            matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Circle(c)) if (c.frame().origin().z - 1.0).abs() < 1e-9)
        })
        .unwrap();
    let (r, _) = blend(K, &b, &[equal(b.edge(foot).unwrap().name, 0.2)]).unwrap();
    assert_eq!(check_intersections(&r), []);
    let ring = 2.0 * PI * (1.0 + 0.2 / 3.0) * 0.5 * 0.2 * 0.2;
    near(volume(&r, 1e-5), 16.0 + PI + ring, 2e-4);
}

/// A slot: a stadium of length 2 and width 1 extruded 1 along `z`.
fn slot() -> Body {
    use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
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
            t1: t0 + PI,
        }
    };
    let profile = Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![
                line(11, (0.0, 0.0), (2.0, 0.0)),
                arc(12, 2.0, -PI / 2.0),
                line(13, (2.0, 1.0), (0.0, 1.0)),
                arc(14, 0.0, PI / 2.0),
            ],
        }],
    };
    let profile = ValidProfile::new(profile).unwrap();
    extrude(OpKey(1), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
        .unwrap()
        .0
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_slots_top_all_round() {
    let b = slot();
    let top: Vec<Name> = b
        .edges()
        .filter(|&e| {
            let ed = b.edge(e).unwrap();
            [ed.start, ed.end]
                .iter()
                .all(|&v| b.vertex(v).unwrap().point.z > 0.5)
        })
        .map(|e| b.edge(e).unwrap().name)
        .collect();
    let blends: Vec<EdgeBlend> = top.iter().map(|&e| equal(e, 0.1)).collect();
    let (r, _) = blend(K, &b, &blends).unwrap();
    for f in check_intersections(&r) {
        assert!(
            matches!(f, ryukei::check::Fault::CannotCheck { .. }),
            "{f:?}"
        );
    }
    let c = r.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (10, 20, 12));
    let tri = 0.5 * 0.1 * 0.1;
    let want = 2.0 + PI / 4.0 - 4.0 * tri - 2.0 * PI * (0.5 - 0.1 / 3.0) * tri;
    near(volume(&r, 1e-5), want, 2e-4);
}

/// A cylinder of radius 1 from `z = 0` to `2`, cut by a plane through
/// `(0, 0, 1.5)` tilted 0.3 rad about `x`: its top rim is an ellipse.
fn slanted() -> Body {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    let cyl = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let cutter = primitive::block(OpKey(2), 6.0, 6.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let x = Vector3::new(1.0, 0.0, 0.0).normalize().unwrap();
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

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn an_oblique_rim_is_a_ruled_band() {
    let b = slanted();
    let rim = b
        .edges()
        .find(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Ellipse(_))))
        .unwrap();
    let n = b.edge(rim).unwrap().name;
    let (r, _) = blend(K, &b, &[equal(n, 0.1)]).unwrap();
    assert_eq!(faces_on(&r, |s| matches!(s, Surface::Nurbs(_))), 1);
    let (before, after) = (volume(&b, 1e-4), volume(&r, 1e-4));
    // About the triangle all round the rim, which is a little longer
    // than its circle.
    let tri = 0.5 * 0.1 * 0.1 * 2.0 * PI;
    assert!(
        after < before - 0.9 * tri && after > before - 1.2 * tri,
        "{before} {after}"
    );
    // An angle needs the same wedge all along: not round an oblique rim.
    let fin = b.edge_fins(rim).next().unwrap();
    let side = b.loop_(b.fin(fin).unwrap().loop_).unwrap().face;
    let angle = Chamfer::DistanceAngle {
        distance: 0.1,
        angle: 0.5,
        face: b.face(side).unwrap().name,
    };
    assert_eq!(
        blend(K, &b, &[chamfer(n, angle)]).unwrap_err(),
        Error::InvalidBlend(BlendError::Unsupported { at: n })
    );
    // Half the rim: a ruled chamfer that would end on the cut face, which
    // can't be intersected with it yet.
    let half = primitive::block(
        OpKey(4),
        3.0,
        6.0,
        6.0,
        &Rigid::translation(Vector3::new(-3.0, -3.0, -1.0)),
    )
    .unwrap()
    .0;
    let tool = ryukei::boolean::Tool {
        key: ryukei::boolean::ToolKey(9),
        body: &half,
        place: Rigid::IDENTITY,
    };
    let h = ryukei::boolean::boolean(OpKey(5), ryukei::boolean::BooleanOp::Subtract, &b, &[tool])
        .unwrap()
        .0;
    let arc = h
        .edges()
        .find(|&e| matches!(h.curve(h.edge(e).unwrap().curve), Some(Curve::Ellipse(_))))
        .unwrap();
    let err = blend(K, &h, &[equal(h.edge(arc).unwrap().name, 0.1)]).unwrap_err();
    assert!(
        matches!(err, Error::InvalidBlend(BlendError::Unsupported { .. })),
        "{err:?}"
    );
}

#[test]
fn refusals() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let e = edge(k, FaceRole::PlusX, FaceRole::PlusZ);
    // A wedge of 90° and a chamfer at 100° to a face: past the other face.
    let angle = Chamfer::DistanceAngle {
        distance: 0.2,
        angle: 1.5,
        face: face(k, FaceRole::PlusZ),
    };
    assert!(blend(K, &b, &[chamfer(e, angle)]).is_ok());
    let r = blend(K, &b, &[equal(e, 1.5)]);
    assert!(
        matches!(r, Err(Error::InvalidBlend(BlendError::Overflow { .. }))),
        "{r:?}"
    );
}
