//! Contour builders: polygons, rectangles with treated corners, circles.

use super::ShapeError;
use crate::geom::{Arc, Contour, Dir, Edge, Nm, Point};
use crate::symbol::Corners;

/// A polygon through `points`, counterclockwise as given; repeated points
/// (a corner a rounding used up) are dropped.
pub fn polygon(points: &[Point]) -> Result<Contour, ShapeError> {
    let mut p: Vec<Point> = Vec::with_capacity(points.len());
    for &q in points {
        if p.last() != Some(&q) {
            p.push(q);
        }
    }
    while p.len() > 1 && p.first() == p.last() {
        p.pop();
    }
    if p.len() < 3 {
        return Err(ShapeError::BadParameters("a polygon needs three corners"));
    }
    let n = p.len();
    Ok(
        Contour::new((0..n).map(|i| Edge::line(p[i], p[(i + 1) % n])).collect())
            .expect("closed by construction"),
    )
}

/// A full circle about `c`, counterclockwise from its rightmost point.
pub fn circle(c: Point, r: Nm) -> Result<Contour, ShapeError> {
    if r <= 0 {
        return Err(ShapeError::BadParameters("a circle needs a radius"));
    }
    let s = c.offset(r, 0);
    Ok(Contour::new(vec![Edge::Arc(Arc {
        start: s,
        end: s,
        centre: c,
        dir: Dir::Ccw,
    })])
    .expect("one full circle"))
}

/// The rectangle `[x0, x1] × [y0, y1]`, counterclockwise, its corners
/// rounded or chamfered as `corners` says (ODB++'s numbers: 1 top right,
/// 2 top left, 3 bottom left, 4 bottom right).
pub fn rect(x0: Nm, y0: Nm, x1: Nm, y1: Nm, corners: &Corners) -> Result<Contour, ShapeError> {
    if x1 <= x0 || y1 <= y0 {
        return Err(ShapeError::BadParameters(
            "a rectangle needs a width and a height",
        ));
    }
    if corners.round < 0 || corners.chamfer < 0 {
        return Err(ShapeError::BadParameters("corner sizes can't be negative"));
    }
    // Corners counterclockwise from the bottom right, with the unit
    // directions of the sides arriving at and leaving each.
    type Corner = (u8, Point, (Nm, Nm), (Nm, Nm));
    let order: [Corner; 4] = [
        (4, Point::new(x1, y0), (1, 0), (0, 1)),
        (1, Point::new(x1, y1), (0, 1), (-1, 0)),
        (2, Point::new(x0, y1), (-1, 0), (0, -1)),
        (3, Point::new(x0, y0), (0, -1), (1, 0)),
    ];
    let cut = |n: u8| -> Result<(Nm, bool), ShapeError> {
        let (r, c) = (corners.rounded.has(n), corners.chamfered.has(n));
        match (r, c) {
            (true, true) => Err(ShapeError::BadParameters(
                "a corner both rounded and chamfered",
            )),
            (true, false) => Ok((corners.round, true)),
            (false, true) => Ok((corners.chamfer, false)),
            (false, false) => Ok((0, false)),
        }
    };
    let cuts: Vec<(Nm, bool)> = order.iter().map(|o| cut(o.0)).collect::<Result<_, _>>()?;
    // Each side must hold the cuts at both its ends.
    let (w, h) = (x1 - x0, y1 - y0);
    for i in 0..4 {
        let side = if i % 2 == 0 { h } else { w };
        if cuts[i].0 + cuts[(i + 1) % 4].0 > side {
            return Err(ShapeError::BadParameters("corners larger than their sides"));
        }
    }
    let mut edges = Vec::new();
    let mut last_exit: Option<Point> = None;
    let mut first_entry = None;
    for (i, (_, p, din, dout)) in order.iter().enumerate() {
        let (size, round) = cuts[i];
        let entry = p.offset(-size * din.0, -size * din.1);
        let exit = p.offset(size * dout.0, size * dout.1);
        if let Some(prev) = last_exit {
            if prev != entry {
                edges.push(Edge::line(prev, entry));
            }
        } else {
            first_entry = Some(entry);
        }
        if size > 0 {
            edges.push(if round {
                Edge::Arc(Arc {
                    start: entry,
                    end: exit,
                    centre: entry.offset(size * dout.0, size * dout.1),
                    dir: Dir::Ccw,
                })
            } else {
                Edge::line(entry, exit)
            });
        }
        last_exit = Some(exit);
    }
    let (first, last) = (
        first_entry.expect("four corners"),
        last_exit.expect("four corners"),
    );
    if first != last {
        edges.push(Edge::line(last, first));
    }
    Contour::new(edges).map_err(|_| ShapeError::BadParameters("corners larger than their sides"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::CornerSet;

    #[test]
    fn sharp_rectangles_are_four_segments() {
        let c = rect(-10, -5, 10, 5, &Corners::SHARP).unwrap();
        assert_eq!(c.edges().len(), 4);
        assert_eq!(c.polygon_area2(), Some(2 * 200));
    }

    #[test]
    fn rounded_corners() {
        let c = rect(-10, -5, 10, 5, &Corners::rounded(2, CornerSet::ALL)).unwrap();
        assert_eq!(c.edges().len(), 8);
        let pi = std::f64::consts::PI;
        // 200 less four corners of (4 - π) each.
        assert!((c.area2() - 2.0 * (200.0 - 4.0 * (4.0 - pi))).abs() < 1e-9);
        // Rounding a whole short side away leaves no straight edge there.
        let full = rect(-10, -5, 10, 5, &Corners::rounded(5, CornerSet::ALL)).unwrap();
        assert_eq!(full.edges().len(), 6);
        assert!(rect(-10, -5, 10, 5, &Corners::rounded(6, CornerSet::ALL)).is_err());
        // Only corners 1 and 3 may take the whole height together.
        let some = rect(
            -10,
            -5,
            10,
            5,
            &Corners::rounded(10, CornerSet::of(&[1, 3]).unwrap()),
        );
        assert!(some.is_ok());
    }

    #[test]
    fn chamfers_and_mixtures() {
        let c = rect(
            0,
            0,
            10,
            10,
            &Corners::chamfered(2, CornerSet::of(&[1]).unwrap()),
        )
        .unwrap();
        assert_eq!(c.polygon_area2(), Some(2 * 100 - 4));
        let both = Corners {
            round: 1,
            rounded: CornerSet::ALL,
            chamfer: 1,
            chamfered: CornerSet::of(&[2]).unwrap(),
        };
        assert!(rect(0, 0, 10, 10, &both).is_err());
    }

    #[test]
    fn polygons_and_circles() {
        let p = polygon(&[
            Point::new(0, 0),
            Point::new(4, 0),
            Point::new(4, 0),
            Point::new(0, 3),
            Point::new(0, 0),
        ])
        .unwrap();
        assert_eq!(p.edges().len(), 3);
        assert_eq!(p.polygon_area2(), Some(12));
        assert!(polygon(&[Point::new(0, 0), Point::new(1, 1)]).is_err());
        assert!(circle(Point::ORIGIN, 0).is_err());
        assert_eq!(circle(Point::new(1, 1), 5).unwrap().edges().len(), 1);
    }
}
