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

/// A selection that keeps nothing leaves nothing to build: `resolve`
/// says so rather than building an empty body.
#[test]
fn resolve_with_nothing_kept_is_none() {
    let b = block();
    let pairs = std::collections::BTreeSet::new();
    let none = |_: &[Body],
                _: &crate::boolean::table::Table,
                _: &[crate::split::Piece],
                _: &[usize],
                _: &Meter| Ok(Vec::new());
    let got = resolve(OpKey(9), &b, &pairs, &none, &|_| false, &Meter::free()).unwrap();
    assert!(got.is_none());
}
