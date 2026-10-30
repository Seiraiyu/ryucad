//! The pieces a measurement meets: points, segments on integer lines and
//! centre-form arcs, each exactly as the shape has it.
//!
//! An arc ends where the ray from its centre through its written end meets
//! its circle (stage 0's rule), so its true end is usually irrational:
//! `c + λE` with `E` the written end less the centre and `λ = √(R / |E|²)`.
//! A segment lies on the integer line `b + tU` with `t` running between two
//! [`Param`]s, `0`, `1` or such a `λ`, so the straight join from an arc's
//! true end to its written end (where a contour or a path goes on) is a
//! segment too. Nothing here is rounded.

use ryu_board::geom::{Arc, Dir, Edge, Nm, Point, Rect, V};

/// Where a segment starts or ends along its line `b + tU`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Param {
    Zero,
    One,
    /// `t = √(r2 / |U|²)`: the true end of an arc of squared radius `r2`
    /// centred on the line's base, in the line's direction.
    Lambda(i128),
}

/// A point a measurement meets: an integer point, or an arc's true end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pt {
    At(Point),
    /// `c + λ(p - c)` with `λ = √(r2 / |p - c|²)`, never itself an
    /// integer point of that ray at radius `√r2` (`Pt::end` makes those
    /// `At`).
    End {
        c: Point,
        p: Point,
        r2: i128,
    },
}

impl Pt {
    /// The true end of a ray from `c` through `p` at squared radius `r2`.
    pub fn end(c: Point, p: Point, r2: i128) -> Pt {
        if p.minus(c).len2() == r2 {
            Pt::At(p)
        } else {
            Pt::End { c, p, r2 }
        }
    }

    /// The point in floating point, for estimates and boxes.
    pub fn approx(&self) -> (f64, f64) {
        match *self {
            Pt::At(p) => (p.x as f64, p.y as f64),
            Pt::End { c, p, r2 } => {
                let e = p.minus(c);
                let k = ryu_math::sqrt(r2 as f64 / e.len2() as f64);
                (c.x as f64 + e.0 as f64 * k, c.y as f64 + e.1 as f64 * k)
            }
        }
    }

    /// An integer box holding the point.
    pub fn bounds(&self) -> Rect {
        match *self {
            Pt::At(p) => Rect::of_point(p),
            Pt::End { .. } => {
                let (x, y) = self.approx();
                Rect::new(
                    Point::new(x.floor() as Nm - 1, y.floor() as Nm - 1),
                    Point::new(x.ceil() as Nm + 1, y.ceil() as Nm + 1),
                )
            }
        }
    }
}

/// A segment of the integer line `b + tU`, `t` from `t0` to `t1`
/// (`t0 <= t1`). A point is a segment with `U = 0`. `rev` is set when the
/// outline it belongs to runs from `t1` to `t0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SegP {
    pub b: Point,
    pub u: (i128, i128),
    pub t0: Param,
    pub t1: Param,
    pub rev: bool,
}

impl SegP {
    /// The segment from `a` to `b`.
    pub fn new(a: Point, b: Point) -> SegP {
        let u = b.minus(a);
        SegP {
            b: a,
            u: (u.0, u.1),
            t0: Param::Zero,
            t1: Param::One,
            rev: false,
        }
    }

    pub fn u(&self) -> V {
        V(self.u.0, self.u.1)
    }

    /// The straight join from the true end of an arc centred on `c` with
    /// squared radius `r2` to its written end `p`.
    pub fn join(c: Point, p: Point, r2: i128) -> SegP {
        let u = p.minus(c);
        // The join runs from λ to 1: forwards when the written end lies
        // outside the circle (λ < 1).
        let outside = r2 < u.len2();
        let (t0, t1) = if outside {
            (Param::Lambda(r2), Param::One)
        } else {
            (Param::One, Param::Lambda(r2))
        };
        SegP {
            b: c,
            u: (u.0, u.1),
            t0,
            t1,
            rev: !outside,
        }
    }

    /// The point at `t`.
    pub fn at(&self, t: Param) -> Pt {
        let end = Point::new(self.b.x + self.u.0 as Nm, self.b.y + self.u.1 as Nm);
        match t {
            Param::Zero => Pt::At(self.b),
            Param::One => Pt::At(end),
            Param::Lambda(r2) => Pt::end(self.b, end, r2),
        }
    }

    pub fn ends(&self) -> [Pt; 2] {
        [self.at(self.t0), self.at(self.t1)]
    }

