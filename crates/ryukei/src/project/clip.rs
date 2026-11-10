//! A stretch of an image curve kept inside a circle: a drawing's detail
//! view (design 417, plan 9.B) draws its parent's pieces clipped to its
//! circle, cut where they cross it by the certified plane crossings
//! booleans meet curves with, each stretch between cuts kept by where its
//! middle is.

use super::{Image, ImageCurve};
use crate::geom::intersect2::{Meet2, crossings};
use crate::geom::{Circle2, Curve2, Frame2, Point2, Vector2};
use crate::intersect::Failure;
use crate::tol::LINEAR_RESOLUTION;
use core::f64::consts::TAU;

/// The stretches of `image`'s 3D curve parameter whose images lie inside
/// the circle about `centre` of `radius` (in the view's plane), in order;
/// a stretch that only touches the circle from inside or out is kept or
/// left whole by its middle. `Uncertain` where a crossing can't be
/// certified within resolution of both curves, `Unsupported` for a
/// radius that isn't positive.
pub fn inside_circle(
    image: &ImageCurve,
    centre: Point2,
    radius: f64,
) -> Result<Vec<[f64; 2]>, Failure> {
    if radius.is_nan() || radius <= 0.0 || !radius.is_finite() {
        return Err(Failure::Unsupported);
    }
    let [t0, t1] = image.range;
    let inside = |t: f64| (image.point(t) - centre).length() < radius;
    let (curve, map) = match &image.image {
        Image::Point(_) => {
            return Ok(if inside(t0) {
                vec![[t0, t1]]
            } else {
                Vec::new()
            });
        }
        Image::Curve { curve, map } => (curve, map),
    };
    let frame =
        Frame2::new(centre, Vector2::new(1.0, 0.0), true).map_err(|_| Failure::Unsupported)?;
    let circle = Curve2::Circle2(Circle2::new(frame, radius).map_err(|_| Failure::Unsupported)?);
    let s = [map.apply(t0), map.apply(t1)];
    let meets = crossings((curve, s), (&circle, [0.0, TAU]), LINEAR_RESOLUTION)
        .ok_or(Failure::Uncertain)?;
    let mut cuts = vec![t0, t1];
    for m in meets {
        match m {
            Meet2::Point { ta, .. } => cuts.push(image.param_of(ta)),
            Meet2::Overlap { ta, .. } => cuts.extend(ta.map(|x| image.param_of(x))),
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let mut out: Vec<[f64; 2]> = Vec::new();
    for w in cuts.windows(2) {
        if w[1] <= w[0] || !inside((w[0] + w[1]) / 2.0) {
            continue;
        }
        match out.last_mut() {
            Some(last) if last[1] == w[0] => last[1] = w[1],
            _ => out.push([w[0], w[1]]),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Curve, Frame, Line, Point3, Vector3};
    use crate::project::{View, project_curve};

    fn front() -> View {
        View::new(Vector3::new(0.0, -1.0, 0.0), Vector3::new(0.0, 0.0, 1.0)).unwrap()
    }

    #[test]
    fn a_line_through_the_circle_keeps_its_chord() {
        let line = Curve::Line(
            Line::new(Point3::new(-10.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)).unwrap(),
        );
        let im = project_curve(&line, [0.0, 20.0], &front()).unwrap();
        let kept = inside_circle(&im[0], Point2::new(0.0, 0.0), 3.0).unwrap();
        assert_eq!(kept.len(), 1);
        assert!(
            (kept[0][0] - 7.0).abs() < 1e-9 && (kept[0][1] - 13.0).abs() < 1e-9,
            "{kept:?}"
        );
    }

    #[test]
    fn a_circle_crossing_twice_keeps_its_arc_inside_and_one_outside_nothing() {
        let f = Frame::new(
            Point3::new(4.0, 0.0, 0.0),
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let c = Curve::Circle(crate::geom::Circle::new(f, 2.0).unwrap());
        let im = project_curve(&c, [0.0, TAU], &front()).unwrap();
        let kept: Vec<[f64; 2]> = im
            .iter()
            .flat_map(|i| inside_circle(i, Point2::new(0.0, 0.0), 3.0).unwrap())
            .collect();
        // The arc of the circle about (4, 0) of radius 2 inside radius 3
        // about the origin: its points at x < 2.625 (where they cross).
        let total: f64 = kept.iter().map(|r| r[1] - r[0]).sum();
        let half = crate::math::acos((2.625 - 4.0) / 2.0);
        assert!(
            (total - 2.0 * (core::f64::consts::PI - half)).abs() < 1e-9,
            "{total}"
        );
        let far = inside_circle(&im[0], Point2::new(100.0, 0.0), 1.0).unwrap();
        assert!(far.is_empty());
    }

    #[test]
    fn a_curve_wholly_inside_is_kept_whole_and_a_bad_radius_refused() {
        let line = Curve::Line(
            Line::new(Point3::new(0.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)).unwrap(),
        );
        let im = project_curve(&line, [0.0, 1.0], &front()).unwrap();
        assert_eq!(
            inside_circle(&im[0], Point2::new(0.0, 0.0), 5.0).unwrap(),
            vec![[0.0, 1.0]]
        );
        assert_eq!(
            inside_circle(&im[0], Point2::new(0.0, 0.0), 0.0),
            Err(Failure::Unsupported)
        );
    }
}
