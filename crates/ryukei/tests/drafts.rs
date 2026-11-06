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
