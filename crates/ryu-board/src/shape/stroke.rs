//! The outlines of lines and arcs drawn with a pen.

use super::ShapeError;
use super::build::{circle, polygon};
use crate::geom::{Arc, Contour, Dir, Edge, Nm, Pen, Point, Region, half, orient, round_nm, span};

/// The unit normal to the left of the path at the start (`at_end` false)
/// or the end of an edge.
fn left_normal(e: &Edge, at_end: bool) -> (f64, f64) {
    match e {
        Edge::Segment(s) => {
            let (dx, dy) = ((s.b.x - s.a.x) as f64, (s.b.y - s.a.y) as f64);
            let l = ryu_math::sqrt(dx * dx + dy * dy);
            (-dy / l, dx / l)
        }
        Edge::Arc(a) => {
            let p = if at_end { a.end } else { a.start };
            let (dx, dy) = ((a.centre.x - p.x) as f64, (a.centre.y - p.y) as f64);
            let l = ryu_math::sqrt(dx * dx + dy * dy);
            match a.dir {
                Dir::Ccw => (dx / l, dy / l),
                Dir::Cw => (-dx / l, -dy / l),
            }
        }
    }
}

fn offset(p: Point, n: (f64, f64), h: Nm) -> (Point, Point) {
    let (ox, oy) = (round_nm(n.0 * h as f64), round_nm(n.1 * h as f64));
    (p.offset(ox, oy), p.offset(-ox, -oy))
}

fn moved(e: &Edge, a: Point, b: Point) -> Edge {
    match e {
        Edge::Segment(_) => Edge::line(a, b),
        Edge::Arc(arc) => Edge::Arc(Arc {
            start: a,
            end: b,
            centre: arc.centre,
            dir: arc.dir,
        }),
    }
}

/// The region a round pen of half-width `h` sweeps along a path whose
/// edges meet tangentially: the right side forward, a half circle round
/// the end, the left side back and a half circle round the start.
///
/// `None` where that outline isn't a simple region the caller can rely
/// on: an empty path, a zero pen, or an arc no larger than the pen. The
/// caller makes sure the ends don't reach each other.
pub fn path_outline(path: &[Edge], h: Nm) -> Option<Region> {
    if path.is_empty() || h <= 0 {
        return None;
    }
    for e in path {
        match e {
            Edge::Arc(a) if a.is_degenerate() || a.radius() <= h as f64 => return None,
            Edge::Segment(s) if s.a == s.b => return None,
            _ => {}
        }
    }
    // Offset points at every joint, from the normal where the edge before
    // ends, so neighbours share them exactly.
    let n = path.len();
    let mut left = Vec::with_capacity(n + 1);
    let mut right = Vec::with_capacity(n + 1);
    let (l, r) = offset(path[0].start(), left_normal(&path[0], false), h);
    left.push(l);
    right.push(r);
    for e in path {
        let (l, r) = offset(e.end(), left_normal(e, true), h);
        left.push(l);
        right.push(r);
    }
    let mut edges = Vec::with_capacity(2 * n + 2);
    for (i, e) in path.iter().enumerate() {
        edges.push(moved(e, right[i], right[i + 1]));
    }
    let end = path[n - 1].end();
    edges.push(Edge::Arc(Arc {
        start: right[n],
        end: left[n],
        centre: end,
        dir: Dir::Ccw,
    }));
    for (i, e) in path.iter().enumerate().rev() {
        edges.push(moved(e, left[i], left[i + 1]).reversed());
    }
    let start = path[0].start();
    edges.push(Edge::Arc(Arc {
        start: left[0],
        end: right[0],
        centre: start,
        dir: Dir::Ccw,
    }));
    Contour::new(edges).ok().map(Region::solid)
}

/// The region a line from `a` to `b` sweeps with a pen: a stadium for a
/// round pen (a disc when `a == b`), the convex hull of the pen at both
/// ends for a square one. `None` for a pen of no width.
pub fn line_outline(a: Point, b: Point, pen: Pen) -> Option<Region> {
    match pen {
        Pen::Round(w) => {
            let h = half(w);
            if h <= 0 {
                return None;
            }
            if a == b {
                return circle(a, h).ok().map(Region::solid);
            }
            path_outline(&[Edge::line(a, b)], h)
        }
        Pen::Square(w) => {
            let (lo, hi) = span(w);
            if lo >= hi {
                return None;
            }
            let mut pts = Vec::with_capacity(8);
            for p in [a, b] {
                for (dx, dy) in [(lo, lo), (hi, lo), (hi, hi), (lo, hi)] {
                    pts.push(p.offset(dx, dy));
                }
            }
            polygon(&hull(pts)).ok().map(Region::solid)
        }
    }
}

