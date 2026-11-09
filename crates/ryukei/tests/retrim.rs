//! Tweaks with `Topology::Change` (ryukei 13.A): faces grow over their
//! neighbours, shrink away and split; every result checks clean, meshes,
//! and holds its closed-form volume; refusals are typed.

use ryukei::boolean::{BooleanOp, Tool, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Point3, Rigid, Surface, UnitVector3, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Event, FaceRole, Name, OpKey, Role, ToolKey};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::tweak::{
    DraftFace, DraftSpec, Hinge, MoveSpec, OffsetFacesSpec, Topology, draft, move_faces,
    offset_faces,
};

const K: OpKey = OpKey(0x13b0);
const T: OpKey = OpKey(0x13b1);

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

fn block() -> Body {
    primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0
}

/// The −x wall drafted 1.3 rad about the bottom: it meets the +x wall at
/// `1 / tan 1.3` high, so the top goes and the box is a wedge.
#[test]
fn a_wall_drafted_past_its_neighbour_is_a_wedge() {
    let b = block();
    let spec = DraftSpec {
        pull: UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap(),
        angle: 1.3,
        hinge: Hinge::Neutral {
            origin: Point3::ORIGIN,
        },
        faces: vec![DraftFace {
            face: face(K, FaceRole::MinusX),
            angle: None,
            flip: false,
        }],
        tangent: false,
        topology: Topology::Change,
    };
    let (o, j, _) = draft(T, &b, &spec).unwrap();
    let h = 1.0 / ryukei::math::tan(1.3);
    near(volume(&o, 1e-4), 0.6 * 0.5 * h, 1e-6);
    assert!(o.by_name(face(K, FaceRole::PlusZ)).is_none());
    assert!(
        j.events()
            .contains(&Event::Deleted(face(K, FaceRole::PlusZ)))
    );
    assert_eq!(o.faces().count(), 5);
}

/// A plate 1 × 1 × 0.2 with a groove 0.1 wide and 0.1 deep along y from
/// 0.2 to 0.8; both its end walls offset 0.3 into the material run it
/// through: the top splits in two, both y sides gain a notch, the end
/// walls go.
fn grooved() -> Body {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (cut, _) = primitive::block(OpKey(0x13b2), 0.1, 0.6, 0.2, &Rigid::IDENTITY).unwrap();
    boolean(
        OpKey(0x13b3),
        BooleanOp::Subtract,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &cut,
            place: Rigid::translation(Vector3::new(0.45, 0.2, 0.1)),
        }],
    )
    .unwrap()
    .0
}

#[test]
fn a_groove_run_through_splits_the_top() {
    let b = grooved();
    near(volume(&b, 1e-4), 0.2 - 0.1 * 0.6 * 0.1, 1e-9);
    // The groove's end walls: planes square to y inside the plate.
    let ends: Vec<Name> = b
        .faces()
        .filter(|&f| match b.surface(b.face(f).unwrap().surface).unwrap() {
            Surface::Plane(p) => {
                let y = p.frame().origin().y;
                p.frame().z().get().y.abs() > 0.9 && (y - 0.2).abs() < 1e-12
                    || (y - 0.8).abs() < 1e-12
            }
            _ => false,
        })
        .map(|f| b.face(f).unwrap().name)
        .collect();
    assert_eq!(ends.len(), 2);
    let spec = OffsetFacesSpec {
        faces: ends.iter().map(|&n| (n, -0.3)).collect(),
        tangent: false,
        topology: Topology::Change,
    };
    let (o, j) = offset_faces(T, &b, &spec).unwrap();
    near(volume(&o, 1e-4), 0.2 - 0.1 * 1.0 * 0.1, 1e-9);
    for n in &ends {
        assert!(o.by_name(*n).is_none());
    }
    let top = face(K, FaceRole::PlusZ);
    assert!(o.by_name(top).is_none());
    assert!(j.events().iter().any(
        |e| matches!(e, Event::Split { parent, children } if *parent == top && children.len() == 2)
    ));
}

/// A square pyramid (two prisms' intersection), one slanted face offset
/// out: its apex splits. The volume is the same intersection with that
/// prism's face moved, built by booleans.
#[test]
fn a_pyramids_apex_splits() {
    let (b, slanted) = pyramid(0.0);
    let spec = OffsetFacesSpec {
        faces: vec![(slanted, 0.02)],
        tangent: false,
        topology: Topology::Change,
    };
    let (o, _) = offset_faces(T, &b, &spec).unwrap();
    let (want, _) = pyramid(0.02);
    near(volume(&o, 1e-4), volume(&want, 1e-4), 1e-6);
    assert_eq!(o.vertices().count(), 6);
}

