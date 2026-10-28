//! Placing a symbol: scale, mirror about y, rotate, translate.

use super::{
    Arc, Contour, Edge, Mdeg, Nm, Pen, Point, Region, Segment, Stroke, div_round, round_nm,
};

/// Where and how a symbol (or anything in its own coordinates) is placed:
/// first scaled about its origin, then mirrored about the y axis (`x`
/// negated), then turned counterclockwise by `rot`, then moved to `at`, as
/// ODB++ orders them.
///
/// Multiples of 90° are exact integer arithmetic. Other angles take
/// `ryu_math::sin_cos` and round each coordinate to the nearest nanometre,
/// ties to even.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    pub at: Point,
    pub rot: Mdeg,
    pub mirror: bool,
    /// A scale in millionths (1,000,000 is none), rounded to the nanometre.
    pub scale: Option<i64>,
}

impl Default for Place {
    fn default() -> Place {
        Place::IDENTITY
    }
}

impl Place {
    pub const IDENTITY: Place = Place {
        at: Point::ORIGIN,
        rot: Mdeg::ZERO,
        mirror: false,
        scale: None,
    };

    pub fn at(at: Point) -> Place {
        Place {
            at,
            ..Place::IDENTITY
        }
    }

    fn scaled(&self, v: Nm) -> Nm {
        match self.scale {
            None => v,
            Some(s) => div_round(i128::from(v) * i128::from(s), 1_000_000) as Nm,
        }
    }

    /// A length (a pen's width) under this placement.
    pub fn length(&self, v: Nm) -> Nm {
        self.scaled(v)
    }

    pub fn point(&self, p: Point) -> Point {
        let (mut x, y) = (self.scaled(p.x), self.scaled(p.y));
        if self.mirror {
            x = -x;
        }
        let (x, y) = match self.rot.quarter_turns() {
            Some(0) => (x, y),
            Some(1) => (-y, x),
            Some(2) => (-x, -y),
            Some(3) => (y, -x),
            _ => {
                let (s, c) = ryu_math::sin_cos(self.rot.radians());
                let (fx, fy) = (x as f64, y as f64);
                (round_nm(fx * c - fy * s), round_nm(fx * s + fy * c))
            }
        };
        Point::new(x + self.at.x, y + self.at.y)
    }

    pub fn arc(&self, a: &Arc) -> Arc {
        Arc {
            start: self.point(a.start),
            end: self.point(a.end),
            centre: self.point(a.centre),
            dir: if self.mirror { a.dir.reversed() } else { a.dir },
        }
    }

    pub fn edge(&self, e: &Edge) -> Edge {
        match e {
            Edge::Segment(s) => Edge::Segment(Segment {
                a: self.point(s.a),
                b: self.point(s.b),
            }),
            Edge::Arc(a) => Edge::Arc(self.arc(a)),
        }
    }

    /// A contour placed, keeping its orientation: a mirror would turn it
    /// over, so it is run the other way.
    pub fn contour(&self, c: &Contour) -> Contour {
        let placed = Contour::new(c.edges().iter().map(|e| self.edge(e)).collect())
            .expect("placing keeps a contour closed");
        if self.mirror {
            placed.reversed()
        } else {
            placed
        }
    }

    pub fn region(&self, r: &Region) -> Region {
        Region {
            outer: self.contour(&r.outer),
            holes: r.holes.iter().map(|h| self.contour(h)).collect(),
        }
    }

    pub fn stroke(&self, s: &Stroke) -> Stroke {
        Stroke {
            path: s.path.iter().map(|e| self.edge(e)).collect(),
            pen: match s.pen {
                Pen::Round(w) => Pen::Round(self.scaled(w)),
                Pen::Square(w) => Pen::Square(self.scaled(w)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::Dir;
    use super::*;

    fn place(rot: i64, mirror: bool) -> Place {
        Place {
            at: Point::new(1000, 2000),
            rot: Mdeg::new(rot),
            mirror,
            scale: None,
        }
    }

    #[test]
    fn right_angles_are_exact() {
        let p = Point::new(3, 1);
        assert_eq!(place(0, false).point(p), Point::new(1003, 2001));
        assert_eq!(place(90_000, false).point(p), Point::new(999, 2003));
        assert_eq!(place(180_000, false).point(p), Point::new(997, 1999));
        assert_eq!(place(270_000, false).point(p), Point::new(1001, 1997));
        // Mirror first, then turn.
        assert_eq!(place(0, true).point(p), Point::new(997, 2001));
        assert_eq!(place(90_000, true).point(p), Point::new(999, 1997));
    }

    #[test]
    fn other_angles_round_to_the_nanometre() {
        let p = Point::new(1_000_000, 0);
        let q = place(45_000, false).point(p);
        // 1e6 / sqrt(2) = 707106.78...
        assert_eq!(q, Point::new(1000 + 707_107, 2000 + 707_107));
        let r = place(30_000, false).point(Point::new(0, 2_000_000));
        assert_eq!(r, Point::new(1000 - 1_000_000, 2000 + 1_732_051));
    }

    #[test]
    fn scale_comes_first() {
        let pl = Place {
            scale: Some(1_500_000),
            ..place(90_000, false)
        };
        assert_eq!(pl.point(Point::new(3, 1)), Point::new(1000 - 2, 2000 + 4));
        assert_eq!(pl.length(10), 15);
    }

    #[test]
    fn mirrors_flip_arcs_and_keep_orientation() {
        let a = Arc {
            start: Point::new(10, 0),
            end: Point::new(0, 10),
            centre: Point::ORIGIN,
            dir: Dir::Ccw,
        };
        assert_eq!(place(0, true).arc(&a).dir, Dir::Cw);
        let sq = Contour::new(vec![
            Edge::line(Point::new(0, 0), Point::new(10, 0)),
            Edge::line(Point::new(10, 0), Point::new(10, 10)),
            Edge::line(Point::new(10, 10), Point::new(0, 0)),
        ])
        .unwrap();
        assert_eq!(place(0, true).contour(&sq).orientation(), Dir::Ccw);
        assert_eq!(place(37_000, true).contour(&sq).orientation(), Dir::Ccw);
    }
}
