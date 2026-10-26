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
//!
//! Faces tangent along an edge they share (a fillet and its support, two
//! blends across a section) can't be traced there. They are certified
//! apart outside a strip round their shared edges as wide as that reach
//! allows a tangent wedge, `sqrt(2 × size × reach)` ([`face_beside`]), or
//! reported as [`Fault::CannotCheck`].
//!
//! [`check_overlaps`] is the part of the check that finds
//! [`Fault::FacesOverlap`] alone: only pairs on coincident surfaces are
//! intersected, which is cheap enough to run on every import.
//!
//! Faces on one surface whose boundaries lie on each other's all round
//! (two solids stacked face to face) leave no rim inside either; for
//! both checks a point inside one (the first of a grid over its pcurves'
//! parameter box that the face holds) inside the other is an overlap,
//! since on a closed surface two faces with one boundary may be each
//! other's complement.

use super::Fault;
use crate::geom::{Curve, Frame, Plane, Point3, Surface};
use crate::intersect::broad::BoxTree;
use crate::intersect::coincide::coincident;
use crate::intersect::{
    FaceMeet, Place, face_beside, face_box, face_face, face_face_within, point_in_face,
};
use crate::topo::{Body, EdgeId, FaceId, VertexId};
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
        .filter_map(|&(i, j)| pair(body, faces[i], faces[j]))
        .collect()
}

/// Faces on coincident surfaces that overlap in an area, by pair in face
/// order: the [`Fault::FacesOverlap`] faults [`check_intersections`]
/// finds, intersecting no other pair. For a body that passes
/// [`super::check_topology`].
pub fn check_overlaps(body: &Body) -> Vec<Fault> {
    let faces: Vec<FaceId> = body.faces().collect();
    let boxes: Vec<_> = faces
        .iter()
        .enumerate()
        .map(|(i, &f)| (face_box(body, f).grow(slack(body, f)), i))
        .collect();
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    let pairs: Vec<(usize, usize)> = BoxTree::new(boxes)
        .self_pairs()
        .into_iter()
        .filter(|&(i, j)| coincident(surface(faces[i]), surface(faces[j])).is_some())
        .collect();
    pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            let (a, b) = (faces[i], faces[j]);
            match face_face((body, a), (body, b)) {
                Ok(meet @ FaceMeet::Coincident { .. }) => overlap(body, a, b, &meet),
                _ => None,
            }
        })
        .collect()
}

/// Whether two faces on one surface overlap in an area: a rim inside the
/// other face, or neither has a boundary, or a point inside one lies
/// inside the other.
fn overlap(body: &Body, a: FaceId, b: FaceId, meet: &FaceMeet) -> Option<Fault> {
    fault(a, b, meet).or_else(|| {
        (inside_other(body, a, b) || inside_other(body, b, a))
            .then_some(Fault::FacesOverlap { a, b })
    })
}

/// Points of a grid in each direction over a face's parameter box when
/// looking for a point inside it.
const PROBES: usize = 16;

/// Whether a point inside face `a` (the first of a grid over its
/// pcurves' parameter box that it holds) lies inside face `b`, on a
/// coincident surface.
fn inside_other(body: &Body, a: FaceId, b: FaceId) -> bool {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for l in body.face_loops(a) {
        for fin in body.loop_fins(l) {
            let e = &body.edges[body.fins[fin].edge];
            for k in 0..=16 {
                let t = e.interval[0] + (e.interval[1] - e.interval[0]) * k as f64 / 16.0;
                if let Some(q) = body.fin_uv(fin, t) {
                    lo = [lo[0].min(q.x), lo[1].min(q.y)];
                    hi = [hi[0].max(q.x), hi[1].max(q.y)];
                }
            }
        }
    }
    if !(lo[0] < hi[0] && lo[1] < hi[1]) {
        return false;
    }
    let (sa, sb) = (
        &body.surfaces[body.faces[a].surface],
        &body.surfaces[body.faces[b].surface],
    );
    for i in 0..PROBES {
        for j in 0..PROBES {
            let uv = crate::geom::Point2::new(
                lo[0] + (hi[0] - lo[0]) * (i as f64 + 0.5) / PROBES as f64,
                lo[1] + (hi[1] - lo[1]) * (j as f64 + 0.5) / PROBES as f64,
            );
            if point_in_face(body, a, uv) != Ok(Place::Inside) {
                continue;
            }
            let (u, v) = sb.project(sa.point(uv.x, uv.y)).param;
            return point_in_face(body, b, crate::geom::Point2::new(u, v)) == Ok(Place::Inside);
        }
    }
    false
}

