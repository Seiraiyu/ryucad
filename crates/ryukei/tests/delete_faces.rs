//! Deleting faces (ryukei 13.B): holes, bosses and pockets capped by
//! their hosts, fillets, chamfers, a rounded box and a step grown back,
//! against exact volumes; refusals; names; one thread and eight.

use ryukei::blend::{BlendKind, Chamfer, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Rigid, Surface, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Event, FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::topo::{Body, EdgeId, FaceId};
use ryukei::tweak::{DeleteSpec, Heal, delete_faces, feature_faces};
use ryukei::{Error, TweakError};

const K: OpKey = OpKey(0x13c0);
const D: OpKey = OpKey(0x13c1);

fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-4, 0.1)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn near(got: f64, want: f64, abs: f64) {
    assert!((got - want).abs() <= abs, "{got} against {want}");
}

fn face(r: FaceRole) -> Name {
    Name::derive(K, Role::Face(r), &[])
}

fn name(b: &Body, f: FaceId) -> Name {
    b.face(f).unwrap().name
}

fn faces_of(b: &Body, e: EdgeId) -> Vec<Name> {
    b.edge_fins(e)
        .map(|fin| name(b, b.loop_(b.fin(fin).unwrap().loop_).unwrap().face))
        .collect()
}

/// The faces whose surfaces `pick` takes.
fn on(b: &Body, pick: fn(&Surface) -> bool) -> Vec<Name> {
    b.faces()
        .filter(|&f| pick(b.surface(b.face(f).unwrap().surface).unwrap()))
        .map(|f| name(b, f))
        .collect()
}

fn not_planes(b: &Body) -> Vec<Name> {
    on(b, |s| !matches!(s, Surface::Plane(_)))
}

fn cut(target: &Body, tool: &Body, at: Vector3, op: BooleanOp) -> Body {
    // A key per call, so a second cut names nothing the first did.
    let key = OpKey(0x13c2 + target.faces().count() as u128);
    boolean(
        key,
        op,
        target,
        &[Tool {
            key: ToolKey(1),
            body: tool,
            place: Rigid::translation(at),
        }],
    )
    .unwrap()
    .0
}

fn plate() -> Body {
    primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY)
        .unwrap()
        .0
}

fn pin(r: f64, h: f64) -> Body {
    primitive::cylinder(OpKey(0x13c3), r, h, &Rigid::IDENTITY)
        .unwrap()
        .0
}

fn spec(faces: Vec<Name>) -> DeleteSpec {
    DeleteSpec {
        faces,
        heal: Heal::Auto,
    }
}

/// The edge between the faces named `a` and `b`.
fn edge_between(b: &Body, a: Name, c: Name) -> Name {
    let e = b
        .edges()
        .find(|&e| {
            let n = faces_of(b, e);
            n.contains(&a) && n.contains(&c)
        })
        .unwrap();
    b.edge(e).unwrap().name
}

fn edged(kind: BlendKind) -> Body {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let edge = edge_between(&b, face(FaceRole::PlusZ), face(FaceRole::MinusY));
    blend(OpKey(0x13c4), &b, &[EdgeBlend { edge, kind }])
        .unwrap()
        .0
}

#[test]
fn a_through_hole_and_a_blind_hole_are_capped() {
    let through = cut(
        &plate(),
        &pin(0.1, 0.6),
        Vector3::new(0.5, 0.5, -0.2),
        BooleanOp::Subtract,
    );
    let (o, _) = delete_faces(D, &through, &spec(not_planes(&through))).unwrap();
    near(volume(&o), 0.2, 1e-9);
    let blind = cut(
        &plate(),
        &pin(0.1, 0.3),
        Vector3::new(0.5, 0.5, 0.1),
        BooleanOp::Subtract,
    );
    let wall = not_planes(&blind)[0];
    let hole = feature_faces(&blind, wall).unwrap();
    assert_eq!(hole.len(), 2);
    let (o, _) = delete_faces(D, &blind, &spec(hole)).unwrap();
    near(volume(&o), 0.2, 1e-9);
}

#[test]
fn a_counterbored_hole_with_its_rim_fillet_is_one_feature() {
    let b = cut(
        &plate(),
        &pin(0.08, 0.6),
        Vector3::new(0.5, 0.5, -0.2),
        BooleanOp::Subtract,
    );
    let b = cut(
        &b,
        &pin(0.15, 0.3),
        Vector3::new(0.5, 0.5, 0.12),
        BooleanOp::Subtract,
    );
    // The counterbore's rim on the top, filleted at 0.02.
    let cbore = on(
        &b,
        |s| matches!(s, Surface::Cylinder(c) if c.radius() > 0.1),
    )[0];
    let edge = edge_between(&b, face(FaceRole::PlusZ), cbore);
    let (b, _) = blend(
        OpKey(0x13c5),
        &b,
        &[EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius: 0.02 },
        }],
    )
    .unwrap();
    let bore = on(
        &b,
        |s| matches!(s, Surface::Cylinder(c) if c.radius() < 0.1),
    )[0];
    let wall = on(
        &b,
        |s| matches!(s, Surface::Cylinder(c) if c.radius() > 0.1),
    )[0];
    // The bore alone is the smallest feature holding it: the counterbore's
    // floor caps it.
    assert_eq!(feature_faces(&b, bore).unwrap(), vec![bore]);
    // From the counterbore's wall: its floor, the fillet and the bore too.
    let f = feature_faces(&b, wall).unwrap();
    assert_eq!(f.len(), 4, "{f:?}");
    let (o, _) = delete_faces(D, &b, &spec(f)).unwrap();
    near(volume(&o), 0.2, 1e-9);
}

