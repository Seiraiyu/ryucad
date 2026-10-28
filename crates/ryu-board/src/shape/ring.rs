//! Cutting a convex contour round the origin by lines parallel to rays
//! from it: what thermals are made of.

use crate::geom::{Arc, Contour, Edge, Point, V, round_nm};

/// A line parallel to the ray at `theta` radians from the origin, `s`
/// nanometres to its left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Line {
    u: (f64, f64),
    n: (f64, f64),
    s: f64,
}

impl Line {
    pub(crate) fn new(theta: f64, s: f64) -> Line {
        let (sn, cs) = ryu_math::sin_cos(theta);
        Line {
            u: (cs, sn),
            n: (-sn, cs),
            s,
        }
    }
}

/// Where a line meets a contour on its ray's side.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hit {
    pub edge: usize,
    /// How far along the edge: a segment's fraction, an arc's angle.
    pub key: f64,
    pub at: Point,
    /// The polar angle of the point, for ordering hits round the origin.
    pub angle: f64,
}

fn dot(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn pt(p: Point) -> (f64, f64) {
    (p.x as f64, p.y as f64)
}

/// The angle from `a` to `b` about the origin in the arc's direction, in
/// `[0, 2π)`.
fn turn(a: (f64, f64), b: (f64, f64), ccw: bool) -> f64 {
    let mut t = ryu_math::atan2(a.0 * b.1 - a.1 * b.0, dot(a, b));
    if !ccw {
        t = -t;
    }
    if t < 0.0 {
        t += 2.0 * std::f64::consts::PI;
    }
    t
}

/// The point where `line` meets the convex contour `c` (which must hold
/// the origin) on its ray's side, rounded to the nanometre. `None` when it
/// misses.
pub(crate) fn hit(c: &Contour, line: &Line) -> Option<Hit> {
    let mut best: Option<(f64, Hit)> = None;
    let mut take = |t: f64, h: Hit| {
        if t > 0.0 && best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, h));
        }
    };
    for (i, e) in c.edges().iter().enumerate() {
        match e {
            Edge::Segment(s) => {
                let (a, b) = (pt(s.a), pt(s.b));
                let (an, bn) = (dot(a, line.n), dot(b, line.n));
                if an == bn {
                    continue;
                }
                let l = (line.s - an) / (bn - an);
                if !(0.0..=1.0).contains(&l) {
                    continue;
                }
                let p = (a.0 + l * (b.0 - a.0), a.1 + l * (b.1 - a.1));
                take(dot(p, line.u), mk(i, l, p));
            }
            Edge::Arc(arc) => {
                let c = pt(arc.centre);
                let r2 = arc.radius2() as f64;
                let uc = dot(line.u, c);
                let q = (line.s * line.n.0 - c.0, line.s * line.n.1 - c.1);
                let disc = uc * uc - dot(q, q) + r2;
                if disc < 0.0 {
                    continue;
                }
                let root = ryu_math::sqrt(disc);
                for t in [uc - root, uc + root] {
                    let p = (
                        line.s * line.n.0 + t * line.u.0,
                        line.s * line.n.1 + t * line.u.1,
                    );
                    let v = (p.0 - c.0, p.1 - c.1);
                    let vi = V(
                        i128::from(round_nm(v.0 * 64.0)),
                        i128::from(round_nm(v.1 * 64.0)),
                    );
                    if vi.is_zero() || !arc.sweeps(vi) {
                        continue;
                    }
                    let s0 = (arc.start.x as f64 - c.0, arc.start.y as f64 - c.1);
                    let key = turn(s0, v, arc.dir == crate::geom::Dir::Ccw);
                    take(t, mk(i, key, p));
                }
            }
        }
    }
    best.map(|(_, h)| h)
}

fn mk(edge: usize, key: f64, p: (f64, f64)) -> Hit {
    Hit {
        edge,
        key,
        at: Point::new(round_nm(p.0), round_nm(p.1)),
        angle: ryu_math::atan2(p.1, p.0),
    }
}

