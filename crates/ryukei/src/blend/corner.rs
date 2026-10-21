//! Corners: three blended edges meeting at a vertex of three edges.
//!
//! Fillets of one radius that all bend the same way meet in a ball
//! corner: the ball touching all three faces, whose centre lies on all
//! three spines. Each fillet ends at the section of that ball in the
//! plane normal to its spine, and the corner face is the sphere of the
//! ball between the three sections, a triangle round the point the ball
//! takes away from the vertex.

use super::chain::{Chain, Class};
use super::convex::{Bend, BendFail, bend};
use super::spec::Spec;
use super::spine::solve3;
use crate::geom::{Frame, Point3, Sphere, Surface, Vector3};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId, Sense, VertexId};
use crate::{BlendError, Error};
use rayon::prelude::*;
use std::collections::BTreeSet;

/// A ball corner.
#[derive(Clone, Debug)]
pub(crate) struct BallCorner {
    pub(crate) vertex: VertexId,
    /// The blended edges there, by their indices into the specs, in name
    /// order.
    pub(crate) edges: [usize; 3],
    /// The faces there, in name order.
    pub(crate) faces: [FaceId; 3],
    pub(crate) radius: f64,
    /// The centre of the ball touching all three faces.
    pub(crate) centre: Point3,
    /// Where it touches each face, in the order of `faces`.
    pub(crate) feet: [Point3; 3],
    /// The sphere of the ball, its seam and poles away from the corner,
    /// and the sense of the corner face on it.
    pub(crate) surface: Surface,
    pub(crate) sense: Sense,
}

impl BallCorner {
    /// Where the ball touches `face`.
    pub(crate) fn foot(&self, face: FaceId) -> Point3 {
        let k = self.faces.iter().position(|&f| f == face);
        self.feet[k.expect("a face of the corner")]
    }
}

/// Every corner of the chains, in order of their vertices' names.
pub(crate) fn corners(
    body: &Body,
    specs: &[Spec],
    chains: &[Chain],
) -> Result<Vec<BallCorner>, Error> {
    let mut vertices: Vec<VertexId> = chains
        .iter()
        .flat_map(|c| {
            c.vertices
                .iter()
                .zip(&c.classes)
                .filter(|(_, class)| matches!(class, Class::Corner))
                .map(|(&v, _)| v)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    vertices.sort_by_key(|&v| body.vertices[v].name);
    vertices
        .par_iter()
        .map(|&v| corner(body, specs, v))
        .collect()
}

/// The ball corner at `v`.
fn corner(body: &Body, specs: &[Spec], v: VertexId) -> Result<BallCorner, Error> {
    let at = body.vertices[v].name;
    let mut edges: Vec<usize> = specs
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let e = &body.edges[s.edge];
            e.start == v || e.end == v
        })
        .map(|(k, _)| k)
        .collect();
    edges.sort_by_key(|&k| specs[k].name);
    let [a, b, c] = edges[..] else {
        return Err(BlendError::Unsupported { at }.into());
    };
    let mut faces: Vec<FaceId> = [a, b, c].iter().flat_map(|&k| specs[k].faces).collect();
    faces.sort_by_key(|&f| body.faces[f].name);
    faces.dedup();
    let [fa, fb, fc] = faces[..] else {
        return Err(BlendError::Unsupported { at }.into());
    };
    let faces = [fa, fb, fc];
    let radius = match specs[a].kind {
        super::BlendKind::Fillet { radius } => radius,
        super::BlendKind::Chamfer(_) => return Err(BlendError::Unsupported { at }.into()),
    };
    let convex = match bend(body, specs[a].edge) {
        Ok(Bend::Convex) => true,
        Ok(Bend::Concave) => false,
        Ok(Bend::Smooth) => {
            return Err(BlendError::SmoothEdge {
                edge: specs[a].name,
            }
            .into());
        }
        Err(BendFail::Changes) => {
            return Err(BlendError::ConvexityChanges {
                edge: specs[a].name,
            }
            .into());
        }
        Err(BendFail::Uncertain) => return Err(BlendError::Uncertain { at }.into()),
    };
    let too_large = || -> Error {
        BlendError::RadiusTooLarge {
            edge: specs[a].name,
        }
        .into()
    };
    let p = body.vertices[v].point;
    let mut offsets = Vec::with_capacity(3);
    let mut rows = [Vector3::ZERO; 3];
    let mut rhs = [0.0; 3];
    for (k, &f) in faces.iter().enumerate() {
        let face = &body.faces[f];
        let s = &body.surfaces[face.surface];
        let outward = if face.sense == Sense::Forward {
            1.0
        } else {
            -1.0
        };
        let d = if convex { -radius } else { radius } * outward;
        let off = s
            .offset(d)
            .ok_or(BlendError::Unsupported { at: face.name })?
            .map_err(|_| too_large())?;
        offsets.push(off);
        // The face's tangent plane at the vertex, moved by the radius.
        let (u, w) = s.project(p).param;
        let n = s.normal(u, w).ok_or_else(too_large)?.get() * outward;
        rows[k] = n;
        rhs[k] = n.dot(p.to_vector()) + if convex { -radius } else { radius };
    }
    let start = solve3(rows, rhs).ok_or(BlendError::Uncertain { at })?;
    let centre = centre(&offsets, start, radius.max(p.to_vector().length()).max(1.0))
        .ok_or_else(too_large)?;
    let feet = faces.map(|f| body.surfaces[body.faces[f].surface].project(centre).point);
    let surface = sphere(centre, radius, &feet).ok_or(BlendError::Uncertain { at })?;
    Ok(BallCorner {
        vertex: v,
        edges: [a, b, c],
        faces,
        radius,
        centre,
        feet,
        surface,
        sense: if convex {
            Sense::Forward
        } else {
            Sense::Reversed
        },
    })
}

