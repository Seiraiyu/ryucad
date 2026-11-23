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

    /// An edge placed. An arc a rounding placement turns over (its ends
    /// and centre each rounded to the nanometre, so its sweep moves by
    /// more than a quarter turn or its radius goes) is placed as its
    /// chord: an arc a few nanometres across, such as a ring's inner
    /// corner rounded by 1 nm, could otherwise sweep nearly a whole turn
    /// and cross its own contour (#944). Exact placements (quarter turns,
    /// unscaled) place every arc as an arc.
    pub fn edge(&self, e: &Edge) -> Edge {
        match e {
            Edge::Segment(s) => Edge::Segment(Segment {
                a: self.point(s.a),
                b: self.point(s.b),
            }),
            Edge::Arc(a) => {
                let placed = self.arc(a);
                let exact = self.scale.is_none() && self.rot.quarter_turns().is_some();
                if exact || !turned_over(a, &placed) {
                    Edge::Arc(placed)
                } else {
                    Edge::line(placed.start, placed.end)
                }
            }
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

/// Whether rounding turned an arc over when it was placed: its radius
/// gone, or its sweep moved by more than a quarter turn (an arc of a few
/// nanometres whose rounded ends passed each other round its centre).
fn turned_over(a: &Arc, placed: &Arc) -> bool {
    if a.is_degenerate() {
        return false;
    }
    placed.is_degenerate() || (placed.sweep() - a.sweep()).abs() > core::f64::consts::FRAC_PI_2
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

    /// #944: a 1 nm corner arc under every turn is placed as an arc that
    /// keeps its sweep within a quarter turn, or as its chord; some turns
    /// need the chord, and quarter turns never do.
    #[test]
    fn an_arc_rounding_turns_over_is_placed_as_its_chord() {
        let c = Point::new(123_457, -98_765);
        let a = Arc {
            start: c.offset(1, 0),
            end: c.offset(0, 1),
            centre: c,
            dir: Dir::Ccw,
        };
        let mut chords = 0;
        for rot in (0..360_000).step_by(997) {
            for mirror in [false, true] {
                match place(rot, mirror).edge(&Edge::Arc(a)) {
                    Edge::Arc(p) => assert!(
                        (p.sweep() - a.sweep()).abs() <= core::f64::consts::FRAC_PI_2,
                        "{rot} {mirror}: {p:?}"
                    ),
                    Edge::Segment(_) => chords += 1,
                }
            }
        }
        assert!(chords > 0);
        for rot in [0, 90_000, 180_000, 270_000] {
            assert_eq!(
                place(rot, false).edge(&Edge::Arc(a)),
                Edge::Arc(place(rot, false).arc(&a))
            );
        }
    }
}
