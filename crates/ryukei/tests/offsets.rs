//! Offset bodies against exact volumes: every result passes the checker
//! and the self-intersection check, meshes without faults, and encloses
//! the volume Steiner's formula (or the shape's own) gives; refusals by
//! name; names that survive a change of distance.

use core::f64::consts::PI;
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Line2, Point2, Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Event, FaceRole, Name, OpKey, Role};
use ryukei::offset::{OffsetSpec, offset_body};
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
use ryukei::topo::Body;
use ryukei::{Error, OffsetCase, OffsetError};

const K: OpKey = OpKey(0xc0);
const OFF: OpKey = OpKey(0xc1);

/// The volume a sound body's mesh encloses, after both checks.
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

fn by(d: f64) -> OffsetSpec {
    OffsetSpec {
        distance: d,
        faces: Vec::new(),
    }
}

fn face(key: OpKey, role: FaceRole) -> Name {
    Name::derive(key, Role::Face(role), &[])
}

fn refusal(r: Result<(Body, ryukei::name::Journal), Error>) -> OffsetError {
    match r {
        Err(Error::InvalidOffset(e)) => e,
        other => panic!("not an offset refusal: {:?}", other.map(|_| ())),
    }
}

/// Steiner's formula for a box `a × b × c` grown by `d`.
fn steiner_box(a: f64, b: f64, c: f64, d: f64) -> f64 {
    a * b * c
        + 2.0 * (a * b + b * c + c * a) * d
        + PI * (a + b + c) * d * d
        + 4.0 / 3.0 * PI * d * d * d
}

#[test]
fn a_box_grows_by_steiners_formula() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    for d in [0.05, 0.2] {
        let (o, _) = offset_body(OFF, &b, &by(d)).unwrap();
        // 6 faces, 12 cylinders, 8 spheres.
        assert_eq!(o.faces().count(), 26);
        near(volume(&o, 2e-4), steiner_box(1.0, 0.6, 0.4, d), 2e-3);
    }
}

#[test]
fn a_box_shrinks_to_a_box() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let t = 0.1;
    let (o, _) = offset_body(OFF, &b, &by(-t)).unwrap();
    assert_eq!(o.counts().faces, 6);
    assert_eq!(o.counts().edges, 12);
    near(volume(&o, 1e-3), 0.8 * 0.4 * 0.2, 1e-9);
}

/// A box grown on every face but its top: the ball's region below the
/// top, its fills ending on the top's plane.
#[test]
fn a_box_grown_but_its_top() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let d = 0.1;
    let spec = OffsetSpec {
        distance: d,
        faces: vec![(face(K, FaceRole::PlusZ), 0.0)],
    };
    let (o, _) = offset_body(OFF, &b, &spec).unwrap();
    let (a, bb) = (1.0, 0.6);
    let above = a * bb * d + PI * d * d / 4.0 * 2.0 * (a + bb) + 2.0 / 3.0 * PI * d * d * d;
    near(
        volume(&o, 2e-4),
        steiner_box(1.0, 0.6, 0.4, d) - above,
        2e-3,
    );
}

/// A cylinder grown: its rims are quarter tori, its edges closed at their
/// vertices so the tori wrap round.
#[test]
fn a_cylinder_grows_and_shrinks() {
    let (r, h) = (0.3, 0.5);
    let (b, _) = primitive::cylinder(K, r, h, &Rigid::IDENTITY).unwrap();
    let d = 0.05;
    let (o, _) = offset_body(OFF, &b, &by(d)).unwrap();
    let want = PI * (r + d) * (r + d) * h
        + 2.0 * PI * r * r * d
        + PI * PI * r * d * d
        + 4.0 / 3.0 * PI * d * d * d;
    near(volume(&o, 2e-4), want, 2e-3);
    let (o, _) = offset_body(OFF, &b, &by(-d)).unwrap();
    near(
        volume(&o, 2e-4),
        PI * (r - d) * (r - d) * (h - 2.0 * d),
        2e-3,
    );
}

