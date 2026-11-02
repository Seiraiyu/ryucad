//! Self-union (design 299, plan 14.C): bodies whose shells cross or whose
//! faces overlap, made into the valid solid of the points their winding
//! number is one or more at, directly and as healing repairs them.
use ryukei::check::{check, check_intersections};
use ryukei::geom::{Point3, Rigid, Vector3};
use ryukei::heal::{HealOptions, Shape, heal};
use ryukei::intersect::{Inclusion, point_in_body, winding};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::repair::self_union;
use ryukei::topo::Body;
use ryukei_testkit::crossing::polyhedron;

fn block(key: u128, x: f64, y: f64, z: f64, at: [f64; 3]) -> Body {
    primitive::block(
        OpKey(key),
        x,
        y,
        z,
        &Rigid::translation(Vector3::new(at[0], at[1], at[2])),
    )
    .unwrap()
    .0
}

fn sound(b: &Body) {
    assert!(check(b).is_empty(), "{:?}", check(b));
    assert!(
        check_intersections(b).is_empty(),
        "{:?}",
        check_intersections(b)
    );
}

#[test]
fn a_valid_body_comes_back_as_it_was() {
    let a = block(1, 1.0, 2.0, 3.0, [0.5, 0.0, 0.0]);
    let (b, journal) = self_union(OpKey(9), &a).unwrap();
    assert_eq!(
        ryukei::codec::encode_body(&b),
        ryukei::codec::encode_body(&a)
    );
    assert!(journal.is_empty());
}

/// An octahedron one of whose corners is pushed through the far side:
/// its four faces at that corner cross the four opposite.
fn pushed_through() -> Shape {
    let p = [
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(0.0, 1.0, 0.0),
        Point3::new(-1.0, 0.0, 0.0),
        Point3::new(0.0, -1.0, 0.0),
        Point3::new(0.9, 0.2, -1.6),
        Point3::new(0.0, 0.0, -1.0),
    ];
    polyhedron(
        &p,
        &[
            [0, 1, 4],
            [1, 2, 4],
            [2, 3, 4],
            [3, 0, 4],
            [1, 0, 5],
            [2, 1, 5],
            [3, 2, 5],
            [0, 3, 5],
        ],
    )
}

#[test]
fn faces_crossing_in_one_shell_are_repaired_by_winding() {
    // Healing doesn't look for faces crossing within a shell: it heals.
    let (h, _, _) = heal(OpKey(7), &pushed_through(), &HealOptions::default()).unwrap();
    assert!(!check_intersections(&h).is_empty());
    let (r, journal) = self_union(OpKey(9), &h).unwrap();
    sound(&r);
    assert!(!journal.is_empty());
    // Inside where the winding number is one or more, on a grid.
    let mut tried = 0;
    for i in 0..12 {
        for j in 0..12 {
            for k in 0..12 {
                let at = |n: i32| -1.2 + 0.2 * f64::from(n) + 0.0123;
                let p = Point3::new(at(i), at(j), at(k) - 0.6);
                let (Ok(w), Ok(inc)) = (winding(&h, p), point_in_body(&r, p)) else {
                    continue;
                };
                if inc == Inclusion::Boundary {
                    continue;
                }
                tried += 1;
                assert_eq!(inc == Inclusion::Inside, w >= 1, "{p:?}: winding {w}");
            }
        }
    }
    assert!(tried > 1000, "{tried}");
}
