//! Booleans on sheets (design 295, plan 12.D): sheets trimmed by solids
//! and solids and sheets divided by sheets, on curved and spline faces,
//! against closed forms; refusals; names that survive edits; one thread
//! and four alike. The plane cases are `boolean::sheet_tests`' unit
//! tests.

use core::f64::consts::{PI, TAU};
use ryukei::boolean::{BooleanOp, Divided, Tool, ToolKey, boolean, divide};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Cylinder, Frame, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::measure::{Entity, Properties, Value, properties};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::{Name, OpKey};
use ryukei::primitive;
use ryukei::sheet::{from_surface, remove};
use ryukei::topo::{Body, BodyKind};
use ryukei::{BooleanError, Error};

fn tool(key: u128, body: &Body) -> Tool<'_> {
    Tool {
        key: ToolKey(key),
        body,
        place: Rigid::IDENTITY,
    }
}

/// A plane sheet through `origin` facing `z`, its x along `x`, over a
/// box of its parameters.
fn plane(key: u128, origin: Point3, z: Vector3, x: Vector3, rect: [[f64; 2]; 2]) -> Body {
    let frame = Frame::new(origin, z, x).unwrap();
    from_surface(OpKey(key), &Surface::Plane(Plane::new(frame)), rect)
        .unwrap()
        .0
}

fn flat(key: u128, z: f64, half: f64) -> Body {
    plane(
        key,
        Point3::new(0.0, 0.0, z),
        Vector3::new(0.0, 0.0, 1.0),
        Vector3::new(1.0, 0.0, 0.0),
        [[-half, half], [-half, half]],
    )
}

/// Within a certified value's bound (and rounding).
fn near(v: Value, expected: f64) -> bool {
    (v.value() - expected).abs() <= v.bound() + 1e-9 * expected.abs().max(1.0)
}

fn area(b: &Body) -> Value {
    match properties(b, Entity::Body).unwrap() {
        Properties::Sheet { area } => area,
        p => panic!("not a sheet: {p:?}"),
    }
}

fn volume(b: &Body) -> Value {
    match properties(b, Entity::Body).unwrap() {
        Properties::Body { volume, .. } => volume,
        p => panic!("not a solid: {p:?}"),
    }
}

/// Sound: checks clean, crosses nothing, meshes.
fn sound(b: &Body) {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-2, 0.5)).unwrap();
    assert_eq!(m.faults, []);
    assert_eq!(check_mesh(b, &m), Ok(()));
}

#[test]
fn a_cylinders_side_is_trimmed_by_a_block() {
    let side = from_surface(
        OpKey(1),
        &Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap()),
        [[0.0, TAU], [0.0, 2.0]],
    )
    .unwrap()
    .0;
    // x > 0, 0.5 < z < 1.5: half the band a unit high.
    let at = Rigid::translation(Vector3::new(0.0, -2.0, 0.5));
    let b = primitive::block(OpKey(2), 2.0, 4.0, 1.0, &at).unwrap().0;
    let (inside, _) = boolean(OpKey(3), BooleanOp::Intersect, &side, &[tool(4, &b)]).unwrap();
    assert_eq!(inside.kind(), BodyKind::Sheet);
    sound(&inside);
    assert!(near(area(&inside), PI), "{:?}", area(&inside));
    let (rest, _) = boolean(OpKey(3), BooleanOp::Subtract, &side, &[tool(4, &b)]).unwrap();
    sound(&rest);
    assert!(near(area(&rest), 3.0 * PI), "{:?}", area(&rest));
}

#[test]
fn a_closed_sheet_is_trimmed_to_a_cap() {
    // A sphere's skin above z = 0.5: a cap of height 0.5.
    let ball = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let skin = remove(OpKey(2), &ball, &[]).unwrap().0;
    let at = Rigid::translation(Vector3::new(-2.0, -2.0, 0.5));
    let b = primitive::block(OpKey(3), 4.0, 4.0, 4.0, &at).unwrap().0;
    let (cap, _) = boolean(OpKey(4), BooleanOp::Intersect, &skin, &[tool(5, &b)]).unwrap();
    sound(&cap);
    assert_eq!(cap.edges().filter(|&e| cap.is_laminar(e)).count(), 1);
    assert!(near(area(&cap), TAU * 0.5), "{:?}", area(&cap));
}

#[test]
fn a_ball_is_divided_by_a_plane_sheet() {
    let ball = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let s = flat(2, 0.3, 2.0);
    let (d, _) = divide(OpKey(3), &ball, &[tool(4, &s)]).unwrap();
    assert_eq!((d.front.len(), d.back.len(), d.apart.len()), (1, 1, 0));
    let h = 0.7;
    let cap = PI * h * h * (3.0 - h) / 3.0;
    sound(&d.front[0]);
    sound(&d.back[0]);
    assert!(near(volume(&d.front[0]), cap), "{:?}", volume(&d.front[0]));
    assert!(
        near(volume(&d.back[0]), 4.0 / 3.0 * PI - cap),
        "{:?}",
        volume(&d.back[0])
    );
}

