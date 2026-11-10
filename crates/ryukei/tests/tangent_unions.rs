//! Solids glued across a plane whose faces run on tangentially across the
//! rim both carry (issue #584: Voron's Z Belt Drives, a torus tube
//! running on into a straight cylinder). As surfaces the torus and the
//! cylinder cross again along a curve through two points of the rim, each
//! beyond the other's face, so the trace stays uncertain; the faces lie
//! either side of the rim's plane and meet only along it. Release only
//! (`cargo test -p ryukei --release --test tangent_unions`): the trace
//! that fails first takes about half a minute.

use core::f64::consts::{FRAC_PI_2, PI, TAU};
use ryukei::boolean::{BooleanOp, Tool, ToolKey, boolean};
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Axis, Circle2, Curve2, Frame, Frame2, Point2, Point3, Rigid, Vector2, Vector3};
use ryukei::mesh::{MeshParams, check_mesh, tessellate};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};
use ryukei::topo::Body;

/// A quarter of a torus tube: the circle of radius `r` about `(big, 0)`
/// in the xy plane, turned a quarter about y, so it leaves the plane
/// z = 0 along -z.
fn elbow(big: f64, r: f64) -> Body {
    let f = Frame2::new(Point2::new(big, 0.0), Vector2::new(1.0, 0.0), true).unwrap();
    let profile = ValidProfile::new(Profile {
        frame: Frame::WORLD,
        loops: vec![Loop {
            key: LoopKey(1),
            segments: vec![Segment {
                key: SegmentKey(2),
                curve: Curve2::Circle2(Circle2::new(f, r).unwrap()),
                t0: 0.0,
                t1: TAU,
            }],
        }],
    })
    .unwrap();
    let axis = Axis::new(Point3::new(0.0, 0.0, 0.0), Vector3::new(0.0, 1.0, 0.0)).unwrap();
    revolve(OpKey(7), &profile, axis, 0.0, FRAC_PI_2).unwrap().0
}

/// The volume a sound body's mesh encloses, and its area.
fn volume(b: &Body) -> (f64, f64) {
    assert_eq!(check(b), []);
    assert_eq!(check_intersections(b), []);
    let m = tessellate(b, &MeshParams::new(CHORD, 0.2)).unwrap();
    assert!(m.faults.is_empty(), "{:?}", m.faults);
    check_mesh(b, &m).unwrap();
    m.welded_triangles()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
            (a.dot(b.cross(c)) / 6.0, (b - a).cross(c - a).length() / 2.0)
        })
        .fold((0.0, 0.0), |(v, s), (dv, ds)| (v + dv, s + ds))
}

const CHORD: f64 = 2e-5;

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "slow in a debug build: CI runs it in release"
)]
fn a_torus_tube_running_on_into_a_cylinder_unites() {
    // Z Belt Drive A's sizes: a 1 mm tube about a 22.46 mm circle.
    let (big, r, h) = (0.02246, 0.001, 0.01);
    let elbow = elbow(big, r);
    let rod = primitive::cylinder(OpKey(8), r, h, &Rigid::IDENTITY)
        .unwrap()
        .0;
    let tool = Tool {
        key: ToolKey(1),
        body: &rod,
        place: Rigid::translation(Vector3::new(big, 0.0, 0.0)),
    };
    let (u, _) = boolean(OpKey(9), BooleanOp::Unite, &elbow, &[tool]).unwrap();
    // Pappus for the quarter tube, and the rod.
    let want = PI * r * r * (FRAC_PI_2 * big) + PI * r * r * h;
    let (got, area) = volume(&u);
    // The mesh lies within the chord of the surface.
    assert!((got - want).abs() <= CHORD * area, "{got} against {want}");
    // One shell: the rim is an inner edge between the tube and the rod.
    assert_eq!(u.faces().count(), 4);
}
