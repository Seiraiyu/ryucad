//! A kernel piece's exact image as a drawing's path: lines, circular and
//! elliptic arcs, and a NURBS as its rational Bézier pieces, never a
//! polyline (design 326's decision, design 417's constraint).

use ryu_draw::{P, Path, Segment};
use ryukei::geom::Curve2;
use ryukei::project::{Image, ImageCurve, ImageMap};

fn pt(p: ryukei::geom::Point2) -> P {
    [p.x, p.y]
}

/// A stretch of an image curve that can't be drawn exactly: a NURBS the
/// kernel can't cut to it, or a curve and map no case here takes. The view
/// counts it and reports it (`drawing_view_uncertain`), never leaves it
/// out unsaid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uncut;

/// The image of `c` in the view's frame (model metres), from the 3D
/// curve's start of the stretch to its end; `Ok(None)` for an end-on edge,
/// whose image is a point, and `Err(Uncut)` for a stretch it can't draw.
pub fn path(c: &ImageCurve) -> Result<Option<Path>, Uncut> {
    let [t0, t1] = c.range;
    let (curve, map) = match &c.image {
        Image::Point(_) => return Ok(None),
        Image::Curve { curve, map } => (curve, map),
    };
    let start = pt(c.point(t0));
    let segments = match (curve, map) {
        // A line, or a conic seen edge-on (its cosine map one to one over
        // the stretch): straight between its ends.
        (Curve2::Line2(_), _) | (_, ImageMap::Cosine { .. }) => {
            vec![Segment::Line(pt(c.point(t1)))]
        }
        (Curve2::Circle2(k), ImageMap::Affine(_)) => {
            let [s0, s1] = [map.apply(t0), map.apply(t1)];
            let f = k.frame();
            let x = f.x();
            let turn = ryu_math::atan2(x.y, x.x);
            let sense = if f.is_ccw() { 1.0 } else { -1.0 };
            vec![Segment::Arc {
                centre: pt(f.origin()),
                radius: k.radius(),
                from: turn + sense * s0,
                sweep: sense * (s1 - s0),
            }]
        }
        (Curve2::Ellipse2(e), ImageMap::Affine(_)) => {
            let [s0, s1] = [map.apply(t0), map.apply(t1)];
            let f = e.frame();
            let (x, y) = (f.x(), f.y());
            vec![Segment::Ellipse {
                centre: pt(f.origin()),
                a: [x.x * e.major(), x.y * e.major()],
                b: [y.x * e.minor(), y.y * e.minor()],
                from: s0,
                sweep: s1 - s0,
            }]
        }
        (Curve2::Nurbs2(n), ImageMap::Affine(_)) => {
            let [s0, s1] = [map.apply(t0), map.apply(t1)];
            let forward = s0 <= s1;
            let part = n.part([s0.min(s1), s0.max(s1)]).map_err(|_| Uncut)?;
            let part = if forward { part } else { part.reversed() };
            bezier_pieces(&part).ok_or(Uncut)?
        }
        _ => return Err(Uncut),
    };
    Ok(Some(Path {
        start,
        segments,
        closed: false,
    }))
}

