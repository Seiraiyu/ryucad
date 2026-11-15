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
    /// the checker's rules. An unbounded carrier's origin the move takes
    /// out of the box slides back first ([`Body::slide_carriers`]).
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
        b.slide_carriers();
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
    /// leaves the size box. An unbounded carrier's origin the reflection
    /// takes out of the box slides back first ([`Body::slide_carriers`]).
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
        b.slide_carriers();
        if crate::check::outside_size_box(&b) {
            return Err(Error::OutsideSizeBox);
        }
        Ok(b)
    }

    /// #800: a line's, plane's or cylinder's origin may lie anywhere along
    /// it, and files put some far from the body; ryustep slides those
    /// outside the size box to the foot of the world origin as it reads
    /// them, but keeps one inside the box, which a move can then carry out
    /// while every point the body uses stays in. Each such origin outside
    /// the box slides back to the foot of the world origin, and the
    /// parameters move with it: a line's edges' intervals and fins' maps,
    /// a plane's or cylinder's pcurves (a copy for each surface). Every
    /// point the body evaluates is where it was, to rounding. An origin
    /// inside the box keeps its bits, so nothing that stood before moves;
    /// one whose foot is outside too stays, and the body is refused.
    fn slide_carriers(&mut self) {
        use super::{CurveId, SurfaceId};
        use crate::geom::{Curve, Cylinder, Frame, Line, Point3, Vector2};
        use crate::tol::in_size_box;
        // A line is o + t d: at o + s d its edges run at t - s.
        let mut lines: BTreeMap<CurveId, f64> = BTreeMap::new();
        let mut slid: Vec<(CurveId, Line)> = Vec::new();
        for (id, c) in self.curves.live() {
            let Curve::Line(l) = c else { continue };
            if in_size_box(l.origin()) {
                continue;
            }
            let d = l.dir().get();
            let s = -l.origin().to_vector().dot(d);
            if let Ok(near) = Line::from_unit(l.origin() + d * s, l.dir()) {
                lines.insert(id, s);
                slid.push((id, near));
            }
        }
        for (id, l) in slid {
            self.curves[id] = Curve::Line(l);
        }
        if !lines.is_empty() {
            for e in self.edges.values_mut() {
                if let Some(&s) = lines.get(&e.curve) {
                    e.interval = [e.interval[0] - s, e.interval[1] - s];
                }
            }
            for f in self.fins.values_mut() {
                let s = lines.get(&self.edges[f.edge].curve).copied();
                if let (Some(s), Some(p)) = (s, f.pcurve.as_mut()) {
                    p.map[0] += p.map[1] * s;
                }
            }
        }
        // A plane's (u, v) are along its x and y, a cylinder's v along its
        // axis: an origin moved by m moves them by -m along those.
        let mut shifts: BTreeMap<SurfaceId, Vector2> = BTreeMap::new();
        let mut moved: Vec<(SurfaceId, Surface)> = Vec::new();
        for (id, s) in self.surfaces.live() {
            let radius = match s {
                Surface::Plane(_) => None,
                Surface::Cylinder(c) => Some(c.radius()),
                _ => continue,
            };
            let Some(f) = s.frame() else { continue };
            let o = f.origin();
            if in_size_box(o) {
                continue;
            }
            let (z, ov) = (f.z().get(), o.to_vector());
            let foot = match radius {
                None => z * ov.dot(z),
                Some(_) => ov - z * ov.dot(z),
            };
            let Ok(q) = Point3::try_new(foot.x, foot.y, foot.z) else {
                continue;
            };
            let Ok(frame) = Frame::from_axes(q, f.z(), f.x()) else {
                continue;
            };
            let m = q - o;
            let (shift, surface) = match radius {
                None => (
                    Vector2::new(-m.dot(f.x().get()), -m.dot(f.y().get())),
                    Surface::Plane(Plane::new(frame)),
                ),
                Some(r) => match Cylinder::new(frame, r) {
                    Ok(c) => (Vector2::new(0.0, -m.dot(z)), Surface::Cylinder(c)),
                    Err(_) => continue,
                },
            };
            shifts.insert(id, shift);
            moved.push((id, surface));
        }
        for (id, s) in moved {
            self.surfaces[id] = s;
        }
        if shifts.is_empty() {
            return;
        }
        let mut copies: BTreeMap<(Curve2Id, SurfaceId), Curve2Id> = BTreeMap::new();
        let fins: Vec<FinId> = self.fins.live().map(|(f, _)| f).collect();
        for f in fins {
            let surface = self.faces[self.loops[self.fins[f].loop_].face].surface;
            let Some(&d) = shifts.get(&surface) else {
                continue;
            };
            let Some(p) = self.fins[f].pcurve else {
                continue;
            };
            let c = match copies.get(&(p.curve, surface)) {
                Some(&c) => c,
                None => {
                    let c = self.curves2.push(self.curves2[p.curve].shifted(d));
                    copies.insert((p.curve, surface), c);
                    c
                }
            };
            if let Some(pc) = self.fins[f].pcurve.as_mut() {
                pc.curve = c;
            }
        }
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

    /// #800: a line's origin a file put far along it, inside the box,
    /// which a move carries out of it, slides back to the foot of the
    /// world origin, its edges' parameters with it: the body stands where
    /// the move puts it, sound, every point where it was.
    #[test]
    fn far_carriers_a_move_carries_out_of_the_box_slide_back() {
        use crate::geom::{Curve, Line};
        use crate::topo::CurveId;
        let (b, _) = primitive::block(OpKey(1), 0.03, 0.02, 0.01, &Rigid::IDENTITY).unwrap();
        // Every line's origin 499.9 m back along it, so each sits near
        // -499.9 m on its axis; intervals and pcurve maps follow.
        let mut far = b.clone();
        let mut shift: BTreeMap<CurveId, f64> = BTreeMap::new();
        for (id, c) in b.curves.live() {
            if let Curve::Line(l) = c {
                let d = l.dir().get();
                let k = if d.x + d.y + d.z > 0.0 { -499.9 } else { 499.9 };
                far.curves[id] = Curve::Line(Line::from_unit(l.origin() + d * k, l.dir()).unwrap());
                shift.insert(id, k);
            }
        }
        for e in far.edges.values_mut() {
            if let Some(&k) = shift.get(&e.curve) {
                e.interval = [e.interval[0] - k, e.interval[1] - k];
            }
        }
        for f in far.fins.values_mut() {
            let c = b.edges[f.edge].curve;
            if let (Some(&k), Some(p)) = (shift.get(&c), f.pcurve.as_mut()) {
                p.map[0] += p.map[1] * k;
            }
        }
        assert!(!shift.is_empty());
        assert_eq!(check(&far), []);
        let r = Rigid::translation(Vector3::new(-0.2, -0.2, -0.2));
        let moved = far
            .transformed(&r)
            .expect("the body stays well inside the box");
        assert_eq!(check(&moved), []);
        let plain = b.transformed(&r).unwrap();
        for e in plain.edges() {
            let (x, y) = (plain.edge(e).unwrap(), moved.edge(e).unwrap());
            let mid = |b: &Body, ed: &crate::topo::Edge| {
                let [t0, t1] = ed.interval;
                b.curve(ed.curve).unwrap().point(0.5 * (t0 + t1))
            };
            assert!(mid(&plain, x).distance(mid(&moved, y)) < 1e-9, "{e:?}");
        }
        for v in plain.vertices() {
            let (p, q) = (
                plain.vertex(v).unwrap().point,
                moved.vertex(v).unwrap().point,
            );
            assert!(p.distance(q) < 1e-12);
        }
        assert!(moved.curves.live().all(|(_, c)| match c {
            Curve::Line(l) => crate::tol::in_size_box(l.origin()),
            _ => true,
        }));
    }
}
