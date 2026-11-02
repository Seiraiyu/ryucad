//! Winding numbers: how many times a body's shells wind round a point,
//! certified by ray casting (design 299's self-union).
//!
//! [`point_in_body`](super::point_in_body) asks whether a valid solid
//! encloses a point, by the parity of a ray's crossings. A body whose
//! faces cross has no inside by parity: its shells may wind round a
//! point twice, or minus once inside a cavity's shell. The winding
//! number is the signed count of a ray's crossings out to past the
//! body's box: `+1` where the ray leaves through a face (from its inner
//! side to its outer, by the face's outward normal), `-1` where it
//! enters. Rays and rules are `point_in_body`'s: a hit on an edge or a
//! vertex, a touch, a stretch lying on a surface, or an intersection
//! that can't be certified abandons the ray for the next of the same
//! fixed list; when none settles it the answer is `Uncertain`.
//!
//! [`winding_off`] counts from a point on a face, along rays leaving to
//! the face's outer side: the winding number just outside the face
//! (and every face on that surface there), which is what the self-union
//! keeps a piece by.

use super::Failure;
use super::cs::{CurveHit, Side, curve_surface_in};
use super::face::face_box;
use super::inface::{Place, on_boundary, point_in_face};
use crate::geom::{Box3, Curve, Line, Point2, Point3, UnitVector3, Vector3};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId, Sense};
use crate::work::Meter;

/// Ray directions, tried in order: `point_in_body`'s.
const DIRECTIONS: [[f64; 3]; 12] = [
    [0.8432, 0.3721, 0.3879],
    [-0.2914, 0.9137, 0.2833],
    [0.1871, -0.4187, 0.8886],
    [-0.7313, -0.5917, 0.3393],
    [0.4419, -0.8431, -0.3064],
    [-0.3709, 0.2357, -0.8983],
    [0.6618, 0.6137, -0.4304],
    [-0.9013, 0.1243, -0.4149],
    [0.0917, 0.7731, -0.6276],
    [0.5137, -0.1123, 0.8506],
    [-0.1543, -0.9611, -0.2291],
    [0.9671, -0.2207, 0.1263],
];

/// The least cosine between a ray from a face and the face's normal:
/// rays nearer the tangent plane are passed over.
const STEEP: f64 = 0.2;

/// The winding number of `body`'s shells round `p`: the signed count of
/// a certified ray's crossings with every face, `+1` leaving through a
/// face, `-1` entering. Fails with `Uncertain` for a point within a
/// face's tolerance of it, or where no ray settles it; a sheet's faces
/// count as a solid's would.
pub fn winding(body: &Body, p: Point3) -> Result<i64, Failure> {
    // unmetered: the public function takes no work
    winding_in(body, p, &Meter::free())
}

/// [`winding`] charging `m`.
pub(crate) fn winding_in(body: &Body, p: Point3, m: &Meter) -> Result<i64, Failure> {
    m.charge(1)?;
    let faces = boxed(body);
    let Some(bounds) = faces.iter().map(|x| x.1).reduce(|a, b| a.union(&b)) else {
        return Ok(0);
    };
    if !bounds.intersects(&Box3::from_point(p)) {
        return Ok(0);
    }
    for &(f, b) in &faces {
        if !b.intersects(&Box3::from_point(p)) {
            continue;
        }
        // A face tested.
        m.charge(1)?;
        if on_boundary(body, f, p, 0.0).is_some() {
            return Err(Failure::Uncertain);
        }
        let surface = &body.surfaces[body.faces[f].surface];
        if surface.distance(p) <= LINEAR_RESOLUTION {
            let (u, v) = surface.project(p).param;
            if point_in_face(body, f, Point2::new(u, v))? != Place::Outside {
                return Err(Failure::Uncertain);
            }
        }
    }
    let reach = 2.0 * (bounds.max() - bounds.min()).length() + 1.0;
    for d in DIRECTIONS {
        // A ray cast.
        m.charge(1)?;
        if let Some(w) = cast(
            body,
            &faces,
            p,
            Vector3::new(d[0], d[1], d[2]),
            reach,
            &[],
            m,
        ) {
            return Ok(w);
        }
    }
    Err(Failure::Uncertain)
}