/// A pyramid as `tests/tweaks.rs` builds it, with the first prism's
/// slanted face moved `d` along its outward normal, and that face's name
/// (when `d` is 0).
fn pyramid(d: f64) -> (Body, Name) {
    use ryukei::geom::{Curve2, Frame, Line2, Point2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
    // The side from (1, 0) to (0.5, 0.5), on u + v = 1, faces (1, 1) / sqrt 2:
    // moved `d` out it is u + v = 1 + sqrt 2 d, meeting the base v = 0 and
    // the other side u = v.
    let s = d / core::f64::consts::SQRT_2;
    let tri = |frame: Frame, base: u128, moved: bool| {
        let m = if moved { s } else { 0.0 };
        let pts = [(0.0, 0.0), (1.0 + 2.0 * m, 0.0), (0.5 + m, 0.5 + m)];
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
    // (extruded along y), as `tests/tweaks.rs` builds them.
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
        &tri(yz, 20, d != 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        0.0,
        1.0,
    )
    .unwrap()
    .0;
    // Moved, the slanted face reaches past y = 1, where the xz prism's cap
    // would cut it off: that prism runs on further.
    let (y0, y1) = if d == 0.0 { (-1.0, 0.0) } else { (-1.5, 0.5) };
    let c = extrude(
        OpKey(0xd8),
        &tri(xz, 30, false),
        Vector3::new(0.0, -1.0, 0.0),
        y0,
        y1,
    )
    .unwrap()
    .0;
    let b = boolean(
        OpKey(0xd9),
        BooleanOp::Intersect,
        &a,
        &[Tool {
            key: ToolKey(2),
            body: &c,
            place: Rigid::IDENTITY,
        }],
    )
    .unwrap()
    .0;
    // The yz prism's slanted face: outward normal up and towards +y.
    let slanted = b
        .faces()
        .find(|&f| {
            let face = b.face(f).unwrap();
            match b.surface(face.surface).unwrap() {
                Surface::Plane(p) => {
                    let n = p.frame().z().get();
                    let n = if face.sense == ryukei::topo::Sense::Forward {
                        n
                    } else {
                        -n
                    };
                    n.y > 0.1 && n.z > 0.1 && n.x.abs() < 1e-9
                }
                _ => false,
            }
        })
        .map(|f| b.face(f).unwrap().name)
        .expect("a slanted face");
    (b, slanted)
}

/// A boss slid half off its plate's edge: its overhang's floor is the
/// plate's top plane facing down (a reversed piece, Task 17), and the
/// body is the plate united with the moved boss.
#[test]
fn a_boss_slid_over_its_plates_edge() {
    let (plate, _) = primitive::block(K, 1.0, 1.0, 0.2, &Rigid::IDENTITY).unwrap();
    let (boss, _) = primitive::cylinder(OpKey(0x13b4), 0.1, 0.15, &Rigid::IDENTITY).unwrap();
    let (b, _) = boolean(
        OpKey(0x13b5),
        BooleanOp::Unite,
        &plate,
        &[Tool {
            key: ToolKey(1),
            body: &boss,
            place: Rigid::translation(Vector3::new(0.5, 0.5, 0.2)),
        }],
    )
    .unwrap();
    let boss_faces: Vec<Name> = b
        .faces()
        .filter(|&f| match b.surface(b.face(f).unwrap().surface).unwrap() {
            Surface::Cylinder(_) => true,
            Surface::Plane(p) => (p.frame().origin().z - 0.35).abs() < 1e-12,
            _ => false,
        })
        .map(|f| b.face(f).unwrap().name)
        .collect();
    let spec = MoveSpec {
        faces: boss_faces,
        motion: Rigid::translation(Vector3::new(-0.45, 0.0, 0.0)),
        tangent: false,
        topology: Topology::Change,
    };
    let (o, _) = move_faces(T, &b, &spec).unwrap();
    // The boss whole, now centred at x = 0.05: 0.05 of it overhangs.
    near(
        volume(&o, 1e-4),
        0.2 + core::f64::consts::PI * 0.01 * 0.15,
        1e-4,
    );
    // The plate's −x side stays whole (the boss meets it along its top
    // edge only), and that edge is cut where the boss's foot crosses it.
    assert!(o.by_name(face(K, FaceRole::MinusX)).is_some());
    assert!(o.edges().count() > b.edges().count());
}
