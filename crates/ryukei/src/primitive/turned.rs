use super::{check_extent, name_by_role, size};
use crate::Error;
use crate::euler::{Corner, mef, mvfs, mvr, set_surface};
use crate::geom::{Circle, Cone, Curve, Cylinder, Frame, Plane, Point3, Rigid, Surface, Vector3};
use crate::math;
use crate::name::{FaceRole, Journal, OpKey};
use crate::topo::{Body, Sense};
use core::f64::consts::TAU;

/// A cylinder of radius `r` and height `h`: in its local frame, the axis is
/// `+z` from the base centre at the origin. 2 vertices, 2 circular edges, 3
/// faces: `side`, `bottom`, `top`. Each vertex is at angle 0 (local `+x`).
pub fn cylinder(key: OpKey, r: f64, h: f64, placement: &Rigid) -> Result<(Body, Journal), Error> {
    let (r, h) = (size(r)?, size(h)?);
    let side = Surface::Cylinder(Cylinder::new(Frame::WORLD, r)?);
    turned(key, "cylinder", side, r, Some(r), h, placement)
}

/// A cone frustum from radius `r0` at the base to `r1` at height `h`. The
/// radii must differ by enough for a cone half-angle (otherwise
/// `Degenerate`: use a cylinder). Topology and names as [`cylinder`].
pub fn frustum(
    key: OpKey,
    r0: f64,
    r1: f64,
    h: f64,
    placement: &Rigid,
) -> Result<(Body, Journal), Error> {
    let (r0, r1, h) = (size(r0)?, size(r1)?, size(h)?);
    // The cone widens away from its frame: upwards when r1 > r0, else
    // downwards from the base.
    let z = if r1 > r0 { 1.0 } else { -1.0 };
    let frame = Frame::new(
        Point3::ORIGIN,
        Vector3::new(0.0, 0.0, z),
        Vector3::new(1.0, 0.0, 0.0),
    )?;
    let half_angle = math::atan2((r1 - r0).abs(), h);
    let side = Surface::Cone(Cone::new(frame, r0, half_angle)?);
    turned(key, "frustum", side, r0, Some(r1), h, placement)
}

/// A pointed cone of base radius `r` with its apex at height `h`. 2
/// vertices (the apex is a lone vertex of the side), 1 circular edge, 2
/// faces: `side` and `bottom`.
pub fn cone(key: OpKey, r: f64, h: f64, placement: &Rigid) -> Result<(Body, Journal), Error> {
    let (r, h) = (size(r)?, size(h)?);
    let frame = Frame::new(
        Point3::ORIGIN,
        Vector3::new(0.0, 0.0, -1.0),
        Vector3::new(1.0, 0.0, 0.0),
    )?;
    let side = Surface::Cone(Cone::new(frame, r, math::atan2(r, h))?);
    turned(key, "cone", side, r, None, h, placement)
}