/// The winding number of `body`'s shells just outside `p`, a point
/// inside a face away from its boundary, `normal` the face's outward
/// normal there: counted along rays leaving `p` to that side, the hits
/// at `p` itself on the faces in `skip` (the face and those on its
/// surface there) left out, and a hit there on any other face
/// abandoning the ray.
pub(crate) fn winding_off(
    body: &Body,
    p: Point3,
    normal: UnitVector3,
    skip: &[FaceId],
    m: &Meter,
) -> Result<i64, Failure> {
    m.charge(1)?;
    let faces = boxed(body);
    let Some(bounds) = faces.iter().map(|x| x.1).reduce(|a, b| a.union(&b)) else {
        return Ok(0);
    };
    let reach = 2.0 * (bounds.max() - bounds.min()).length() + 1.0;
    for d in DIRECTIONS {
        let mut dir = Vector3::new(d[0], d[1], d[2]);
        let Some(unit) = dir.normalize() else {
            continue;
        };
        let cos = unit.get().dot(normal.get());
        if cos.abs() < STEEP {
            continue;
        }
        if cos < 0.0 {
            dir = -dir;
        }
        // A ray cast.
        m.charge(1)?;
        if let Some(w) = cast(body, &faces, p, dir, reach, skip, m) {
            return Ok(w);
        }
    }
    Err(Failure::Uncertain)
}

/// Every face with its box, grown by its boundary's tolerance.
fn boxed(body: &Body) -> Vec<(FaceId, Box3)> {
    body.faces()
        .map(|f| (f, face_box(body, f).grow(face_tolerance(body, f))))
        .collect()
}

/// The largest tolerance on a face's boundary, at least resolution.
fn face_tolerance(body: &Body, f: FaceId) -> f64 {
    let mut t = LINEAR_RESOLUTION;
    for l in body.face_loops(f) {
        if let Some(v) = body.loops[l].vertex {
            t = t.max(body.vertices[v].tolerance.get());
        }
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

/// The signed count of the ray from `p` along `dir`'s crossings, or
/// `None` when the ray settles nothing. Hits at the ray's start on the
/// faces in `skip` are left out.
fn cast(
    body: &Body,
    faces: &[(FaceId, Box3)],
    p: Point3,
    dir: Vector3,
    reach: f64,
    skip: &[FaceId],
    m: &Meter,
) -> Option<i64> {
    let dir = dir.normalize()?;
    let line = Curve::Line(Line::new(p, dir.get()).ok()?);
    let ray_box = Box3::new(p, p + dir.get() * reach);
    let mut winding = 0i64;
    for &(f, b) in faces {
        if !b.intersects(&ray_box) {
            continue;
        }
        let face = &body.faces[f];
        let surface = &body.surfaces[face.surface];
        let hits = curve_surface_in(&line, [0.0, reach], surface, LINEAR_RESOLUTION, m).ok()?;
        for hit in hits {
            let CurveHit::Point {
                t,
                point,
                before,
                after,
            } = hit
            else {
                // A stretch on the surface: at the start on a skipped
                // face it is that face's own; anything else settles
                // nothing.
                return None;
            };
            if t == 0.0 && skip.contains(&f) {
                continue;
            }
            let (u, v) = surface.project(point).param;
            let place = point_in_face(body, f, Point2::new(u, v)).ok()?;
            if t == 0.0 && place != Place::Outside {
                // Another face at the start: nothing is settled there.
                return None;
            }
            match place {
                Place::Outside => {}
                Place::Inside if before != after && t < reach => {
                    // Leaving to the outer side counts +1, entering -1.
                    let outer = match face.sense {
                        Sense::Forward => Side::Positive,
                        Sense::Reversed => Side::Negative,
                    };
                    winding += if after == outer { 1 } else { -1 };
                }
                _ => return None,
            }
        }
    }
    Some(winding)
}
