//! Drafts against closed forms: every result checks clean, meshes,
//! keeps its topology and names, and reports its angles.

use core::f64::consts::PI;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Point3, Rigid, Surface, UnitVector3, Vector3};
use ryukei::math;
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::tweak::{DraftFace, DraftSpec, Hinge, draft};

const K: OpKey = OpKey(0xe0);
const T: OpKey = OpKey(0xe1);

fn volume(b: &Body, chord: f64) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
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

fn near(got: f64, want: f64, rel: f64) {
    assert!(
        (got - want).abs() <= rel * want.abs(),
        "{got} against {want}"
    );
}

fn face(key: OpKey, r: FaceRole) -> Name {
    Name::derive(key, Role::Face(r), &[])
}

fn names(b: &Body) -> Vec<Name> {
    let mut n: Vec<Name> = b
        .faces()
        .map(|f| b.face(f).unwrap().name)
        .chain(b.edges().map(|e| b.edge(e).unwrap().name))
        .chain(b.vertices().map(|v| b.vertex(v).unwrap().name))
        .collect();
    n.sort();
    n
}

fn faces_where(b: &Body, pick: impl Fn(&Surface) -> bool) -> Vec<Name> {
    let mut out: Vec<Name> = b
        .faces()
        .filter(|&f| pick(b.surface(b.face(f).unwrap().surface).unwrap()))
        .map(|f| b.face(f).unwrap().name)
        .collect();
    out.sort();
    out
}

fn up() -> UnitVector3 {
    UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap()
}

fn spec(faces: &[Name], angle: f64, hinge: Hinge, tangent: bool) -> DraftSpec {
    DraftSpec {
        pull: up(),
        angle,
        hinge,
        faces: faces
            .iter()
            .map(|&face| DraftFace {
                face,
                angle: None,
                flip: false,
            })
            .collect(),
        tangent,
    }
}

fn about(z: f64) -> Hinge {
    Hinge::Neutral {
        origin: Point3::new(0.0, 0.0, z),
    }
}

/// A prismatoid's volume from its areas at its ends and middle.
fn prismatoid(h: f64, a: impl Fn(f64) -> f64) -> f64 {
    h / 6.0 * (a(0.0) + 4.0 * a(h / 2.0) + a(h))
}

fn walls() -> Vec<Name> {
    [
        FaceRole::PlusX,
        FaceRole::MinusX,
        FaceRole::PlusY,
        FaceRole::MinusY,
    ]
    .map(|r| face(K, r))
    .to_vec()
}

#[test]
fn a_boxs_walls_drafted_about_its_bottom_are_a_frustum() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let th = 0.05;
    let (o, _, report) = draft(T, &b, &spec(&walls(), th, about(0.0), false)).unwrap();
    let t = math::tan(th);
    let want = prismatoid(0.4, |z| (1.0 - 2.0 * z * t) * (0.6 - 2.0 * z * t));
    near(volume(&o, 1e-4), want, 1e-9);
    assert_eq!(names(&o), names(&b));
    assert!(report.faces.iter().all(|a| (a.least - th).abs() < 1e-12));
}

#[test]
fn a_boxs_walls_drafted_about_its_middle_are_two_frusta() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let th = 0.05;
    let (o, _, _) = draft(T, &b, &spec(&walls(), th, about(0.2), false)).unwrap();
    let t = math::tan(th);
    // Above the middle the walls lean in, below they lean out.
    let a = |z: f64| (1.0 - 2.0 * z * t) * (0.6 - 2.0 * z * t);
    let want = prismatoid(0.2, a) + prismatoid(0.2, |z| a(-z));
    near(volume(&o, 1e-4), want, 1e-9);
    assert_eq!(names(&o), names(&b));
}

/// A plate with a boss of radius 0.1, 0.1 tall, at `(0.5, 0.5)`.
fn bossed() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (boss, _) = primitive::cylinder(OpKey(0xe2), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0xe3),
        BooleanOp::Unite,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &boss,
            place: Rigid::translation(Vector3::new(0.5, 0.5, 0.15)),
        }],
    )
    .unwrap()
    .0
}

