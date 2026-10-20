//! Point in solid: where a point lies against a body, certified by ray
//! casting.
//!
//! A point within a face's tolerance of the face (by [`point_in_face`])
//! is on the boundary. Otherwise a ray is cast from it along each of a
//! fixed list of directions in turn, out past the body's box. Every face
//! whose box the ray's box meets is cut with it by [`curve_surface`], and
//! each hit is placed on its face by [`point_in_face`]: a crossing inside
//! a face counts, a hit outside it doesn't, and anything else (a hit on
//! an edge or a vertex, a touch, a stretch lying on a surface, an
//! intersection that can't be certified) abandons the ray for the next.
//! An odd count is inside. When no ray settles it the answer is
//! [`Failure::Uncertain`].

use super::Failure;
use super::cs::{CurveHit, curve_surface};
use super::face::face_box;
use super::inface::{Place, point_in_face};
use crate::geom::{Box3, Curve, Line, Point2, Point3, Vector3};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};

/// Where a point lies against a solid body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inclusion {
    Inside,
    Outside,
    /// Within the tolerance of a face.
    Boundary,
}

/// Ray directions, tried in order: none along an axis or a diagonal,
/// where models line up.
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

/// Where `p` lies against `body`, a solid that passes
/// [`crate::check::check`].
pub fn point_in_body(body: &Body, p: Point3) -> Result<Inclusion, Failure> {
    let faces: Vec<FaceId> = body.faces().collect();
    point_in_faces(body, &faces, p)
}

