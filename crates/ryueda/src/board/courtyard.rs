//! A footprint's courtyard (design 732): the closed loops its courtyard
//! graphics make (rectangles, circles and polygons close themselves;
//! lines and arcs chain by equal endpoints, either way round). One that
//! doesn't close is its graphics' box, and says so.

use crate::footprint::{Graphic, Shape as Drawn};
use ryu_board::geom::{Arc, Contour, Edge, Point, Rect, Region};
use ryu_board::model::Polarity;
use ryu_board::shape::Shape;

fn pt(p: [i64; 2]) -> Point {
    Point::new(p[0], p[1])
}

fn closed(edges: Vec<Edge>) -> Option<Region> {
    Contour::new(edges)
        .ok()
        .map(|c| Region::solid(super::outline::ccw(c)))
}

fn rect_region(r: Rect) -> Option<Region> {
    let p = [
        r.min,
        Point::new(r.max.x, r.min.y),
        r.max,
        Point::new(r.min.x, r.max.y),
    ];
    closed((0..4).map(|i| Edge::line(p[i], p[(i + 1) % 4])).collect())
}

/// The courtyard of `graphics` (all on one courtyard layer) in the
/// footprint's frame, and whether it was open; `None` with none.
pub fn of_graphics(graphics: &[&Graphic]) -> Option<(Shape, bool)> {
    if graphics.is_empty() {
        return None;
    }
    let mut regions = Vec::new();
    let mut loose: Vec<Edge> = Vec::new();
    let mut open = false;
    for g in graphics {
        match &g.shape {
            Drawn::Rect { start, end, .. } => {
                regions.extend(rect_region(Rect::new(pt(*start), pt(*end))))
            }
            Drawn::Circle {
                center, radius_nm, ..
            } => regions.extend(
                ryu_board::shape::circle(pt(*center), *radius_nm)
                    .ok()
                    .map(Region::solid),
            ),
            Drawn::Polygon { points, .. } if points.len() >= 3 => {
                let p: Vec<Point> = points.iter().map(|q| pt(*q)).collect();
                let n = p.len();
                match closed(
                    (0..n)
                        .map(|i| Edge::line(p[i], p[(i + 1) % n]))
                        .filter(|e| e.start() != e.end())
                        .collect(),
                ) {
                    Some(r) => regions.push(r),
                    None => open = true,
                }
            }
            Drawn::Line { start, end, .. } if start != end => {
                loose.push(Edge::line(pt(*start), pt(*end)))
            }
            Drawn::Arc {
                start, mid, end, ..
            } => match Arc::through(pt(*start), pt(*mid), pt(*end)) {
                Some(a) => loose.push(Edge::Arc(a)),
                None => open = true,
            },
            _ => {}
        }
    }
    // Chain the loose edges into loops.
    let mut used = vec![false; loose.len()];
    for i in 0..loose.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let first = loose[i];
        let mut chain = vec![first];
        let mut end = first.end();
        while end != first.start() {
            let next = (0..loose.len())
                .find(|&j| !used[j] && (loose[j].start() == end || loose[j].end() == end));
            let Some(j) = next else { break };
            used[j] = true;
            let e = if loose[j].start() == end {
                loose[j]
            } else {
                loose[j].reversed()
            };
            end = e.end();
            chain.push(e);
        }
        match (end == first.start()).then(|| closed(chain)).flatten() {
            Some(r) => regions.push(r),
            None => open = true,
        }
    }
    if open || regions.is_empty() {
        let b = graphics
            .iter()
            .filter_map(|g| super::graphic::drawn(&g.shape).bounds())
            .reduce(Rect::union)?;
        return Some((Shape::regions(Polarity::Positive, rect_region(b)), true));
    }
    Some((Shape::regions(Polarity::Positive, regions), false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::Shape as Drawn;

    fn g(shape: Drawn) -> crate::footprint::Graphic {
        serde_json::from_value(serde_json::json!({"layer": "courtyard.top", "type": "line", "start": [0, 0], "end": [1, 0], "width_nm": 50000}))
            .map(|mut x: crate::footprint::Graphic| { x.shape = shape; x })
            .unwrap()
    }

    #[test]
    fn rects_close_lines_chain_and_open_ones_fall_back_to_their_box() {
        let rect = g(Drawn::Rect {
            start: [-1000, 500],
            end: [1000, -500],
            width_nm: 50,
            fill: false,
        });
        let (s, open) = of_graphics(&[&rect]).unwrap();
        assert!(!open);
        assert_eq!(s.bounds().unwrap().max.x, 1000);
        let l = |a: [i64; 2], b: [i64; 2]| {
            g(Drawn::Line {
                start: a,
                end: b,
                width_nm: 50,
            })
        };
        let sq = [
            l([0, 0], [10, 0]),
            l([10, 10], [10, 0]),
            l([10, 10], [0, 10]),
            l([0, 10], [0, 0]),
        ];
        let (s, open) = of_graphics(&sq.iter().collect::<Vec<_>>()).unwrap();
        assert!(!open, "a reversed line still chains");
        assert_eq!(s.parts.len(), 1);
        let (s, open) = of_graphics(&sq[..3].iter().collect::<Vec<_>>()).unwrap();
        assert!(open);
        let b = s.bounds().unwrap();
        // The box of the lines as drawn: half the 50 nm pen beyond them.
        assert_eq!((b.min.x, b.min.y, b.max.x, b.max.y), (-25, -25, 35, 35));
        assert!(of_graphics(&[]).is_none());
    }
}
