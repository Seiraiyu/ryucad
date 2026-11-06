//! Certified rays towards the viewer (design 326, phase 3): how many
//! faces of the placed bodies lie in front of a point.

use super::View;
use crate::geom::{Box3, Curve, Line, Point2, Point3};
use crate::intersect::broad::BoxTree;
use crate::intersect::{
    CurveHit, Place, curve_surface_in, face_box, face_tolerance, point_in_face,
};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, FaceId};
use crate::work::{Meter, Scope};

/// How far past the nearest face a ray runs.
const PAST: f64 = 1e-6;

/// Every face of the placed bodies, boxed in image coordinates and depth.
#[allow(dead_code)] // used from Task 11
pub(super) struct Scene<'a> {
    pub(super) view: &'a View,
    pub(super) bodies: &'a [&'a Body],
    faces: Vec<(usize, FaceId)>,
    tree: BoxTree,
    /// The largest depth any face reaches.
    far: f64,
}

/// What a ray from a point of a piece must know of the piece.
#[allow(dead_code)] // used from Task 11
#[derive(Clone, Copy, Debug)]
pub(super) struct Ray<'b> {
    /// The placed body the piece lies on, and its faces there.
    pub(super) body: usize,
    pub(super) own: &'b [FaceId],
    /// Hits on its own faces this near the point are its own place.
    pub(super) skip_own: f64,
    /// Hits on any face this near the point are its own place.
    pub(super) near: f64,
    /// The piece lies on other pieces: what the ray meets only on a
    /// face's boundary, or along a face, doesn't hide it.
    pub(super) lenient: bool,
}

/// A ray's verdict.
#[allow(dead_code)] // used from Task 11
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Count {
    /// It crosses this many faces in front of the point.
    Settled(u32),
    /// A hit it can't settle: try another point.
    Unsure,
}

#[allow(dead_code)] // used from Task 11
impl<'a> Scene<'a> {
    pub(super) fn new(view: &'a View, bodies: &'a [&'a Body]) -> Self {
        let mut faces = Vec::new();
        let mut boxes = Vec::new();
        let mut far = f64::NEG_INFINITY;
        for (b, &body) in bodies.iter().enumerate() {
            for f in body.faces() {
                let bx = face_box(body, f).grow(face_tolerance(body, f));
                let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                for p in bx.corners() {
                    let (q, d) = (view.image(p), view.depth(p));
                    lo = [lo[0].min(q.x), lo[1].min(q.y), lo[2].min(d)];
                    hi = [hi[0].max(q.x), hi[1].max(q.y), hi[2].max(d)];
                }
                far = far.max(hi[2]);
                let image_box = Box3::new(
                    Point3::new(lo[0], lo[1], lo[2]),
                    Point3::new(hi[0], hi[1], hi[2]),
                );
                boxes.push((image_box, faces.len()));
                faces.push((b, f));
            }
        }
        Self {
            view,
            bodies,
            faces,
            tree: BoxTree::new(boxes),
            far,
        }
    }

    /// The faces whose image box holds `p`'s image and reaches in front
    /// of it, in face order.
    fn candidates(&self, p: Point3) -> Vec<usize> {
        let (q, d) = (self.view.image(p), self.view.depth(p));
        if self.faces.is_empty() || d > self.far {
            return Vec::new();
        }
        let probe = BoxTree::new(vec![(
            Box3::new(Point3::new(q.x, q.y, d), Point3::new(q.x, q.y, self.far)),
            0,
        )]);
        self.tree
            .pairs_with(&probe)
            .into_iter()
            .map(|(i, _)| i)
            .collect()
    }

    /// The faces a ray from `p` towards the viewer crosses in front of it.
    pub(super) fn count(&self, p: Point3, ray: &Ray, m: &Meter) -> Count {
        let Ok(line) = Line::new(p, self.view.towards().get()) else {
            return Count::Unsure;
        };
        let line = Curve::Line(line);
        let reach = (self.far - self.view.depth(p)).max(0.0) + PAST;
        let mut n = 0u32;
        for i in self.candidates(p) {
            // A face a ray meets.
            if m.charge(1).is_err() {
                return Count::Unsure;
            }
            let (b, f) = self.faces[i];
            let body = self.bodies[b];
            let own = b == ray.body && ray.own.contains(&f);
            let skip = if own {
                ray.skip_own.max(ray.near)
            } else {
                ray.near
            };
            let surface = &body.surfaces[body.faces[f].surface];
            let hits = m.capped(Scope::Search, || {
                curve_surface_in(&line, [0.0, reach], surface, LINEAR_RESOLUTION, m)
            });
            let Ok(hits) = hits else {
                return Count::Unsure;
            };
            for hit in hits {
                match hit {
                    CurveHit::On { t } => {
                        if own || ray.lenient || t[1] <= skip {
                            continue;
                        }
                        return Count::Unsure;
                    }
                    CurveHit::Point {
                        t,
                        point,
                        before,
                        after,
                    } => {
                        if t <= skip {
                            continue;
                        }
                        let (u, v) = surface.project(point).param;
                        match point_in_face(body, f, Point2::new(u, v)) {
                            Ok(Place::Outside) => {}
                            Ok(Place::Inside) if before != after => n += 1,
                            Ok(_) if ray.lenient => {}
                            _ => return Count::Unsure,
                        }
                    }
                }
            }
        }
        Count::Settled(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    #[test]
    fn a_ray_counts_the_faces_it_crosses_in_front() {
        let (b, _) = primitive::block(OpKey(1), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
        let v = View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let bodies = [&b];
        let scene = Scene::new(&v, &bodies);
        // unmetered: a unit test
        let m = Meter::free();
        let ray = Ray {
            body: 0,
            own: &[],
            skip_own: 0.0,
            near: 1e-6,
            lenient: false,
        };
        let at = |y: f64| scene.count(Point3::new(0.0041, y, 0.0063), &ray, &m);
        assert_eq!(at(-0.001), Count::Settled(0));
        assert_eq!(at(0.005), Count::Settled(1));
        assert_eq!(at(0.02), Count::Settled(2));
        // A point on the back face, its own: only the front face hides it.
        let back: Vec<FaceId> = b
            .faces()
            .filter(|&f| {
                let bx = face_box(&b, f);
                (bx.min().y - 0.01).abs() < 1e-9 && (bx.max().y - 0.01).abs() < 1e-9
            })
            .collect();
        assert_eq!(back.len(), 1);
        let own = Ray {
            own: &back,
            skip_own: 1e-6,
            ..ray
        };
        assert_eq!(
            scene.count(Point3::new(0.0041, 0.01, 0.0063), &own, &m),
            Count::Settled(1)
        );
    }

    #[test]
    fn a_ray_along_an_edge_is_unsure_unless_the_piece_lies_on_others() {
        let (b, _) = primitive::block(OpKey(2), 0.01, 0.01, 0.01, &Rigid::IDENTITY).unwrap();
        let v = View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let bodies = [&b];
        let scene = Scene::new(&v, &bodies);
        // unmetered: a unit test
        let m = Meter::free();
        let ray = Ray {
            body: 0,
            own: &[],
            skip_own: 0.0,
            near: 1e-6,
            lenient: false,
        };
        let p = Point3::new(0.01, 0.02, 0.005);
        assert_eq!(scene.count(p, &ray, &m), Count::Unsure);
        let lenient = Ray {
            lenient: true,
            ..ray
        };
        assert_eq!(scene.count(p, &lenient, &m), Count::Settled(0));
    }
}
