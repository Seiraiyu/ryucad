//! Integer boxes and exact bounds of edges.

use super::{Edge, Point, V, ceil_sqrt};

/// An integer box, both corners included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    pub fn of_point(p: Point) -> Rect {
        Rect { min: p, max: p }
    }

    pub fn new(a: Point, b: Point) -> Rect {
        Rect {
            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    pub fn with(self, p: Point) -> Rect {
        self.union(Rect::of_point(p))
    }

    pub fn union(self, o: Rect) -> Rect {
        Rect {
            min: Point::new(self.min.x.min(o.min.x), self.min.y.min(o.min.y)),
            max: Point::new(self.max.x.max(o.max.x), self.max.y.max(o.max.y)),
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        self.min.x <= p.x && p.x <= self.max.x && self.min.y <= p.y && p.y <= self.max.y
    }

    pub fn contains_rect(&self, o: &Rect) -> bool {
        self.contains(o.min) && self.contains(o.max)
    }

    /// The smallest integer box holding an edge: a segment's ends; an
    /// arc's written ends and, for each axis direction it sweeps (decided
    /// exactly), the circle's extreme there.
    pub fn of_edge(e: &Edge) -> Rect {
        match e {
            Edge::Segment(s) => Rect::new(s.a, s.b),
            Edge::Arc(a) => {
                let mut r = Rect::new(a.start, a.end);
                if a.is_degenerate() {
                    return r;
                }
                let rad = ceil_sqrt(a.radius2()) as i64;
                let c = a.centre;
                for (v, p) in [
                    (V(1, 0), Point::new(c.x + rad, c.y)),
                    (V(0, 1), Point::new(c.x, c.y + rad)),
                    (V(-1, 0), Point::new(c.x - rad, c.y)),
                    (V(0, -1), Point::new(c.x, c.y - rad)),
                ] {
                    if a.sweeps(v) {
                        r = r.with(p);
                    }
                }
                r
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Arc, Dir};
    use super::*;

    #[test]
    fn segment_boxes() {
        let r = Rect::of_edge(&Edge::line(Point::new(5, -3), Point::new(-2, 7)));
        assert_eq!(
            r,
            Rect {
                min: Point::new(-2, -3),
                max: Point::new(5, 7)
            }
        );
        assert!(r.contains(Point::new(0, 0)));
        assert!(!r.contains(Point::new(6, 0)));
    }

    #[test]
    fn arc_boxes_take_the_extremes_they_sweep() {
        // From 45° to 135° counterclockwise passes the top only.
        let a = Arc {
            start: Point::new(700, 700),
            end: Point::new(-700, 700),
            centre: Point::ORIGIN,
            dir: Dir::Ccw,
        };
        // r² = 980000, ceil(sqrt) = 990.
        assert_eq!(
            Rect::of_edge(&Edge::Arc(a)),
            Rect {
                min: Point::new(-700, 700),
                max: Point::new(700, 990)
            }
        );
        // The other way round passes right, bottom and left.
        let b = Arc { dir: Dir::Cw, ..a };
        assert_eq!(
            Rect::of_edge(&Edge::Arc(b)),
            Rect {
                min: Point::new(-990, -990),
                max: Point::new(990, 700)
            }
        );
        let full = Arc { end: a.start, ..a };
        assert_eq!(
            Rect::of_edge(&Edge::Arc(full)),
            Rect {
                min: Point::new(-990, -990),
                max: Point::new(990, 990)
            }
        );
    }
}
