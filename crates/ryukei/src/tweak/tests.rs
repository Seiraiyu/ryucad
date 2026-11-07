use super::*;
use crate::geom::{Rigid, Vector3};
use crate::name::{FaceRole, Name, OpKey, Role};
use crate::primitive;
use crate::topo::Body;
use crate::{Error, TweakError};
use std::collections::BTreeMap;

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

/// A block with its top's `+X` edge filleted.
pub(crate) fn filleted() -> Body {
    let b = block();
    let mut f = [face(FaceRole::PlusZ), face(FaceRole::PlusX)];
    f.sort();
    let edge = Name::derive(K, Role::Edge, &f);
    crate::blend::blend(
        OpKey(11),
        &b,
        &[crate::blend::EdgeBlend {
            edge,
            kind: crate::blend::BlendKind::Fillet { radius: 0.1 },
        }],
    )
    .unwrap()
    .0
}

#[test]
fn a_tops_tangent_chain_takes_its_fillet_and_the_side_beyond() {
    let b = filleted();
    let top = match b.by_name(face(FaceRole::PlusZ)) {
        Some(crate::topo::EntityId::Face(f)) => f,
        _ => unreachable!(),
    };
    let up = Motion::Rigid(Rigid::translation(Vector3::new(0.0, 0.0, 0.05)));
    let grown = chain::grow(&b, BTreeMap::from([(top, up)])).unwrap();
    // The top, the fillet, and `+X` beyond it.
    assert_eq!(grown.len(), 3);
    assert!(grown.values().all(|m| *m == up));
}

#[test]
fn moving_a_top_beside_its_fillet_needs_the_tangent_option() {
    let b = filleted();
    let up = Rigid::translation(Vector3::new(0.0, 0.0, 0.05));
    let spec = |tangent| MoveSpec {
        faces: vec![face(FaceRole::PlusZ)],
        motion: up,
        tangent,
    };
    assert!(matches!(
        refusal(move_faces(T, &b, &spec(false))),
        TweakError::TangencyBroken { .. }
    ));
    let (o, _) = move_faces(T, &b, &spec(true)).unwrap();
    assert_eq!(crate::check::check(&o), []);
    assert_eq!(o.faces().count(), b.faces().count());
}

#[test]
fn two_neighbouring_faces_offset_keep_their_sharp_edge() {
    let b = block();
    let spec = OffsetFacesSpec {
        faces: vec![(face(FaceRole::PlusZ), 0.1), (face(FaceRole::PlusX), 0.2)],
        tangent: false,
    };
    let (o, _) = offset_faces(T, &b, &spec).unwrap();
    assert_eq!((o.faces().count(), o.edges().count()), (6, 12));
    assert_eq!(crate::check::check(&o), []);
    let far =
        o.vertices()
            .map(|v| o.vertices[v].point)
            .fold(crate::geom::Point3::ORIGIN, |a, p| {
                if p.x + p.z > a.x + a.z { p } else { a }
            });
    assert!(
        far.distance(crate::geom::Point3::new(1.2, far.y, 0.5)) < 1e-9,
        "{far:?}"
    );
}

#[test]
fn offsets_are_checked_as_offsets_are() {
    let b = block();
    let spec = |d| OffsetFacesSpec {
        faces: vec![(face(FaceRole::PlusZ), d)],
        tangent: false,
    };
    assert_eq!(
        refusal(offset_faces(T, &b, &spec(1e-9))),
        TweakError::BadDistance
    );
    assert_eq!(
        refusal(offset_faces(T, &b, &spec(f64::INFINITY))),
        TweakError::NotFinite
    );
    let (o, j) = offset_faces(T, &b, &spec(0.0)).unwrap();
    assert_eq!(crate::codec::encode_body(&o), crate::codec::encode_body(&b));
    assert!(j.events().is_empty());
}

fn plane(origin: crate::geom::Point3, z: Vector3) -> crate::geom::Surface {
    let x = if z.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    crate::geom::Surface::Plane(crate::geom::Plane::new(
        crate::geom::Frame::new(origin, z, x).unwrap(),
    ))
}

