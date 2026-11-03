//! Thickened and offset sheets (design 295, plan 12.E): sheets of planes,
//! cylinders and spheres, an L of two box faces and a closed sphere
//! thickened against closed forms of the parallel set; sheets offset;
//! refusals; names that survive a new distance.

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Cylinder, Frame, Plane, Point3, Rigid, Sphere, Surface, Vector3};
use ryukei::measure::{Entity, Properties, Value, properties};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::offset::{OffsetSpec, ThickenSpec, offset_body, thicken};
use ryukei::primitive;
use ryukei::sheet::{extract, from_surface, remove};
use ryukei::topo::{Body, BodyKind};
use ryukei::{Error, OffsetCase, OffsetError};

const K: OpKey = OpKey(7);

/// Within a certified value's bound (and rounding).
fn near(v: Value, expected: f64) -> bool {
    (v.value() - expected).abs() <= v.bound() + 1e-9 * expected.abs().max(1.0)
}

fn volume(b: &Body) -> Value {
    match properties(b, Entity::Body).unwrap() {
        Properties::Body { volume, .. } => volume,
        p => panic!("not a solid: {p:?}"),
    }
}

fn area(b: &Body) -> Value {
    match properties(b, Entity::Body).unwrap() {
        Properties::Sheet { area } => area,
        p => panic!("not a sheet: {p:?}"),
    }
}

/// Sound: checks clean, crosses nothing, meshes.
fn sound(b: &Body) {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-3, 0.5)).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(b, &m), Ok(()));
}

fn by(front: f64, back: f64) -> ThickenSpec {
    ThickenSpec {
        front,
        back,
        faces: Vec::new(),
    }
}

fn square() -> Body {
    from_surface(
        OpKey(1),
        &Surface::Plane(Plane::new(Frame::WORLD)),
        [[0.0, 1.0], [0.0, 1.0]],
    )
    .unwrap()
    .0
}

fn thick(sheet: &Body, front: f64, back: f64) -> Body {
    let (b, _) = thicken(K, sheet, &by(front, back)).unwrap();
    assert_eq!(b.kind(), BodyKind::Solid);
    sound(&b);
    b
}

#[test]
fn a_square_thickens_to_a_slab() {
    let s = square();
    for (f, b) in [(0.1, 0.0), (0.0, 0.1), (0.1, 0.05)] {
        let t = thick(&s, f, b);
        assert!(near(volume(&t), f + b), "{f} {b}: {:?}", volume(&t));
        assert_eq!(t.faces().count(), 6);
    }
}

#[test]
fn a_cylinders_quarter_thickens_to_its_parallel_set() {
    let (r, h) = (1.0, 0.5);
    let s = from_surface(
        OpKey(1),
        &Surface::Cylinder(Cylinder::new(Frame::WORLD, r).unwrap()),
        [[0.0, FRAC_PI_2], [0.0, h]],
    )
    .unwrap()
    .0;
    let (f, b) = (0.1, 0.2);
    let t = thick(&s, f, b);
    let exact = FRAC_PI_2 * h * ((r + f) * (r + f) - (r - b) * (r - b)) / 2.0;
    assert!(near(volume(&t), exact), "{:?} {exact}", volume(&t));
}

#[test]
fn a_hemisphere_thickens_to_half_a_shell() {
    let r = 1.0;
    let s = from_surface(
        OpKey(1),
        &Surface::Sphere(Sphere::new(Frame::WORLD, r).unwrap()),
        [[0.0, TAU], [0.0, FRAC_PI_2]],
    )
    .unwrap()
    .0;
    let (f, b) = (0.1, 0.1);
    let t = thick(&s, f, b);
    let exact = TAU / 3.0 * ((r + f).powi(3) - (r - b).powi(3));
    assert!(near(volume(&t), exact), "{:?} {exact}", volume(&t));
}