/// A plate with a through hole of radius 0.1 at `(0.5, 0.5)`.
fn drilled() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0xe2), 0.1, 0.4, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0xe3),
        BooleanOp::Subtract,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &pin,
            place: Rigid::translation(Vector3::new(0.5, 0.5, -0.1)),
        }],
    )
    .unwrap()
    .0
}

fn frustum(h: f64, r0: f64, r1: f64) -> f64 {
    PI * h / 3.0 * (r0 * r0 + r0 * r1 + r1 * r1)
}

#[test]
fn a_boss_drafted_about_its_foot_is_a_frustum() {
    let b = bossed();
    let side = faces_where(&b, |s| matches!(s, Surface::Cylinder(_)));
    let th = 0.1;
    let (o, _, report) = draft(T, &b, &spec(&side, th, about(0.2), false)).unwrap();
    let cone = faces_where(&o, |s| matches!(s, Surface::Cone(_)));
    assert_eq!(cone, side);
    let want = 0.2 + frustum(0.1, 0.1, 0.1 - 0.1 * math::tan(th));
    near(volume(&o, 1e-5), want, 1e-4);
    assert_eq!(names(&o), names(&b));
    assert!((report.faces[0].least - th).abs() < 1e-12);
}

#[test]
fn a_hole_drafted_about_the_plates_bottom_opens_upwards() {
    let b = drilled();
    let wall = faces_where(&b, |s| matches!(s, Surface::Cylinder(_)));
    let th = 0.1;
    let (o, _, _) = draft(T, &b, &spec(&wall, th, about(0.0), false)).unwrap();
    let want = 0.2 - frustum(0.2, 0.1, 0.1 + 0.2 * math::tan(th));
    near(volume(&o, 1e-5), want, 1e-4);
    assert_eq!(names(&o), names(&b));
}

#[test]
fn a_spheres_band_drafted_about_its_equator_is_a_frustum() {
    // A ball of radius 0.2 cut to the slab |z| <= 0.1.
    let (ball, _) = primitive::sphere(K, 0.2, &Rigid::IDENTITY).unwrap();
    let (slab, _) = primitive::block(OpKey(0xe4), 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (b, _) = boolean(
        OpKey(0xe5),
        BooleanOp::Intersect,
        &ball,
        &[Tool {
            key: ToolKey(1),
            body: &slab,
            place: Rigid::translation(Vector3::new(-0.5, -0.5, -0.1)),
        }],
    )
    .unwrap();
    let band = faces_where(&b, |s| matches!(s, Surface::Sphere(_)));
    let th = 0.1;
    let (o, _, _) = draft(T, &b, &spec(&band, th, about(0.0), false)).unwrap();
    let t = math::tan(th);
    let want = frustum(0.2, 0.2 + 0.1 * t, 0.2 - 0.1 * t);
    near(volume(&o, 1e-5), want, 1e-4);
    assert_eq!(names(&o), names(&b));
}

/// A 1.0 × 0.6 × 0.4 box with its four vertical edges rounded to `r`.
fn rounded_prism(r: f64) -> Body {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let pairs = [
        (FaceRole::PlusX, FaceRole::PlusY),
        (FaceRole::PlusY, FaceRole::MinusX),
        (FaceRole::MinusX, FaceRole::MinusY),
        (FaceRole::MinusY, FaceRole::PlusX),
    ];
    let blends: Vec<_> = pairs
        .iter()
        .map(|&(a, c)| {
            let mut f = [face(K, a), face(K, c)];
            f.sort();
            ryukei::blend::EdgeBlend {
                edge: Name::derive(K, Role::Edge, &f),
                kind: ryukei::blend::BlendKind::Fillet { radius: r },
            }
        })
        .collect();
    ryukei::blend::blend(OpKey(0xe6), &b, &blends).unwrap().0
}

#[test]
fn a_rounded_prisms_walls_drafted_with_their_corners_stay_tangent() {
    let r = 0.1;
    let b = rounded_prism(r);
    let th = 0.05;
    let t = math::tan(th);
    // Without `tangent`, a wall alone breaks its tangency with a corner.
    let alone = draft(T, &b, &spec(&walls()[..1], th, about(0.0), false));
    assert!(
        matches!(
            alone,
            Err(ryukei::Error::InvalidTweak(
                ryukei::TweakError::TangencyBroken { .. }
            ))
        ),
        "{alone:?}"
    );
    let (o, _, report) = draft(T, &b, &spec(&walls(), th, about(0.0), true)).unwrap();
    assert_eq!(report.faces.len(), 8);
    let a =
        |z: f64| (1.0 - 2.0 * z * t) * (0.6 - 2.0 * z * t) - (4.0 - PI) * (r - z * t) * (r - z * t);
    near(volume(&o, 1e-5), prismatoid(0.4, a), 1e-4);
    assert_eq!(names(&o), names(&b));
    assert_eq!(faces_where(&o, |s| matches!(s, Surface::Cone(_))).len(), 4);
}

/// A 1.0 × 0.6 × 0.2 box with a 0.8 × 0.4 × 0.2 box on its top, inset
/// 0.1 all round: the ledge between them is a parting line.
fn ledged() -> Body {
    let (low, _) = primitive::block(K, 1.0, 0.6, 0.2, &Rigid::IDENTITY).unwrap();
    let (high, _) = primitive::block(OpKey(0xe7), 0.8, 0.4, 0.2, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0xe8),
        BooleanOp::Unite,
        &low,
        &[Tool {
            key: ToolKey(1),
            body: &high,
            place: Rigid::translation(Vector3::new(0.1, 0.1, 0.2)),
        }],
    )
    .unwrap()
    .0
}