/// The fault two faces of a body make, if any.
fn pair(body: &Body, a: FaceId, b: FaceId) -> Option<Fault> {
    // Faces tangent along an edge they share can't be traced there: they
    // are certified apart beside it, and only traced if that fails.
    let shared = shared_edges(body, a, b);
    let tangent = tangent(body, a, b, &shared);
    if tangent && apart_beside(body, a, b, &shared) {
        return None;
    }
    // Faces touching at a vertex they share touch at a corner of both
    // boxes, where the region's sides cut the trace: again with the
    // region reaching an eighth of the smaller face's size past it.
    let meet = face_face((body, a), (body, b)).or_else(|e| {
        if shared_vertices(body, a, b).is_empty() {
            return Err(e);
        }
        let size = |f: FaceId| {
            let x = face_box(body, f);
            (x.max() - x.min()).length()
        };
        face_face_within((body, a), (body, b), size(a).min(size(b)) / 8.0)
    });
    match meet {
        Ok(meet @ FaceMeet::Coincident { .. }) => overlap(body, a, b, &meet),
        Ok(meet) => fault(a, b, &prune(body, a, b, meet)),
        // Faces crossing at an edge they share where it can't be traced (a
        // pipe cut off at its spine's end), or touching at vertices they
        // share, apart beside them all the same.
        Err(_) if !tangent && !shared.is_empty() && apart_beside(body, a, b, &shared) => None,
        Err(_) if shared.is_empty() && apart_at_vertices(body, a, b) => None,
        Err(_) => Some(Fault::CannotCheck { a, b }),
    }
}

