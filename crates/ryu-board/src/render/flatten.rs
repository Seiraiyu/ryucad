//! Shapes as polygons in image units, for the rasterizer: arcs cut into
//! chords within a tolerance, pens swept along paths.
//!
//! A part becomes fills, each a set of rings filled by the nonzero
//! winding rule; the part covers what any of its fills covers. A region
//! is one fill (its holes run the other way). A path drawn with a pen is
//! the union of each chord swept by the pen: for a round or square pen
//! that is the convex hull of the pen at both ends; for a pen of any
//! shape, the pen at both ends and the parallelogram each of its edges
//! sweeps, which cover every point the moving pen passes over.

use std::f64::consts::PI;

use super::Map;
use crate::geom::{Arc, Contour, Dir, Edge, Pen, Region};
use crate::model::Polarity;
use crate::shape::{Piece, Shape};

/// A closed ring of points; the last joins the first.
pub(crate) type Ring = Vec<(f64, f64)>;

/// Rings filled together by the nonzero winding rule.
pub(crate) type Fill = Vec<Ring>;

/// One part of a shape, flattened.
#[derive(Clone, Debug, Default)]
pub(crate) struct Flat {
    pub positive: bool,
    pub fills: Vec<Fill>,
}

/// The most chords one arc is cut into, however large it is drawn.
const MAX_CHORDS: f64 = 65_536.0;

/// The chords an arc of `radius` (image units) sweeping `sweep` radians
/// needs to stay within `tol` of it: a chord across `θ` lies within
/// `r (1 - cos(θ/2)) <= r θ² / 8` of its arc, so `θ = sqrt(8 tol / r)`
/// is enough; at least one per quarter turn.
fn chords(radius: f64, sweep: f64, tol: f64) -> usize {
    let by_tol = if radius > tol {
        sweep / ryu_math::sqrt(8.0 * tol / radius)
    } else {
        1.0
    };
    by_tol.max(sweep * 2.0 / PI).ceil().clamp(1.0, MAX_CHORDS) as usize
}

/// The points of `arc` after its start and before its end, at angles
/// spread evenly along it from the start's, as image vectors from its
/// centre: the caller adds where the centre lands.
fn arc_offsets(arc: &Arc, map: &Map, tol: f64) -> Vec<(f64, f64)> {
    if arc.is_degenerate() {
        return Vec::new();
    }
    let r = arc.radius();
    let sweep = arc.sweep();
    let n = chords(r * map.scale(), sweep, tol);
    let s = arc.start.minus(arc.centre);
    let a0 = ryu_math::atan2(s.1 as f64, s.0 as f64);
    let sign = match arc.dir {
        Dir::Ccw => 1.0,
        Dir::Cw => -1.0,
    };
    (1..n)
        .map(|k| {
            let a = a0 + sign * sweep * (k as f64) / (n as f64);
            let (sin, cos) = ryu_math::sin_cos(a);
            map.vector(r * cos, r * sin)
        })
        .collect()
}

/// `edge` as image points from its start up to (not including) its end.
fn edge_points(edge: &Edge, map: &Map, tol: f64, out: &mut Ring) {
    out.push(map.point(edge.start()));
    if let Edge::Arc(a) = edge {
        let c = map.point(a.centre);
        out.extend(
            arc_offsets(a, map, tol)
                .into_iter()
                .map(|v| (c.0 + v.0, c.1 + v.1)),
        );
    }
}

/// A contour as a ring.
fn contour(c: &Contour, map: &Map, tol: f64) -> Ring {
    let mut ring = Vec::new();
    for e in c.edges() {
        edge_points(e, map, tol, &mut ring);
    }
    ring
}

/// A region as one fill: its outer contour and its holes.
fn region(r: &Region, map: &Map, tol: f64) -> Fill {
    std::iter::once(&r.outer)
        .chain(&r.holes)
        .map(|c| contour(c, map, tol))
        .collect()
}

/// A path's points in order, each edge's end included.
fn path(edges: &[Edge], map: &Map, tol: f64) -> Ring {
    let mut pts = Vec::new();
    for e in edges {
        edge_points(e, map, tol, &mut pts);
        pts.push(map.point(e.end()));
    }
    pts.dedup();
    pts
}

/// The convex hull of `points`, counterclockwise in image units (y down,
/// so clockwise as seen), by Andrew's monotone chain.
fn hull(mut points: Vec<(f64, f64)>) -> Ring {
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lower: Ring = Vec::new();
    for &p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Ring = Vec::new();
    for &p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A round or square pen about the origin, in image units.
fn pen_ring(pen: Pen, map: &Map, tol: f64) -> Ring {
    match pen {
        Pen::Round(w) => {
            let r = w as f64 / 2.0 * map.scale();
            let n = chords(r, 2.0 * PI, tol).max(4);
            (0..n)
                .map(|k| {
                    let (sin, cos) = ryu_math::sin_cos(2.0 * PI * (k as f64) / (n as f64));
                    (r * cos, r * sin)
                })
                .collect()
        }
        Pen::Square(w) => {
            let h = w as f64 / 2.0 * map.scale();
            vec![(-h, -h), (h, -h), (h, h), (-h, h)]
        }
    }
}

fn moved(ring: &Ring, by: (f64, f64)) -> Ring {
    ring.iter().map(|p| (p.0 + by.0, p.1 + by.1)).collect()
}

/// A path drawn with a round or square pen: one fill of the hulls of
/// each chord's two ends.
fn stroke(edges: &[Edge], pen: Pen, map: &Map, tol: f64) -> Fill {
    let pen = pen_ring(pen, map, tol);
    let pts = path(edges, map, tol);
    if pts.len() == 1 {
        return vec![moved(&pen, pts[0])];
    }
    pts.windows(2)
        .map(|w| {
            let mut both = moved(&pen, w[0]);
            both.extend(moved(&pen, w[1]));
            hull(both)
        })
        .collect()
}

/// The signed area of a ring, twice over.
fn area2(ring: &Ring) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum()
}

