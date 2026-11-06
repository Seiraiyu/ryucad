use super::*;
use crate::geom::{Rigid, Vector3};
use crate::name::{FaceRole, Name, OpKey, Role};
use crate::primitive;
use crate::topo::Body;
use crate::{Error, TweakError};

pub(crate) const K: OpKey = OpKey(9);
pub(crate) const T: OpKey = OpKey(10);

pub(crate) fn block() -> Body {
    primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY)
        .unwrap()
        .0
}

pub(crate) fn face(r: FaceRole) -> Name {
    Name::derive(K, Role::Face(r), &[])
}

pub(crate) fn refusal<T>(r: Result<T, Error>) -> TweakError {
    match r {
        Err(Error::InvalidTweak(e)) => e,
        Err(e) => panic!("not a tweak's refusal: {e:?}"),
        Ok(_) => panic!("not refused"),
    }
}

fn mv(faces: Vec<Name>, motion: Rigid) -> MoveSpec {
    MoveSpec {
        faces,
        motion,
        tangent: false,
    }
}

#[test]
fn a_whole_body_moved_is_the_body_moved() {
    let b = block();
    let r = Rigid::translation(Vector3::new(0.25, -0.5, 1.0));
    let all: Vec<Name> = b.faces().map(|f| b.faces[f].name).collect();
    let (o, j) = move_faces(T, &b, &mv(all, r)).unwrap();
    assert_eq!(
        crate::codec::encode_body(&o),
        crate::codec::encode_body(&b.transformed(&r).unwrap())
    );
    assert_eq!(j.events().len(), 6 + 12 + 8);
}

#[test]
fn a_plane_slid_along_itself_changes_nothing() {
    let b = block();
    let r = Rigid::translation(Vector3::new(0.1, 0.05, 0.0));
    let (o, j) = move_faces(T, &b, &mv(vec![face(FaceRole::PlusZ)], r)).unwrap();
    assert_eq!(crate::codec::encode_body(&o), crate::codec::encode_body(&b));
    assert!(j.events().is_empty());
}

#[test]
fn a_raised_top_makes_a_taller_box() {
    let b = block();
    let r = Rigid::translation(Vector3::new(0.0, 0.0, 0.1));
    let (o, _) = move_faces(T, &b, &mv(vec![face(FaceRole::PlusZ)], r)).unwrap();
    assert_eq!(crate::check::check(&o), []);
    let top = o
        .vertices()
        .map(|v| o.vertices[v].point.z)
        .fold(0.0, f64::max);
    assert!((top - 0.5).abs() < 1e-12);
}

#[test]
fn bad_specs_are_refused() {
    let b = block();
    let r = Rigid::translation(Vector3::new(0.0, 0.0, 0.1));
    assert_eq!(
        refusal(move_faces(T, &b, &mv(vec![], r))),
        TweakError::NoFaces
    );
    let ghost = Name::from_u128(42);
    assert_eq!(
        refusal(move_faces(T, &b, &mv(vec![ghost], r))),
        TweakError::NoSuchFace { face: ghost }
    );
    let top = face(FaceRole::PlusZ);
    assert_eq!(
        refusal(move_faces(T, &b, &mv(vec![top, top], r))),
        TweakError::DuplicateFace { face: top }
    );
    let nan = Rigid::translation(Vector3::new(f64::NAN, 0.0, 0.0));
    assert_eq!(
        refusal(move_faces(T, &b, &mv(vec![top], nan))),
        TweakError::NotFinite
    );
}
