//! Measurement against exact references: volumes, areas, centres and
//! inertias of primitives, booleans and blends that closed forms give;
//! lengths and areas of single entities; distances between placed
//! bodies against their closed forms and against sampling; and the same
//! bits on one thread and many.

use ryukei::blend::{BlendKind, EdgeBlend, blend};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::geom::{Point3, Rigid, Surface, Vector3};
use ryukei::measure::{
    DistanceOptions, EdgeShape, Entity, FaceShape, MassOptions, MassProperties, Properties, Value,
    distance, mass, properties,
};
use ryukei::mesh::{MeshParams, tessellate};
use ryukei::name::{FaceRole, Name, OpKey, Role};
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::Lcg;
use std::f64::consts::PI;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

fn weigh(b: &Body) -> MassProperties {
    mass(b, 1.0, &MassOptions::default()).unwrap()
}

/// The value holds the truth within its bound, and is within `rel` of it.
fn near(v: Value, want: f64, rel: f64) {
    assert!(
        v.holds(want, 1e-14 * want.abs().max(1.0)),
        "{v:?} doesn't hold {want}"
    );
    assert!(
        (v.value() - want).abs() <= rel * want.abs().max(1e-12),
        "{v:?} against {want}"
    );
}

fn run(op: BooleanOp, target: &Body, tools: &[(&Body, Rigid)]) -> Body {
    let tools: Vec<Tool> = tools
        .iter()
        .enumerate()
        .map(|(i, (b, place))| Tool {
            key: ToolKey(i as u128 + 1),
            body: b,
            place: *place,
        })
        .collect();
    boolean(OpKey(100), op, target, &tools).unwrap().0
}

#[test]
fn a_torus_and_a_cylinder_against_their_closed_forms() {
    let (big, small) = (1.0, 0.25);
    let t = primitive::torus(OpKey(1), big, small, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let m = weigh(&t);
    let v = 2.0 * PI * PI * big * small * small;
    near(m.volume, v, 1e-9);
    near(m.area, 4.0 * PI * PI * big * small, 1e-9);
    let izz = v * (big * big + 0.75 * small * small);
    let ixx = v * (big * big / 2.0 + 0.625 * small * small);
    assert!(
        (m.inertia[2][2] - izz).abs() <= 1e-8 * izz,
        "{:?}",
        m.inertia
    );
    assert!(
        (m.inertia[0][0] - ixx).abs() <= 1e-8 * ixx,
        "{:?}",
        m.inertia
    );
    assert!((m.inertia[2][2] - izz).abs() <= m.inertia_bound);
    let (r, h) = (0.3, 2.0);
    let c = primitive::cylinder(OpKey(2), r, h, &at(1.0, 2.0, 3.0))
        .unwrap()
        .0;
    let m = mass(&c, 7850.0, &MassOptions::default()).unwrap();
    let kg = 7850.0 * PI * r * r * h;
    near(m.mass, kg, 1e-9);
    assert!(m.centre.distance(Point3::new(1.0, 2.0, 4.0)) <= 1e-9);
    assert!(m.centre.distance(Point3::new(1.0, 2.0, 4.0)) <= m.centre_bound);
    let (iz, ix) = (kg * r * r / 2.0, kg * (3.0 * r * r + h * h) / 12.0);
    assert!((m.principal_moments[0] - iz).abs() <= 1e-8 * iz);
    assert!((m.principal_moments[2] - ix).abs() <= 1e-8 * ix);
    assert!(m.principal_axes[0].get().z.abs() > 1.0 - 1e-9);
}

#[test]
fn booleans_against_their_closed_forms() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let holed = run(BooleanOp::Subtract, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    let m = weigh(&holed);
    near(m.volume, 16.0 - PI * 0.25, 1e-9);
    near(m.area, 2.0 * (16.0 - PI * 0.25) + 16.0 + PI, 1e-9);
    assert!(m.centre.distance(Point3::new(2.0, 2.0, 0.5)) <= 1e-9);
    let s = primitive::sphere(OpKey(3), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let lens = run(BooleanOp::Intersect, &s, &[(&s, at(1.0, 0.0, 0.0))]);
    near(weigh(&lens).volume, 5.0 * PI / 12.0, 1e-8);
}

#[test]
fn a_filleted_edge_against_its_closed_form() {
    let k = OpKey(1);
    let b = primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let mut faces = [
        Name::derive(k, Role::Face(FaceRole::PlusX), &[]),
        Name::derive(k, Role::Face(FaceRole::PlusZ), &[]),
    ];
    faces.sort();
    let edge = Name::derive(k, Role::Edge, &faces);
    let r = 0.25;
    let (f, _) = blend(
        OpKey(0xb0),
        &b,
        &[EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius: r },
        }],
    )
    .unwrap();
    let corner = (1.0 - PI / 4.0) * r * r;
    near(weigh(&f).volume, 6.0 - corner * 2.0, 1e-9);
}

