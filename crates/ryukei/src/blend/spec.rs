//! Resolving a blend's edges: their names, their sizes, the faces either
//! side, and whether the blend can work on those faces.

use super::{BlendKind, Chamfer, EdgeBlend};
use crate::geom::Surface;
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, EntityId, FaceId, FinId, Sense};
use crate::{BlendError, Error};
use core::f64::consts::FRAC_PI_2;

/// The smallest radius or distance a blend takes.
pub(crate) const MIN_SIZE: f64 = 10.0 * LINEAR_RESOLUTION;

/// One edge to blend, resolved.
#[derive(Clone, Debug)]
pub(crate) struct Spec {
    pub(crate) edge: EdgeId,
    pub(crate) name: Name,
    pub(crate) kind: BlendKind,
    /// The edge's fins: `[0]` runs along the edge, `[1]` against it.
    pub(crate) fins: [FinId; 2],
    /// The faces of those fins.
    pub(crate) faces: [FaceId; 2],
}

/// Whether a blend may lie on a face on this surface: every one but a
/// sweep's. A sweep offsets, but its path can't be carried past its ends
/// without changing its frame, so a spine along it ends on the domain's
/// edge, where the intersection is uncertain.
pub(crate) fn supported(s: &Surface) -> bool {
    !matches!(s, Surface::Sweep(_))
}

/// The edges of `blends` in name order, each with its faces. Fails with
/// the first refusal in that order: a name that is not an edge's, an
/// edge named twice, a bad size, a chamfer naming a face the edge does
/// not bound, or a face the blend can't lie on (two pipes, or a sweep).
pub(crate) fn resolve(body: &Body, blends: &[EdgeBlend]) -> Result<Vec<Spec>, Error> {
    let mut sorted: Vec<&EdgeBlend> = blends.iter().collect();
    sorted.sort_by_key(|b| b.edge);
    let mut out: Vec<Spec> = Vec::with_capacity(sorted.len());
    for b in sorted {
        let name = b.edge;
        if out.last().is_some_and(|s| s.name == name) {
            return Err(BlendError::DuplicateEdge { edge: name }.into());
        }
        let Some(EntityId::Edge(edge)) = body.by_name(name) else {
            return Err(BlendError::NoSuchEdge { edge: name }.into());
        };
        let fins: Vec<FinId> = body.edge_fins(edge).collect();
        let face_of = |f: FinId| body.loops[body.fins[f].loop_].face;
        let [f0, f1] = fins[..] else {
            return Err(BlendError::Unsupported { at: name }.into());
        };
        let fins = if body.fins[f0].sense == Sense::Forward {
            [f0, f1]
        } else {
            [f1, f0]
        };
        let faces = fins.map(face_of);
        if faces[0] == faces[1] {
            return Err(BlendError::Unsupported { at: name }.into());
        }
        check_size(&b.kind, name)?;
        let face_names = faces.map(|f| body.faces[f].name);
        if let BlendKind::Chamfer(
            Chamfer::Distances {
                first_face: face, ..
            }
            | Chamfer::DistanceAngle { face, .. },
        ) = b.kind
            && !face_names.contains(&face)
        {
            return Err(BlendError::NotAFace { edge: name, face }.into());
        }
        let surfaces = faces.map(|f| &body.surfaces[body.faces[f].surface]);
        for k in 0..2 {
            if !supported(surfaces[k]) {
                return Err(BlendError::Unsupported { at: face_names[k] }.into());
            }
        }
        if surfaces.iter().all(|s| matches!(s, Surface::Pipe(_))) {
            return Err(BlendError::Unsupported { at: name }.into());
        }
        out.push(Spec {
            edge,
            name,
            kind: b.kind,
            fins,
            faces,
        });
    }
    Ok(out)
}

