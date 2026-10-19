//! Bodies hold NURBS geometry like any other: primitives whose every curve
//! and surface is swapped for its exact NURBS form, and a disc bounded by
//! an ellipse, pass the checker.

use proptest::prelude::*;
use ryukei::check::{Fault, check};
use ryukei::euler::{self, Corner};
use ryukei::geom::{Curve, Ellipse, Frame, Plane, Point3, Rigid, Surface, Vector3};
use ryukei::name::OpKey;
use ryukei::primitive;
use ryukei::topo::Body;
use ryukei_testkit::nurbs::convert_body;

fn all_nurbs(b: &Body) -> bool {
    b.edges()
        .all(|e| matches!(b.curve(b.edge(e).unwrap().curve), Some(Curve::Nurbs(_))))
        && b.faces().all(|f| {
            matches!(
                b.surface(b.face(f).unwrap().surface),
                Some(Surface::Nurbs(_))
            )
        })
}

/// A disc on the world xy plane bounded by the ellipse with semi-axes 2
/// and 1: two faces, one each side, joined by one closed edge.
fn ellipse_disc() -> Body {
    let mut b = Body::new();
    let s = b.add_surface(Surface::Plane(Plane::new(Frame::WORLD)));
    let (m, _) = euler::mvfs(&mut b, OpKey(1), Point3::new(2.0, 0.0, 0.0), s).unwrap();
    let e = Ellipse::new(Frame::WORLD.flipped(), 2.0, 1.0).unwrap();
    let c = b.add_curve(Curve::Ellipse(e));
    let lone = Corner::Lone(m.loop_);
    let tau = core::f64::consts::TAU;
    euler::mef(&mut b, OpKey(2), lone, lone, c, [0.0, tau]).unwrap();
    ryukei::pcurve::fill(&mut b).unwrap();
    b
}

fn names(b: &Body) -> Vec<String> {
    b.dump()
        .lines()
        .filter_map(|l| l.split_once(" name=").map(|x| x.1.to_string()))
        .collect()
}

#[test]
fn a_block_with_nurbs_faces() {
    let at = Rigid::translation(Vector3::new(1.0, -2.0, 3.0));
    let (mut b, _) = primitive::block(OpKey(7), 1.5, 2.0, 0.5, &at).unwrap();
    let before = names(&b);
    convert_body(&mut b).unwrap();
    assert!(all_nurbs(&b));
    assert_eq!(check(&b), []);
    assert_eq!(names(&b), before);
}

#[test]
fn a_cylinder_with_exact_nurbs() {
    let (mut b, _) = primitive::cylinder(OpKey(8), 0.75, 2.0, &Rigid::IDENTITY).unwrap();
    convert_body(&mut b).unwrap();
    assert!(all_nurbs(&b));
    assert_eq!(check(&b), []);
}

#[test]
fn an_ellipse_edge_on_a_plane() {
    let mut b = ellipse_disc();
    assert_eq!(check(&b), []);
    convert_body(&mut b).unwrap();
    assert!(all_nurbs(&b));
    assert_eq!(check(&b), []);
}

#[test]
fn closed_nurbs_surfaces_cannot_be_loopless() {
    let (mut b, _) = primitive::sphere(OpKey(9), 1.0, &Rigid::IDENTITY).unwrap();
    convert_body(&mut b).unwrap();
    let f = b.faces().next().unwrap();
    assert_eq!(check(&b), [Fault::Loopless { face: f }]);
}

fn placement() -> impl Strategy<Value = Rigid> {
    (-1.0f64..1.0, -1.0f64..1.0, -3.0f64..3.0, -20.0f64..20.0).prop_map(|(x, y, angle, shift)| {
        let axis = Vector3::new(x, y, 1.0).normalize().unwrap();
        Rigid::translation(Vector3::new(shift, -shift / 2.0, shift / 3.0))
            .then_after(&Rigid::rotation(axis, angle))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn swapping_in_nurbs_keeps_bodies_valid(
        kind in 0u8..4,
        a in 0.2f64..3.0,
        b in 0.2f64..3.0,
        at in placement(),
    ) {
        let key = OpKey(42);
        let (mut body, _) = match kind {
            0 => primitive::block(key, a, b, a + b, &at),
            1 => primitive::cylinder(key, a, b, &at),
            2 => primitive::cone(key, a, b, &at),
            _ => primitive::frustum(key, a, b / 2.0, b, &at),
        }
        .unwrap();
        convert_body(&mut body).unwrap();
        prop_assert!(all_nurbs(&body));
        prop_assert_eq!(check(&body), []);
    }
}