#[test]
fn entities_of_a_cylinder() {
    let (r, h) = (0.5, 2.0);
    let c = primitive::cylinder(OpKey(2), r, h, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for e in c.edges() {
        match properties(&c, Entity::Edge(e)).unwrap() {
            Properties::Edge { length, shape, .. } => {
                assert_eq!(length, Value::Exact(2.0 * PI * r));
                let EdgeShape::Arc { radius, angle, .. } = shape else {
                    panic!("{shape:?}");
                };
                assert_eq!((radius, angle), (r, 2.0 * PI));
            }
            p => panic!("{p:?}"),
        }
    }
    for f in c.faces() {
        let Properties::Face { area, shape } = properties(&c, Entity::Face(f)).unwrap() else {
            panic!();
        };
        match shape {
            FaceShape::Cylinder { radius, .. } => {
                assert_eq!(radius, r);
                near(area, 2.0 * PI * r * h, 1e-9);
            }
            FaceShape::Plane { .. } => near(area, PI * r * r, 1e-9),
            s => panic!("{s:?}"),
        }
    }
    assert!(matches!(
        properties(&c, Entity::Body).unwrap(),
        Properties::Body { .. }
    ));
}

#[test]
fn seeded_balls_against_their_gaps() {
    let mut g = Lcg::new(0x6d65_6173);
    for _ in 0..6 {
        let (ra, rb) = (g.next_f64(0.1, 0.5), g.next_f64(0.1, 0.5));
        let a = primitive::sphere(OpKey(1), ra, &Rigid::IDENTITY).unwrap().0;
        let b = primitive::sphere(OpKey(2), rb, &Rigid::IDENTITY).unwrap().0;
        let c = Vector3::new(
            g.next_f64(-2.0, 2.0),
            g.next_f64(-2.0, 2.0),
            g.next_f64(-2.0, 2.0),
        );
        let place = Rigid::translation(c);
        let d = distance(
            (&a, Entity::Body, &Rigid::IDENTITY),
            (&b, Entity::Body, &place),
            &DistanceOptions::default(),
        )
        .unwrap();
        let want = (c.length() - ra - rb).max(0.0);
        assert!(d.distance.holds(want, 1e-12), "{d:?} against {want}");
        assert!(
            (d.distance.value() - want).abs() <= 1e-9,
            "{d:?} against {want}"
        );
    }
}

/// Sampled points of two placed bodies are never closer than the lower
/// end of the distance's bound.
#[test]
fn a_box_and_a_torus_against_sampling() {
    let t = primitive::torus(OpKey(1), 0.6, 0.2, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let b = primitive::block(OpKey(2), 0.5, 0.5, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let place = Rigid::translation(Vector3::new(0.9, -0.25, 0.15)).then_after(&Rigid::rotation(
        Vector3::new(1.0, 1.0, 0.0).normalize().unwrap(),
        0.4,
    ));
    let d = distance(
        (&t, Entity::Body, &Rigid::IDENTITY),
        (&b, Entity::Body, &place),
        &DistanceOptions::default(),
    )
    .unwrap();
    assert!((d.points[0].distance(d.points[1]) - d.distance.value()).abs() < 1e-12);
    let mesh = |body: &Body| {
        tessellate(body, &MeshParams::new(2e-3, 0.2))
            .unwrap()
            .positions
    };
    let (pt, pb) = (mesh(&t), mesh(&b));
    let lo = d.distance.value() - d.distance.bound() - 1e-12;
    for p in &pt {
        for q in &pb {
            assert!(p.distance(place.apply_point(*q)) >= lo, "{d:?}");
        }
    }
}

#[test]
fn the_same_bits_on_one_thread_and_many() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let holed = run(BooleanOp::Subtract, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    let work = || {
        let m = weigh(&holed);
        let d = distance(
            (&holed, Entity::Body, &Rigid::IDENTITY),
            (&rod, Entity::Body, &at(2.0, 2.0, 1.5)),
            &DistanceOptions::default(),
        )
        .unwrap();
        (m, d)
    };
    let pool = |n| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
    };
    let (one, four) = (pool(1).install(work), pool(4).install(work));
    assert_eq!(one, four);
    // The rod stands half a metre above the plate's top.
    assert!(one.1.distance.holds(0.5, 1e-12), "{:?}", one.1);
}

#[test]
fn refusals() {
    use ryukei::{Error, MeasureError};
    let gone = Body::new();
    let not = Err(Error::InvalidMeasure(MeasureError::NotAnEntity));
    assert_eq!(mass(&gone, 1.0, &MassOptions::default()), not);
    let c = primitive::cylinder(OpKey(2), 0.5, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let s = primitive::sphere(OpKey(3), 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    // A sphere has one face and no edges: the cylinder's first edge ID
    // names nothing in it.
    let e = c.edges().next().unwrap();
    assert_eq!(
        properties(&s, Entity::Edge(e)).map(|_| ()),
        not.map(|_: MassProperties| ())
    );
    assert_eq!(
        distance(
            (&s, Entity::Edge(e), &Rigid::IDENTITY),
            (&c, Entity::Body, &Rigid::IDENTITY),
            &DistanceOptions::default()
        )
        .map(|_| ()),
        Err(Error::InvalidMeasure(MeasureError::NotAnEntity))
    );
}

/// A face is measured from a solid by its own points: a block's face far
/// from a small block holding one of its other vertices is 0.9 away, not
/// touching (review of #165).
#[test]
fn a_face_far_from_a_body_holding_another_vertex() {
    let (a, _) = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
    let v0 = a.vertices().next().unwrap();
    let p = a.vertex(v0).unwrap().point;
    let place = Rigid::translation(Vector3::new(p.x - 0.1, p.y - 0.1, p.z - 0.1));
    let (b, _) = primitive::block(OpKey(2), 0.2, 0.2, 0.2, &place).unwrap();
    let far = a
        .faces()
        .find(|&f| {
            let Surface::Plane(pl) = a.surface(a.face(f).unwrap().surface).unwrap() else {
                return false;
            };
            ((p - pl.frame().origin()).dot(pl.frame().z().get())).abs() > 0.5
        })
        .unwrap();
    let d = distance(
        (&a, Entity::Face(far), &Rigid::IDENTITY),
        (&b, Entity::Body, &Rigid::IDENTITY),
        &DistanceOptions::default(),
    )
    .unwrap();
    near(d.distance, 0.9, 1e-3);
}

/// Coincident faces with no boundary meet at a point of both: two balls
/// in one place, away from the origin.
#[test]
fn coincident_balls_meet_on_their_surface() {
    let (a, _) = primitive::sphere(OpKey(1), 0.5, &at(3.0, 0.0, 0.0)).unwrap();
    let (b, _) = primitive::sphere(OpKey(2), 0.5, &at(3.0, 0.0, 0.0)).unwrap();
    let fa = a.faces().next().unwrap();
    let fb = b.faces().next().unwrap();
    let d = distance(
        (&a, Entity::Face(fa), &Rigid::IDENTITY),
        (&b, Entity::Face(fb), &Rigid::IDENTITY),
        &DistanceOptions::default(),
    )
    .unwrap();
    assert_eq!(d.distance, Value::Exact(0.0), "{d:?}");
    let centre = Point3::new(3.0, 0.0, 0.0);
    for q in d.points {
        assert!(
            ((q - centre).length() - 0.5).abs() < 1e-9,
            "{q:?} is off the balls"
        );
    }
}

/// A lens whose larger cap holds both poles of its ball: at one pole the
/// coarse mesh's wedge lies across the seam, its corner off the pole two
/// corners at one position (seed 15 of `measure_props`). Against the
/// lens's closed form and both caps' areas.
#[test]
fn a_lens_whose_cap_holds_both_poles() {
    let (ra, rb) = (0.2983476435008765, 0.46682039834906247);
    let ca = [
        -0.1469880736387692,
        0.07777114788807216,
        -0.22934746127039213,
    ];
    let cb = [
        0.05676326597382425,
        0.016367300060060153,
        -0.1192567499405949,
    ];
    let (a, _) = primitive::sphere(OpKey(1), ra, &at(ca[0], ca[1], ca[2])).unwrap();
    let (b, _) = primitive::sphere(OpKey(2), rb, &at(cb[0], cb[1], cb[2])).unwrap();
    let sq = |x: f64| x * x;
    let lens = run(BooleanOp::Intersect, &a, &[(&b, Rigid::IDENTITY)]);
    let d = (sq(ca[0] - cb[0]) + sq(ca[1] - cb[1]) + sq(ca[2] - cb[2])).sqrt();
    // The plane of the rim, from a's centre towards b's.
    let x = (d * d + ra * ra - rb * rb) / (2.0 * d);
    let volume =
        PI * sq(ra + rb - d) * (d * d + 2.0 * d * (ra + rb) - 3.0 * sq(ra - rb)) / (12.0 * d);
    near(weigh(&lens).volume, volume, 1e-7);
    let mut areas: Vec<f64> = lens
        .faces()
        .map(|f| match properties(&lens, Entity::Face(f)).unwrap() {
            Properties::Face { area, .. } => area.value(),
            _ => unreachable!(),
        })
        .collect();
    areas.sort_by(f64::total_cmp);
    let caps = [2.0 * PI * rb * (rb - (d - x)), 2.0 * PI * ra * (ra - x)];
    for (got, want) in areas.iter().zip(caps) {
        assert!((got - want).abs() <= 1e-7 * want, "{got} against {want}");
    }
}
