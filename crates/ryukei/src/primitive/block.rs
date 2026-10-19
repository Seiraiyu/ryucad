use super::{check_extent, name_by_role, size};
use crate::Error;
use crate::euler::{Corner, mef, mev, mvfs, set_surface};
use crate::geom::{Curve, Frame, Line, Plane, Point3, Rigid, Surface, Vector3};
use crate::name::{FaceRole, Journal, OpKey};
use crate::topo::{Body, Sense};

/// A box spanning `[0, dx] × [0, dy] × [0, dz]` in its local frame, moved
/// by `placement`: 8 vertices, 12 edges, 6 planar faces named `+X` to `-Z`
/// by their outward normals.
pub fn block(
    key: OpKey,
    dx: f64,
    dy: f64,
    dz: f64,
    placement: &Rigid,
) -> Result<(Body, Journal), Error> {
    let (dx, dy, dz) = (size(dx)?, size(dy)?, size(dz)?);
    check_extent(placement, Point3::ORIGIN, Point3::new(dx, dy, dz))?;
    let local = [
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(dx, 0.0, 0.0),
        Point3::new(dx, dy, 0.0),
        Point3::new(0.0, dy, 0.0),
        Point3::new(0.0, 0.0, dz),
        Point3::new(dx, 0.0, dz),
        Point3::new(dx, dy, dz),
        Point3::new(0.0, dy, dz),
    ];
    let p = local.map(|q| placement.apply_point(q));
    let mut b = Body::new();
    let plane = |b: &mut Body, origin: usize, z: Vector3, x: Vector3| -> Result<_, Error> {
        let frame = Frame::new(local[origin], z, x)?;
        Ok(b.add_surface(Surface::Plane(Plane::new(frame)).transformed(placement)))
    };
    let (ex, ey, ez) = (
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    );
    let planes = [
        (plane(&mut b, 1, ex, ey)?, FaceRole::PlusX),
        (plane(&mut b, 0, -ex, ey)?, FaceRole::MinusX),
        (plane(&mut b, 3, ey, ex)?, FaceRole::PlusY),
        (plane(&mut b, 0, -ey, ex)?, FaceRole::MinusY),
        (plane(&mut b, 4, ez, ex)?, FaceRole::PlusZ),
        (plane(&mut b, 0, -ez, ex)?, FaceRole::MinusZ),
    ];
    let edge = |b: &mut Body, i: usize, j: usize| -> Result<_, Error> {
        let c = b.add_curve(Curve::Line(Line::through(p[i], p[j])?));
        Ok((c, [0.0, p[i].distance(p[j])]))
    };
    // The bottom square, as a path closed by MEF: the face against the
    // closing edge winds clockwise seen from above, so it is the bottom.
    let (m, _) = mvfs(&mut b, key, p[0], planes[5].0)?;
    let (c, t) = edge(&mut b, 0, 1)?;
    let (e01, _) = mev(&mut b, key, Corner::Lone(m.loop_), p[1], c, t)?;
    let (c, t) = edge(&mut b, 1, 2)?;
    let (e12, _) = mev(&mut b, key, Corner::Before(e01.fins[1]), p[2], c, t)?;
    let (c, t) = edge(&mut b, 2, 3)?;
    let (e23, _) = mev(&mut b, key, Corner::Before(e12.fins[1]), p[3], c, t)?;
    let (c, t) = edge(&mut b, 3, 0)?;
    let from = Corner::Before(e23.fins[1]);
    let (base, _) = mef(&mut b, key, from, Corner::Before(e01.fins[0]), c, t)?;
    let bottom = base.faces[1];
    // The four uprights, from the corners of the face above the bottom.
    let mut up = Vec::new();
    for (i, at) in [e01.fins[0], e12.fins[0], e23.fins[0], base.fins[0]]
        .into_iter()
        .enumerate()
    {
        let (c, t) = edge(&mut b, i, i + 4)?;
        let (u, _) = mev(&mut b, key, Corner::Before(at), p[i + 4], c, t)?;
        up.push(u);
    }
    // Round the top, from each upright's top to the next: each MEF cuts
    // off a side face against its edge, leaving the top along the last.
    // After a full turn the first top edge starts at the first upright.
    let mut sides: Vec<crate::euler::Mef> = Vec::new();
    for i in 0..4 {
        let j = (i + 1) % 4;
        let to = if j == 0 {
            sides[0].fins[0]
        } else {
            up[j].fins[1]
        };
        let (c, t) = edge(&mut b, i + 4, j + 4)?;
        let from = Corner::Before(up[i].fins[1]);
        let (m, _) = mef(&mut b, key, from, Corner::Before(to), c, t)?;
        sides.push(m);
    }
    let top = sides[3].faces[0];
    let faces = [
        (sides[1].faces[1], 0),
        (sides[3].faces[1], 1),
        (sides[2].faces[1], 2),
        (sides[0].faces[1], 3),
        (top, 4),
        (bottom, 5),
    ];
    let mut roles = Vec::new();
    for (f, k) in faces {
        set_surface(&mut b, f, planes[k].0, Sense::Forward)?;
        roles.push((f, planes[k].1));
    }
    let journal = name_by_role(&mut b, "block", key, &roles)?;
    Ok((b, journal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::{FaceRole, Name, Role};
    use crate::primitive::tests::{K, solid, sound, tilted};
    use crate::topo::EntityId;

    #[test]
    fn a_box_is_sound() {
        let b = sound(|k, r| block(k, 1.0, 2.0, 3.0, r), solid(8, 12, 6, 6));
        for (role, normal) in [
            (FaceRole::PlusX, Vector3::new(1.0, 0.0, 0.0)),
            (FaceRole::MinusY, Vector3::new(0.0, -1.0, 0.0)),
            (FaceRole::PlusZ, Vector3::new(0.0, 0.0, 1.0)),
        ] {
            let name = Name::derive(K, Role::Face(role), &[]);
            let Some(EntityId::Face(f)) = b.by_name(name) else {
                panic!("no {role:?} face");
            };
            let face = b.face(f).unwrap();
            let n = b
                .surface(face.surface)
                .unwrap()
                .normal(0.0, 0.0)
                .unwrap()
                .get();
            assert_eq!(n, normal);
            assert_eq!(
                b.recipe(name).map(|r| r.starts_with("block#2a/")),
                Some(true)
            );
        }
    }

    #[test]
    fn corners_are_named_by_their_faces() {
        let (b, _) = block(K, 1.0, 2.0, 3.0, &tilted()).unwrap();
        let face = |r| Name::derive(K, Role::Face(r), &[]);
        let mut three = [
            face(FaceRole::PlusX),
            face(FaceRole::PlusY),
            face(FaceRole::PlusZ),
        ];
        three.sort();
        let corner = Name::derive(K, Role::Vertex, &three);
        let Some(EntityId::Vertex(v)) = b.by_name(corner) else {
            panic!("no +X+Y+Z corner");
        };
        let expected = tilted().apply_point(Point3::new(1.0, 2.0, 3.0));
        assert!(b.vertex(v).unwrap().point.distance(expected) <= 1e-13);
        let mut two = [face(FaceRole::MinusX), face(FaceRole::MinusZ)];
        two.sort();
        let edge = Name::derive(K, Role::Edge, &two);
        let Some(EntityId::Edge(e)) = b.by_name(edge) else {
            panic!("no -X-Z edge");
        };
        let e = b.edge(e).unwrap();
        assert!((e.interval[1] - 2.0).abs() <= 1e-13);
    }

    #[test]
    fn bad_dimensions_and_placements() {
        let id = Rigid::IDENTITY;
        assert_eq!(block(K, 0.0, 1.0, 1.0, &id).err(), Some(Error::Degenerate));
        assert_eq!(
            block(K, 1.0, f64::NAN, 1.0, &id).err(),
            Some(Error::NonFinite)
        );
        assert_eq!(
            block(K, 1.0, 1.0, 600.0, &id).err(),
            Some(Error::OutsideSizeBox)
        );
        let far = Rigid::translation(Vector3::new(-500.5, 0.0, 0.0));
        assert_eq!(
            block(K, 1.0, 1.0, 1.0, &far).err(),
            Some(Error::OutsideSizeBox)
        );
    }
}