/// Whether hits run strictly counterclockwise round the origin, all
/// within one turn of the first.
pub(crate) fn in_order(hits: &[Hit]) -> bool {
    if hits.is_empty() {
        return true;
    }
    let tau = 2.0 * std::f64::consts::PI;
    let mut last = 0.0;
    for h in &hits[1..] {
        let rel = (h.angle - hits[0].angle).rem_euclid(tau);
        if rel <= last {
            return false;
        }
        last = rel;
    }
    true
}

/// Part of an edge, from `a` to `b` on it; `None` when they are one point.
fn part(e: &Edge, a: Point, b: Point) -> Option<Edge> {
    if a == b {
        return None;
    }
    Some(match e {
        Edge::Segment(_) => Edge::line(a, b),
        Edge::Arc(arc) => Edge::Arc(Arc {
            start: a,
            end: b,
            centre: arc.centre,
            dir: arc.dir,
        }),
    })
}

/// The path along `c` from hit `a` to hit `b`, the contour's way round.
pub(crate) fn sub_path(c: &Contour, a: &Hit, b: &Hit) -> Vec<Edge> {
    let edges = c.edges();
    let n = edges.len();
    if a.edge == b.edge && a.key < b.key {
        return part(&edges[a.edge], a.at, b.at).into_iter().collect();
    }
    let mut out: Vec<Edge> = part(&edges[a.edge], a.at, edges[a.edge].end())
        .into_iter()
        .collect();
    let mut i = (a.edge + 1) % n;
    while i != b.edge {
        out.push(edges[i]);
        i = (i + 1) % n;
    }
    out.extend(part(&edges[b.edge], edges[b.edge].start(), b.at));
    out
}

#[cfg(test)]
mod tests {
    use super::super::build::{circle, rect};
    use super::*;
    use crate::symbol::Corners;

    #[test]
    fn lines_meet_squares_and_circles() {
        let sq = rect(-100, -100, 100, 100, &Corners::SHARP).unwrap();
        let h = hit(&sq, &Line::new(0.0, 10.0)).unwrap();
        assert_eq!(h.at, Point::new(100, 10));
        let h = hit(&sq, &Line::new(std::f64::consts::PI, 10.0)).unwrap();
        assert_eq!(h.at, Point::new(-100, -10));
        let c = circle(Point::ORIGIN, 100).unwrap();
        let h = hit(&c, &Line::new(std::f64::consts::FRAC_PI_2, 60.0)).unwrap();
        assert_eq!(h.at, Point::new(-60, 80));
        assert!(hit(&c, &Line::new(0.0, 200.0)).is_none());
    }

    #[test]
    fn sub_paths_run_round() {
        let sq = rect(-100, -100, 100, 100, &Corners::SHARP).unwrap();
        let a = hit(&sq, &Line::new(0.0, 10.0)).unwrap();
        let b = hit(&sq, &Line::new(0.0, -10.0)).unwrap();
        // From just above the x axis on the right, all the way round.
        let p = sub_path(&sq, &a, &b);
        assert_eq!(p.first().unwrap().start(), Point::new(100, 10));
        assert_eq!(p.last().unwrap().end(), Point::new(100, -10));
        assert_eq!(p.len(), 5);
        let q = sub_path(&sq, &b, &a);
        assert_eq!(q.len(), 1);
        let c = circle(Point::ORIGIN, 100).unwrap();
        let a = hit(&c, &Line::new(0.0, 60.0)).unwrap();
        let b = hit(&c, &Line::new(0.0, -60.0)).unwrap();
        // Counterclockwise from above the axis to below is the long way,
        // one arc; from below to above crosses the circle's start, two.
        assert_eq!(sub_path(&c, &a, &b).len(), 1);
        assert_eq!(sub_path(&c, &b, &a).len(), 2);
        assert!(in_order(&[b, a]));
        assert!(!in_order(&[a, b, a]));
    }
}
