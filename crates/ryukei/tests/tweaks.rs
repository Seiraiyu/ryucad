//! Face tweaks against exact volumes: every result checks clean, meshes,
//! keeps its topology and names; refusals by name; the same bits on one
//! thread and eight.

use core::f64::consts::PI;
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{
    Curve2, Frame, KnotVector, Line2, NurbsSurface, Plane, Point2, Point3, Rigid, Surface, Vector3,
};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
use ryukei::topo::{Body, EntityId};
use ryukei::tweak::{
    MoveSpec, OffsetFacesSpec, ReplaceSpec, move_faces, offset_faces, replace_faces,
};
use ryukei::{Error, TweakError};
use std::sync::Arc;

const K: OpKey = OpKey(0xd0);
const T: OpKey = OpKey(0xd1);

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

/// The surface face `f` lies on.
fn surface_of(b: &Body, f: ryukei::topo::FaceId) -> &Surface {
    b.surface(b.face(f).unwrap().surface).unwrap()
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

fn block() -> Body {
    primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0
}

fn refusal<X>(r: Result<X, Error>) -> TweakError {
    match r {
        Err(Error::InvalidTweak(e)) => e,
        Err(e) => panic!("not a tweak's refusal: {e:?}"),
        Ok(_) => panic!("not refused"),
    }
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

fn moved(faces: Vec<Name>, motion: Rigid) -> MoveSpec {
    MoveSpec {
        faces,
        motion,
        tangent: false,
    }
}

/// A turn by `angle` about the line through `c` along `axis`.
fn turn_about(c: Point3, axis: Vector3, angle: f64) -> Rigid {
    let to = Rigid::translation(c.to_vector());
    let back = Rigid::translation(-c.to_vector());
    to.then_after(&Rigid::rotation(axis.normalize().unwrap(), angle).then_after(&back))
}

/// The faces of `b` on surfaces `pick` accepts, by name.
fn faces_where(b: &Body, pick: impl Fn(&Surface) -> bool) -> Vec<Name> {
    let mut out: Vec<Name> = b
        .faces()
        .filter(|&f| pick(surface_of(b, f)))
        .map(|f| b.face(f).unwrap().name)
        .collect();
    out.sort();
    out
}

#[test]
fn a_boxs_top_raised_and_tilted() {
    let b = block();
    let top = face(K, FaceRole::PlusZ);
    let (o, j) = move_faces(
        T,
        &b,
        &moved(vec![top], Rigid::translation(Vector3::new(0.0, 0.0, 0.1))),
    )
    .unwrap();
    near(volume(&o, 1e-3), 0.6 * 0.5, 1e-9);
    assert_eq!(names(&o), names(&b));
    assert!(!j.events().is_empty());
    // Tilted about its middle line along y: a wedge of the same volume.
    let tilt = turn_about(Point3::new(0.5, 0.3, 0.4), Vector3::new(0.0, 1.0, 0.0), 0.2);
    let (o, _) = move_faces(T, &b, &moved(vec![top], tilt)).unwrap();
    near(volume(&o, 1e-3), 0.6 * 0.4, 1e-9);
    assert_eq!(names(&o), names(&b));
}

/// A plate with a through hole of radius 0.1 at `(0.5, 0.5)`.
fn drilled() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (pin, _) = primitive::cylinder(OpKey(0xd2), 0.1, 0.4, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0xd3),
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

#[test]
fn a_hole_moved_across_its_plate() {
    let b = drilled();
    let wall = faces_where(&b, |s| matches!(s, Surface::Cylinder(_)));
    let (o, _) = move_faces(
        T,
        &b,
        &moved(
            wall.clone(),
            Rigid::translation(Vector3::new(0.2, 0.0, 0.0)),
        ),
    )
    .unwrap();
    near(volume(&o, 1e-4), 0.2 - PI * 0.01 * 0.2, 1e-3);
    let Some(EntityId::Face(f)) = o.by_name(wall[0]) else {
        panic!()
    };
    let c = surface_of(&o, f);
    assert!(matches!(c, Surface::Cylinder(_)));
    assert!((c.frame().unwrap().origin().x - 0.7).abs() < 1e-12);
    assert_eq!(names(&o), names(&b));
}

/// A plate with a boss of radius 0.1, 0.1 tall, at `(0.5, 0.5)`.
fn bossed() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (boss, _) = primitive::cylinder(OpKey(0xd2), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0xd3),
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

fn boss_faces(b: &Body) -> Vec<Name> {
    faces_where(b, |s| match s {
        Surface::Cylinder(_) => true,
        Surface::Plane(p) => (p.frame().origin().z - 0.3).abs() < 1e-12,
        _ => false,
    })
}

#[test]
fn a_boss_slid_along_its_plate() {
    let b = bossed();
    let boss = boss_faces(&b);
    assert_eq!(boss.len(), 2);
    let (o, _) = move_faces(
        T,
        &b,
        &moved(boss, Rigid::translation(Vector3::new(-0.25, 0.1, 0.0))),
    )
    .unwrap();
    near(volume(&o, 1e-4), 0.2 + PI * 0.01 * 0.1, 1e-3);
    assert_eq!(names(&o), names(&b));
}

/// `[0, 2] × [0, 2]` less `[1, 2] × [1, 2]`, `h` high.
fn l_block(h: f64) -> Body {
    let pts = [
        (0.0, 0.0),
        (2.0, 0.0),
        (2.0, 1.0),
        (1.0, 1.0),
        (1.0, 2.0),
        (0.0, 2.0),
    ];
    let segments = (0..6)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 6]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap();
    extrude(K, &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, h)
        .unwrap()
        .0
}

#[test]
fn an_ls_end_offset() {
    let h = 0.6;
    let b = l_block(h);
    let end = faces_where(&b, |s| match s {
        Surface::Plane(p) => {
            p.frame().z().get().x > 0.5 && (p.frame().origin().x - 2.0).abs() < 1e-12
        }
        _ => false,
    });
    assert_eq!(end.len(), 1);
    let spec = OffsetFacesSpec {
        faces: vec![(end[0], 0.1)],
        tangent: false,
    };
    let (o, _) = offset_faces(T, &b, &spec).unwrap();
    near(volume(&o, 1e-3), (3.0 + 0.1) * h, 1e-9);
    assert_eq!(names(&o), names(&b));
}

/// Steiner's formula for a box `a × b × c` grown by `d`.
fn steiner_box(a: f64, b: f64, c: f64, d: f64) -> f64 {
    a * b * c
        + 2.0 * (a * b + b * c + c * a) * d
        + PI * (a + b + c) * d * d
        + 4.0 / 3.0 * PI * d * d * d
}

/// A box with every edge rounded to `r`.
fn rounded(r: f64) -> Body {
    let b = block();
    let sides = [
        FaceRole::PlusX,
        FaceRole::MinusX,
        FaceRole::PlusY,
        FaceRole::MinusY,
        FaceRole::PlusZ,
        FaceRole::MinusZ,
    ];
    let opposite = |a: FaceRole, b: FaceRole| {
        matches!(
            (a, b),
            (FaceRole::PlusX, FaceRole::MinusX)
                | (FaceRole::PlusY, FaceRole::MinusY)
                | (FaceRole::PlusZ, FaceRole::MinusZ)
        )
    };
    let mut blends = Vec::new();
    for (i, &a) in sides.iter().enumerate() {
        for &c in &sides[i + 1..] {
            if !opposite(a, c) {
                let mut f = [face(K, a), face(K, c)];
                f.sort();
                blends.push(ryukei::blend::EdgeBlend {
                    edge: Name::derive(K, Role::Edge, &f),
                    kind: ryukei::blend::BlendKind::Fillet { radius: r },
                });
            }
        }
    }
    ryukei::blend::blend(OpKey(0xd5), &b, &blends).unwrap().0
}

#[test]
fn a_rounded_boxs_side_offset_with_its_tangent_chain() {
    let (r, d) = (0.1, 0.05);
    let b = rounded(r);
    let side = face(K, FaceRole::PlusX);
    let spec = |tangent| OffsetFacesSpec {
        faces: vec![(side, d)],
        tangent,
    };
    assert!(matches!(
        refusal(offset_faces(T, &b, &spec(false))),
        TweakError::TangencyBroken { .. }
    ));
    // Every face of a rounded box is tangent to its neighbours: the chain
    // takes them all, and the box grows as its core does by `r + d`.
    let (o, _) = offset_faces(T, &b, &spec(true)).unwrap();
    near(volume(&o, 2e-4), steiner_box(0.8, 0.4, 0.2, r + d), 2e-3);
    assert_eq!(names(&o), names(&b));
}

#[test]
fn a_top_replaced_by_a_tilted_plane_and_by_a_spline() {
    let b = block();
    let top = face(K, FaceRole::PlusZ);
    let tilt = turn_about(
        Point3::new(0.5, 0.3, 0.4),
        Vector3::new(0.0, 1.0, 0.0),
        -0.15,
    );
    let tilted = Surface::Plane(Plane::new(Frame::WORLD))
        .transformed(&tilt.then_after(&Rigid::translation(Vector3::new(0.0, 0.0, 0.4))));
    let (o, _) = replace_faces(
        T,
        &b,
        &ReplaceSpec {
            faces: vec![(top, tilted)],
        },
    )
    .unwrap();
    near(volume(&o, 1e-3), 0.6 * 0.4, 1e-9);
    // A flat bilinear spline at 0.45, wider than the box.
    let knots = || KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
    let pts = vec![
        Point3::new(-1.0, -1.0, 0.45),
        Point3::new(-1.0, 2.0, 0.45),
        Point3::new(2.0, -1.0, 0.45),
        Point3::new(2.0, 2.0, 0.45),
    ];
    let spline = Surface::Nurbs(Arc::new(
        NurbsSurface::new([knots(), knots()], pts, None).unwrap(),
    ));
    let (o, _) = replace_faces(
        T,
        &b,
        &ReplaceSpec {
            faces: vec![(top, spline)],
        },
    )
    .unwrap();
    near(volume(&o, 1e-3), 0.6 * 0.45, 1e-9);
    assert_eq!(names(&o), names(&b));
}

#[test]
fn a_top_beside_its_fillet_moves_only_with_its_chain() {
    let b = block();
    let mut f = [face(K, FaceRole::PlusZ), face(K, FaceRole::PlusX)];
    f.sort();
    let edge = Name::derive(K, Role::Edge, &f);
    let r = 0.1;
    let b = ryukei::blend::blend(
        OpKey(0xd6),
        &b,
        &[ryukei::blend::EdgeBlend {
            edge,
            kind: ryukei::blend::BlendKind::Fillet { radius: r },
        }],
    )
    .unwrap()
    .0;
    let up = Rigid::translation(Vector3::new(0.0, 0.0, 0.05));
    let top = face(K, FaceRole::PlusZ);
    assert!(matches!(
        refusal(move_faces(T, &b, &moved(vec![top], up))),
        TweakError::TangencyBroken { .. }
    ));
    let spec = MoveSpec {
        faces: vec![top],
        motion: up,
        tangent: true,
    };
    let (o, _) = move_faces(T, &b, &spec).unwrap();
    near(
        volume(&o, 2e-4),
        0.6 * (0.45 - r * r * (1.0 - PI / 4.0)),
        1e-3,
    );
}

/// A square pyramid on `[0, 1]²`, its apex at `(0.5, 0.5, 0.5)`: two
/// triangular prisms intersected. Its apex is a vertex of four faces.
fn pyramid() -> Body {
    let tri = |frame: Frame, base: u128| {
        let pts = [(0.0, 0.0), (1.0, 0.0), (0.5, 0.5)];
        let segments = (0..3)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % 3]);
                let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
                Segment {
                    key: SegmentKey(base + i as u128),
                    curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                    t0: 0.0,
                    t1: p.distance(q),
                }
            })
            .collect();
        ValidProfile::new(Profile {
            frame,
            loops: vec![Loop {
                key: LoopKey(base),
                segments,
            }],
        })
        .unwrap()
    };
    // Triangles in the yz plane (extruded along x) and the xz plane
    // (extruded along y).
    let yz = Frame::new(
        Point3::ORIGIN,
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
    )
    .unwrap();
    let xz = Frame::new(
        Point3::ORIGIN,
        Vector3::new(0.0, -1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
    )
    .unwrap();
    let a = extrude(
        OpKey(0xd7),
        &tri(yz, 20),
        Vector3::new(1.0, 0.0, 0.0),
        0.0,
        1.0,
    )
    .unwrap()
    .0;
    let b = extrude(
        OpKey(0xd8),
        &tri(xz, 30),
        Vector3::new(0.0, -1.0, 0.0),
        -1.0,
        0.0,
    )
    .unwrap()
    .0;
    boolean(
        OpKey(0xd9),
        BooleanOp::Intersect,
        &a,
        &[Tool {
            key: ToolKey(2),
            body: &b,
            place: Rigid::IDENTITY,
        }],
    )
    .unwrap()
    .0
}