#[test]
fn a_parting_line_at_a_ledge_drafts_both_halves_away_from_it() {
    let b = ledged();
    let vertical = |s: &Surface| match s {
        Surface::Plane(p) => p.frame().z().get().z.abs() < 1e-12,
        _ => false,
    };
    let walls = faces_where(&b, vertical);
    assert_eq!(walls.len(), 8);
    // The edges at z = 0.2 between a wall and the ledge.
    let parting: Vec<Name> = b
        .edges()
        .filter(|&e| {
            let edge = b.edge(e).unwrap();
            let c = b.curve(edge.curve).unwrap();
            let [t0, t1] = edge.interval;
            (c.point(t0).z - 0.2).abs() < 1e-12 && (c.point(t1).z - 0.2).abs() < 1e-12
        })
        .map(|e| b.edge(e).unwrap().name)
        .collect();
    assert_eq!(parting.len(), 8);
    let th = 0.05;
    let t = math::tan(th);
    let (o, _, _) = draft(T, &b, &spec(&walls, th, Hinge::PartingLine(parting), false)).unwrap();
    // The top half narrows going up from the ledge, the bottom going down.
    let want = prismatoid(0.2, |z| (0.8 - 2.0 * z * t) * (0.4 - 2.0 * z * t))
        + prismatoid(0.2, |z| (1.0 - 2.0 * z * t) * (0.6 - 2.0 * z * t));
    near(volume(&o, 1e-4), want, 1e-9);
    assert_eq!(names(&o), names(&b));
}

#[test]
fn a_new_angle_renames_nothing() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let (a, ja, _) = draft(T, &b, &spec(&walls(), 0.03, about(0.0), false)).unwrap();
    let (c, jc, _) = draft(T, &b, &spec(&walls(), 0.07, about(0.0), false)).unwrap();
    assert_eq!(names(&a), names(&c));
    assert_eq!(ja.events(), jc.events());
}