#[test]
fn a_boss_with_its_foot_fillet_and_a_pocket_are_capped() {
    let b = cut(
        &plate(),
        &pin(0.1, 0.15),
        Vector3::new(0.5, 0.5, 0.2),
        BooleanOp::Unite,
    );
    let side = on(&b, |s| matches!(s, Surface::Cylinder(_)))[0];
    let edge = edge_between(&b, face(FaceRole::PlusZ), side);
    let (b, _) = blend(
        OpKey(0x13c5),
        &b,
        &[EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius: 0.02 },
        }],
    )
    .unwrap();
    let side = on(&b, |s| matches!(s, Surface::Cylinder(_)))[0];
    let boss = feature_faces(&b, side).unwrap();
    assert_eq!(boss.len(), 3);
    let (o, _) = delete_faces(D, &b, &spec(boss)).unwrap();
    near(volume(&o), 0.2, 1e-9);
    let (pocket, _) = primitive::block(OpKey(0x13c6), 0.3, 0.3, 0.2, &Rigid::IDENTITY).unwrap();
    let p = cut(
        &plate(),
        &pocket,
        Vector3::new(0.35, 0.35, 0.1),
        BooleanOp::Subtract,
    );
    let floor = p
        .faces()
        .find(|&f| {
            let bx = ryukei::intersect::face_box(&p, f);
            (bx.min().z - 0.1).abs() < 1e-9 && (bx.max().z - 0.1).abs() < 1e-9
        })
        .unwrap();
    let f = feature_faces(&p, name(&p, floor)).unwrap();
    assert_eq!(f.len(), 5);
    let (o, _) = delete_faces(D, &p, &spec(f)).unwrap();
    near(volume(&o), 0.2, 1e-9);
}

#[test]
fn an_edges_fillet_and_chamfer_grow_back_to_the_box() {
    for kind in [
        BlendKind::Fillet { radius: 0.1 },
        BlendKind::Chamfer(Chamfer::Equal { distance: 0.1 }),
    ] {
        let b = edged(kind);
        let blend_face = b
            .faces()
            .find(|&f| {
                let n = name(&b, f);
                ![
                    FaceRole::PlusX,
                    FaceRole::MinusX,
                    FaceRole::PlusY,
                    FaceRole::MinusY,
                    FaceRole::PlusZ,
                    FaceRole::MinusZ,
                ]
                .iter()
                .any(|&r| face(r) == n)
            })
            .unwrap();
        let n = name(&b, blend_face);
        let (o, j) = delete_faces(D, &b, &spec(vec![n])).unwrap();
        near(volume(&o), 0.24, 1e-9);
        let sharp = edge_between(&o, face(FaceRole::PlusZ), face(FaceRole::MinusY));
        assert!(
            j.events().contains(&Event::Replaced {
                old: n,
                new: vec![sharp]
            }),
            "{:?}",
            j.events()
        );
        assert_eq!(
            delete_faces(
                D,
                &b,
                &DeleteSpec {
                    faces: vec![n],
                    heal: Heal::Cap
                }
            )
            .unwrap_err(),
            TweakError::CapNotOnFace { face: n }.into()
        );
    }
}

#[test]
fn a_rounded_boxs_every_blend_and_a_step_grow_back() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let all: Vec<EdgeBlend> = b
        .edges()
        .map(|e| EdgeBlend {
            edge: b.edge(e).unwrap().name,
            kind: BlendKind::Fillet { radius: 0.05 },
        })
        .collect();
    let (r, _) = blend(OpKey(0x13c7), &b, &all).unwrap();
    let (o, _) = delete_faces(
        D,
        &r,
        &DeleteSpec {
            faces: not_planes(&r),
            heal: Heal::Grow,
        },
    )
    .unwrap();
    assert_eq!(o.faces().count(), 6);
    near(volume(&o), 0.24, 1e-9);
    // An L: the block less its corner x > 0.6, z > 0.2; the riser and the
    // tread go, the top and the end grow to meet.
    let (corner, _) = primitive::block(OpKey(0x13c8), 0.5, 0.8, 0.3, &Rigid::IDENTITY).unwrap();
    let l = cut(
        &b,
        &corner,
        Vector3::new(0.6, -0.1, 0.2),
        BooleanOp::Subtract,
    );
    let flat = |x: bool, at: f64| {
        l.faces()
            .find(|&f| {
                let bx = ryukei::intersect::face_box(&l, f);
                let (a, c) = if x {
                    (bx.min().x, bx.max().x)
                } else {
                    (bx.min().z, bx.max().z)
                };
                (a - at).abs() < 1e-6 && (c - at).abs() < 1e-6
            })
            .map(|f| name(&l, f))
            .unwrap()
    };
    let (o, _) = delete_faces(D, &l, &spec(vec![flat(true, 0.6), flat(false, 0.2)])).unwrap();
    near(volume(&o), 0.24, 1e-9);
}