#[test]
fn a_patch_of_a_sphere_thickens_with_cone_walls() {
    let r = 1.0;
    let s = from_surface(
        OpKey(1),
        &Surface::Sphere(Sphere::new(Frame::WORLD, r).unwrap()),
        [[0.0, 1.0], [0.2, 0.9]],
    )
    .unwrap()
    .0;
    let (f, b) = (0.1, 0.05);
    let t = thick(&s, f, b);
    let lat = ryukei::math::sin(0.9) - ryukei::math::sin(0.2);
    let exact = 1.0 * lat * ((r + f).powi(3) - (r - b).powi(3)) / 3.0;
    assert!(near(volume(&t), exact), "{:?} {exact}", volume(&t));
}

/// The top and front faces of an `a × b × c` box, as one sheet bent along
/// their shared edge.
fn ell(a: f64, b: f64, c: f64) -> Body {
    let (block, _) = primitive::block(OpKey(2), a, b, c, &Rigid::IDENTITY).unwrap();
    let face = |r| Name::derive(OpKey(2), Role::Face(r), &[]);
    extract(
        OpKey(3),
        &block,
        &[face(FaceRole::PlusZ), face(FaceRole::MinusY)],
    )
    .unwrap()
    .0
}

#[test]
fn an_l_thickens_with_a_rounded_outside_and_a_square_inside() {
    let (a, b, c) = (1.0, 0.6, 0.4);
    let s = ell(a, b, c);
    let (t, u) = (0.05, 0.03);
    let solid = thick(&s, t, u);
    let exact = a * (b + c) * (t + u) + a * PI * t * t / 4.0 - a * u * u;
    assert!(near(volume(&solid), exact), "{:?} {exact}", volume(&solid));
}

/// A box `a × b × c` without its top.
fn open_box(a: f64, b: f64, c: f64) -> Body {
    let (block, _) = primitive::block(OpKey(2), a, b, c, &Rigid::IDENTITY).unwrap();
    let top = Name::derive(OpKey(2), Role::Face(FaceRole::PlusZ), &[]);
    remove(OpKey(3), &block, &[top]).unwrap().0
}

#[test]
fn an_open_box_thickens_inwards_to_a_tray_and_offsets_outwards() {
    let (a, b, c, t) = (1.0, 0.6, 0.4, 0.02);
    let s = open_box(a, b, c);
    let tray = thick(&s, 0.0, t);
    let exact = a * b * c - (a - 2.0 * t) * (b - 2.0 * t) * (c - t);
    assert!(near(volume(&tray), exact), "{:?} {exact}", volume(&tray));
    let (o, _) = offset_body(
        K,
        &s,
        &OffsetSpec {
            distance: t,
            faces: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(o.kind(), BodyKind::Sheet);
    sound(&o);
    // Steiner's formula for the open box's skin: its faces, a quarter
    // cylinder along each of its eight inner edges, an eighth of a ball
    // at each of its four bottom corners.
    let edges = 2.0 * (a + b) + 4.0 * c;
    let exact = a * b + 2.0 * (a + b) * c + edges * FRAC_PI_2 * t + 4.0 * FRAC_PI_2 * t * t;
    assert!(near(area(&o), exact), "{:?} {exact}", area(&o));
}

#[test]
fn a_closed_sphere_thickens_to_a_shell_with_a_cavity() {
    let r = 1.0;
    let (ball, _) = primitive::sphere(OpKey(2), r, &Rigid::IDENTITY).unwrap();
    let (s, _) = remove(OpKey(3), &ball, &[]).unwrap();
    assert_eq!(s.kind(), BodyKind::Sheet);
    for (f, b) in [(0.1, 0.1), (0.2, 0.0), (0.0, 0.2)] {
        let t = thick(&s, f, b);
        assert_eq!(t.shells().count(), 2);
        let exact = 4.0 * PI / 3.0 * ((r + f).powi(3) - (r - b).powi(3));
        assert!(near(volume(&t), exact), "{f} {b}: {:?} {exact}", volume(&t));
    }
}

#[test]
fn front_and_back_add_up() {
    let s = ell(1.0, 0.6, 0.4);
    let both = volume(&thick(&s, 0.05, 0.03)).value();
    let front = volume(&thick(&s, 0.05, 0.0)).value();
    let back = volume(&thick(&s, 0.0, 0.03)).value();
    assert!((both - front - back).abs() < 1e-6, "{both} {front} {back}");
}

#[test]
fn refusals() {
    let s = square();
    for (f, b) in [(0.0, 0.0), (-0.1, 0.0), (f64::NAN, 0.1), (1e-9, 0.0)] {
        assert_eq!(
            thicken(K, &s, &by(f, b)).unwrap_err(),
            Error::InvalidOffset(OffsetError::BadDistance),
            "{f} {b}"
        );
    }
    let (block, _) = primitive::block(OpKey(2), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
    assert_eq!(
        thicken(K, &block, &by(0.1, 0.0)).unwrap_err(),
        Error::NotASheet { op: "thicken" }
    );
    // Two sides of a square prism extruded askew: across their shared
    // edge the laminar edges' walls lie on two planes.
    let err = thicken(K, &oblique_pair(), &by(0.05, 0.0)).unwrap_err();
    assert!(
        matches!(
            err,
            Error::InvalidOffset(OffsetError::Unsupported {
                case: OffsetCase::LaminarVertex,
                ..
            })
        ),
        "{err:?}"
    );
}

fn square_profile() -> ryukei::sweep::ValidProfile {
    use ryukei::geom::{Curve2, Line2, Point2};
    use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};
    let pts = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let segments = (0..4)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % 4]);
            let (p, q) = (Point2::new(p.0, p.1), Point2::new(q.0, q.1));
            Segment {
                key: SegmentKey(10 + i as u128),
                curve: Curve2::Line2(Line2::through(p, q).unwrap()),
                t0: 0.0,
                t1: p.distance(q),
            }
        })
        .collect();
    ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments,
        }],
    })
    .unwrap()
}