#[test]
fn drafts_are_the_same_on_one_thread_and_eight() {
    let b = rounded_prism(0.1);
    let s = spec(&walls(), 0.05, about(0.1), true);
    let run = |n| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
            .install(|| draft(T, &b, &s).unwrap())
    };
    let (a, ja, ra) = run(1);
    let (c, jc, rc) = run(8);
    assert_eq!(
        ryukei::codec::encode_body(&a),
        ryukei::codec::encode_body(&c)
    );
    assert_eq!((ja.events(), ra), (jc.events(), rc));
}

#[test]
fn drafts_refuse_by_name() {
    use ryukei::TweakError::*;
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let refused = |s: DraftSpec| match draft(T, &b, &s) {
        Err(ryukei::Error::InvalidTweak(e)) => e,
        other => panic!("{other:?}"),
    };
    let top = face(K, FaceRole::PlusZ);
    assert_eq!(refused(spec(&walls(), 0.0, about(0.0), false)), BadAngle);
    assert_eq!(
        refused(spec(&[top], 0.05, about(0.2), false)),
        NoHinge { face: top }
    );
    assert_eq!(
        refused(spec(&walls()[..1], 0.05, about(9.0), false)),
        NoHinge { face: walls()[0] }
    );
    assert_eq!(
        refused(spec(&walls()[..1], 0.05, Hinge::PartingLine(vec![]), false)),
        HingeNotOnFace { face: walls()[0] }
    );
    // The top drafted about one of its own edges: its normal is the pull.
    let mut f = [face(K, FaceRole::PlusZ), face(K, FaceRole::PlusX)];
    f.sort();
    let edge = Name::derive(K, Role::Edge, &f);
    assert_eq!(
        refused(spec(&[top], 0.05, Hinge::PartingLine(vec![edge]), false)),
        HingeAlongPull { face: top }
    );
    // A wall drafted so far that its top edge passes the opposite wall.
    let deep = refused(spec(&walls()[..2], 1.4, about(0.0), false));
    assert!(
        matches!(
            deep,
            EdgeVanishes { .. }
                | Uncertain { .. }
                | VertexSplits { .. }
                | SelfIntersecting { .. }
                | FaceCollapses { .. }
        ),
        "{deep:?}"
    );
}

/// A block whose +x wall is a quadratic spline bulging out: an extruded
/// profile with a curved side, drafted to a sweep.
#[test]
#[cfg_attr(debug_assertions, ignore)]
fn a_spline_wall_drafts_to_a_sweep_at_its_angle() {
    use ryukei::geom::{Curve2, Frame, KnotVector, Line2, NurbsCurve2, Point2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    use std::sync::Arc;
    let p = Point2::new;
    let line = |key: u128, a: Point2, b: Point2| Segment {
        key: SegmentKey(key),
        curve: Curve2::Line2(Line2::through(a, b).unwrap()),
        t0: 0.0,
        t1: a.distance(b),
    };
    let bump = NurbsCurve2::new(
        KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap(),
        vec![p(1.0, 0.0), p(1.15, 0.3), p(1.0, 0.6)],
        None,
    )
    .unwrap();
    let segments = vec![
        line(1, p(0.0, 0.0), p(1.0, 0.0)),
        Segment {
            key: SegmentKey(2),
            curve: Curve2::Nurbs2(Arc::new(bump)),
            t0: 0.0,
            t1: 1.0,
        },
        line(3, p(1.0, 0.6), p(0.0, 0.6)),
        line(4, p(0.0, 0.6), p(0.0, 0.0)),
    ];
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    let (b, _) = extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 0.4).unwrap();
    let wall = faces_where(&b, |s| matches!(s, Surface::Extrusion(_)));
    assert_eq!(wall.len(), 1);
    let th = 0.05;
    let (o, _, report) = draft(T, &b, &spec(&wall, th, about(0.0), false)).unwrap();
    volume(&o, 1e-4);
    let a = report.faces[0];
    assert!(a.least <= th + 1e-9 && a.greatest >= th - 1e-9, "{a:?}");
    // Certified over cells a thousandth of a radian wide at most.
    assert!(a.greatest - a.least < 2e-3, "{a:?}");
    assert_eq!(names(&o), names(&b));
}