/// The region an arc sweeps with a round pen: a ring for a full circle,
/// else the band between its offsets with half circles at its ends.
/// `None` where that isn't a simple region (a pen at least as wide as
/// the radius, ends whose half circles reach each other, a square pen):
/// the caller keeps the arc as a stroke for stage 1.
pub fn arc_outline(arc: &Arc, pen: Pen) -> Result<Option<Region>, ShapeError> {
    let Pen::Round(w) = pen else { return Ok(None) };
    let h = half(w);
    if h <= 0 {
        return Ok(None);
    }
    if arc.is_degenerate() {
        return Ok(line_outline(arc.start, arc.end, pen));
    }
    let r = arc.radius();
    if arc.is_full() {
        let outer = circle(arc.centre, round_nm(r + h as f64))?;
        let inner = round_nm(r - h as f64);
        return Ok(Some(if inner > 0 {
            Region::ring(outer, circle(arc.centre, inner)?)
        } else {
            Region::solid(outer)
        }));
    }
    if r <= h as f64 {
        return Ok(None);
    }
    // The true end: where the ray through the written end meets the circle.
    let (ex, ey) = (
        (arc.end.x - arc.centre.x) as f64,
        (arc.end.y - arc.centre.y) as f64,
    );
    let k = r / ryu_math::sqrt(ex * ex + ey * ey);
    let end = arc.centre.offset(round_nm(ex * k), round_nm(ey * k));
    let (dx, dy) = ((end.x - arc.start.x) as f64, (end.y - arc.start.y) as f64);
    if dx * dx + dy * dy < (2 * h) as f64 * (2 * h) as f64 {
        return Ok(None);
    }
    Ok(path_outline(&[Edge::Arc(Arc { end, ..*arc })], h))
}

/// The convex hull of points, counterclockwise, by exact orientation
/// (Andrew's monotone chain), with no collinear points.
fn hull(mut pts: Vec<Point>) -> Vec<Point> {
    pts.sort();
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<Point> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && orient(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && orient(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    const PI: f64 = std::f64::consts::PI;

    #[test]
    fn round_lines_are_stadiums() {
        let r = line_outline(Point::new(0, 0), Point::new(1000, 0), Pen::Round(200)).unwrap();
        assert!((r.area2() / 2.0 - (1000.0 * 200.0 + PI * 10_000.0)).abs() < 1e-6);
        assert_eq!(r.outer.orientation(), Dir::Ccw);
        let dot = line_outline(Point::new(5, 5), Point::new(5, 5), Pen::Round(200)).unwrap();
        assert!((dot.area2() / 2.0 - PI * 10_000.0).abs() < 1e-6);
        assert!(line_outline(Point::new(0, 0), Point::new(1, 0), Pen::Round(0)).is_none());
        // Diagonal: the offsets round, but the area stays within a few nm².
        let d = line_outline(Point::new(0, 0), Point::new(3000, 4000), Pen::Round(1000)).unwrap();
        assert!((d.area2() / 2.0 - (5000.0 * 1000.0 + PI * 250_000.0)).abs() < 5_000.0);
    }

    #[test]
    fn square_lines_are_hulls() {
        let r = line_outline(Point::new(0, 0), Point::new(1000, 0), Pen::Square(200)).unwrap();
        assert_eq!(r.outer.polygon_area2(), Some(2 * 1200 * 200));
        let d = line_outline(Point::new(0, 0), Point::new(1000, 1000), Pen::Square(200)).unwrap();
        // A hexagon: the square plus the band its diagonal sweeps.
        assert_eq!(d.outer.edges().len(), 6);
        assert_eq!(
            d.outer.polygon_area2(),
            Some(2 * (200 * 200 + 2 * 1000 * 200))
        );
    }

    #[test]
    fn arcs() {
        let a = Arc {
            start: Point::new(10_000, 0),
            end: Point::new(0, 10_000),
            centre: Point::ORIGIN,
            dir: Dir::Ccw,
        };
        let r = arc_outline(&a, Pen::Round(2000)).unwrap().unwrap();
        // A quarter band from 9,000 to 11,000 and two half discs.
        let band = PI / 4.0 * ((11_000.0 * 11_000.0) - (9_000.0 * 9_000.0));
        assert!((r.area2() / 2.0 - (band + PI * 1e6)).abs() < 1.0);
        let cw = Arc { dir: Dir::Cw, ..a };
        assert_eq!(
            arc_outline(&cw, Pen::Round(2000))
                .unwrap()
                .unwrap()
                .outer
                .orientation(),
            Dir::Ccw
        );
        // A pen wider than the radius, and ends that meet, stay strokes.
        assert!(arc_outline(&a, Pen::Round(20_000)).unwrap().is_none());
        let nearly = Arc {
            end: Point::new(10_000, -100),
            ..a
        };
        assert!(arc_outline(&nearly, Pen::Round(2000)).unwrap().is_none());
        assert!(arc_outline(&a, Pen::Square(2000)).unwrap().is_none());
        let full = Arc { end: a.start, ..a };
        let ring = arc_outline(&full, Pen::Round(2000)).unwrap().unwrap();
        assert!(
            (ring.area2() / 2.0 - PI * ((11_000.0 * 11_000.0) - (9_000.0 * 9_000.0))).abs() < 1e-3
        );
    }

    #[test]
    fn tangent_paths() {
        // A line into a half circle: one band, two caps.
        let path = [
            Edge::line(Point::new(0, 0), Point::new(10_000, 0)),
            Edge::Arc(Arc {
                start: Point::new(10_000, 0),
                end: Point::new(10_000, 10_000),
                centre: Point::new(10_000, 5_000),
                dir: Dir::Ccw,
            }),
        ];
        let r = path_outline(&path, 1000).unwrap();
        let expect =
            10_000.0 * 2_000.0 + PI / 2.0 * ((6_000.0 * 6_000.0) - (4_000.0 * 4_000.0)) + PI * 1e6;
        assert!((r.area2() / 2.0 - expect).abs() < 1.0);
    }
}