/// The point on all three offsets, by Newton's method on their tangent
/// planes from `start`. `None` if it does not settle.
fn centre(offsets: &[Surface], start: Point3, scale: f64) -> Option<Point3> {
    let mut c = start;
    for _ in 0..50 {
        let mut rows = [Vector3::ZERO; 3];
        let mut rhs = [0.0; 3];
        for (k, off) in offsets.iter().enumerate() {
            let q = off.project(c);
            let (u, v) = q.param;
            let m = off.normal(u, v)?.get();
            rows[k] = m;
            rhs[k] = m.dot(q.point.to_vector());
        }
        let next = solve3(rows, rhs)?;
        let step = next.distance(c);
        c = next;
        if step <= 1e-15 * scale {
            break;
        }
    }
    offsets
        .iter()
        .all(|o| o.distance(c) <= LINEAR_RESOLUTION)
        .then_some(c)
}

/// The sphere about `centre` through the feet, with its seam opposite
/// them and its poles square to the direction they lie in.
fn sphere(centre: Point3, radius: f64, feet: &[Point3; 3]) -> Option<Surface> {
    let m = feet
        .iter()
        .fold(Vector3::ZERO, |acc, &f| acc + (f - centre))
        .normalize()?
        .get();
    let z = (feet[0] - centre).cross(m);
    let z = if z.length() > 1e-6 * radius {
        z
    } else {
        m.cross(Vector3::new(1.0, 0.0, 0.0))
    };
    let frame = Frame::new(centre, z, -m).ok()?;
    Some(Surface::Sphere(Sphere::new(frame, radius).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::chain::chains;
    use crate::blend::spec::resolve;
    use crate::blend::tests::{block, edge_of};
    use crate::blend::{BlendKind, EdgeBlend};
    use crate::name::FaceRole;

    #[test]
    fn a_box_corner_is_a_ball_touching_three_faces() {
        let b = block();
        let r = 0.2;
        let blends: Vec<EdgeBlend> = [
            (FaceRole::PlusX, FaceRole::PlusZ),
            (FaceRole::PlusY, FaceRole::PlusZ),
            (FaceRole::PlusX, FaceRole::PlusY),
        ]
        .iter()
        .map(|&(x, y)| EdgeBlend {
            edge: edge_of(&b, x, y),
            kind: BlendKind::Fillet { radius: r },
        })
        .collect();
        let specs = resolve(&b, &blends).unwrap();
        let ch = chains(&b, &specs).unwrap();
        let c = corners(&b, &specs, &ch).unwrap();
        assert_eq!(c.len(), 1);
        let c = &c[0];
        assert!(c.centre.distance(Point3::new(0.8, 0.8, 0.8)) < 1e-15);
        for f in c.feet {
            assert!((f.distance(c.centre) - r).abs() < 1e-15);
        }
        // The sphere's seam is behind the corner.
        assert!(matches!(c.surface, Surface::Sphere(_)));
        let x = c.surface.frame().unwrap().x().get();
        assert!(x.dot(Vector3::new(1.0, 1.0, 1.0)) < -1.7);
    }
}
