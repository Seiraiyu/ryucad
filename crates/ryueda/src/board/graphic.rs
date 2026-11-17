//! A footprint's drawn shapes (`footprint::Shape`: lines, rectangles,
//! arcs, circles, polygons, Béziers) as `ryu-board` shapes in the
//! footprint's own frame. Text is [`super::text`]'s.

use crate::footprint::Shape as Drawn;
use ryu_board::geom::{Arc, Contour, Edge, Pen, Point, Region, Stroke, div_round, half};
use ryu_board::model::Polarity;
use ryu_board::shape::{Part, Piece, Shape, circle, polygon};

fn pt(p: [i64; 2]) -> Point {
    Point::new(p[0], p[1])
}

fn stroke(path: Vec<Edge>, w: i64) -> Part {
    Part { polarity: Polarity::Positive, piece: Piece::Stroke(Stroke { path, pen: Pen::Round(w) }) }
}

fn region(r: Region) -> Part {
    Part { polarity: Polarity::Positive, piece: Piece::Region(r) }
}

/// A closed polygon through `points`, counterclockwise.
fn ccw_polygon(points: &[Point]) -> Option<Contour> {
    let c = polygon(points).ok()?;
    Some(if c.polygon_area2().unwrap_or(0) < 0 { c.reversed() } else { c })
}

fn closed_path(points: &[Point]) -> Vec<Edge> {
    let n = points.len();
    (0..n).map(|i| Edge::line(points[i], points[(i + 1) % n])).collect()
}

/// A cubic Bézier as 16 straight segments, by exact integer
/// de Casteljau at t = k/16.
fn bezier(p: [Point; 4]) -> Vec<Edge> {
    let at = |k: i128| {
        let (t, u) = (k, 16 - k);
        let w = [u * u * u, 3 * u * u * t, 3 * u * t * t, t * t * t];
        let x: i128 = (0..4).map(|i| w[i] * i128::from(p[i].x)).sum();
        let y: i128 = (0..4).map(|i| w[i] * i128::from(p[i].y)).sum();
        Point::new(div_round(x, 4096) as i64, div_round(y, 4096) as i64)
    };
    let pts: Vec<Point> = (0..=16).map(at).collect();
    pts.windows(2).filter(|w| w[0] != w[1]).map(|w| Edge::line(w[0], w[1])).collect()
}

/// A drawn shape as parts in the footprint's frame; empty when it draws
/// nothing (no width and no fill). Text gives nothing here.
pub fn drawn(s: &Drawn) -> Shape {
    let mut shape = Shape::default();
    let parts = &mut shape.parts;
    match s {
        Drawn::Line { start, end, width_nm } => {
            if *width_nm > 0 && start != end {
                parts.push(stroke(vec![Edge::line(pt(*start), pt(*end))], *width_nm));
            }
        }
        Drawn::Rect { start, end, width_nm, fill } => {
            let (a, b) = (pt(*start), pt(*end));
            let corners = [a, Point::new(b.x, a.y), b, Point::new(a.x, b.y)];
            if *fill
                && let Some(c) = ccw_polygon(&corners)
            {
                parts.push(region(Region::solid(c)));
            }
            if *width_nm > 0 {
                parts.push(stroke(closed_path(&corners), *width_nm));
            }
        }
        Drawn::Arc { start, mid, end, width_nm } => {
            if *width_nm > 0
                && let Some(a) = Arc::through(pt(*start), pt(*mid), pt(*end))
            {
                parts.push(stroke(vec![Edge::Arc(a)], *width_nm));
            }
        }
        Drawn::Circle { center, radius_nm, width_nm, fill } => {
            let c = pt(*center);
            let (r, w) = (*radius_nm, *width_nm);
            let outer = r + half(w);
            if *fill {
                if let Ok(k) = circle(c, outer) {
                    parts.push(region(Region::solid(k)));
                }
            } else if w > 0 {
                let inner = r - half(w);
                match (circle(c, outer), circle(c, inner)) {
                    (Ok(o), Ok(i)) => parts.push(region(Region::ring(o, i))),
                    (Ok(o), Err(_)) => parts.push(region(Region::solid(o))),
                    _ => {}
                }
            }
        }
        Drawn::Polygon { points, width_nm, fill } => {
            let pts: Vec<Point> = points.iter().copied().map(pt).collect();
            if *fill
                && let Some(c) = ccw_polygon(&pts)
            {
                parts.push(region(Region::solid(c)));
            }
            if *width_nm > 0 && pts.len() > 1 {
                parts.push(stroke(closed_path(&pts), *width_nm));
            }
        }
        Drawn::Bezier { points, width_nm } => {
            let path = bezier(points.map(pt));
            if *width_nm > 0 && !path.is_empty() {
                parts.push(stroke(path, *width_nm));
                // Within a nanometre of rounding per point; the chords'
                // own deviation is left to the approximate mark.
                shape.approximate = Some(1);
            }
        }
        Drawn::Text { .. } => {}
    }
    shape
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(v: serde_json::Value) -> Drawn {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn lines_rects_and_circles_have_their_written_sizes() {
        let l = drawn(&d(json!({"type": "line", "start": [0, 0], "end": [1000, 0], "width_nm": 100})));
        let b = l.bounds().unwrap();
        assert_eq!((b.min, b.max), (Point::new(-50, -50), Point::new(1050, 50)));
        let r = drawn(&d(json!({"type": "rect", "start": [0, 1000], "end": [2000, 0], "width_nm": 0, "fill": true})));
        let Piece::Region(reg) = &r.parts[0].piece else { panic!() };
        assert_eq!(reg.outer.polygon_area2(), Some(2 * 2000 * 1000));
        let c = drawn(&d(json!({"type": "circle", "center": [0, 0], "radius_nm": 1000, "width_nm": 200})));
        let Piece::Region(ring) = &c.parts[0].piece else { panic!() };
        assert_eq!(ring.holes.len(), 1);
        assert_eq!(c.bounds().unwrap().max, Point::new(1100, 1100));
    }

    #[test]
    fn polygons_turn_counterclockwise_and_beziers_are_marked() {
        let p = drawn(&d(json!({"type": "polygon", "points": [[0, 0], [0, 10], [10, 10], [10, 0]], "width_nm": 0, "fill": true})));
        let Piece::Region(reg) = &p.parts[0].piece else { panic!() };
        assert!(reg.outer.polygon_area2().unwrap() > 0);
        let b = drawn(&d(json!({"type": "bezier", "points": [[0, 0], [0, 100], [100, 100], [100, 0]], "width_nm": 10})));
        assert_eq!(b.approximate, Some(1));
        let Piece::Stroke(s) = &b.parts[0].piece else { panic!() };
        assert_eq!(s.path.first().unwrap().start(), Point::new(0, 0));
        assert_eq!(s.path.last().unwrap().end(), Point::new(100, 0));
        assert!(drawn(&d(json!({"type": "line", "start": [0, 0], "end": [5, 0], "width_nm": 0}))).parts.is_empty());
    }
}
