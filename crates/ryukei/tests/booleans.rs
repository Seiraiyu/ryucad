//! Booleans against exact volumes: every result passes the checker and
//! the self-intersection check, meshes without faults, and encloses the
//! volume it should.

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use std::f64::consts::PI;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

/// The volume a sound body's mesh encloses.
fn volume(b: &Body) -> f64 {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(1e-3, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
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

fn near(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 2e-3 * want.abs().max(1e-3),
        "{got} against {want}"
    );
}

#[test]
fn a_hole_through_a_plate() {
    let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let r = run(BooleanOp::Subtract, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    near(volume(&r), 16.0 - PI * 0.25);
    let c = r.counts();
    assert_eq!((c.faces, c.edges, c.vertices), (7, 14, 10));
    // The rod kept to the plate's thickness, and the two together.
    let i = run(BooleanOp::Intersect, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    near(volume(&i), PI * 0.25);
    let u = run(BooleanOp::Unite, &plate, &[(&rod, at(2.0, 2.0, -1.0))]);
    near(volume(&u), 16.0 + PI * 0.25 * 2.0);
}

#[test]
fn crossing_blocks() {
    let a = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let b = primitive::block(OpKey(2), 2.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let place = at(1.0, 0.5, 0.5);
    near(volume(&run(BooleanOp::Unite, &a, &[(&b, place)])), 9.0);
    near(volume(&run(BooleanOp::Subtract, &a, &[(&b, place)])), 7.0);
    near(volume(&run(BooleanOp::Intersect, &a, &[(&b, place)])), 1.0);
}

#[test]
fn a_lens_and_a_steinmetz_solid() {
    let s = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let lens = run(BooleanOp::Intersect, &s, &[(&s, at(1.0, 0.0, 0.0))]);
    near(volume(&lens), 5.0 * PI / 12.0);
    // Two cylinders of radius 1 crossing at right angles.
    let c = primitive::cylinder(OpKey(1), 1.0, 4.0, &at(0.0, 0.0, -2.0))
        .unwrap()
        .0;
    let across = Rigid::rotation(Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(), PI / 2.0);
    let steinmetz = run(BooleanOp::Intersect, &c, &[(&c, across)]);
    near(volume(&steinmetz), 16.0 / 3.0);
}

#[test]
fn torus_and_cone_cuts() {
    let t = primitive::torus(OpKey(1), 3.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let below = primitive::block(OpKey(2), 10.0, 10.0, 5.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let half = run(BooleanOp::Subtract, &t, &[(&below, at(-5.0, -5.0, -5.0))]);
    near(volume(&half), PI * PI * 3.0);
    // A cone of radius 1 and height 2, its top half cut away: a frustum.
    let cone = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let top = primitive::block(OpKey(2), 4.0, 4.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let frustum = run(BooleanOp::Subtract, &cone, &[(&top, at(-2.0, -2.0, 1.0))]);
    near(volume(&frustum), PI * 2.0 / 3.0 - PI * 0.25 / 3.0);
}

#[test]
fn three_holes_at_once() {
    let plate = primitive::block(OpKey(1), 6.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let rod = primitive::cylinder(OpKey(2), 0.4, 3.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tools: Vec<(&Body, Rigid)> = (0..3)
        .map(|i| (&rod, at(1.0 + 2.0 * i as f64, 1.0, -1.0)))
        .collect();
    let r = run(BooleanOp::Subtract, &plate, &tools);
    near(volume(&r), 12.0 - 3.0 * PI * 0.16);
}
