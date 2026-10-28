//! The angle between two entities with directions.

use super::{Entity, Value};
use crate::Error;
use crate::MeasureError;
use crate::geom::{Curve, Rigid, Surface, UnitVector3};
use crate::math;
use crate::topo::{Body, Sense};
use core::f64::consts::{FRAC_PI_2, PI};

/// The angle between two entities and its supplement, both in `[0, π]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Angle {
    pub angle: Value,
    pub supplement: Value,
}

/// What gives an entity its direction.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Direction {
    /// A straight edge, from its start to its end.
    Line(UnitVector3),
    /// A planar face's outward normal.
    Normal(UnitVector3),
    /// The axis of a face turned about one, or a circular edge.
    Axis(UnitVector3),
}

fn direction(body: &Body, entity: Entity, place: &Rigid) -> Result<Direction, Error> {
    let not = || Error::from(MeasureError::NotAnEntity);
    Ok(match entity {
        Entity::Edge(e) => {
            let e = body.edge(e).ok_or_else(not)?;
            match body.curve(e.curve).ok_or_else(not)? {
                Curve::Line(l) => Direction::Line(place.apply_unit(l.dir())),
                Curve::Circle(c) => Direction::Axis(place.apply_unit(c.frame().z())),
                Curve::Ellipse(el) => Direction::Axis(place.apply_unit(el.frame().z())),
                _ => return Err(not()),
            }
        }
        Entity::Face(f) => {
            let face = body.face(f).ok_or_else(not)?;
            let s = body.surface(face.surface).ok_or_else(not)?;
            let z = s.frame().map(|fr| place.apply_unit(fr.z()));
            match (s, z) {
                (Surface::Plane(_), Some(z)) => Direction::Normal(match face.sense {
                    Sense::Forward => z,
                    Sense::Reversed => -z,
                }),
                (Surface::Cylinder(_) | Surface::Cone(_) | Surface::Torus(_), Some(z)) => {
                    Direction::Axis(z)
                }
                _ => return Err(not()),
            }
        }
        Entity::Vertex(_) | Entity::Body => return Err(not()),
    })
}

/// The angle between `a`'s and `b`'s directions, each entity of its body
/// placed by its `Rigid`: two straight edges or axes by their directions,
/// two planes by their outward normals, a line or an axis and a plane
/// between the line and the plane (so in `[0, π/2]`). Exact up to
/// rounding. Fails with `NotAnEntity` for an entity with no direction
/// (a vertex, a sphere, a spline).
pub fn angle(a: (&Body, Entity, &Rigid), b: (&Body, Entity, &Rigid)) -> Result<Angle, Error> {
    let (da, db) = (direction(a.0, a.1, a.2)?, direction(b.0, b.1, b.2)?);
    let unit = |d: Direction| match d {
        Direction::Line(u) | Direction::Normal(u) | Direction::Axis(u) => u,
    };
    let between = |u: UnitVector3, v: UnitVector3| {
        let (u, v) = (u.get(), v.get());
        math::atan2(u.cross(v).length(), u.dot(v))
    };
    let theta = between(unit(da), unit(db));
    let angle = match (da, db) {
        (Direction::Normal(_), Direction::Normal(_)) => theta,
        (Direction::Normal(_), _) | (_, Direction::Normal(_)) => (FRAC_PI_2 - theta).abs(),
        _ => theta,
    };
    Ok(Angle {
        angle: Value::Exact(angle),
        supplement: Value::Exact(PI - angle),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vector3;
    use crate::name::OpKey;
    use crate::primitive;
    use crate::topo::FaceId;

    fn face_along(b: &Body, n: Vector3) -> FaceId {
        b.faces()
            .find(|&f| {
                let face = &b.faces[f];
                let Surface::Plane(p) = &b.surfaces[face.surface] else {
                    return false;
                };
                let z = match face.sense {
                    Sense::Forward => p.frame().z(),
                    Sense::Reversed => -p.frame().z(),
                };
                z.get().dot(n) > 0.999
            })
            .unwrap()
    }

    #[test]
    fn faces_of_a_box_and_a_turned_one() {
        let (b, _) = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        let (x, y) = (
            face_along(&b, Vector3::new(1.0, 0.0, 0.0)),
            face_along(&b, Vector3::new(0.0, 1.0, 0.0)),
        );
        let id = Rigid::IDENTITY;
        let a = angle((&b, Entity::Face(x), &id), (&b, Entity::Face(y), &id)).unwrap();
        assert!((a.angle.value() - FRAC_PI_2).abs() < 1e-15);
        let mx = face_along(&b, Vector3::new(-1.0, 0.0, 0.0));
        let a = angle((&b, Entity::Face(x), &id), (&b, Entity::Face(mx), &id)).unwrap();
        assert!((a.angle.value() - PI).abs() < 1e-15 && a.supplement.value().abs() < 1e-15);
        let turn = Rigid::rotation(UnitVector3::Z, 0.3);
        let a = angle((&b, Entity::Face(x), &id), (&b, Entity::Face(x), &turn)).unwrap();
        assert!((a.angle.value() - 0.3).abs() < 1e-15);
    }

    #[test]
    fn a_line_against_a_plane_and_a_cylinder_axis() {
        let (b, _) = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        let top = face_along(&b, Vector3::new(0.0, 0.0, 1.0));
        let vertical = b
            .edges()
            .find(|&e| match &b.curves[b.edges[e].curve] {
                Curve::Line(l) => l.dir().get().z.abs() > 0.999,
                _ => false,
            })
            .unwrap();
        let id = Rigid::IDENTITY;
        let a = angle(
            (&b, Entity::Edge(vertical), &id),
            (&b, Entity::Face(top), &id),
        )
        .unwrap();
        assert!((a.angle.value() - FRAC_PI_2).abs() < 1e-15);
        let (c, _) = primitive::cylinder(OpKey(2), 0.5, 1.0, &id).unwrap();
        let side = c
            .faces()
            .find(|&f| matches!(c.surfaces[c.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let a = angle(
            (&b, Entity::Edge(vertical), &id),
            (&c, Entity::Face(side), &id),
        )
        .unwrap();
        assert!(a.angle.value() < 1e-15 || (a.angle.value() - PI).abs() < 1e-15);
        assert_eq!(
            angle(
                (&b, Entity::Vertex(b.vertices().next().unwrap()), &id),
                (&c, Entity::Face(side), &id)
            ),
            Err(MeasureError::NotAnEntity.into())
        );
    }
}