/// Where `p` lies against the closed shells made of `faces` of `body`, as
/// for [`point_in_body`].
pub(crate) fn point_in_faces(
    body: &Body,
    faces: &[FaceId],
    p: Point3,
) -> Result<Inclusion, Failure> {
    let faces: Vec<(FaceId, Box3, f64)> = faces
        .iter()
        .map(|&f| {
            let tol = face_tolerance(body, f);
            (f, face_box(body, f).grow(tol), tol)
        })
        .collect();
    let Some(bounds) = faces.iter().map(|x| x.1).reduce(|a, b| a.union(&b)) else {
        return Ok(Inclusion::Outside);
    };
    if !bounds.intersects(&Box3::from_point(p)) {
        return Ok(Inclusion::Outside);
    }
    for &(f, b, tol) in &faces {
        if !b.intersects(&Box3::from_point(p)) {
            continue;
        }
        let surface = &body.surfaces[body.faces[f].surface];
        if surface.distance(p) <= tol {
            let (u, v) = surface.project(p).param;
            if point_in_face(body, f, Point2::new(u, v))? != Place::Outside {
                return Ok(Inclusion::Boundary);
            }
        }
    }
    // Far enough to leave the body's box from anywhere inside it.
    let reach = 2.0 * (bounds.max() - bounds.min()).length() + 1.0;
    for d in DIRECTIONS {
        let dir = Vector3::new(d[0], d[1], d[2]);
        if let Some(inside) = cast(body, &faces, p, dir, reach) {
            return Ok(if inside {
                Inclusion::Inside
            } else {
                Inclusion::Outside
            });
        }
    }
    Err(Failure::Uncertain)
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

/// Whether the ray from `p` along `dir` crosses the body's faces an odd
/// number of times, or `None` when the ray settles nothing.
fn cast(
    body: &Body,
    faces: &[(FaceId, Box3, f64)],
    p: Point3,
    dir: Vector3,
    reach: f64,
) -> Option<bool> {
    let dir = dir.normalize()?;
    let line = Curve::Line(Line::new(p, dir.get()).ok()?);
    let ray_box = Box3::new(p, p + dir.get() * reach);
    let mut crossings = 0usize;
    for &(f, b, tol) in faces {
        if !b.intersects(&ray_box) {
            continue;
        }
        let surface = &body.surfaces[body.faces[f].surface];
        let hits = curve_surface(&line, [0.0, reach], surface, tol).ok()?;
        for hit in hits {
            let CurveHit::Point {
                t,
                point,
                before,
                after,
            } = hit
            else {
                return None;
            };
            let (u, v) = surface.project(point).param;
            match point_in_face(body, f, Point2::new(u, v)).ok()? {
                Place::Outside => {}
                Place::Inside if before != after && t > 0.0 && t < reach => crossings += 1,
                _ => return None,
            }
        }
    }
    Some(crossings % 2 == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rigid;
    use crate::math::hypot;
    use crate::name::OpKey;
    use crate::primitive;
    use core::f64::consts::PI;

    /// Points of a grid over `[-3, 3]³`, offset so none sits on a model's
    /// round numbers.
    fn grid() -> Vec<Point3> {
        let mut out = Vec::new();
        for i in 0..9 {
            for j in 0..9 {
                for k in 0..9 {
                    let at = |n: i32| -3.0 + 0.75 * f64::from(n) + 0.0123;
                    out.push(Point3::new(at(i), at(j), at(k)));
                }
            }
        }
        out
    }

    /// Checks `point_in_body` against an exact signed distance on the
    /// grid: negative inside.
    fn agrees(body: &Body, signed: impl Fn(Point3) -> f64) {
        for p in grid() {
            let d = signed(p);
            let got = point_in_body(body, p).unwrap();
            let want = if d.abs() <= 1e-9 {
                Inclusion::Boundary
            } else if d < 0.0 {
                Inclusion::Inside
            } else {
                Inclusion::Outside
            };
            assert_eq!(got, want, "{p:?} at {d}");
        }
    }

    #[test]
    fn primitives() {
        let k = OpKey(1);
        let at = Rigid::translation(Vector3::new(-1.0, -1.5, -0.5));
        let block = primitive::block(k, 2.0, 3.0, 1.0, &at).unwrap().0;
        agrees(&block, |p| {
            let q = Point3::new(p.x.abs() - 1.0, p.y.abs() - 1.5, p.z.abs() - 0.5);
            q.x.max(q.y).max(q.z)
        });
        let down = Rigid::translation(Vector3::new(0.0, 0.0, -1.0));
        let cyl = primitive::cylinder(k, 1.3, 2.0, &down).unwrap().0;
        agrees(&cyl, |p| (hypot(p.x, p.y) - 1.3).max(p.z.abs() - 1.0));
        let sphere = primitive::sphere(k, 2.0, &Rigid::IDENTITY).unwrap().0;
        agrees(&sphere, |p| p.to_vector().length() - 2.0);
        let torus = primitive::torus(k, 2.0, 0.7, &Rigid::IDENTITY).unwrap().0;
        agrees(&torus, |p| hypot(hypot(p.x, p.y) - 2.0, p.z) - 0.7);
        let cone = primitive::cone(k, 1.5, 2.5, &down).unwrap().0;
        let slope = 1.5 / 2.5;
        agrees(&cone, |p| {
            // Inside below the side and above the base; the exact
            // distance is not needed away from the boundary.
            let r = 1.5 - slope * (p.z + 1.0);
            let side = (hypot(p.x, p.y) - r) / hypot(1.0, slope);
            side.max(-1.0 - p.z).max(p.z - 1.5)
        });
    }

    #[test]
    fn points_on_the_boundary_and_far_away() {
        let (b, _) = primitive::block(OpKey(1), 1.0, 1.0, 1.0, &Rigid::IDENTITY).unwrap();
        for p in [
            Point3::new(0.5, 0.5, 1.0),
            Point3::new(1.0, 0.25, 0.75),
            Point3::new(1.0, 1.0, 0.3),
            Point3::new(0.0, 0.0, 0.0),
        ] {
            assert_eq!(point_in_body(&b, p), Ok(Inclusion::Boundary), "{p:?}");
        }
        assert_eq!(
            point_in_body(&b, Point3::new(40.0, 0.0, 0.0)),
            Ok(Inclusion::Outside)
        );
        // At the centre a ray along a diagonal would run into a corner;
        // these directions don't, and any that did would be abandoned.
        assert_eq!(
            point_in_body(&b, Point3::new(0.5, 0.5, 0.5)),
            Ok(Inclusion::Inside)
        );
        assert_eq!(
            point_in_body(&Body::new(), Point3::ORIGIN),
            Ok(Inclusion::Outside)
        );
    }

    #[test]
    fn hollow_and_holed_sweeps() {
        use crate::geom::{Axis, Circle2, Curve2, Frame, Frame2, Line2};
        use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, revolve};
        // A ring of square section 1 to 2 from the axis, with a round hole
        // through its middle turned whole: a hollow ring.
        let corners = [
            Point2::new(1.0, -0.5),
            Point2::new(2.0, -0.5),
            Point2::new(2.0, 0.5),
            Point2::new(1.0, 0.5),
        ];
        let outer: Vec<Segment> = (0..4)
            .map(|i| {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                Segment {
                    key: SegmentKey(i as u128 + 1),
                    curve: Curve2::Line2(Line2::through(a, b).unwrap()),
                    t0: 0.0,
                    t1: a.distance(b),
                }
            })
            .collect();
        let hole = Segment {
            key: SegmentKey(9),
            curve: Curve2::Circle2(
                Circle2::new(
                    Frame2::new(
                        Point2::new(1.5, 0.0),
                        crate::geom::Vector2::new(1.0, 0.0),
                        true,
                    )
                    .unwrap(),
                    0.25,
                )
                .unwrap(),
            ),
            t0: 0.0,
            t1: 2.0 * PI,
        };
        let frame = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let profile = ValidProfile::new(Profile {
            frame,
            loops: vec![
                Loop {
                    key: LoopKey(1),
                    segments: outer,
                },
                Loop {
                    key: LoopKey(2),
                    segments: vec![hole],
                },
            ],
        })
        .unwrap();
        let axis = Axis::new(Point3::ORIGIN, Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let (ring, _) = revolve(OpKey(3), &profile, axis, 0.0, 2.0 * PI).unwrap();
        agrees(&ring, |p| {
            let r = hypot(p.x, p.y);
            let solid = (r - 1.5).abs().max(p.z.abs()) - 0.5;
            let tube = 0.25 - hypot(r - 1.5, p.z);
            solid.max(tube)
        });
    }
}