/// An L-shaped prism: `[0, 2] × [0, 2]` less `[1, 2] × [1, 2]`, `h` high.
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
                curve: ryukei::geom::Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    let profile = ValidProfile::new(Profile {
        frame: ryukei::geom::Frame::WORLD,
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

/// Shrunk, the L's reflex edge opens: a quarter cylinder ending on the
/// top and bottom.
#[test]
fn an_l_shrinks_round_its_inner_corner() {
    let h = 0.6;
    let b = l_block(h);
    let t = 0.1;
    let (o, _) = offset_body(OFF, &b, &by(-t)).unwrap();
    let area = (2.0 - 2.0 * t) * (1.0 - 2.0 * t) + (1.0 - 2.0 * t) + t * t * (1.0 - PI / 4.0);
    near(volume(&o, 2e-4), area * (h - 2.0 * t), 2e-3);
}

/// Grown, the L's fills at its inner corner trim each other; the result
/// encloses every point within the distance of the L.
#[test]
fn an_l_grows_round_its_inner_corner() {
    let h = 0.6;
    let b = l_block(h);
    let d = 0.1;
    let (o, _) = offset_body(OFF, &b, &by(d)).unwrap();
    // Points within `d` of the prism `P × [0, h]`: at height `z` past its
    // ends by `s`, those within `√(d² - s²)` of `P` in the plane, whose
    // area is `|P| + perimeter r + (5π/4 - 1) r²` (five right-angled
    // convex corners' sectors, less the square the strips of the reflex
    // corner's edges share).
    let area = |r2: f64, r: f64| 3.0 + 8.0 * r + (5.0 * PI / 4.0 - 1.0) * r2;
    let want = h * area(d * d, d)
        + 2.0 * (3.0 * d + 8.0 * PI * d * d / 4.0 + (5.0 * PI / 4.0 - 1.0) * 2.0 * d * d * d / 3.0);
    near(volume(&o, 2e-4), want, 2e-3);
}

#[test]
fn refusals() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    assert_eq!(
        refusal(offset_body(OFF, &b, &by(f64::NAN))),
        OffsetError::BadDistance
    );
    assert_eq!(
        refusal(offset_body(OFF, &b, &by(0.0))),
        OffsetError::BadDistance
    );
    let ghost = Name::derive(OpKey(9), Role::Face(FaceRole::PlusZ), &[]);
    let spec = OffsetSpec {
        distance: 0.1,
        faces: vec![(ghost, 0.0)],
    };
    assert_eq!(
        refusal(offset_body(OFF, &b, &spec)),
        OffsetError::NoSuchFace { face: ghost }
    );
    // A cylinder shrunk past its axis.
    let (c, _) = primitive::cylinder(K, 0.3, 2.0, &Rigid::IDENTITY).unwrap();
    assert!(matches!(
        refusal(offset_body(OFF, &c, &by(-0.35))),
        OffsetError::FaceCollapses { .. }
    ));
    // A plate shrunk by more than half its thickness: its edges vanish.
    assert!(matches!(
        refusal(offset_body(OFF, &b, &by(-0.25))),
        OffsetError::EdgeVanishes { .. }
    ));
    let _ = OffsetCase::MixedVertex;
}

/// Faces keep their names, fills are named from what they replace, and a
/// new distance renames nothing.
#[test]
fn names_survive_a_new_distance() {
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let names = |o: &Body| {
        let mut n: Vec<Name> = o
            .faces()
            .map(|f| o.face(f).unwrap().name)
            .chain(o.edges().map(|e| o.edge(e).unwrap().name))
            .chain(o.vertices().map(|v| o.vertex(v).unwrap().name))
            .collect();
        n.sort();
        n
    };
    let (a, ja) = offset_body(OFF, &b, &by(0.05)).unwrap();
    let (c, _) = offset_body(OFF, &b, &by(0.2)).unwrap();
    assert_eq!(names(&a), names(&c));
    for f in b.faces() {
        let n = b.face(f).unwrap().name;
        assert!(a.by_name(n).is_some());
    }
    let replaced = ja
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Replaced { .. }))
        .count();
    // 12 edges and 8 vertices.
    assert_eq!(replaced, 20);
}

/// A primitive's edge between two faces.
fn edge(key: OpKey, a: FaceRole, b: FaceRole) -> Name {
    let mut f = [face(key, a), face(key, b)];
    f.sort();
    Name::derive(key, Role::Edge, &f)
}

const SIDES: [FaceRole; 6] = [
    FaceRole::PlusX,
    FaceRole::MinusX,
    FaceRole::PlusY,
    FaceRole::MinusY,
    FaceRole::PlusZ,
    FaceRole::MinusZ,
];

/// Every edge of a primitive box.
fn box_edges(key: OpKey) -> Vec<Name> {
    let mut out = Vec::new();
    for (i, &a) in SIDES.iter().enumerate() {
        for &b in &SIDES[i + 1..] {
            let opposite = matches!(
                (a, b),
                (FaceRole::PlusX, FaceRole::MinusX)
                    | (FaceRole::PlusY, FaceRole::MinusY)
                    | (FaceRole::PlusZ, FaceRole::MinusZ)
            );
            if !opposite {
                out.push(edge(key, a, b));
            }
        }
    }
    out
}

/// A box with every edge rounded to `r`: the core box grown by `r`.
fn rounded(a: f64, b: f64, c: f64, r: f64) -> Body {
    let (bx, _) = primitive::block(K, a, b, c, &Rigid::IDENTITY).unwrap();
    let blends: Vec<ryukei::blend::EdgeBlend> = box_edges(K)
        .into_iter()
        .map(|e| ryukei::blend::EdgeBlend {
            edge: e,
            kind: ryukei::blend::BlendKind::Fillet { radius: r },
        })
        .collect();
    ryukei::blend::blend(OpKey(0xc5), &bx, &blends).unwrap().0
}