#[test]
fn a_box_is_divided_by_a_spline_sheet() {
    // The plane z = 0.75 as a NURBS: the same halves.
    let p = Plane::new(Frame::WORLD.transformed(&Rigid::translation(Vector3::new(0.0, 0.0, 0.75))));
    let (nurbs, _) = Surface::Plane(p)
        .to_nurbs([[-1.0, 3.0], [-1.0, 3.0]])
        .unwrap();
    let surface = Surface::Nurbs(nurbs.into());
    let s = from_surface(OpKey(1), &surface, surface.domain())
        .unwrap()
        .0;
    let b = primitive::block(OpKey(2), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let (d, _) = divide(OpKey(3), &b, &[tool(4, &s)]).unwrap();
    assert_eq!((d.front.len(), d.back.len()), (1, 1));
    // 2 × 2 × 1.25 above, 2 × 2 × 0.75 below.
    assert!(near(volume(&d.front[0]), 5.0), "{:?}", volume(&d.front[0]));
    assert!(near(volume(&d.back[0]), 3.0), "{:?}", volume(&d.back[0]));
}

#[test]
fn crossing_tools_are_refused() {
    // Two sheets crossing inside the box: their union has edges of four
    // faces there.
    let b = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let a = flat(2, 1.0, 3.0);
    let c = plane(
        3,
        Point3::new(1.0, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        [[-1.0, 3.0], [-1.0, 3.0]],
    );
    let r = divide(OpKey(4), &b, &[tool(5, &a), tool(6, &c)]);
    assert!(
        matches!(
            r,
            Err(Error::InvalidBoolean(BooleanError::NonManifold { .. }))
        ),
        "{r:?}"
    );
}

/// Every name in a division's bodies, front then back, sorted per body.
fn names(d: &Divided) -> Vec<Vec<Name>> {
    d.front
        .iter()
        .chain(&d.back)
        .map(|b| {
            let mut n: Vec<Name> = b.faces().map(|f| b.face(f).unwrap().name).collect();
            n.extend(b.edges().map(|e| b.edge(e).unwrap().name));
            n.extend(b.vertices().map(|v| b.vertex(v).unwrap().name));
            n.sort();
            n
        })
        .collect()
}

#[test]
fn moving_or_tilting_the_tool_renames_nothing() {
    let b = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let at = |z: f64, tilt: f64| {
        let s = plane(
            2,
            Point3::new(1.0, 1.0, z),
            Vector3::new(tilt, 0.0, 1.0),
            Vector3::new(1.0, 0.0, -tilt),
            [[-3.0, 3.0], [-3.0, 3.0]],
        );
        names(&divide(OpKey(3), &b, &[tool(4, &s)]).unwrap().0)
    };
    assert_eq!(at(1.0, 0.0), at(0.4, 0.0));
    assert_eq!(at(1.0, 0.0), at(1.0, 0.2));
}

#[test]
fn one_thread_and_four_divide_alike() {
    let ball = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let s = flat(2, 0.3, 2.0);
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let (d, j) = divide(OpKey(3), &ball, &[tool(4, &s)]).unwrap();
            let mut out: Vec<String> = d.front.iter().chain(&d.back).map(Body::dump).collect();
            out.push(format!("{j:?}"));
            out
        })
    };
    assert_eq!(run(1), run(4));
}

#[test]
fn a_sheet_trimmed_and_its_rest_make_the_sheet() {
    // A sheet through a drilled plate's hole: what is inside the plate
    // and what is outside add up to the whole.
    let plate = primitive::block(OpKey(1), 2.0, 2.0, 0.5, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(
        OpKey(2),
        0.4,
        2.0,
        &Rigid::translation(Vector3::new(1.0, 1.0, -0.5)),
    )
    .unwrap()
    .0;
    let (drilled, _) = boolean(OpKey(3), BooleanOp::Subtract, &plate, &[tool(4, &rod)]).unwrap();
    let s = plane(
        5,
        Point3::new(1.0, 1.0, 0.25),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        [[-2.0, 2.0], [-1.0, 1.0]],
    );
    let (inside, _) = boolean(OpKey(6), BooleanOp::Intersect, &s, &[tool(7, &drilled)]).unwrap();
    let (outside, _) = boolean(OpKey(6), BooleanOp::Subtract, &s, &[tool(7, &drilled)]).unwrap();
    sound(&inside);
    sound(&outside);
    // Inside: the plate's section less the hole's, 2 × 0.5 less 0.8 × 0.5.
    assert!(near(area(&inside), 0.6), "{:?}", area(&inside));
    assert!(near(area(&outside), 8.0 - 0.6), "{:?}", area(&outside));
    assert_eq!(inside.shells().count(), 2);
}
