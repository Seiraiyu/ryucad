//! Fillets against exact volumes: every result passes the checker and
//! the self-intersection check, meshes without faults, and encloses the
//! volume it should.

use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::check::{check, check_intersections};
use ryukei::geom::Rigid;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::Body;
use std::f64::consts::PI;

const K: OpKey = OpKey(0xb0);

/// The volume a sound body's mesh encloses.
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

fn fillet(edge: Name, radius: f64) -> EdgeBlend {
    EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius },
    }
}

/// A primitive's edge between two faces.
fn edge(key: OpKey, a: FaceRole, b: FaceRole) -> Name {
    let mut f = [
        Name::derive(key, Role::Face(a), &[]),
        Name::derive(key, Role::Face(b), &[]),
    ];
    f.sort();
    Name::derive(key, Role::Edge, &f)
}

/// The area of the corner a fillet of radius `r` takes from a square
/// edge, and how far its centroid is from the edge along each face.
fn corner(r: f64) -> (f64, f64) {
    (
        (1.0 - PI / 4.0) * r * r,
        r * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI),
    )
}

#[test]
fn one_edge_of_a_box() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (r, _) = blend(
        K,
        &b,
        &[fillet(edge(k, FaceRole::PlusX, FaceRole::PlusZ), 0.25)],
    )
    .unwrap();
    assert_eq!(check_intersections(&r), []);
    let c = r.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (7, 15, 10));
    let (area, _) = corner(0.25);
    near(volume(&r, 1e-5), 6.0 - area * 2.0, 1e-5);
}

#[test]
fn a_cylinders_rim() {
    let b = primitive::cylinder(OpKey(1), 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rim = b.edges().next().unwrap();
    let (r, _) = blend(K, &b, &[fillet(b.edge(rim).unwrap().name, 0.2)]).unwrap();
    assert_eq!(check_intersections(&r), []);
    let (area, d) = corner(0.2);
    near(volume(&r, 1e-5), PI - 2.0 * PI * (1.0 - d) * area, 2e-4);
}

/// A plate 4 × 4 × 1 with a hole of radius 0.5 through its middle.
fn plate() -> Body {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::Vector3;
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &rod,
        place: Rigid::translation(Vector3::new(2.0, 2.0, -1.0)),
    };
    boolean(OpKey(3), BooleanOp::Subtract, &plate, &[tool])
        .unwrap()
        .0
}

/// The circular edges of a body at height `z`.
fn circles_at(b: &Body, z: f64) -> Vec<Name> {
    use ryukei::geom::Curve;
    b.edges()
        .filter(|&e| {
            let ed = b.edge(e).unwrap();
            matches!(b.curve(ed.curve), Some(Curve::Circle(c)) if (c.frame().origin().z - z).abs() < 1e-9)
        })
        .map(|e| b.edge(e).unwrap().name)
        .collect()
}

#[test]
fn a_holes_rim() {
    let b = plate();
    let rim = circles_at(&b, 1.0);
    assert_eq!(rim.len(), 1);
    let (r, _) = blend(K, &b, &[fillet(rim[0], 0.2)]).unwrap();
    assert_eq!(check_intersections(&r), []);
    let (area, d) = corner(0.2);
    // The ring the fillet removes has its centroid at radius 0.5 + d.
    near(
        volume(&r, 1e-5),
        16.0 - PI * 0.25 - 2.0 * PI * (0.5 + d) * area,
        2e-4,
    );
}

#[test]
fn a_bosss_foot_is_filled() {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::Vector3;
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
    let foot = circles_at(&b, 1.0);
    assert_eq!(foot.len(), 1);
    let (r, _) = blend(K, &b, &[fillet(foot[0], 0.2)]).unwrap();
    assert_eq!(check_intersections(&r), []);
    let (area, d) = corner(0.2);
    // The ring the fillet adds has its centroid at radius 1 + d.
    near(
        volume(&r, 1e-5),
        16.0 + PI + 2.0 * PI * (1.0 + d) * area,
        2e-4,
    );
}

/// A slot: a stadium of length 2 and width 1 extruded 1 along `z`.
fn slot() -> Body {
    use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2, Vector3};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    use std::f64::consts::FRAC_PI_2;
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
    extrude(OpKey(1), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0)
        .unwrap()
        .0
}