/// A rounded box is its core grown by its radius: every edge smooth, it
/// offsets to the core grown by the radius plus the distance.
#[test]
fn a_rounded_box_offsets_smoothly() {
    let r = 0.1;
    let b = rounded(1.0, 0.6, 0.4, r);
    let core = (1.0 - 2.0 * r, 0.6 - 2.0 * r, 0.4 - 2.0 * r);
    for d in [0.05, -0.05] {
        let (o, j) = offset_body(OFF, &b, &by(d)).unwrap();
        assert_eq!(o.counts().faces, b.counts().faces);
        assert!(
            !j.events()
                .iter()
                .any(|e| matches!(e, Event::Replaced { .. }))
        );
        near(
            volume(&o, 2e-4),
            steiner_box(core.0, core.1, core.2, r + d),
            2e-3,
        );
    }
}

/// A box hollowed to a wall, open at its top.
#[test]
fn a_box_shelled_open_at_its_top() {
    use ryukei::offset::{ShellSpec, shell};
    let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
    let t = 0.05;
    let spec = ShellSpec {
        thickness: t,
        outward: false,
        open: vec![face(K, FaceRole::PlusZ)],
        faces: Vec::new(),
    };
    let (o, j) = shell(OpKey(0xc6), &b, &spec).unwrap();
    near(
        volume(&o, 1e-3),
        0.24 - (1.0 - 2.0 * t) * (0.6 - 2.0 * t) * (0.4 - t),
        1e-9,
    );
    // Every face keeps its name: the open face is the walls' rim now,
    // with the opening as a hole.
    for role in SIDES {
        assert!(o.by_name(face(K, role)).is_some(), "{role:?}");
    }
    let Some(ryukei::topo::EntityId::Face(top)) = o.by_name(face(K, FaceRole::PlusZ)) else {
        panic!("the top");
    };
    assert_eq!(o.face_loops(top).count(), 2);
    assert!(j.events().iter().any(|e| matches!(e, Event::Placed { .. })));
    // Outwards: the box grown below its top, less the box.
    let spec = ShellSpec {
        outward: true,
        ..spec
    };
    let (o, _) = shell(OpKey(0xc6), &b, &spec).unwrap();
    let (a, bb) = (1.0, 0.6);
    let above = a * bb * t + PI * t * t / 4.0 * 2.0 * (a + bb) + 2.0 / 3.0 * PI * t * t * t;
    near(
        volume(&o, 2e-4),
        steiner_box(1.0, 0.6, 0.4, t) - above - 0.24,
        2e-3,
    );
    // No open face: a cavity.
    let spec = ShellSpec {
        outward: false,
        open: Vec::new(),
        ..spec
    };
    let (o, _) = shell(OpKey(0xc6), &b, &spec).unwrap();
    assert_eq!(o.counts().shells, 2);
    near(
        volume(&o, 1e-3),
        0.24 - (1.0 - 2.0 * t) * (0.6 - 2.0 * t) * (0.4 - 2.0 * t),
        1e-9,
    );
}

/// A cylinder hollowed to a cup.
#[test]
fn a_cylinder_shelled_to_a_cup() {
    use ryukei::offset::{ShellSpec, shell};
    let (r, h, t) = (0.3, 0.5, 0.04);
    let (b, _) = primitive::cylinder(K, r, h, &Rigid::IDENTITY).unwrap();
    let spec = ShellSpec {
        thickness: t,
        outward: false,
        open: vec![face(K, FaceRole::Top)],
        faces: Vec::new(),
    };
    let (o, _) = shell(OpKey(0xc7), &b, &spec).unwrap();
    near(
        volume(&o, 2e-4),
        PI * r * r * h - PI * (r - t) * (r - t) * (h - t),
        2e-3,
    );
}

/// A box with its four upright edges rounded, shelled open at its top:
/// where the top meets the fillets, the wall's inner face ends on the top
/// along a circle.
#[test]
fn a_rounded_tray() {
    use ryukei::offset::{ShellSpec, shell};
    let (a, bb, c, r, t) = (1.0, 0.6, 0.4, 0.15, 0.05);
    let (bx, _) = primitive::block(K, a, bb, c, &Rigid::IDENTITY).unwrap();
    let upright = [
        (FaceRole::PlusX, FaceRole::PlusY),
        (FaceRole::PlusX, FaceRole::MinusY),
        (FaceRole::MinusX, FaceRole::PlusY),
        (FaceRole::MinusX, FaceRole::MinusY),
    ];
    let blends: Vec<ryukei::blend::EdgeBlend> = upright
        .iter()
        .map(|&(x, y)| ryukei::blend::EdgeBlend {
            edge: edge(K, x, y),
            kind: ryukei::blend::BlendKind::Fillet { radius: r },
        })
        .collect();
    let (b, _) = ryukei::blend::blend(OpKey(0xc8), &bx, &blends).unwrap();
    let spec = ShellSpec {
        thickness: t,
        outward: false,
        open: vec![face(K, FaceRole::PlusZ)],
        faces: Vec::new(),
    };
    let (o, _) = shell(OpKey(0xc9), &b, &spec).unwrap();
    let plan = |a: f64, b: f64, r: f64| a * b - (4.0 - PI) * r * r;
    let want = plan(a, bb, r) * c - plan(a - 2.0 * t, bb - 2.0 * t, r - t) * (c - t);
    near(volume(&o, 2e-4), want, 2e-3);
}
