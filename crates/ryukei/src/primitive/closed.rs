use super::{check_extent, name_by_role, size};
use crate::Error;
use crate::euler::{kvr, mvfs};
use crate::geom::{Frame, Point3, Rigid, Sphere, Surface, Torus};
use crate::name::{FaceRole, Journal, OpKey};
use crate::topo::Body;

/// A sphere of radius `r` centred on the local origin: one face, `side`,
/// with no loops, edges or vertices.
pub fn sphere(key: OpKey, r: f64, placement: &Rigid) -> Result<(Body, Journal), Error> {
    let r = size(r)?;
    check_extent(placement, Point3::new(-r, -r, -r), Point3::new(r, r, r))?;
    let surface = Surface::Sphere(Sphere::new(Frame::WORLD, r)?);
    closed(key, "sphere", surface, Point3::new(r, 0.0, 0.0), placement)
}

/// A ring torus about local `+z`, centred on the origin, with tube radius
/// `minor` around a circle of radius `major`. `major` must exceed `minor`
/// by at least resolution: horn and spindle tori are `Degenerate`. One
/// face, `side`, with no loops, edges or vertices.
pub fn torus(
    key: OpKey,
    major: f64,
    minor: f64,
    placement: &Rigid,
) -> Result<(Body, Journal), Error> {
    let (major, minor) = (size(major)?, size(minor)?);
    let (out, r) = (major + minor, minor);
    check_extent(
        placement,
        Point3::new(-out, -out, -r),
        Point3::new(out, out, r),
    )?;
    let surface = Surface::Torus(Torus::new(Frame::WORLD, major, minor)?);
    closed(key, "torus", surface, Point3::new(out, 0.0, 0.0), placement)
}

/// A single face on a closed surface: MVFS at a point on it, then KVR
/// takes the lone vertex away.
fn closed(
    key: OpKey,
    kind: &str,
    surface: Surface,
    on: Point3,
    placement: &Rigid,
) -> Result<(Body, Journal), Error> {
    let mut b = Body::new();
    let s = b.add_surface(surface.transformed(placement));
    let (m, _) = mvfs(&mut b, key, placement.apply_point(on), s)?;
    kvr(&mut b, m.loop_)?;
    let journal = name_by_role(&mut b, kind, key, &[(m.face, FaceRole::Side)]);
    Ok((b, journal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vector3;
    use crate::primitive::tests::{K, solid, sound, tilted};
    use crate::topo::Sense;

    /// Every sampled normal of the one face points away from `inside(p)`,
    /// the nearest point of the solid's core.
    fn outward(b: &Body, inside: impl Fn(Point3) -> Point3) {
        let f = b.faces().next().unwrap();
        let face = b.face(f).unwrap();
        assert_eq!(face.sense, Sense::Forward);
        let s = b.surface(face.surface).unwrap();
        for i in 0..8 {
            for j in 0..8 {
                let (u, v) = (i as f64 * 0.8, j as f64 * 0.4 - 1.4);
                let p = s.point(u, v);
                assert!(s.normal(u, v).get().dot(p - inside(p)) > 0.0);
            }
        }
    }

    #[test]
    fn a_sphere_is_sound_and_outward() {
        sound(|k, r| sphere(k, 2.0, r), solid(0, 0, 1, 0));
        let (b, _) = sphere(K, 2.0, &tilted()).unwrap();
        let centre = tilted().apply_point(Point3::ORIGIN);
        outward(&b, |_| centre);
    }

    #[test]
    fn a_torus_is_sound_and_outward() {
        sound(|k, r| torus(k, 3.0, 1.0, r), solid(0, 0, 1, 0));
        let (b, _) = torus(K, 3.0, 1.0, &tilted()).unwrap();
        let (c, z) = (
            tilted().apply_point(Point3::ORIGIN),
            tilted().apply_vector(Vector3::new(0.0, 0.0, 1.0)),
        );
        // The tube's centre circle, nearest to p.
        outward(&b, |p| {
            let d = p - c;
            let flat = d - z * d.dot(z);
            c + flat * (3.0 / flat.length())
        });
    }

    #[test]
    fn bad_dimensions() {
        let id = Rigid::IDENTITY;
        assert_eq!(sphere(K, 0.0, &id).err(), Some(Error::Degenerate));
        assert_eq!(torus(K, 1.0, 1.0, &id).err(), Some(Error::Degenerate));
        assert_eq!(torus(K, 1.0, 2.0, &id).err(), Some(Error::Degenerate));
        assert_eq!(sphere(K, 500.5, &id).err(), Some(Error::OutsideSizeBox));
    }
}