/// The edges of a body with both ends at height `z`.
fn edges_at(b: &Body, z: f64) -> Vec<Name> {
    b.edges()
        .filter(|&e| {
            let ed = b.edge(e).unwrap();
            [ed.start, ed.end]
                .iter()
                .all(|&v| (b.vertex(v).unwrap().point.z - z).abs() < 1e-9)
        })
        .map(|e| b.edge(e).unwrap().name)
        .collect()
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_slots_top_all_round() {
    use ryukei::geom::Surface;
    let b = slot();
    let top = edges_at(&b, 1.0);
    assert_eq!(top.len(), 4);
    let blends: Vec<EdgeBlend> = top.iter().map(|&e| fillet(e, 0.1)).collect();
    let (r, _) = blend(K, &b, &blends).unwrap();
    // Blend faces meeting tangentially along a section can't be checked
    // apart until plan 6.D: every other pair can.
    for f in check_intersections(&r) {
        let ryukei::check::Fault::CannotCheck { a, b } = f else {
            panic!("{f:?}")
        };
        let blend_face = |x| {
            matches!(
                r.surface(r.face(x).unwrap().surface),
                Some(Surface::Cylinder(c)) if c.radius() == 0.1
            ) || matches!(
                r.surface(r.face(x).unwrap().surface),
                Some(Surface::Torus(_))
            )
        };
        assert!(blend_face(a) && blend_face(b));
    }
    let c = r.counts();
    // Four blend faces, four rails on the top, four sections between the
    // blend faces, and the four corners where the rails meet.
    assert_eq!((c.faces, c.edges, c.vertices), (10, 20, 12));
    let (area, d) = corner(0.1);
    let want = 2.0 + PI / 4.0 - 4.0 * area - 2.0 * PI * (0.5 - d) * area;
    near(volume(&r, 1e-5), want, 2e-4);
}

#[test]
fn an_edge_split_in_two() {
    use ryukei::euler;
    let k = OpKey(1);
    let mut b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let n = edge(k, FaceRole::PlusX, FaceRole::PlusZ);
    let Some(ryukei::topo::EntityId::Edge(e)) = b.by_name(n) else {
        panic!()
    };
    let [t0, t1] = b.edge(e).unwrap().interval;
    let p = b
        .curve(b.edge(e).unwrap().curve)
        .unwrap()
        .point((t0 + t1) / 2.0);
    let (s, _) = euler::semv(&mut b, OpKey(5), e, (t0 + t1) / 2.0, p).unwrap();
    ryukei::pcurve::fill(&mut b).unwrap();
    let halves = [b.edge(e).unwrap().name, b.edge(s.edge).unwrap().name];
    let (r, _) = blend(K, &b, &[fillet(halves[0], 0.25), fillet(halves[1], 0.25)]).unwrap();
    assert_eq!(check_intersections(&r), []);
    let c = r.counts();
    // As for the whole edge: the halves are one rail again.
    assert_eq!((c.faces, c.edges, c.vertices), (7, 15, 10));
    let (area, _) = corner(0.25);
    near(volume(&r, 1e-5), 6.0 - area * 2.0, 1e-5);
}

/// A cylinder of radius 1 from `z = 0` to `2`, cut by a plane through
/// `(0, 0, 1.5)` tilted 0.3 rad about `x`: its top rim is an ellipse.
fn slanted() -> Body {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::Vector3;
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
fn an_oblique_rim_is_rounded_by_a_pipe() {
    use ryukei::geom::{Curve, Surface};
    let b = slanted();
    let rim = b
        .edges()
        .find(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Ellipse(_))))
        .unwrap();
    let (r, _) = blend(K, &b, &[fillet(b.edge(rim).unwrap().name, 0.2)]).unwrap();
    assert!(r.faces().any(|f| matches!(
        r.surface(r.face(f).unwrap().surface),
        Some(Surface::Pipe(_))
    )));
    let before = volume(&b, 1e-4);
    let after = volume(&r, 1e-4);
    // Between the body and the body less the corner all round (the rim is
    // a little longer than its circle).
    let (area, _) = corner(0.2);
    assert!(
        after < before && after > before - 1.1 * 2.0 * PI * area / ryukei::math::cos(0.3),
        "{before} {after}"
    );
}

/// A shaft of radius 1 along `x` and a boss of radius 0.5 standing on it.
fn shaft() -> Body {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::Vector3;
    let y = Vector3::new(0.0, 1.0, 0.0).normalize().unwrap();
    let along_x = Rigid::rotation(y, std::f64::consts::FRAC_PI_2)
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

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_boss_on_a_shaft_is_filled_by_a_pipe() {
    use ryukei::geom::{Curve, Surface};
    let b = shaft();
    let foot = b
        .edges()
        .find(|&e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Nurbs(_))))
        .unwrap();
    let (r, _) = blend(K, &b, &[fillet(b.edge(foot).unwrap().name, 0.1)]).unwrap();
    assert!(r.faces().any(|f| matches!(
        r.surface(r.face(f).unwrap().surface),
        Some(Surface::Pipe(_))
    )));
    let before = volume(&b, 1e-4);
    let after = volume(&r, 1e-4);
    assert!(after > before, "{before} {after}");
}

/// A plate 4 × 4 × 1 with holes of radius 0.5 through it at `centres`.
fn holes(centres: &[(f64, f64)]) -> Body {
    use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
    use ryukei::geom::Vector3;
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools: Vec<Tool> = centres
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| Tool {
            key: ToolKey(i as u128 + 1),
            body: &rod,
            place: Rigid::translation(Vector3::new(x, y, -1.0)),
        })
        .collect();
    boolean(OpKey(3), BooleanOp::Subtract, &plate, &tools)
        .unwrap()
        .0
}

#[test]
fn other_holes_stay_on_their_face() {
    // One rim of two: the other hole stays on the top.
    let b = holes(&[(1.0, 1.0), (3.0, 3.0)]);
    let rims = circles_at(&b, 1.0);
    assert_eq!(rims.len(), 2);
    let (area, d) = corner(0.2);
    let want = 16.0 - 2.0 * PI * 0.25 - 2.0 * PI * (0.5 + d) * area;
    for rim in rims {
        let (r, _) = blend(K, &b, &[fillet(rim, 0.2)]).unwrap();
        assert_eq!(check_intersections(&r), []);
        near(volume(&r, 1e-5), want, 2e-4);
    }
    // Each edge of the plate's top beside a hole: the hole stays on the
    // top, whichever face the blend's new rail goes on.
    let b = holes(&[(2.0, 2.0)]);
    let (area, _) = corner(0.25);
    for side in [
        FaceRole::PlusX,
        FaceRole::MinusX,
        FaceRole::PlusY,
        FaceRole::MinusY,
    ] {
        let e = edge(OpKey(1), side, FaceRole::PlusZ);
        let (r, _) = blend(K, &b, &[fillet(e, 0.25)]).unwrap();
        assert_eq!(check_intersections(&r), []);
        near(volume(&r, 1e-5), 16.0 - PI * 0.25 - 4.0 * area, 1e-4);
    }
}