#[test]
fn a_pyramids_face_offset_would_split_its_apex() {
    let b = pyramid();
    near(volume(&b, 1e-3), 1.0 / 6.0, 1e-9);
    let slanted = faces_where(&b, |s| match s {
        Surface::Plane(p) => p.frame().z().get().z.abs() > 0.1 && p.frame().z().get().z.abs() < 0.9,
        _ => false,
    });
    assert_eq!(slanted.len(), 4);
    let spec = OffsetFacesSpec {
        faces: vec![(slanted[0], 0.02)],
        tangent: false,
    };
    assert!(matches!(
        refusal(offset_faces(T, &b, &spec)),
        TweakError::VertexSplits { .. }
    ));
}

#[test]
fn a_nurbs_boxs_top_raised_carries_its_sides() {
    let mut b = block();
    ryukei_testkit::nurbs::convert_body(&mut b).unwrap();
    let top = face(K, FaceRole::PlusZ);
    let (o, _) = move_faces(
        T,
        &b,
        &moved(vec![top], Rigid::translation(Vector3::new(0.0, 0.0, 0.3))),
    )
    .unwrap();
    near(volume(&o, 1e-3), 0.6 * 0.7, 1e-6);
    assert_eq!(names(&o), names(&b));
}

#[test]
fn a_sheets_top_raised_stretches_its_side() {
    let b = block();
    let (s, _) = ryukei::sheet::extract(
        OpKey(0xda),
        &b,
        &[face(K, FaceRole::PlusZ), face(K, FaceRole::PlusX)],
    )
    .unwrap();
    let up = Rigid::translation(Vector3::new(0.0, 0.0, 0.05));
    let (o, _) = move_faces(T, &s, &moved(vec![face(K, FaceRole::PlusZ)], up)).unwrap();
    assert_eq!(check(&o), []);
    assert_eq!(names(&o), names(&s));
    let top = o
        .vertices()
        .map(|v| o.vertex(v).unwrap().point.z)
        .fold(0.0, f64::max);
    assert!((top - 0.45).abs() < 1e-12);
}

#[test]
fn one_thread_and_eight_alike() {
    let b = bossed();
    let boss = boss_faces(&b);
    let run = |threads| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let spec = moved(
                    boss.clone(),
                    Rigid::translation(Vector3::new(-0.25, 0.1, 0.0)),
                );
                ryukei::codec::encode_body(&move_faces(T, &b, &spec).unwrap().0)
            })
    };
    assert_eq!(run(1), run(8));
}
