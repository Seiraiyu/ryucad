//! Moving a body.

use super::Body;
use crate::Error;
use crate::geom::Rigid;

impl Body {
    /// A copy moved by `r`: every vertex, curve and surface transformed;
    /// topology, pcurves, tolerances and names unchanged. A rigid motion
    /// keeps each surface's parameterization, so the pcurves still fit.
    /// Fails with `OutsideSizeBox` when anything leaves the size box, by
    /// the checker's rules.
    pub fn transformed(&self, r: &Rigid) -> Result<Body, Error> {
        let mut b = self.clone();
        for v in b.vertices.values_mut() {
            v.point = r.apply_point(v.point);
        }
        for c in b.curves.values_mut() {
            *c = c.transformed(r);
        }
        for s in b.surfaces.values_mut() {
            *s = s.transformed(r);
        }
        if crate::check::outside_size_box(&b) {
            return Err(Error::OutsideSizeBox);
        }
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::check;
    use crate::geom::{Point3, Vector3};
    use crate::mesh::{MeshParams, tessellate};
    use crate::name::OpKey;
    use crate::primitive;

    fn volume(b: &Body) -> f64 {
        let m = tessellate(b, &MeshParams::new(1e-3, 0.3)).unwrap();
        assert!(m.faults.is_empty());
        m.welded_triangles()
            .map(|t| {
                let [a, b, c] = t.map(|i| m.positions[i as usize].to_vector());
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn moved_bodies_stay_sound_with_their_names() {
        let axis = Vector3::new(1.0, -2.0, 0.5).normalize().unwrap();
        let r = Rigid::translation(Vector3::new(3.0, -1.0, 7.0))
            .then_after(&Rigid::rotation(axis, 1.1));
        let k = OpKey(4);
        for b in [
            primitive::block(k, 1.0, 2.0, 3.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            primitive::cylinder(k, 1.0, 2.0, &Rigid::IDENTITY)
                .unwrap()
                .0,
            primitive::cone(k, 1.0, 2.0, &Rigid::IDENTITY).unwrap().0,
            primitive::sphere(k, 1.0, &Rigid::IDENTITY).unwrap().0,
            primitive::torus(k, 2.0, 0.5, &Rigid::IDENTITY).unwrap().0,
        ] {
            let moved = b.transformed(&r).unwrap();
            assert_eq!(check(&moved), []);
            assert_eq!(moved.dump_unnamed().len(), b.dump_unnamed().len());
            assert_eq!(
                moved.names.keys().collect::<Vec<_>>(),
                b.names.keys().collect::<Vec<_>>()
            );
            assert!((volume(&moved) - volume(&b)).abs() < 1e-9 * volume(&b).max(1.0) + 1e-3);
            let p = b.vertices().next().map(|v| b.vertex(v).unwrap().point);
            if let Some(p) = p {
                let q = moved
                    .vertices()
                    .next()
                    .map(|v| moved.vertex(v).unwrap().point)
                    .unwrap();
                assert!(q.distance(r.apply_point(p)) < 1e-12);
            }
        }
    }

    #[test]
    fn leaving_the_size_box_is_refused() {
        let (b, _) = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        let far = Rigid::translation(Vector3::new(499.5, 0.0, 0.0));
        assert_eq!(b.transformed(&far).err(), Some(Error::OutsideSizeBox));
        let near = Rigid::translation(Vector3::new(498.0, 0.0, 0.0));
        let moved = b.transformed(&near).unwrap();
        assert!(
            moved
                .vertices()
                .all(|v| moved.vertex(v).unwrap().point.x >= 498.0 - 1e-12)
        );
        assert_eq!(Point3::ORIGIN, Point3::new(0.0, 0.0, 0.0));
    }
}
