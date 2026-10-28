//! Closed contours, regions with holes, and strokes.

use super::{Dir, Edge, Nm, Rect};

/// A closed sequence of edges, each starting where the last ended.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Contour {
    edges: Vec<Edge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContourError {
    Empty,
    /// Edge `at` doesn't start where the one before it ends.
    Open {
        at: usize,
    },
}

impl Contour {
    /// Checks that the edges close up. A single full-circle arc is a
    /// contour of one edge.
    pub fn new(edges: Vec<Edge>) -> Result<Contour, ContourError> {
        if edges.is_empty() {
            return Err(ContourError::Empty);
        }
        for i in 0..edges.len() {
            let next = &edges[(i + 1) % edges.len()];
            if edges[i].end() != next.start() {
                return Err(ContourError::Open {
                    at: (i + 1) % edges.len(),
                });
            }
        }
        Ok(Contour { edges })
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Twice the signed area: positive counterclockwise. Deterministic
    /// `f64`: each arc adds its circular segment, `r² (θ - sin θ)`.
    pub fn area2(&self) -> f64 {
        let mut sum = 0.0;
        for e in &self.edges {
            let (a, b) = (e.start(), e.end());
            sum += (a.x as f64) * (b.y as f64) - (b.x as f64) * (a.y as f64);
            if let Edge::Arc(arc) = e {
                let t = arc.sweep();
                let seg = arc.radius2() as f64 * (t - ryu_math::sin(t));
                match arc.dir {
                    Dir::Ccw => sum += seg,
                    Dir::Cw => sum -= seg,
                }
            }
        }
        sum
    }

    /// Twice the signed area exactly, when every edge is straight.
    pub fn polygon_area2(&self) -> Option<i128> {
        let mut sum = 0i128;
        for e in &self.edges {
            let Edge::Segment(s) = e else { return None };
            sum += i128::from(s.a.x) * i128::from(s.b.y) - i128::from(s.b.x) * i128::from(s.a.y);
        }
        Some(sum)
    }

    pub fn orientation(&self) -> Dir {
        if self.area2() >= 0.0 {
            Dir::Ccw
        } else {
            Dir::Cw
        }
    }

    /// The same contour run the other way.
    pub fn reversed(&self) -> Contour {
        Contour {
            edges: self.edges.iter().rev().map(Edge::reversed).collect(),
        }
    }

    /// The smallest integer box holding the contour.
    pub fn bounds(&self) -> Rect {
        let mut r = Rect::of_edge(&self.edges[0]);
        for e in &self.edges[1..] {
            r = r.union(Rect::of_edge(e));
        }
        r
    }
}

/// An outer contour, counterclockwise, and its holes, clockwise: ODB++'s
/// island and holes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Region {
    pub outer: Contour,
    pub holes: Vec<Contour>,
}

impl Region {
    pub fn solid(outer: Contour) -> Region {
        Region {
            outer,
            holes: Vec::new(),
        }
    }

    /// A ring: `outer` with `inner` cut out (both given counterclockwise).
    pub fn ring(outer: Contour, inner: Contour) -> Region {
        Region {
            outer,
            holes: vec![inner.reversed()],
        }
    }

    /// Twice the area, holes taken away.
    pub fn area2(&self) -> f64 {
        self.outer.area2() + self.holes.iter().map(Contour::area2).sum::<f64>()
    }

    pub fn bounds(&self) -> Rect {
        self.outer.bounds()
    }
}

/// What a stroke is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pen {
    /// A round pen of this diameter.
    Round(Nm),
    /// A square pen of this side, kept square to the axes.
    Square(Nm),
}

/// A path of edges drawn with a pen.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Stroke {
    pub path: Vec<Edge>,
    pub pen: Pen,
}

#[cfg(test)]
mod tests {
    use super::super::{Arc, Point};
    use super::*;

    fn square() -> Contour {
        let p = [
            Point::new(0, 0),
            Point::new(10, 0),
            Point::new(10, 10),
            Point::new(0, 10),
        ];
        Contour::new((0..4).map(|i| Edge::line(p[i], p[(i + 1) % 4])).collect()).unwrap()
    }

    #[test]
    fn contours_close() {
        assert_eq!(Contour::new(vec![]), Err(ContourError::Empty));
        let open = vec![
            Edge::line(Point::new(0, 0), Point::new(1, 0)),
            Edge::line(Point::new(1, 1), Point::new(0, 0)),
        ];
        assert_eq!(Contour::new(open), Err(ContourError::Open { at: 1 }));
    }

    #[test]
    fn areas_and_orientation() {
        let s = square();
        assert_eq!(s.polygon_area2(), Some(200));
        assert_eq!(s.area2(), 200.0);
        assert_eq!(s.orientation(), Dir::Ccw);
        assert_eq!(s.reversed().polygon_area2(), Some(-200));
        let c = Contour::new(vec![Edge::Arc(Arc {
            start: Point::new(1000, 0),
            end: Point::new(1000, 0),
            centre: Point::ORIGIN,
            dir: Dir::Ccw,
        })])
        .unwrap();
        let pi = std::f64::consts::PI;
        assert!((c.area2() - 2.0 * pi * 1e6).abs() < 1e-6);
        assert_eq!(c.polygon_area2(), None);
        let ring = Region::ring(
            c.clone(),
            Contour::new(vec![Edge::Arc(Arc {
                start: Point::new(500, 0),
                end: Point::new(500, 0),
                centre: Point::ORIGIN,
                dir: Dir::Ccw,
            })])
            .unwrap(),
        );
        assert!((ring.area2() - 2.0 * pi * 750_000.0).abs() < 1e-6);
    }

    #[test]
    fn a_half_disc() {
        // A semicircle above the x axis, closed by its diameter.
        let c = Contour::new(vec![
            Edge::line(Point::new(-100, 0), Point::new(100, 0)),
            Edge::Arc(Arc {
                start: Point::new(100, 0),
                end: Point::new(-100, 0),
                centre: Point::ORIGIN,
                dir: Dir::Ccw,
            }),
        ])
        .unwrap();
        let pi = std::f64::consts::PI;
        assert!((c.area2() - pi * 10_000.0).abs() < 1e-9);
    }
}