/// Fails with `BadSize` unless every size is finite and at least
/// [`MIN_SIZE`], and an angle is in `(0, π/2]`.
fn check_size(kind: &BlendKind, edge: Name) -> Result<(), Error> {
    let size = |x: f64| x.is_finite() && x >= MIN_SIZE;
    let ok = match *kind {
        BlendKind::Fillet { radius } => size(radius),
        BlendKind::Chamfer(Chamfer::Equal { distance }) => size(distance),
        BlendKind::Chamfer(Chamfer::Distances { first, second, .. }) => size(first) && size(second),
        BlendKind::Chamfer(Chamfer::DistanceAngle {
            distance, angle, ..
        }) => size(distance) && angle > 0.0 && angle <= FRAC_PI_2,
    };
    if ok {
        Ok(())
    } else {
        Err(BlendError::BadSize { edge }.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::tests::{block, edge_of, face_of};
    use crate::name::FaceRole;

    fn fillet(edge: Name, radius: f64) -> EdgeBlend {
        EdgeBlend {
            edge,
            kind: BlendKind::Fillet { radius },
        }
    }

    #[test]
    fn resolves_in_name_order_with_the_forward_fin_first() {
        let b = block();
        let e1 = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let e2 = edge_of(&b, FaceRole::MinusY, FaceRole::MinusZ);
        let specs = resolve(&b, &[fillet(e1, 0.1), fillet(e2, 0.2)]).unwrap();
        assert_eq!(specs.len(), 2);
        assert!(specs[0].name < specs[1].name);
        for s in &specs {
            assert_eq!(b.fin(s.fins[0]).unwrap().sense, Sense::Forward);
            assert_eq!(b.fin(s.fins[1]).unwrap().sense, Sense::Reversed);
            assert_eq!(b.fin(s.fins[0]).unwrap().edge, s.edge);
            assert_ne!(s.faces[0], s.faces[1]);
        }
        let s = specs.iter().find(|s| s.name == e1).unwrap();
        let mut faces = s.faces;
        faces.sort();
        let mut want = [face_of(&b, FaceRole::PlusX), face_of(&b, FaceRole::PlusZ)];
        want.sort();
        assert_eq!(faces, want);
    }

    #[test]
    fn refuses_names_that_are_not_edges_and_duplicates() {
        let b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let face = b.faces[face_of(&b, FaceRole::PlusX)].name;
        let ghost = Name::from_u128(7);
        assert_eq!(
            resolve(&b, &[fillet(ghost, 0.1)]).unwrap_err(),
            BlendError::NoSuchEdge { edge: ghost }.into()
        );
        assert_eq!(
            resolve(&b, &[fillet(face, 0.1)]).unwrap_err(),
            BlendError::NoSuchEdge { edge: face }.into()
        );
        assert_eq!(
            resolve(&b, &[fillet(e, 0.1), fillet(e, 0.2)]).unwrap_err(),
            BlendError::DuplicateEdge { edge: e }.into()
        );
    }

    #[test]
    fn refuses_bad_sizes() {
        let b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let bad = BlendError::BadSize { edge: e }.into();
        for r in [0.0, 5e-8, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(resolve(&b, &[fillet(e, r)]).unwrap_err(), bad);
        }
        assert!(resolve(&b, &[fillet(e, MIN_SIZE)]).is_ok());
        let face = b.faces[face_of(&b, FaceRole::PlusX)].name;
        let angle = |angle: f64| EdgeBlend {
            edge: e,
            kind: BlendKind::Chamfer(Chamfer::DistanceAngle {
                distance: 0.1,
                angle,
                face,
            }),
        };
        for a in [0.0, -0.1, FRAC_PI_2 + 1e-9, f64::NAN] {
            assert_eq!(resolve(&b, &[angle(a)]).unwrap_err(), bad);
        }
        assert!(resolve(&b, &[angle(FRAC_PI_2)]).is_ok());
        let two = EdgeBlend {
            edge: e,
            kind: BlendKind::Chamfer(Chamfer::Distances {
                first: 0.1,
                second: 0.0,
                first_face: face,
            }),
        };
        assert_eq!(resolve(&b, &[two]).unwrap_err(), bad);
    }

    #[test]
    fn a_chamfer_names_one_of_its_faces() {
        let b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let other = b.faces[face_of(&b, FaceRole::MinusY)].name;
        let c = EdgeBlend {
            edge: e,
            kind: BlendKind::Chamfer(Chamfer::Distances {
                first: 0.1,
                second: 0.2,
                first_face: other,
            }),
        };
        assert_eq!(
            resolve(&b, &[c]).unwrap_err(),
            BlendError::NotAFace {
                edge: e,
                face: other
            }
            .into()
        );
    }

    #[test]
    fn refuses_faces_it_cannot_lie_on() {
        // A sweep's face: its path can't be carried past its ends.
        let mut b = block();
        let e = edge_of(&b, FaceRole::PlusX, FaceRole::PlusZ);
        let f = face_of(&b, FaceRole::PlusX);
        let w = crate::geom::path_sweep::tests::straight();
        let s = b.add_surface(Surface::Sweep(std::sync::Arc::new(w)));
        b.faces[f].surface = s;
        assert_eq!(
            resolve(&b, &[fillet(e, 0.1)]).unwrap_err(),
            BlendError::Unsupported {
                at: b.faces[f].name
            }
            .into()
        );
        // Every other surface is supported.
        use crate::geom::{Circle, Curve, Extrusion, Frame, Vector3};
        let circle = Circle::new(Frame::WORLD, 1.0).unwrap();
        let x = Extrusion::new(Curve::Circle(circle), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        assert!(supported(&Surface::Extrusion(std::sync::Arc::new(x))));
    }
}
