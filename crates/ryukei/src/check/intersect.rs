//! The self-intersection check.
//!
//! Every pair of faces whose boxes overlap ([`BoxTree`] over
//! [`face_box`], grown by the faces' tolerances) is intersected
//! ([`face_face`]), in parallel. Faces may meet only along an edge they
//! both use or at a vertex they both have: a stretch where they meet must
//! lie on one shared edge, and a touching point on one shared vertex or
//! edge; by the touch rule, a stretch within 100 tolerances of a shared
//! vertex or of a shared edge along all its length is benign too (two
//! fits of one intersection curve may lie that far apart where the faces
//! meet at a shallow angle). Out of a vertex the faces share, a stretch
//! inside at most one of them (an edge of one lying on the other) is
//! benign for as long as surfaces meeting tangentially stay within that
//! reach, `sqrt(2 × size × reach)`: the tip of a tangent wedge. Anything
//! else is
//! [`Fault::FacesCross`]; faces on one surface
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
                Ok(meet) => fault(a, b, &prune(body, a, b, meet)),
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

/// A stretch no longer than this many tolerances, all within as many of
/// a vertex both faces have, touches at the vertex: the touch rule of
/// [`crate::intersect::curve_surface`].
const TOUCH: f64 = 100.0;

/// Drops the stretches and contacts that, by the touch rule, are only the
/// faces touching at a vertex they share.
fn prune(body: &Body, a: FaceId, b: FaceId, meet: FaceMeet) -> FaceMeet {
    let FaceMeet::Meet {
        stretches,
        contacts,
    } = meet
    else {
        return meet;
    };
    let vertices = |f: FaceId| -> Vec<crate::topo::VertexId> {
        let mut out = Vec::new();
        for l in body.face_loops(f) {
            if let Some(v) = body.loop_(l).and_then(|l| l.vertex) {
                out.push(v);
            }
            for fin in body.loop_fins(l) {
                out.extend(body.fin_start(fin));
            }
        }
        out
    };
    let (va, vb) = (vertices(a), vertices(b));
    let shared: Vec<crate::geom::Point3> = va
        .iter()
        .filter(|v| vb.contains(v))
        .map(|&v| body.vertices[v].point)
        .collect();
    let at_shared = |pts: &[crate::geom::Point3], reach: f64| {
        shared
            .iter()
            .any(|q| pts.iter().all(|p| p.distance(*q) <= reach))
    };
    // Edges both faces use: a stretch fitted to the same curve as one of
    // them may lie farther from it than both tolerances where the faces
    // meet at a shallow angle, but within the touch rule's reach.
    let edges = |f: FaceId| -> Vec<crate::topo::EdgeId> {
        body.face_loops(f)
            .flat_map(|l| {
                body.loop_fins(l)
                    .map(|fin| body.fins[fin].edge)
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let (ea, eb) = (edges(a), edges(b));
    let shared_edges: Vec<crate::topo::EdgeId> =
        ea.iter().copied().filter(|e| eb.contains(e)).collect();
    let along_shared = |pts: &[crate::geom::Point3], reach: f64| {
        shared_edges.iter().any(|&e| {
            let x = &body.edges[e];
            let c = &body.curves[x.curve];
            pts.iter().all(|&p| {
                let foot = c.project(p);
                let t = foot.param;
                let inside = match c.period() {
                    Some(per) => {
                        let t = x.interval[0] + (t - x.interval[0]).rem_euclid(per);
                        t <= x.interval[1]
                    }
                    None => t >= x.interval[0] && t <= x.interval[1],
                };
                (inside && foot.distance <= reach)
                    || c.point(x.interval[0]).distance(p) <= reach
                    || c.point(x.interval[1]).distance(p) <= reach
            })
        })
    };
    let tol = slack(body, a).max(slack(body, b));
    let size = {
        let bx = face_box(body, a).union(&face_box(body, b));
        (bx.max() - bx.min()).length()
    };
    let stretches = stretches
        .into_iter()
        .filter(|s| {
            let reach = TOUCH * s.tolerance.max(tol);
            let pts: Vec<_> = (0..=8)
                .map(|k| {
                    s.curve
                        .point(s.range[0] + (s.range[1] - s.range[0]) * f64::from(k) / 8.0)
                })
                .collect();
            // Out of a tangent wedge's tip at a vertex the faces share: an
            // edge of one lying on the other (or both edges running
            // together) within the reach, for no longer than surfaces
            // meeting tangentially stay that close.
            let length: f64 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
            let from_shared = !s.places.iter().all(|p| *p == Place::Inside)
                && length <= (2.0 * size * reach).sqrt()
                && shared
                    .iter()
                    .any(|q| pts[0].distance(*q) <= reach || pts[8].distance(*q) <= reach);
            !at_shared(&pts, reach) && !along_shared(&pts, reach) && !from_shared
        })
        .collect();
    let contacts = contacts
        .into_iter()
        .filter(|c| !at_shared(&[c.point], TOUCH * tol))
        .collect();
    FaceMeet::Meet {
        stretches,
        contacts,
    }
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