/// A path drawn with a pen of any shape: the pen at each point of the
/// path, and one fill of the parallelograms its edges sweep, each turned
/// to wind the same way.
fn swept(path_edge: &Edge, pen: &[Region], map: &Map, tol: f64) -> Vec<Fill> {
    let pts = path(std::slice::from_ref(path_edge), map, tol);
    let pen_map = |r: &Region| -> Fill {
        std::iter::once(&r.outer)
            .chain(&r.holes)
            .map(|c| {
                let mut ring = Vec::new();
                for e in c.edges() {
                    let s = e.start();
                    ring.push(map.vector(s.x as f64, s.y as f64));
                    if let Edge::Arc(a) = e {
                        let c = map.vector(a.centre.x as f64, a.centre.y as f64);
                        ring.extend(
                            arc_offsets(a, map, tol)
                                .into_iter()
                                .map(|v| (c.0 + v.0, c.1 + v.1)),
                        );
                    }
                }
                ring
            })
            .collect()
    };
    let pens: Vec<Fill> = pen.iter().map(pen_map).collect();
    let mut fills = Vec::new();
    for p in &pts {
        for f in &pens {
            fills.push(f.iter().map(|r| moved(r, *p)).collect());
        }
    }
    let mut sweeps: Fill = Vec::new();
    for w in pts.windows(2) {
        for f in &pens {
            for ring in f {
                for i in 0..ring.len() {
                    let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                    let mut quad = vec![
                        (a.0 + w[0].0, a.1 + w[0].1),
                        (b.0 + w[0].0, b.1 + w[0].1),
                        (b.0 + w[1].0, b.1 + w[1].1),
                        (a.0 + w[1].0, a.1 + w[1].1),
                    ];
                    if area2(&quad) < 0.0 {
                        quad.reverse();
                    }
                    sweeps.push(quad);
                }
            }
        }
    }
    if !sweeps.is_empty() {
        fills.push(sweeps);
    }
    fills
}

/// Every part of `shape` flattened, in drawing order.
pub(crate) fn shape(shape: &Shape, map: &Map, tol: f64) -> Vec<Flat> {
    shape
        .parts
        .iter()
        .map(|p| Flat {
            positive: p.polarity == Polarity::Positive,
            fills: match &p.piece {
                Piece::Region(r) => vec![region(r, map, tol)],
                Piece::Stroke(s) => vec![stroke(&s.path, s.pen, map, tol)],
                Piece::Swept { path, pen } => swept(path, pen, map, tol),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point, Rect};
    use crate::render::View;
    use crate::shape::{circle, rect};
    use crate::symbol::Corners;

    fn map() -> Map {
        View {
            window: Rect::new(Point::new(-1000, -1000), Point::new(1000, 1000)),
            width: 200,
            height: 200,
            mirror: false,
        }
        .map(1.0)
    }

    #[test]
    fn arcs_are_cut_within_the_tolerance() {
        assert_eq!(chords(100.0, 2.0 * PI, 0.25), 45);
        assert_eq!(chords(0.1, 2.0 * PI, 0.25), 4);
        assert_eq!(chords(1e12, 2.0 * PI, 0.25), MAX_CHORDS as usize);
        let c = circle(Point::ORIGIN, 1000).unwrap();
        let ring = contour(&c, &map(), 0.25);
        // Every point on the circle of radius 100 pixels about (100, 100).
        for p in &ring {
            let d = ryu_math::sqrt((p.0 - 100.0) * (p.0 - 100.0) + (p.1 - 100.0) * (p.1 - 100.0));
            assert!((d - 100.0).abs() < 1e-9, "{p:?}");
        }
        assert_eq!(ring.len(), 45);
    }

    #[test]
    fn squares_stay_squares() {
        let r = rect(-500, -500, 500, 500, &Corners::SHARP).unwrap();
        let ring = contour(&r, &map(), 0.25);
        assert_eq!(
            ring,
            [(150.0, 150.0), (150.0, 50.0), (50.0, 50.0), (50.0, 150.0)]
        );
    }

    #[test]
    fn hulls_wind_one_way() {
        let h = hull(vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (0.5, 0.5),
        ]);
        assert_eq!(h.len(), 4);
        assert!(area2(&h) > 0.0);
    }

    #[test]
    fn a_square_pen_sweeps_a_hexagon_along_a_diagonal() {
        let f = stroke(
            &[Edge::line(Point::new(0, 0), Point::new(500, 500))],
            Pen::Square(200),
            &map(),
            0.25,
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].len(), 6);
        assert!((area2(&f[0]) / 2.0 - (400.0 + 20.0 * (50.0 + 50.0))).abs() < 1e-9);
    }
}