    /// An integer point on the segment.
    pub fn probe(&self) -> Point {
        match self.ends() {
            [Pt::At(p), _] | [_, Pt::At(p)] => p,
            // Both ends irrational can't happen: a join ends at its
            // written end.
            _ => self.b,
        }
    }
}

/// A centre-form arc with its squared radius and its true end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArcP {
    pub arc: Arc,
    pub r2: i128,
    pub q: Pt,
}

impl ArcP {
    /// An arc that isn't degenerate (see [`Arc::is_degenerate`]).
    pub fn new(arc: Arc) -> ArcP {
        let r2 = arc.radius2();
        let q = if arc.is_full() {
            Pt::At(arc.start)
        } else {
            Pt::end(arc.centre, arc.end, r2)
        };
        ArcP { arc, r2, q }
    }

    /// Whether the arc passes the direction `v` from its centre. Exact.
    pub fn sweeps(&self, v: V) -> bool {
        self.arc.sweeps(v)
    }
}

/// The most boxes a slanted segment is cut into for trees.
pub const MAX_SPLIT: i128 = 64;

/// How wide a slanted segment's box may be across its narrow side before
/// it is cut: 50 µm.
pub const SPLIT_SIZE: i128 = 50_000;

/// One piece of a measurable outline or path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Core {
    Seg(SegP),
    Arc(ArcP),
}

impl Core {
    /// An integer point on the piece.
    pub fn probe(&self) -> Point {
        match self {
            Core::Seg(s) => s.probe(),
            Core::Arc(a) => a.arc.start,
        }
    }

    /// Boxes that between them hold the piece grown by `grow`, for trees:
    /// one, or for a long segment that runs aslant, a box per stretch of
    /// it whose box is [`SPLIT_SIZE`] across its narrow side, up to
    /// [`MAX_SPLIT`], so a long slanted segment's box doesn't cover
    /// everything it never comes near.
    pub fn boxes(&self, grow: Nm, out: &mut Vec<Rect>) {
        let grown = |r: Rect| Rect {
            min: r.min.offset(-grow, -grow),
            max: r.max.offset(grow, grow),
        };
        let Core::Seg(s) = self else {
            out.push(grown(self.bounds()));
            return;
        };
        let (dx, dy) = (s.u.0.abs(), s.u.1.abs());
        let lo = dx.min(dy);
        if (s.t0, s.t1) != (Param::Zero, Param::One) || lo == 0 {
            out.push(grown(self.bounds()));
            return;
        }
        let n = ((lo + SPLIT_SIZE - 1) / SPLIT_SIZE).clamp(1, MAX_SPLIT);
        if n == 1 {
            out.push(grown(self.bounds()));
            return;
        }
        // The point k/n along, rounded down and up.
        let at = |k: i128| {
            let x = i128::from(s.b.x) * n + s.u.0 * k;
            let y = i128::from(s.b.y) * n + s.u.1 * k;
            (
                Point::new(x.div_euclid(n) as Nm, y.div_euclid(n) as Nm),
                Point::new((-((-x).div_euclid(n))) as Nm, (-((-y).div_euclid(n))) as Nm),
            )
        };
        for k in 0..n {
            let (a0, a1) = at(k);
            let (b0, b1) = at(k + 1);
            out.push(grown(Rect::new(a0, b0).with(a1).with(b1)));
        }
    }

    /// An integer box holding the piece.
    pub fn bounds(&self) -> Rect {
        match self {
            Core::Seg(s) => {
                let [a, b] = s.ends();
                a.bounds().union(b.bounds())
            }
            Core::Arc(a) => Rect::of_edge(&Edge::Arc(a.arc)).union(a.q.bounds()),
        }
    }
}

/// The pieces of an edge: a segment; an arc, and the join from its true
/// end to its written end where they differ; a degenerate arc as the
/// segment between its ends.
pub fn edge_cores(e: &Edge, out: &mut Vec<Core>) {
    match e {
        Edge::Segment(s) => out.push(Core::Seg(SegP::new(s.a, s.b))),
        Edge::Arc(a) if a.is_degenerate() => out.push(Core::Seg(SegP::new(a.start, a.end))),
        Edge::Arc(a) => {
            let p = ArcP::new(*a);
            out.push(Core::Arc(p));
            if let Pt::End { c, p: end, r2 } = p.q {
                out.push(Core::Seg(SegP::join(c, end, r2)));
            }
        }
    }
}