/// A solid turned about local `+z` from 0 to `h`: the side on `side`, a
/// base disk of radius `r0`, and a top disk of radius `r1` or an apex.
///
/// The base circle runs clockwise seen from above and the top circle
/// counterclockwise, so each disk (the new face, along its circle) winds
/// counterclockwise about its outward normal, and the side keeps the
/// reverse fins.
fn turned(
    key: OpKey,
    kind: &str,
    side: Surface,
    r0: f64,
    r1: Option<f64>,
    h: f64,
    placement: &Rigid,
) -> Result<(Body, Journal), Error> {
    let r = r0.max(r1.unwrap_or(0.0));
    check_extent(placement, Point3::new(-r, -r, 0.0), Point3::new(r, r, h))?;
    let ex = Vector3::new(1.0, 0.0, 0.0);
    let ez = Vector3::new(0.0, 0.0, 1.0);
    let base_frame = Frame::new(Point3::ORIGIN, -ez, ex)?;
    let top_frame = Frame::new(Point3::new(0.0, 0.0, h), ez, ex)?;
    let mut b = Body::new();
    let side = b.add_surface(side.transformed(placement));
    let base_plane = b.add_surface(Surface::Plane(Plane::new(base_frame)).transformed(placement));
    let top_plane = b.add_surface(Surface::Plane(Plane::new(top_frame)).transformed(placement));
    let at = |x: f64, z: f64| placement.apply_point(Point3::new(x, 0.0, z));
    let circle = |b: &mut Body, frame: Frame, radius: f64| -> Result<_, Error> {
        let c = Curve::Circle(Circle::new(frame, radius)?).transformed(placement);
        Ok(b.add_curve(c))
    };
    let base_circle = circle(&mut b, base_frame, r0)?;
    let top_circle = match r1 {
        Some(r1) => Some(circle(&mut b, top_frame, r1)?),
        None => None,
    };
    // Start from the apex, or from the base vertex.
    let start = if r1.is_some() {
        at(r0, 0.0)
    } else {
        at(0.0, h)
    };
    let (m, _) = mvfs(&mut b, key, start, side)?;
    let base_loop = match r1 {
        Some(_) => m.loop_,
        None => mvr(&mut b, key, m.face, at(r0, 0.0))?.0.loop_,
    };
    let lone = Corner::Lone(base_loop);
    let (base, _) = mef(&mut b, key, lone, lone, base_circle, [0.0, TAU])?;
    set_surface(&mut b, base.faces[0], base_plane, Sense::Forward)?;
    let mut roles = vec![(m.face, FaceRole::Side), (base.faces[0], FaceRole::Bottom)];
    if let (Some(r1), Some(c)) = (r1, top_circle) {
        let (ring, _) = mvr(&mut b, key, m.face, at(r1, h))?;
        let lone = Corner::Lone(ring.loop_);
        let (top, _) = mef(&mut b, key, lone, lone, c, [0.0, TAU])?;
        set_surface(&mut b, top.faces[0], top_plane, Sense::Forward)?;
        roles.push((top.faces[0], FaceRole::Top));
    }
    let journal = name_by_role(&mut b, kind, key, &roles);
    Ok((b, journal))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::primitive::tests::{K, solid, sound, tilted};
    use crate::topo::FaceId;

    /// The face's outward normal at a point on it.
    fn face_normal(b: &Body, f: FaceId, p: Point3) -> Vector3 {
        let face = b.face(f).unwrap();
        let s = b.surface(face.surface).unwrap();
        let (u, v) = s.params_of(p);
        let n = s.normal(u, v).get();
        if face.sense == Sense::Forward { n } else { -n }
    }

    /// Orientation for bodies turned about the placement's `z` axis, which
    /// the checker can only decide for planar bodies: every face normal
    /// points away from the solid, and every fin has its face on its left
    /// (towards a disk's centre, or along the side away from its circle).
    pub(crate) fn outward(b: &Body, placement: &Rigid, h: f64) {
        let axis = placement.apply_vector(Vector3::new(0.0, 0.0, 1.0));
        let centre = placement.apply_point(Point3::ORIGIN);
        let height = |p: Point3| (p - centre).dot(axis);
        for fin in b.fins() {
            let f = b.fin(fin).unwrap();
            let e = b.edge(f.edge).unwrap();
            let curve = b.curve(e.curve).unwrap();
            let t = (e.interval[0] + e.interval[1]) / 2.0;
            let p = curve.point(t);
            let dir = curve.tangent(t).unwrap().get();
            let dir = if f.sense == Sense::Forward { dir } else { -dir };
            let face = b.loop_(f.loop_).unwrap().face;
            let n = face_normal(b, face, p);
            let left = n.cross(dir);
            let ring_centre = centre + axis * height(p);
            let is_disk = matches!(
                b.surface(b.face(face).unwrap().surface),
                Some(Surface::Plane(_))
            );
            if is_disk {
                assert!(left.dot(ring_centre - p) > 0.0, "disk fin {fin:?}");
                let up = n.dot(axis);
                assert!(if height(p) < h / 2.0 {
                    up < -0.99
                } else {
                    up > 0.99
                });
            } else {
                let away = if height(p) < h / 2.0 { 1.0 } else { -1.0 };
                assert!(left.dot(axis) * away > 0.0, "side fin {fin:?}");
                assert!(n.dot(p - ring_centre) > 0.0, "side normal at {p:?}");
            }
        }
    }

    #[test]
    fn a_cylinder_is_sound_and_outward() {
        let b = sound(|k, r| cylinder(k, 1.0, 2.0, r), solid(2, 2, 3, 4));
        outward(&b, &Rigid::IDENTITY, 2.0);
        let (b, _) = cylinder(K, 1.0, 2.0, &tilted()).unwrap();
        outward(&b, &tilted(), 2.0);
    }

    #[test]
    fn frustums_either_way_are_sound_and_outward() {
        for (r0, r1) in [(1.0, 0.5), (0.5, 1.0)] {
            let b = sound(|k, r| frustum(k, r0, r1, 2.0, r), solid(2, 2, 3, 4));
            outward(&b, &Rigid::IDENTITY, 2.0);
            let (b, _) = frustum(K, r0, r1, 2.0, &tilted()).unwrap();
            outward(&b, &tilted(), 2.0);
        }
    }

    #[test]
    fn a_pointed_cone_is_sound_and_outward() {
        let b = sound(|k, r| cone(k, 1.0, 2.0, r), solid(2, 1, 2, 3));
        outward(&b, &Rigid::IDENTITY, 2.0);
        let apex = b
            .vertices()
            .find(|&v| b.vertex(v).unwrap().lone.is_some())
            .unwrap();
        assert_eq!(b.vertex(apex).unwrap().point, Point3::new(0.0, 0.0, 2.0));
    }

    #[test]
    fn bad_dimensions() {
        let id = Rigid::IDENTITY;
        assert_eq!(cylinder(K, -1.0, 1.0, &id).err(), Some(Error::Degenerate));
        assert_eq!(
            frustum(K, 1.0, 1.0, 1.0, &id).err(),
            Some(Error::Degenerate)
        );
        assert_eq!(
            cone(K, 1.0, f64::INFINITY, &id).err(),
            Some(Error::NonFinite)
        );
        let far = Rigid::translation(Vector3::new(0.0, 499.5, 0.0));
        assert_eq!(
            cylinder(K, 1.0, 1.0, &far).err(),
            Some(Error::OutsideSizeBox)
        );
    }
}
