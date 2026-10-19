//! The self-intersection check.
//!
//! Every pair of faces whose boxes overlap ([`BoxTree`] over
//! [`face_box`], grown by the faces' tolerances) is intersected
//! ([`face_face`]), in parallel. Faces may meet only along an edge they
//! both use or at a vertex they both have: a stretch where they meet must
//! lie on one shared edge, and a touching point on one shared vertex or
//! edge. Anything else is [`Fault::FacesCross`]; faces on one surface
//! whose regions overlap in an area are [`Fault::FacesOverlap`]; a pair
//! that can't be intersected is [`Fault::CannotCheck`].

use super::Fault;
use crate::intersect::broad::BoxTree;
use crate::intersect::{FaceMeet, Place, face_box, face_face};
use crate::topo::{Body, FaceId};
use rayon::prelude::*;

/// Faces that cross, overlap or can't be checked, by pair in face order.
/// For a body that passes [`super::check`].
pub fn check_intersections(body: &Body) -> Vec<Fault> {
    let faces: Vec<FaceId> = body.faces().collect();
    let boxes: Vec<_> = faces
        .iter()
        .enumerate()
        .map(|(i, &f)| (face_box(body, f).grow(slack(body, f)), i))
        .collect();
    let pairs = BoxTree::new(boxes).self_pairs();
    pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            let (a, b) = (faces[i], faces[j]);
            match face_face((body, a), (body, b)) {
                Err(_) => Some(Fault::CannotCheck { a, b }),
                Ok(meet) => fault(a, b, &meet),
            }
        })
        .collect()
}

/// The largest tolerance on a face's boundary.
fn slack(body: &Body, f: FaceId) -> f64 {
    let mut t = crate::tol::LINEAR_RESOLUTION;
    for l in body.face_loops(f) {
        for fin in body.loop_fins(l) {
            let e = &body.edges[body.fins[fin].edge];
            t = t
                .max(e.tolerance.get())
                .max(body.vertices[e.start].tolerance.get())
                .max(body.vertices[e.end].tolerance.get());
        }
    }
    t
}

/// The fault two faces' meeting makes, if any.
fn fault(a: FaceId, b: FaceId, meet: &FaceMeet) -> Option<Fault> {
    match meet {
        FaceMeet::Coincident { rims, whole, .. } => (*whole
            || rims.iter().any(|r| r.place == Place::Inside))
        .then_some(Fault::FacesOverlap { a, b }),
        FaceMeet::Meet {
            stretches,
            contacts,
        } => {
            let shared_edge =
                |p: [Place; 2]| matches!(p, [Place::OnEdge(x), Place::OnEdge(y)] if x == y);
            let shared = |p: [Place; 2]| {
                shared_edge(p) || matches!(p, [Place::OnVertex(x), Place::OnVertex(y)] if x == y)
            };
            let bad = stretches.iter().any(|s| !shared_edge(s.places))
                || contacts.iter().any(|c| !shared(c.places));
            bad.then_some(Fault::FacesCross { a, b })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point3, Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::topo::fixture::polyhedron;

    #[test]
    fn primitives_do_not_cross_themselves() {
        let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        let k = OpKey(1);
        for b in [
            primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
            primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
            primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
            primitive::frustum(k, 1.0, 0.3, 2.0, &t).unwrap().0,
            primitive::sphere(k, 1.0, &t).unwrap().0,
            primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
        ] {
            assert_eq!(check_intersections(&b), []);
        }
    }

    /// A triangular bipyramid on the base (0,0,0), (2,0,0), (0,2,0), with
    /// apexes `d` and `e`.
    fn bipyramid(d: Point3, e: Point3) -> Body {
        let pts = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(0.0, 2.0, 0.0),
            d,
            e,
        ];
        polyhedron(
            &pts,
            &[
                &[3, 0, 1],
                &[3, 1, 2],
                &[3, 2, 0],
                &[4, 1, 0],
                &[4, 2, 1],
                &[4, 0, 2],
            ],
        )
    }

    #[test]
    fn a_folded_bipyramid_crosses_itself() {
        let sound = bipyramid(Point3::new(0.5, 0.5, 1.0), Point3::new(0.5, 0.5, -1.0));
        assert_eq!(check_intersections(&sound), []);
        // Both apexes above the base, one leaning across the other's
        // faces.
        let folded = bipyramid(Point3::new(0.5, 0.5, 1.0), Point3::new(1.5, 1.5, 2.0));
        let faults = check_intersections(&folded);
        assert!(!faults.is_empty());
        assert!(
            faults.iter().all(|f| matches!(f, Fault::FacesCross { .. })),
            "{faults:?}"
        );
    }

    #[test]
    fn the_rules() {
        use crate::intersect::{Contact, Rim};
        use crate::topo::{EdgeId, Id, VertexId};
        let (a, b) = (FaceId::from_index(0), FaceId::from_index(1));
        let (e, f) = (EdgeId::from_index(0), EdgeId::from_index(1));
        let (v, w) = (VertexId::from_index(0), VertexId::from_index(1));
        let overlap = |place| FaceMeet::Coincident {
            same: true,
            rims: vec![Rim {
                of: 0,
                edge: e,
                range: [0.0, 1.0],
                place,
            }],
            whole: false,
        };
        assert_eq!(
            fault(a, b, &overlap(Place::Inside)),
            Some(Fault::FacesOverlap { a, b })
        );
        assert_eq!(fault(a, b, &overlap(Place::OnEdge(f))), None);
        let touch = |places| FaceMeet::Meet {
            stretches: vec![],
            contacts: vec![Contact {
                point: Point3::ORIGIN,
                places,
            }],
        };
        assert_eq!(
            fault(a, b, &touch([Place::OnVertex(v), Place::OnVertex(v)])),
            None
        );
        assert_eq!(
            fault(a, b, &touch([Place::OnVertex(v), Place::OnVertex(w)])),
            Some(Fault::FacesCross { a, b })
        );
        assert_eq!(
            fault(a, b, &touch([Place::OnEdge(e), Place::Inside])),
            Some(Fault::FacesCross { a, b })
        );
    }
}
