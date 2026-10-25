//! Moving and mirroring a body.

use super::{Body, Curve2Id, FinId};
use crate::Error;
use crate::geom::{Mirror, ParamFlip, Plane, Rigid, Surface};
use std::collections::BTreeMap;

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

    /// Its mirror image in `plane`: every vertex, curve and surface
    /// reflected, each curve keeping its parameters ([`Curve::mirrored`])
    /// and each surface mapping its own as [`Surface::mirrored`] says,
    /// with every pcurve mapped to match. A reflection turns the faces'
    /// loops clockwise, so every loop runs the other way, and a face whose
    /// surface keeps its parameters (a NURBS or an extrusion) takes the
    /// other sense, so every face still points out of the solid. Names and
    /// tolerances are unchanged. Fails with `OutsideSizeBox` when anything
    /// leaves the size box.
    ///
    /// [`Curve::mirrored`]: crate::geom::Curve::mirrored
    pub fn mirrored(&self, plane: &Plane) -> Result<Body, Error> {
        let m = Mirror::in_plane_of(plane.frame());
        let mut b = self.clone();
        for v in b.vertices.values_mut() {
            v.point = m.apply_point(v.point);
        }
        for c in b.curves.values_mut() {
            *c = c.mirrored(&m)?;
        }
        let mut flips: Vec<ParamFlip> = vec![ParamFlip::Kept; b.surfaces.slot_count()];
        for (id, s) in self.surfaces.live() {
            let (mirrored, flip): (Surface, ParamFlip) = s.mirrored(&m)?;
            b.surfaces[id] = mirrored;
            flips[id.index()] = flip;
        }
        // A pcurve shared by faces whose surfaces map alike is mapped
        // once; one shared across maps is copied for each map but the
        // first.
        let mut mapped: BTreeMap<Curve2Id, ParamFlip> = BTreeMap::new();
        let mut copies: BTreeMap<(Curve2Id, u8), Curve2Id> = BTreeMap::new();
        let fins: Vec<FinId> = b.fins.live().map(|(f, _)| f).collect();
        for f in fins {
            let face = b.loops[b.fins[f].loop_].face;
            let flip = flips[b.faces[face].surface.index()];
            let Some(p) = b.fins[f].pcurve else { continue };
            let kind = match flip {
                ParamFlip::Kept => 0,
                ParamFlip::U(_) => 1,
                ParamFlip::V(_) => 2,
            };
            let curve = match mapped.get(&p.curve) {
                None => {
                    let c = self.curves2[p.curve].mirrored(flip);
                    b.curves2[p.curve] = c;
                    mapped.insert(p.curve, flip);
                    p.curve
                }
                Some(done) if *done == flip => p.curve,
                Some(_) => *copies.entry((p.curve, kind)).or_insert_with(|| {
                    let c = self.curves2[p.curve].mirrored(flip);
                    b.curves2.push(c)
                }),
            };
            if let Some(pc) = b.fins[f].pcurve.as_mut() {
                pc.curve = curve;
            }
        }
        // Every loop runs the other way: its fins in reverse order, each
        // against its edge as it ran with it. Each ring of fins round an
        // edge is reversed too, so the radial order stays consistent.
        for fin in b.fins.values_mut() {
            core::mem::swap(&mut fin.next, &mut fin.prev);
            fin.sense = fin.sense.flipped();
        }
        let edges: Vec<_> = b.edges.live().map(|(e, x)| (e, x.fin)).collect();
        for (_, first) in edges {
            let mut ring = vec![first];
            let mut f = self.fins[first].around;
            while f != first {
                ring.push(f);
                f = self.fins[f].around;
            }
            for (i, &f) in ring.iter().enumerate() {
                b.fins[f].around = ring[(i + ring.len() - 1) % ring.len()];
            }
        }
        let faces: Vec<_> = b.faces.live().map(|(f, x)| (f, x.surface)).collect();
        for (f, s) in faces {
            if flips[s.index()].flips_normal() {
                b.faces[f].sense = b.faces[f].sense.flipped();
            }
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
