//! Hinges: what a drafted face turns about.

use crate::geom::{Box3, Curve, Surface};
use crate::intersect::{Branch, SurfaceHit, face_box, surface_surface_in};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use crate::work::Meter;
use crate::{Error, TweakError};

/// How far past a face's box a hinge is searched for, as a share of the
/// box's diagonal each way.
const GROW: f64 = 0.5;

/// A hinge: a curve over a range of its parameter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HingeCurve {
    pub(crate) curve: Curve,
    pub(crate) range: [f64; 2],
}

/// The diagonal of a face's box, at least ten resolutions.
pub(crate) fn size_of(b: &Box3) -> f64 {
    b.min().distance(b.max()).max(10.0 * LINEAR_RESOLUTION)
}

/// Whether some of 17 points along the branch lie in `bx`.
fn crosses(b: &Branch, bx: &Box3) -> bool {
    let [t0, t1] = b.range;
    let (lo, hi) = (bx.min(), bx.max());
    (0..17).any(|i| {
        let p = b.curve.point(t0 + (t1 - t0) * f64::from(i) / 16.0);
        (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && (lo.z..=hi.z).contains(&p.z)
    })
}

/// Face `f`'s hinge on the neutral `plane`: the one branch of its
/// surface's meeting with the plane that comes into its box (grown by a
/// hundredth of its diagonal). `NoHinge` for none (or a face lying in the
/// plane), `HingeAmbiguous` for several, `Uncertain` where the
/// intersection is.
pub(crate) fn neutral(
    body: &Body,
    f: FaceId,
    plane: &Surface,
    meter: &Meter,
) -> Result<HingeCurve, Error> {
    let face = &body.faces[f];
    let at = face.name;
    let s = &body.surfaces[face.surface];
    let bx = face_box(body, f);
    let size = size_of(&bx);
    let hit = surface_surface_in(s, plane, &bx.grow(GROW * size), meter)
        .map_err(|_| TweakError::Uncertain { at })?;
    let SurfaceHit::Meet { branches, .. } = hit else {
        return Err(TweakError::NoHinge { face: at }.into());
    };
    let near = bx.grow(0.01 * size);
    let mut found: Vec<Branch> = branches.into_iter().filter(|b| crosses(b, &near)).collect();
    match found.len() {
        0 => Err(TweakError::NoHinge { face: at }.into()),
        1 => {
            let b = found.remove(0);
            Ok(HingeCurve {
                curve: b.curve,
                range: b.range,
            })
        }
        _ => Err(TweakError::HingeAmbiguous { face: at }.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame, Plane, Point3, Rigid, UnitVector3, Vector3};
    use crate::name::{FaceRole, Name, OpKey, Role};
    use crate::primitive;
    use crate::work::Meter;

    const K: OpKey = OpKey(11);

    fn plane_at(z: f64) -> Surface {
        Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(0.0, 0.0, z),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap(),
        ))
    }

    fn face(b: &Body, r: FaceRole) -> FaceId {
        let n = Name::derive(K, Role::Face(r), &[]);
        b.faces().find(|&f| b.faces[f].name == n).unwrap()
    }

    fn err(r: Result<HingeCurve, Error>) -> TweakError {
        match r {
            Err(Error::InvalidTweak(e)) => e,
            other => panic!("{other:?}"),
        }
    }

    fn side_of(c: &Body) -> FaceId {
        c.faces()
            .find(|&f| matches!(c.surfaces[c.faces[f].surface], Surface::Cylinder(_)))
            .unwrap()
    }

    #[test]
    fn a_walls_hinge_is_a_line_and_a_cylinders_a_circle() {
        // unmetered: a unit test.
        let m = Meter::free();
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let h = neutral(&b, face(&b, FaceRole::PlusX), &plane_at(0.1), &m).unwrap();
        let Curve::Line(l) = &h.curve else {
            panic!("{h:?}")
        };
        assert!((l.origin().x - 1.0).abs() < 1e-12 && (l.origin().z - 0.1).abs() < 1e-12);
        assert!(l.dir().get().cross(Vector3::new(0.0, 1.0, 0.0)).length() < 1e-12);
        let (c, _) = primitive::cylinder(K, 0.1, 0.4, &Rigid::IDENTITY).unwrap();
        let h = neutral(&c, side_of(&c), &plane_at(0.2), &m).unwrap();
        let Curve::Circle(k) = &h.curve else {
            panic!("{h:?}")
        };
        assert!((k.radius() - 0.1).abs() < 1e-12);
        assert!((k.frame().origin().z - 0.2).abs() < 1e-12);
    }

    #[test]
    fn a_plane_missing_a_face_or_crossing_it_twice_is_refused() {
        // unmetered: a unit test.
        let m = Meter::free();
        let (b, _) = primitive::block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let wall = face(&b, FaceRole::PlusX);
        let at = b.faces[wall].name;
        assert_eq!(
            err(neutral(&b, wall, &plane_at(5.0), &m)),
            TweakError::NoHinge { face: at }
        );
        let top = face(&b, FaceRole::PlusZ);
        let t = b.faces[top].name;
        assert_eq!(
            err(neutral(&b, top, &plane_at(0.2), &m)),
            TweakError::NoHinge { face: t }
        );
        // A cylinder lying along x, the plane through its axis: two lines.
        let y = UnitVector3::exact(Vector3::new(0.0, 1.0, 0.0)).unwrap();
        let lying = Rigid::rotation(y, core::f64::consts::FRAC_PI_2);
        let (c, _) = primitive::cylinder(K, 0.1, 0.4, &lying).unwrap();
        let side = side_of(&c);
        let n = c.faces[side].name;
        assert_eq!(
            err(neutral(&c, side, &plane_at(0.0), &m)),
            TweakError::HingeAmbiguous { face: n }
        );
    }
}