/// The edges two faces both use, each once.
fn shared_edges(body: &Body, a: FaceId, b: FaceId) -> Vec<EdgeId> {
    let edges = |f: FaceId| -> Vec<EdgeId> {
        body.face_loops(f)
            .flat_map(|l| {
                body.loop_fins(l)
                    .map(|fin| body.fins[fin].edge)
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let eb = edges(b);
    let mut out: Vec<EdgeId> = edges(a).into_iter().filter(|e| eb.contains(e)).collect();
    out.sort();
    out.dedup();
    out
}

/// Whether two faces meet tangentially along an edge they share: their
/// surfaces' normals are parallel, within a millionth of a radian, at
/// the middle of one of them. This only chooses how the pair is checked; both
/// ways are certified.
fn tangent(body: &Body, a: FaceId, b: FaceId, shared: &[EdgeId]) -> bool {
    shared.iter().any(|&e| {
        let x = &body.edges[e];
        let p = body.curves[x.curve].point((x.interval[0] + x.interval[1]) / 2.0);
        let normal = |f: FaceId| {
            let s = &body.surfaces[body.faces[f].surface];
            let (u, v) = s.project(p).param;
            s.normal(u, v)
        };
        match (normal(a), normal(b)) {
            (Some(na), Some(nb)) => na.get().cross(nb.get()).length() <= 1e-6,
            _ => false,
        }
    })
}

/// Whether two faces tangent along the edges they share are certified to
/// meet nowhere else, outside a strip round those edges as wide as
/// surfaces meeting tangentially stay within the touch rule's reach of
/// each other, `sqrt(2 × size × reach)`: one face keeps off the other's
/// surface there ([`face_beside`]), as a fillet does off the face it
/// rolls on, or the two lie on either side of the plane of a planar edge
/// they share, as blends do across a section.
fn apart_beside(body: &Body, a: FaceId, b: FaceId, shared: &[EdgeId]) -> bool {
    let reach = TOUCH * slack(body, a).max(slack(body, b));
    let size = {
        let bx = face_box(body, a).union(&face_box(body, b));
        (bx.max() - bx.min()).length()
    };
    let strip = (2.0 * size * reach).sqrt();
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    let side = |x: FaceId, of: &Surface| face_beside(body, x, of, shared, &[], strip).ok();
    // The more curved face first: a fillet keeps off the surface of the
    // face it rolls on, while that face may well run on past the fillet's
    // surface.
    let (x, y) = if rank(surface(a)) >= rank(surface(b)) {
        (a, b)
    } else {
        (b, a)
    };
    if side(x, surface(y)).is_some() || side(y, surface(x)).is_some() {
        return true;
    }
    let Some(plane) = section_plane(body, shared) else {
        return false;
    };
    match (side(a, &plane), side(b, &plane)) {
        (Some(sa), Some(sb)) => {
            let one = |s: [bool; 2]| s != [true, true];
            one(sa)
                && one(sb)
                && (sa[0] && sb[1] || sa[1] && sb[0] || sa == [false; 2] || sb == [false; 2])
        }
        _ => false,
    }
}

/// The vertices two faces both have, each once.
fn shared_vertices(body: &Body, a: FaceId, b: FaceId) -> Vec<VertexId> {
    let vertices = |f: FaceId| -> Vec<VertexId> {
        body.face_loops(f)
            .flat_map(|l| {
                body.loop_fins(l)
                    .filter_map(|fin| body.fin_start(fin))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let vb = vertices(b);
    let mut shared: Vec<VertexId> = vertices(a).into_iter().filter(|v| vb.contains(v)).collect();
    shared.sort();
    shared.dedup();
    shared
}

/// Whether two faces sharing vertices but no edge, which may touch at
/// those vertices as tangent wedges do, are certified apart outside the
/// balls round them as wide as the reach allows a tangent wedge
/// ([`face_beside`]), one face keeping off the other's surface there.
fn apart_at_vertices(body: &Body, a: FaceId, b: FaceId) -> bool {
    let shared = shared_vertices(body, a, b);
    if shared.is_empty() {
        return false;
    }
    let reach = TOUCH * slack(body, a).max(slack(body, b));
    let size = {
        let bx = face_box(body, a).union(&face_box(body, b));
        (bx.max() - bx.min()).length()
    };
    let strip = (2.0 * size * reach).sqrt();
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    face_beside(body, a, surface(b), &[], &shared, strip).is_ok()
        || face_beside(body, b, surface(a), &[], &shared, strip).is_ok()
}

/// How curved a surface is, by kind: a plane least, a pipe most.
fn rank(s: &Surface) -> u8 {
    match s {
        Surface::Plane(_) => 0,
        Surface::Cylinder(_) | Surface::Cone(_) => 1,
        Surface::Sphere(_) => 2,
        Surface::Torus(_) => 3,
        _ => 4,
    }
}

/// The plane of a lone shared edge that is planar: a circle's or an
/// ellipse's, or the plane through three of a NURBS's control points
/// (spread as far apart as they go) when every control point lies within
/// the edge's tolerance of it (so the curve does, inside their hull).
/// The plane only separates the faces; the strip round the edge covers
/// how far the edge lies from it.
fn section_plane(body: &Body, shared: &[EdgeId]) -> Option<Surface> {
    let [e] = shared else {
        return None;
    };
    let frame = match &body.curves[body.edges[*e].curve] {
        Curve::Circle(c) => *c.frame(),
        Curve::Ellipse(c) => *c.frame(),
        Curve::Nurbs(n) => {
            let pts: Vec<Point3> = n.control_points().collect();
            // Three control points spread as far apart as they go: the
            // first, the farthest from it, and the farthest from the line
            // through both.
            let a = pts[0];
            let far = |f: &dyn Fn(Point3) -> f64| {
                pts.iter()
                    .copied()
                    .max_by(|p, q| f(*p).total_cmp(&f(*q)))
                    .expect("control points")
            };
            let b = far(&|p| p.distance(a));
            let c = far(&|p| (p - a).cross(b - a).length());
            let frame = Frame::new(a, (b - a).cross(c - a), b - a).ok()?;
            let off = |p: Point3| (p - a).dot(frame.z().get()).abs();
            if pts.iter().any(|&p| off(p) > body.edges[*e].tolerance.get()) {
                return None;
            }
            frame
        }
        _ => return None,
    };
    Some(Surface::Plane(Plane::new(frame)))
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
    fn a_flat_cones_side_meets_its_base_at_an_edge() {
        // Radius 82, height a millimetre: the side meets the base at
        // about 1e-5 rad, which is no tangency to be checked beside it.
        let t = Rigid::rotation(Vector3::new(0.2, -0.2, 1.0).normalize().unwrap(), 2.9);
        let b = primitive::cone(OpKey(1), 81.8, 0.001, &t).unwrap().0;
        assert_eq!(check_intersections(&b), []);
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
