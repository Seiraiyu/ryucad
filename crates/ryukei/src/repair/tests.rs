use super::*;
use crate::geom::{Rigid, Vector3};
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
