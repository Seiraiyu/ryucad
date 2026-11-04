//! The exact image of a curve in a view's plane (design 326, phase 1).
//!
//! Orthographic projection is affine, so a line's image is a line, a
//! conic's a conic (or a segment edge-on) and a NURBS's the NURBS of its
//! projected control points with the same knots and weights. A curve is
//! cut where its tangent runs along the view (a fold), so each piece is
//! one to one onto its image; depth stays with the 3D curve and its
//! range.

use super::View;
use crate::geom::{Circle2, Curve, Curve2, Ellipse2, Frame2, Line2, Point2, Point3, Vector3};
use crate::intersect::Failure;
use crate::math;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use core::f64::consts::{FRAC_PI_2, PI};

/// The image of a stretch of a curve.
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    /// The stretch runs along the view: its image is a point (an end-on
    /// edge).
    Point(Point2),
    /// A plane curve, and how its parameter follows the 3D curve's.
    Curve { curve: Curve2, map: ImageMap },
}

/// How an image curve's parameter `s` follows its 3D curve's `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageMap {
    /// `s = a + b t`.
    Affine([f64; 2]),
    /// `s = amplitude cos(t − phase)`: a circle or an ellipse seen
    /// edge-on, along a `Line2`; one to one between `phase + kπ`.
    Cosine { amplitude: f64, phase: f64 },
}

impl ImageMap {
    pub fn apply(self, t: f64) -> f64 {
        match self {
            Self::Affine([a, b]) => a + b * t,
            Self::Cosine { amplitude, phase } => amplitude * math::cos(t - phase),
        }
    }
}

/// One stretch `range` of a 3D curve and its image, one to one.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageCurve {
    pub image: Image,
    pub range: [f64; 2],
}

impl ImageCurve {
    /// The image of the 3D curve's point at `t`.
    pub fn point(&self, t: f64) -> Point2 {
        match &self.image {
            Image::Point(p) => *p,
            Image::Curve { curve, map } => curve.point(map.apply(t)),
        }
    }
}

/// The image of `curve` over `range` (increasing) in `view`, cut at its
/// folds. `Uncertain` for a range that doesn't increase, or a NURBS
/// stretch running along the view that is neither a point nor a fold.
pub fn project_curve(
    curve: &Curve,
    range: [f64; 2],
    view: &View,
) -> Result<Vec<ImageCurve>, Failure> {
    if range[0].partial_cmp(&range[1]) != Some(core::cmp::Ordering::Less) {
        return Err(Failure::Uncertain);
    }
    match curve {
        Curve::Line(l) => Ok(vec![line(view, l.origin(), l.dir().get(), range)?]),
        Curve::Circle(c) => {
            let f = c.frame();
            let r = c.radius();
            let axes = (f.x().get(), f.y().get(), f.z().get());
            conic(view, f.origin(), axes, (r, r), true, range)
        }
        Curve::Ellipse(e) => {
            let f = e.frame();
            let axes = (f.x().get(), f.y().get(), f.z().get());
            conic(view, f.origin(), axes, (e.major(), e.minor()), false, range)
        }
        Curve::Nurbs(n) => super::folds::nurbs(n, range, view),
    }
}

fn line(view: &View, origin: Point3, dir: Vector3, range: [f64; 2]) -> Result<ImageCurve, Failure> {
    let d = view.image_vector(dir);
    let speed = d.length();
    if speed <= ANGULAR_RESOLUTION {
        let mid = (range[0] + range[1]) / 2.0;
        return Ok(ImageCurve {
            image: Image::Point(view.image(origin + dir * mid)),
            range,
        });
    }
    // A unit direction: `Line2::new` refuses one shorter than resolution.
    let l = Line2::new(view.image(origin), d / speed).map_err(|_| Failure::Uncertain)?;
    Ok(ImageCurve {
        image: Image::Curve {
            curve: Curve2::Line2(l),
            map: ImageMap::Affine([0.0, speed]),
        },
        range,
    })
}