/// Two neighbouring sides of a square prism extruded askew: the sheet's
/// laminar edges at their foot don't lie square to the edge they share.
fn oblique_pair() -> Body {
    let (prism, _) = ryukei::sweep::extrude(
        OpKey(4),
        &square_profile(),
        Vector3::new(0.5, 0.0, 1.0),
        0.0,
        1.0,
    )
    .unwrap();
    // The two sides meeting along the lateral edge at (1, 0): those
    // whose planes hold that point and aren't the caps.
    let corner = Point3::new(1.0, 0.0, 0.0);
    let sides: Vec<Name> = prism
        .faces()
        .filter(|&f| {
            let s = prism.surface(prism.face(f).unwrap().surface).unwrap();
            let Surface::Plane(p) = s else { return false };
            let z = p.frame().z().get();
            z.z.abs() < 0.5 && (corner - p.frame().origin()).dot(z).abs() < 1e-12
        })
        .map(|f| prism.face(f).unwrap().name)
        .collect();
    assert_eq!(sides.len(), 2);
    extract(OpKey(5), &prism, &sides).unwrap().0
}

#[test]
fn a_sheet_offsets_to_a_sheet() {
    let s = square();
    let (o, _) = offset_body(
        K,
        &s,
        &OffsetSpec {
            distance: 0.1,
            faces: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(o.kind(), BodyKind::Sheet);
    sound(&o);
    assert!(near(area(&o), 1.0));
    // An L grown outwards gains a quarter cylinder along its bend.
    let (a, b, c, d) = (1.0, 0.6, 0.4, 0.05);
    let (o, _) = offset_body(
        K,
        &ell(a, b, c),
        &OffsetSpec {
            distance: d,
            faces: Vec::new(),
        },
    )
    .unwrap();
    sound(&o);
    let exact = a * (b + c) + a * FRAC_PI_2 * d;
    assert!(near(area(&o), exact), "{:?} {exact}", area(&o));
}

#[test]
fn a_new_thickness_renames_nothing() {
    let s = ell(1.0, 0.6, 0.4);
    let names = |b: &Body| {
        let mut n: Vec<Name> = b.faces().map(|f| b.face(f).unwrap().name).collect();
        n.extend(b.edges().map(|e| b.edge(e).unwrap().name));
        n.extend(b.vertices().map(|v| b.vertex(v).unwrap().name));
        n.sort();
        n
    };
    let a = names(&thick(&s, 0.05, 0.03));
    let b = names(&thick(&s, 0.08, 0.02));
    assert_eq!(a, b);
}