/// A clamped NURBS's Bézier pieces, in order: each interior knot raised
/// to the degree, then the control points read a degree at a time.
fn bezier_pieces(n: &ryukei::geom::NurbsCurve2) -> Option<Vec<Segment>> {
    let p = n.degree();
    let k = n.knots();
    let inner: Vec<f64> = k
        .knots()
        .iter()
        .zip(k.mults())
        .skip(1)
        .take(k.knots().len().saturating_sub(2))
        .flat_map(|(t, m)| std::iter::repeat_n(*t, p.saturating_sub(*m)))
        .collect();
    let full = if inner.is_empty() {
        n.clone()
    } else {
        n.refine(&inner).ok()?
    };
    let pts: Vec<P> = full.control_points().map(pt).collect();
    let w = full.weights();
    let rational = w.iter().any(|x| *x != w[0]);
    let mut out = Vec::new();
    let mut i = 0;
    while i + p < pts.len() {
        out.push(Segment::Bezier {
            points: pts[i + 1..=i + p].to_vec(),
            weights: rational.then(|| w[i..=i + p].to_vec()),
        });
        i += p;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryukei::geom::{Circle2, Frame2, Line2, NurbsCurve2, Point2, Vector2};

    fn ends(p: &Path) -> (P, P) {
        (p.start, p.segments.last().unwrap().end())
    }

    fn near(a: P, b: P) -> bool {
        (a[0] - b[0]).abs() < 1e-12 && (a[1] - b[1]).abs() < 1e-12
    }

    #[test]
    fn lines_arcs_and_splines_keep_their_ends_and_kinds() {
        // A line's stretch, the 3D parameter twice the image's.
        let l = Line2::new(Point2::new(1.0, 2.0), Vector2::new(1.0, 0.0)).unwrap();
        let c = ImageCurve {
            image: Image::Curve {
                curve: Curve2::Line2(l),
                map: ImageMap::Affine([0.0, 2.0]),
            },
            range: [0.0, 1.5],
        };
        let p = path(&c).unwrap().unwrap();
        assert!(near(ends(&p).0, [1.0, 2.0]) && near(ends(&p).1, [4.0, 2.0]));
        // A clockwise circle's quarter: an arc turning negatively.
        let f = Frame2::new(Point2::new(0.0, 0.0), Vector2::new(0.0, 1.0), false).unwrap();
        let k = Circle2::new(f, 2.0).unwrap();
        let c = ImageCurve {
            image: Image::Curve {
                curve: Curve2::Circle2(k),
                map: ImageMap::Affine([0.0, 1.0]),
            },
            range: [0.0, core::f64::consts::FRAC_PI_2],
        };
        let p = path(&c).unwrap().unwrap();
        let (a, b) = ends(&p);
        assert!(near(a, [0.0, 2.0]) && near(b, [2.0, 0.0]), "{a:?} {b:?}");
        match &p.segments[0] {
            Segment::Arc { sweep, .. } => assert!(*sweep < 0.0),
            s => panic!("{s:?}"),
        }
        // A cubic of two spans: two Bézier pieces, through its points.
        let knots = ryukei::geom::KnotVector::clamped(3, &[0.0, 0.5, 1.0], &[4, 1, 4]).unwrap();
        let n = NurbsCurve2::new(
            knots,
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, -1.0),
                Point2::new(3.0, 1.0),
                Point2::new(4.0, 0.0),
            ],
            None,
        )
        .unwrap();
        let c = ImageCurve {
            image: Image::Curve {
                curve: Curve2::Nurbs2(std::sync::Arc::new(n.clone())),
                map: ImageMap::Affine([0.0, 1.0]),
            },
            range: [0.25, 1.0],
        };
        let p = path(&c).unwrap().unwrap();
        assert_eq!(p.segments.len(), 2);
        let q = n.point(0.25);
        assert!(near(p.start, [q.x, q.y]));
        assert!(near(ends(&p).1, [4.0, 0.0]));
        // An end-on edge draws nothing.
        let c = ImageCurve {
            image: Image::Point(Point2::new(0.0, 0.0)),
            range: [0.0, 1.0],
        };
        assert_eq!(path(&c), Ok(None));
    }

    #[test]
    fn a_spline_stretch_that_cant_be_cut_is_uncut_not_nothing() {
        let knots = ryukei::geom::KnotVector::clamped(1, &[0.0, 1.0], &[2, 2]).unwrap();
        let n = NurbsCurve2::new(
            knots,
            vec![Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)],
            None,
        )
        .unwrap();
        // A stretch past the clamped domain: the kernel refuses to cut it.
        let c = ImageCurve {
            image: Image::Curve {
                curve: Curve2::Nurbs2(std::sync::Arc::new(n)),
                map: ImageMap::Affine([0.0, 1.0]),
            },
            range: [0.5, 1.5],
        };
        assert_eq!(path(&c), Err(Uncut));
    }
}