#[test]
fn a_top_replaced_by_a_higher_plane_either_way_up() {
    let b = block();
    let top = face(FaceRole::PlusZ);
    for z in [1.0, -1.0] {
        let spec = ReplaceSpec {
            faces: vec![(
                top,
                plane(
                    crate::geom::Point3::new(0.0, 0.0, 0.45),
                    Vector3::new(0.0, 0.0, z),
                ),
            )],
        };
        let (o, _) = replace_faces(T, &b, &spec).unwrap();
        assert_eq!(crate::check::check(&o), []);
        let h = o
            .vertices()
            .map(|v| o.vertices[v].point.z)
            .fold(0.0, f64::max);
        assert!((h - 0.45).abs() < 1e-12);
    }
}

#[test]
fn a_replacement_square_to_its_face_is_ambiguous() {
    let b = block();
    let top = face(FaceRole::PlusZ);
    let spec = ReplaceSpec {
        faces: vec![(
            top,
            plane(
                crate::geom::Point3::new(0.5, 0.3, 0.4),
                Vector3::new(1.0, 0.0, 0.0),
            ),
        )],
    };
    assert_eq!(
        refusal(replace_faces(T, &b, &spec)),
        TweakError::Ambiguous { face: top }
    );
}

#[test]
fn a_boxs_walls_drafted_about_its_bottom_lean_in_and_report_their_angle() {
    use crate::geom::{Point3, UnitVector3};
    let b = block();
    let walls = [
        FaceRole::PlusX,
        FaceRole::MinusX,
        FaceRole::PlusY,
        FaceRole::MinusY,
    ];
    let spec = DraftSpec {
        pull: UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap(),
        angle: 0.05,
        hinge: Hinge::Neutral {
            origin: Point3::ORIGIN,
        },
        faces: walls
            .iter()
            .map(|&r| DraftFace {
                face: face(r),
                angle: None,
                flip: false,
            })
            .collect(),
        tangent: false,
    };
    let (o, _, report) = draft(T, &b, &spec).unwrap();
    assert_eq!(crate::check::check(&o), []);
    let t = 0.4 * crate::math::tan(0.05);
    for p in o
        .vertices()
        .map(|v| o.vertices[v].point)
        .filter(|p| p.z > 0.2)
    {
        assert!(
            ((p.x - 0.5).abs() - (0.5 - t)).abs() < 1e-9
                && ((p.y - 0.3).abs() - (0.3 - t)).abs() < 1e-9,
            "{p:?}"
        );
    }
    assert_eq!(report.faces.len(), 4);
    for a in &report.faces {
        assert!(
            (a.least - 0.05).abs() < 1e-12 && (a.greatest - 0.05).abs() < 1e-12,
            "{a:?}"
        );
    }
}

#[test]
fn a_draft_checks_its_angle_and_faces() {
    use crate::geom::{Point3, UnitVector3};
    let b = block();
    let wall = face(FaceRole::PlusX);
    let top = face(FaceRole::PlusZ);
    let spec = |angle: f64, face: Name| DraftSpec {
        pull: UnitVector3::exact(Vector3::new(0.0, 0.0, 1.0)).unwrap(),
        angle,
        hinge: Hinge::Neutral {
            origin: Point3::new(0.0, 0.0, 0.2),
        },
        faces: vec![DraftFace {
            face,
            angle: None,
            flip: false,
        }],
        tangent: false,
    };
    assert_eq!(
        refusal(draft(T, &b, &spec(0.0, wall))),
        TweakError::BadAngle
    );
    assert_eq!(
        refusal(draft(T, &b, &spec(1.57, wall))),
        TweakError::BadAngle
    );
    assert_eq!(
        refusal(draft(T, &b, &spec(f64::NAN, wall))),
        TweakError::BadAngle
    );
    assert_eq!(
        refusal(draft(T, &b, &spec(0.05, top))),
        TweakError::NoHinge { face: top }
    );
}
