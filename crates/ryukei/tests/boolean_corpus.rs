//! Booleans on the cases agents produce all day: flush, coincident and
//! tangent contact, results with several lumps, cavities or nothing, and
//! contact the result can't have.

use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Rigid, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei::{BooleanError, Error};
use std::f64::consts::PI;

fn at(x: f64, y: f64, z: f64) -> Rigid {
    Rigid::translation(Vector3::new(x, y, z))
}

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

fn run(op: BooleanOp, target: &Body, tools: &[(&Body, Rigid)]) -> Result<Body, Error> {
    let tools: Vec<Tool> = tools
        .iter()
        .enumerate()
        .map(|(i, (b, place))| Tool {
            key: ToolKey(i as u128 + 1),
            body: b,
            place: *place,
        })
        .collect();
    boolean(OpKey(100), op, target, &tools).map(|x| x.0)
}

fn near(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 2e-3 * want.abs().max(1e-3),
        "{got} against {want}"
    );
}

fn faces(b: &Body) -> usize {
    b.counts().faces
}

#[test]
fn a_body_with_itself() {
    let a = primitive::block(OpKey(1), 2.0, 3.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let u = run(BooleanOp::Unite, &a, &[(&a, Rigid::IDENTITY)]).unwrap();
    near(volume(&u), 6.0);
    assert_eq!(u.counts(), a.counts());
    let i = run(BooleanOp::Intersect, &a, &[(&a, Rigid::IDENTITY)]).unwrap();
    near(volume(&i), 6.0);
    assert_eq!(i.counts(), a.counts());
    let s = run(BooleanOp::Subtract, &a, &[(&a, Rigid::IDENTITY)]).unwrap();
    assert_eq!(faces(&s), 0);
    assert_eq!(s.regions().count(), 1);
    let c = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let u = run(BooleanOp::Unite, &c, &[(&c, Rigid::IDENTITY)]).unwrap();
    near(volume(&u), 2.0 * PI);
    let s = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let d = run(BooleanOp::Subtract, &s, &[(&s, Rigid::IDENTITY)]).unwrap();
    assert_eq!(faces(&d), 0);
}

#[test]
fn flush_blocks() {
    // A block on a block, the upper smaller: one solid, the lower's top
    // with the upper's footprint taken out.
    let a = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let b = primitive::block(OpKey(2), 2.0, 2.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let u = run(BooleanOp::Unite, &a, &[(&b, at(1.0, 1.0, 1.0))]).unwrap();
    near(volume(&u), 20.0);
    assert_eq!(faces(&u), 11);
    // Standing on it: subtracting or intersecting changes nothing.
    let s = run(BooleanOp::Subtract, &a, &[(&b, at(1.0, 1.0, 1.0))]).unwrap();
    near(volume(&s), 16.0);
    assert_eq!(s.counts(), a.counts());
    let i = run(BooleanOp::Intersect, &a, &[(&b, at(1.0, 1.0, 1.0))]).unwrap();
    assert_eq!(faces(&i), 0);
    // Flush with a side: a step.
    let side = run(BooleanOp::Unite, &a, &[(&b, at(0.0, 1.0, 1.0))]).unwrap();
    near(volume(&side), 20.0);
    assert_eq!(faces(&side), 10);
}

#[test]
fn coaxial_cylinders() {
    let c = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let thin = primitive::cylinder(OpKey(2), 0.5, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    // A bore through, and a boss through it.
    let bore = run(BooleanOp::Subtract, &c, &[(&thin, at(0.0, 0.0, -1.0))]).unwrap();
    near(volume(&bore), 2.0 * PI * 0.75);
    let boss = run(BooleanOp::Unite, &c, &[(&thin, at(0.0, 0.0, -1.0))]).unwrap();
    near(volume(&boss), 2.0 * PI + 2.0 * PI * 0.25);
    // Same radius, overlapping along the axis: one longer cylinder.
    let longer = run(BooleanOp::Unite, &c, &[(&c, at(0.0, 0.0, 1.0))]).unwrap();
    near(volume(&longer), 3.0 * PI);
    assert_eq!(longer.counts().faces, 3);
    // A blind bore from the top, flush.
    let blind = run(BooleanOp::Subtract, &c, &[(&thin, at(0.0, 0.0, 1.0))]).unwrap();
    near(volume(&blind), 2.0 * PI - PI * 0.25);
}

#[test]
fn a_slot_of_a_box_and_two_tangent_cylinders() {
    // A box 4 by 1 and a cylinder of radius 0.5 at each end, tangent to
    // its long sides.
    let bx = primitive::block(OpKey(1), 4.0, 1.0, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let end = primitive::cylinder(OpKey(2), 0.5, 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let slot = run(
        BooleanOp::Unite,
        &bx,
        &[(&end, at(0.0, 0.5, 0.0)), (&end, at(4.0, 0.5, 0.0))],
    )
    .unwrap();
    near(volume(&slot), 4.0 + PI * 0.25);
    assert_eq!(faces(&slot), 6);
    // Cut out of a plate: a slotted hole.
    let plate = primitive::block(OpKey(3), 6.0, 3.0, 1.0, &at(-1.0, -1.0, 0.0))
        .unwrap()
        .0;
    let hole = run(BooleanOp::Subtract, &plate, &[(&slot, Rigid::IDENTITY)]);
    let hole = hole.unwrap();
    near(volume(&hole), 18.0 - 4.0 - PI * 0.25);
}

#[test]
fn spheres_in_cylinders_and_tangent_cuts() {
    // A sphere inscribed in a cylinder, touching it all round its middle:
    // subtracting it leaves the cylinder's waist zero thick, refused.
    let cyl = primitive::cylinder(OpKey(1), 1.0, 4.0, &at(0.0, 0.0, -2.0))
        .unwrap()
        .0;
    let ball = primitive::sphere(OpKey(2), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let s = run(BooleanOp::Subtract, &cyl, &[(&ball, Rigid::IDENTITY)]);
    assert!(
        matches!(
            s,
            Err(Error::InvalidBoolean(BooleanError::NonManifold { .. }))
        ),
        "{s:?}"
    );
    let u = run(BooleanOp::Unite, &cyl, &[(&ball, Rigid::IDENTITY)]).unwrap();
    near(volume(&u), 4.0 * PI);
    let i = run(BooleanOp::Intersect, &cyl, &[(&ball, Rigid::IDENTITY)]).unwrap();
    near(volume(&i), 4.0 * PI / 3.0);
    // A block touching another from outside along a face, an edge, or a
    // point: nothing to subtract.
    let a = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for place in [at(2.0, 0.0, 0.0), at(2.0, 2.0, 0.0), at(2.0, 2.0, 2.0)] {
        let s = run(BooleanOp::Subtract, &a, &[(&a, place)]).unwrap();
        assert_eq!(s.counts(), a.counts());
        near(volume(&s), 8.0);
    }
    // A cylinder lying on a block's top: nothing to subtract either.
    let rod = primitive::cylinder(OpKey(3), 0.5, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let lying = Rigid::rotation(Vector3::new(0.0, 1.0, 0.0).normalize().unwrap(), PI / 2.0);
    let s = run(
        BooleanOp::Subtract,
        &a,
        &[(&rod, at(-1.0, 1.0, 2.5).then_after(&lying))],
    )
    .unwrap();
    near(volume(&s), 8.0);
}

#[test]
fn edge_and_point_contact_are_refused() {
    let a = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    for place in [at(2.0, 2.0, 0.0), at(2.0, 2.0, 2.0)] {
        let got = run(BooleanOp::Unite, &a, &[(&a, place)]);
        assert!(
            matches!(
                got,
                Err(Error::InvalidBoolean(BooleanError::NonManifold { .. }))
            ),
            "{got:?}"
        );
    }
    // Touching along a face is fine: a longer block.
    let u = run(BooleanOp::Unite, &a, &[(&a, at(2.0, 0.0, 0.0))]).unwrap();
    near(volume(&u), 16.0);
    assert_eq!(faces(&u), 6);
}

#[test]
fn cavities_lumps_and_nothing() {
    let a = primitive::block(OpKey(1), 4.0, 4.0, 4.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let ball = primitive::sphere(OpKey(2), 1.0, &Rigid::IDENTITY)
        .unwrap()
        .0;
    // A ball taken out of the middle: a cavity.
    let hollow = run(BooleanOp::Subtract, &a, &[(&ball, at(2.0, 2.0, 2.0))]).unwrap();
    near(volume(&hollow), 64.0 - 4.0 * PI / 3.0);
    assert_eq!(hollow.counts().shells, 2);
    assert_eq!(hollow.regions().count(), 2);
    // A ball and a block apart: two lumps.
    let apart = run(BooleanOp::Unite, &a, &[(&ball, at(10.0, 0.0, 0.0))]).unwrap();
    near(volume(&apart), 64.0 + 4.0 * PI / 3.0);
    assert_eq!(apart.regions().count(), 3);
    // Nothing in common.
    let none = run(BooleanOp::Intersect, &a, &[(&ball, at(10.0, 0.0, 0.0))]).unwrap();
    assert_eq!(faces(&none), 0);
    // A ball back into the hollow: solid again.
    let full = run(BooleanOp::Unite, &hollow, &[(&ball, at(2.0, 2.0, 2.0))]).unwrap();
    near(volume(&full), 64.0);
}
