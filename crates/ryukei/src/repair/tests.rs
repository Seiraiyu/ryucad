use super::*;
use crate::geom::{Point3, Rigid, Vector3};
use crate::intersect::winding;
use crate::primitive;

fn block() -> Body {
    primitive::block(
        OpKey(1),
        1.0,
        2.0,
        3.0,
        &Rigid::translation(Vector3::new(0.5, 0.0, 0.0)),
    )
    .unwrap()
    .0
}

#[test]
fn a_valid_body_comes_back_as_it_was() {
    let b = block();
    let (r, journal) = self_union(OpKey(9), &b).unwrap();
    assert_eq!(crate::codec::encode_body(&r), crate::codec::encode_body(&b));
    assert!(journal.is_empty());
}

#[test]
fn a_sheet_is_not_a_solid() {
    let b = block();
    let f = b.faces().next().unwrap();
    let (sheet, _) = crate::sheet::extract(OpKey(2), &b, &[b.faces[f].name]).unwrap();
    assert_eq!(
        self_union(OpKey(9), &sheet).unwrap_err(),
        Error::NotASolid { op: "self_union" }
    );
}

#[test]
fn winding_counts_a_block_once() {
    let b = block();
    assert_eq!(winding(&b, Point3::new(1.0, 1.0, 1.5)), Ok(1));
    assert_eq!(winding(&b, Point3::new(3.0, 1.0, 1.5)), Ok(0));
    // On a face nothing is settled.
    assert!(winding(&b, Point3::new(0.5, 1.0, 1.5)).is_err());
}

#[test]
fn just_outside_a_face_the_winding_is_zero() {
    let b = block();
    // The face at x = 0.5, its outward normal -x.
    let f = b
        .faces()
        .find(|&f| b.surfaces[b.faces[f].surface].distance(Point3::new(0.5, 1.0, 1.5)) < 1e-12)
        .unwrap();
    let w = crate::intersect::winding_off(
        &b,
        Point3::new(0.5, 1.0, 1.5),
        -crate::geom::UnitVector3::X,
        &[f],
        &Meter::free(), // unmetered: a test
    );
    assert_eq!(w, Ok(0));
    // Leaving to the inside, it is one.
    let w = crate::intersect::winding_off(
        &b,
        Point3::new(0.5, 1.0, 1.5),
        crate::geom::UnitVector3::X,
        &[f],
        &Meter::free(), // unmetered: a test
    );
    assert_eq!(w, Ok(1));
}