/// `origin + a cos t x + b sin t y` (a circle when `circle`, `a == b`)
/// in the plane with normal `z`.
fn conic(
    view: &View,
    origin: Point3,
    (x, y, z): (Vector3, Vector3, Vector3),
    (a, b): (f64, f64),
    circle: bool,
    [t0, t1]: [f64; 2],
) -> Result<Vec<ImageCurve>, Failure> {
    let bad = |_| Failure::Uncertain;
    let t = view.towards().get();
    let c = z.dot(t);
    let o = view.image(origin);
    let one = |curve: Curve2, map: ImageMap| {
        Ok(vec![ImageCurve {
            image: Image::Curve { curve, map },
            range: [t0, t1],
        }])
    };
    // Facing: the plane square to the view. Its image keeps the frame,
    // turned clockwise when seen from behind.
    if z.cross(t).length() <= ANGULAR_RESOLUTION {
        let frame = Frame2::new(o, view.image_vector(x), c > 0.0).map_err(bad)?;
        let curve = if circle {
            Curve2::Circle2(Circle2::new(frame, a).map_err(bad)?)
        } else {
            Curve2::Ellipse2(Ellipse2::new(frame, a, b).map_err(bad)?)
        };
        return one(curve, ImageMap::Affine([0.0, 1.0]));
    }
    // Oblique: the image's principal axes from its conjugate ones
    // `U = a x̄`, `V = b ȳ`: `U cos t + V sin t = A cos(t − t₀) + B sin(t − t₀)`
    // with `tan 2t₀ = 2 U·V / (U·U − V·V)`, `A` the longer.
    let (u, v) = (view.image_vector(x) * a, view.image_vector(y) * b);
    let mut t0p = 0.5 * math::atan2(2.0 * u.dot(v), u.dot(u) - v.dot(v));
    let axis = |s: f64| {
        let (sn, cs) = math::sin_cos(s);
        u * cs + v * sn
    };
    if axis(t0p).length() < axis(t0p + FRAC_PI_2).length() {
        t0p += FRAC_PI_2;
    }
    let (major, minor) = (axis(t0p), axis(t0p + FRAC_PI_2));
    let edge_on = c.abs() <= ANGULAR_RESOLUTION || minor.length() < LINEAR_RESOLUTION;
    if !edge_on {
        let frame = Frame2::new(o, major, major.cross(minor) > 0.0).map_err(bad)?;
        let e = Ellipse2::new(frame, major.length(), minor.length()).map_err(bad)?;
        return one(Curve2::Ellipse2(e), ImageMap::Affine([-t0p, 1.0]));
    }
    // Edge-on: along `w`, the plane's direction square to the view, the
    // image is `(a x·w) cos t + (b y·w) sin t = R cos(t − φ)`, folding at
    // `φ + kπ`; every image point is within resolution of the segment.
    let w = z.cross(t);
    let w = w / w.length();
    let (p, q) = (a * x.dot(w), b * y.dot(w));
    let amplitude = math::sqrt(p * p + q * q);
    let phase = math::atan2(q, p);
    let l = Line2::new(o, view.image_vector(w)).map_err(bad)?;
    let map = ImageMap::Cosine { amplitude, phase };
    let first = ((t0 - phase) / PI).floor() as i64 + 1;
    let mut ends = vec![t0];
    let mut k = first;
    loop {
        let f = phase + k as f64 * PI;
        if f >= t1 {
            break;
        }
        if f > t0 {
            ends.push(f);
        }
        k += 1;
    }
    ends.push(t1);
    Ok(ends
        .windows(2)
        .map(|r| ImageCurve {
            image: Image::Curve {
                curve: Curve2::Line2(l),
                map,
            },
            range: [r[0], r[1]],
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Ellipse, Frame, Line};
    use core::f64::consts::{FRAC_PI_2, PI, TAU};

    /// Every piece's image follows the curve's projection at 33 samples,
    /// one to one: its image parameter is monotone along the piece.
    pub(crate) fn follows(curve: &Curve, range: [f64; 2], view: &View, pieces: &[ImageCurve]) {
        assert_eq!(pieces[0].range[0], range[0]);
        assert_eq!(pieces[pieces.len() - 1].range[1], range[1]);
        for w in pieces.windows(2) {
            assert_eq!(w[0].range[1], w[1].range[0]);
        }
        for p in pieces {
            let mut last: Option<f64> = None;
            let mut dir = 0.0;
            for i in 0..=32 {
                let t = p.range[0] + (p.range[1] - p.range[0]) * i as f64 / 32.0;
                let want = view.image(curve.point(t));
                let got = p.point(t);
                assert!(got.distance(want) <= 1e-12, "{t}: {got:?} against {want:?}");
                if let Image::Curve { map, .. } = &p.image {
                    let s = map.apply(t);
                    if let Some(l) = last {
                        let d = s - l;
                        assert!(d * dir >= -1e-15, "not one to one at {t}");
                        if dir == 0.0 {
                            dir = d.signum();
                        }
                    }
                    last = Some(s);
                }
            }
        }
    }

    fn front() -> View {
        View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap()
    }

    fn askew() -> View {
        View::new(Vector3::new(1.0, -2.0, 0.5), Vector3::new(0.0, 0.0, 1.0)).unwrap()
    }

    #[test]
    fn a_line_projects_to_a_line_or_to_a_point_end_on() {
        let l = Curve::Line(
            Line::new(Point3::new(0.1, 0.2, 0.3), Vector3::new(1.0, 1.0, 1.0)).unwrap(),
        );
        for v in [front(), askew()] {
            let p = project_curve(&l, [-0.5, 2.0], &v).unwrap();
            assert_eq!(p.len(), 1);
            follows(&l, [-0.5, 2.0], &v, &p);
        }
        let along = Curve::Line(
            Line::new(Point3::new(0.1, 0.2, 0.3), Vector3::new(0.0, 1.0, 0.0)).unwrap(),
        );
        let p = project_curve(&along, [0.0, 1.0], &front()).unwrap();
        assert!(matches!(p[0].image, Image::Point(q) if q.distance(Point2::new(0.1, 0.3)) < 1e-15));
    }

    #[test]
    fn a_circle_facing_the_view_is_a_circle_of_its_radius() {
        let f = Frame::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let c = Curve::Circle(Circle::new(f, 0.25).unwrap());
        let p = project_curve(&c, [0.0, TAU], &front()).unwrap();
        assert!(
            matches!(&p[0].image, Image::Curve { curve: Curve2::Circle2(k), map: ImageMap::Affine([0.0, 1.0]) } if k.radius() == 0.25)
        );
        follows(&c, [0.0, TAU], &front(), &p);
        // Its back, seen from behind, runs clockwise.
        let back = View::new(Vector3::new(0.0, 1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap();
        let q = project_curve(&c, [0.0, TAU], &back).unwrap();
        follows(&c, [0.0, TAU], &back, &q);
    }

    #[test]
    fn an_oblique_circle_is_an_ellipse_of_radius_r_and_r_cos() {
        let f = Frame::new(
            Point3::new(0.1, 0.0, 0.2),
            Vector3::new(0.0, -1.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let c = Curve::Circle(Circle::new(f, 0.5).unwrap());
        let p = project_curve(&c, [0.3, 2.0], &front()).unwrap();
        let Image::Curve {
            curve: Curve2::Ellipse2(e),
            ..
        } = &p[0].image
        else {
            panic!("{p:?}")
        };
        assert!((e.major() - 0.5).abs() < 1e-15);
        assert!((e.minor() - 0.5 * core::f64::consts::FRAC_1_SQRT_2).abs() < 1e-15);
        follows(&c, [0.3, 2.0], &front(), &p);
    }

    #[test]
    fn an_ellipse_projects_through_its_conjugate_axes() {
        let f = Frame::new(
            Point3::new(0.0, 0.3, 0.0),
            Vector3::new(0.2, 0.4, 1.0),
            Vector3::new(1.0, -0.3, 0.0),
        )
        .unwrap();
        let e = Curve::Ellipse(Ellipse::new(f, 0.6, 0.2).unwrap());
        for v in [front(), askew()] {
            let p = project_curve(&e, [-1.0, 5.0], &v).unwrap();
            assert_eq!(p.len(), 1);
            follows(&e, [-1.0, 5.0], &v, &p);
        }
    }

    #[test]
    fn an_edge_on_circle_is_a_segment_cut_at_its_two_folds() {
        let f = Frame::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.6, 0.8, 0.0),
        )
        .unwrap();
        let c = Curve::Circle(Circle::new(f, 0.5).unwrap());
        let p = project_curve(&c, [0.0, TAU], &front()).unwrap();
        assert_eq!(p.len(), 3, "{p:?}");
        for piece in &p {
            assert!(matches!(
                piece.image,
                Image::Curve {
                    curve: Curve2::Line2(_),
                    map: ImageMap::Cosine { .. }
                }
            ));
        }
        follows(&c, [0.0, TAU], &front(), &p);
        // The folds are where the circle's tangent runs along the view.
        for t in [p[0].range[1], p[1].range[1]] {
            let d = c.eval(t).d1;
            assert!(d.cross(Vector3::new(0.0, 1.0, 0.0)).length() < 1e-12 * d.length());
        }
        let _ = (FRAC_PI_2, PI);
    }

    #[test]
    fn a_spline_projects_with_its_own_knots_and_no_folds_in_general() {
        use crate::geom::{KnotVector, NurbsCurve};
        use std::sync::Arc;
        let kv = KnotVector::clamped(3, &[0.0, 0.5, 1.0], &[4, 1, 4]).unwrap();
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.2, 0.3, 0.1),
            Point3::new(0.5, -0.1, 0.4),
            Point3::new(0.8, 0.2, 0.2),
            Point3::new(1.0, 0.0, 0.5),
        ];
        let n = Curve::Nurbs(Arc::new(
            NurbsCurve::new(kv, pts, Some(vec![1.0, 0.8, 1.2, 0.9, 1.0])).unwrap(),
        ));
        for v in [front(), askew()] {
            let p = project_curve(&n, [0.1, 0.9], &v).unwrap();
            assert_eq!(p.len(), 1, "{p:?}");
            follows(&n, [0.1, 0.9], &v, &p);
        }
    }

    #[test]
    fn a_spline_circle_seen_edge_on_folds_twice() {
        let f = Frame::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.6, 0.8, 0.0),
        )
        .unwrap();
        let (n, _) = Circle::new(f, 0.5).unwrap().to_nurbs().unwrap();
        let c = Curve::Nurbs(std::sync::Arc::new(n));
        let [d0, d1] = c.domain();
        let p = project_curve(&c, [d0, d1], &front()).unwrap();
        assert_eq!(p.len(), 3, "{p:?}");
        follows(&c, [d0, d1], &front(), &p);
    }

    #[test]
    fn a_spline_along_the_view_is_a_point() {
        use crate::geom::{KnotVector, NurbsCurve};
        let kv = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let pts = vec![
            Point3::new(0.1, 0.0, 0.2),
            Point3::new(0.1, 0.5, 0.2),
            Point3::new(0.1, 1.0, 0.2),
        ];
        let c = Curve::Nurbs(std::sync::Arc::new(NurbsCurve::new(kv, pts, None).unwrap()));
        let p = project_curve(&c, [0.0, 1.0], &front()).unwrap();
        assert!(matches!(
            p[..],
            [ImageCurve {
                image: Image::Point(_),
                ..
            }]
        ));
    }
}