#[test]
fn refusals_are_typed() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let side = face(FaceRole::PlusX);
    assert_eq!(
        delete_faces(D, &b, &spec(vec![side])).unwrap_err(),
        TweakError::GapNotHealed { face: side }.into()
    );
    let all: Vec<Name> = b.faces().map(|f| name(&b, f)).collect();
    assert!(matches!(
        delete_faces(D, &b, &spec(all)),
        Err(Error::InvalidTweak(TweakError::GapNotHealed { .. }))
    ));
    assert_eq!(
        delete_faces(D, &b, &spec(vec![])).unwrap_err(),
        TweakError::NoFaces.into()
    );
    let none = Name::from_u128(5);
    assert_eq!(
        delete_faces(D, &b, &spec(vec![none])).unwrap_err(),
        TweakError::NoSuchFace { face: none }.into()
    );
    assert_eq!(
        delete_faces(D, &b, &spec(vec![side, side])).unwrap_err(),
        TweakError::DuplicateFace { face: side }.into()
    );
    assert_eq!(
        feature_faces(&b, side).unwrap_err(),
        TweakError::NoFeature { face: side }.into()
    );
    let h = cut(
        &plate(),
        &pin(0.1, 0.6),
        Vector3::new(0.5, 0.5, -0.2),
        BooleanOp::Subtract,
    );
    let wall = not_planes(&h);
    assert_eq!(
        delete_faces(
            D,
            &h,
            &DeleteSpec {
                faces: wall.clone(),
                heal: Heal::Grow
            }
        )
        .unwrap_err(),
        TweakError::GrowWouldCap { face: wall[0] }.into()
    );
}

#[test]
fn names_and_threads() {
    let h = cut(
        &plate(),
        &pin(0.1, 0.6),
        Vector3::new(0.5, 0.5, -0.2),
        BooleanOp::Subtract,
    );
    let wall = not_planes(&h);
    let (o, j) = delete_faces(D, &h, &spec(wall.clone())).unwrap();
    for r in [FaceRole::PlusZ, FaceRole::MinusZ] {
        assert!(o.by_name(face(r)).is_some());
        assert!(j.events().contains(&Event::Modified(face(r))));
    }
    assert!(j.events().contains(&Event::Deleted(wall[0])));
    let b = edged(BlendKind::Fillet { radius: 0.1 });
    let n = not_planes(&b);
    let run = |t: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build()
            .unwrap()
            .install(|| delete_faces(D, &b, &spec(n.clone())).unwrap().0.dump())
    };
    assert_eq!(run(1), run(8));
}

/// Two fillets far apart, the top's +x edge's and the bottom's -x edge's:
/// deleted together they give the bits deleting one, then the other, does
/// (NIST CTC 01's three fillets, each grown alone in 0.7 s, together
/// passed the walk's 4,096 choices).
#[test]
fn far_apart_sets_grow_as_one_after_another() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let edges = [
        edge_between(&b, face(FaceRole::PlusZ), face(FaceRole::PlusX)),
        edge_between(&b, face(FaceRole::MinusZ), face(FaceRole::MinusX)),
    ]
    .map(|edge| EdgeBlend {
        edge,
        kind: BlendKind::Fillet { radius: 0.05 },
    });
    let (b, _) = blend(OpKey(1), &b, &edges).unwrap();
    let mut cyl = not_planes(&b);
    cyl.sort();
    assert_eq!(cyl.len(), 2);
    let grow = |faces: Vec<Name>| DeleteSpec {
        faces,
        heal: Heal::Grow,
    };
    let (both, _) = delete_faces(D, &b, &grow(cyl.clone())).unwrap();
    let (first, _) = delete_faces(D, &b, &grow(vec![cyl[0]])).unwrap();
    let (then, _) = delete_faces(D, &first, &grow(vec![cyl[1]])).unwrap();
    assert_eq!(both.dump(), then.dump());
    assert!((volume(&both) - 0.24).abs() < 1e-6);
}

/// A drilled plate's side deleted: nothing grows over the gap, as on a
/// plain block, so a typed refusal. Its hole's plug is no answer (#692:
/// with the whole plate in the region, the walk was seeded by the hole's
/// wall alone and closed its plug).
#[test]
fn deleting_a_drilled_plates_side_is_refused() {
    let drilled = cut(
        &plate(),
        &pin(0.1, 0.4),
        Vector3::new(0.5, 0.5, -0.1),
        BooleanOp::Subtract,
    );
    let r = delete_faces(D, &drilled, &spec(vec![face(FaceRole::PlusX)]));
    assert!(
        matches!(
            r,
            Err(Error::InvalidTweak(
                TweakError::GapNotHealed { .. } | TweakError::HealAmbiguous { .. }
            ))
        ),
        "{:?}",
        r.map(|(b, _)| b.counts().faces)
    );
}