/// The arc's direction, as a sign: 1 counterclockwise, -1 clockwise.
pub fn turn(a: &Arc) -> i128 {
    match a.dir {
        Dir::Ccw => 1,
        Dir::Cw => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn ends_on_the_circle_are_integer() {
        assert_eq!(Pt::end(p(0, 0), p(3, 4), 25), Pt::At(p(3, 4)));
        let q = Pt::end(p(0, 0), p(3, 4), 26);
        assert!(matches!(q, Pt::End { .. }));
        let (x, y) = q.approx();
        assert!((x - 3.0 * (26f64 / 25.0).sqrt()).abs() < 1e-12);
        assert!((y - 4.0 * (26f64 / 25.0).sqrt()).abs() < 1e-12);
        assert!(q.bounds().contains(p(3, 4)));
    }

    #[test]
    fn arcs_off_their_circle_get_a_join() {
        // Radius 1000, the written end 1 nm outside the circle.
        let a = Arc {
            start: p(1000, 0),
            end: p(0, 1001),
            centre: p(0, 0),
            dir: Dir::Ccw,
        };
        let mut out = Vec::new();
        edge_cores(&Edge::Arc(a), &mut out);
        assert_eq!(out.len(), 2);
        let Core::Seg(j) = out[1] else { panic!() };
        // λ = 1000/1001 < 1: the join runs from λ to 1.
        assert_eq!(j.t0, Param::Lambda(1_000_000));
        assert_eq!(j.t1, Param::One);
        assert_eq!(j.ends()[1], Pt::At(p(0, 1001)));
        assert_eq!(
            j.ends()[0],
            Pt::End {
                c: p(0, 0),
                p: p(0, 1001),
                r2: 1_000_000
            }
        );
        // Inside the circle, the join runs from 1 to λ.
        let inside = Arc {
            end: p(0, 999),
            ..a
        };
        out.clear();
        edge_cores(&Edge::Arc(inside), &mut out);
        let Core::Seg(j) = out[1] else { panic!() };
        assert_eq!((j.t0, j.t1), (Param::One, Param::Lambda(1_000_000)));
        assert!(j.rev);
        // On the circle, full, or degenerate: no join.
        for e in [
            Arc {
                end: p(0, 1000),
                ..a
            },
            Arc {
                end: p(1000, 0),
                ..a
            },
        ] {
            out.clear();
            edge_cores(&Edge::Arc(e), &mut out);
            assert_eq!(out.len(), 1);
        }
        out.clear();
        edge_cores(
            &Edge::Arc(Arc {
                centre: p(1000, 0),
                ..a
            }),
            &mut out,
        );
        assert_eq!(out, vec![Core::Seg(SegP::new(p(1000, 0), p(0, 1001)))]);
    }

    #[test]
    fn slanted_segments_take_several_boxes() {
        let mut out = Vec::new();
        // Along an axis: one box.
        Core::Seg(SegP::new(p(0, 0), p(1000, 0))).boxes(5, &mut out);
        assert_eq!(out, vec![Rect::new(p(-5, -5), p(1005, 5))]);
        // A short one at 45°: one.
        out.clear();
        Core::Seg(SegP::new(p(0, 0), p(40_000, 40_000))).boxes(0, &mut out);
        assert_eq!(out.len(), 1);
        // 3.2 mm at 45°: 64 boxes along it, each holding its stretch.
        out.clear();
        let s = Core::Seg(SegP::new(p(0, 0), p(3_200_000, 3_200_000)));
        s.boxes(0, &mut out);
        assert_eq!(out.len(), 64);
        assert_eq!(out[0], Rect::new(p(0, 0), p(50_000, 50_000)));
        assert_eq!(
            out[63],
            Rect::new(p(3_150_000, 3_150_000), p(3_200_000, 3_200_000))
        );
        // A shallow slope: as many as its narrow side needs. Every point of
        // the segment is in one.
        out.clear();
        let t = Core::Seg(SegP::new(p(7, 3), p(1_000_007, 120_003)));
        t.boxes(0, &mut out);
        assert_eq!(out.len(), 3);
        for k in 0..=1000 {
            let x = 7 + 1000 * k;
            let y = 3 + 120 * k;
            assert!(out.iter().any(|b| b.contains(p(x, y))), "{k}");
        }
    }

    #[test]
    fn boxes_hold_the_true_end() {
        let a = ArcP::new(Arc {
            start: p(1000, 0),
            end: p(0, 999),
            centre: p(0, 0),
            dir: Dir::Ccw,
        });
        let b = Core::Arc(a).bounds();
        assert!(b.contains(p(0, 1000)));
        assert!(b.contains(p(1000, 0)));
    }
}
