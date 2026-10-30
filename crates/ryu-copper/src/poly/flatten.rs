//! Shapes as integer rings: every arc on chords within a bound of it both
//! ways, pens swept as the polygons they cover, holes as discs and slots.
//!
//! An arc's chords have their inner vertices pushed out by half their
//! sagitta `s`, so each lies within `s` of the arc either way (an end
//! chord, from a point on the arc, reaches `s` inside), and rounding a
//! vertex to the nanometre adds under one. A disc's vertices lie half a
//! sagitta out, so it is within `s / 2 + 1`. A path drawn with a pen is
//! within the sum of its path's and its pen's deviations. Each part's
//! rings carry the deviation they reached, at most half the options'
//! bound: what a boolean adds stays inside the bound.

use ryu_board::geom::{Arc, Contour, Dir, Edge, Nm, Pen, Point, Region, round_nm, span};
use ryu_board::model::Polarity;
use ryu_board::shape::{Piece, Shape, line_outline};

use super::{Rings, area2};
use crate::layout::HoleShape;
use crate::measure::body::{convex, hull};

/// How shapes are flattened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlattenOptions {
    bound: Nm,
}

impl FlattenOptions {
    /// The default bound, in nanometres.
    pub const DEFAULT_BOUND: Nm = 100;
    /// The finest bound a caller may ask for.
    pub const MIN_BOUND: Nm = 10;

    /// Options with this bound, raised to [`Self::MIN_BOUND`] if finer.
    pub fn new(bound: Nm) -> FlattenOptions {
        FlattenOptions {
            bound: bound.max(Self::MIN_BOUND),
        }
    }

    pub fn bound(&self) -> Nm {
        self.bound
    }

    /// What one flattened part may deviate by: half the bound.
    fn budget(&self) -> f64 {
        self.bound as f64 / 2.0
    }
}

impl Default for FlattenOptions {
    fn default() -> FlattenOptions {
        FlattenOptions::new(Self::DEFAULT_BOUND)
    }
}

/// The most chords one arc or disc is cut into.
const MAX_CHORDS: f64 = 65_536.0;

/// How many chords a turn of `sweep` at radius `r` needs for a sagitta of
/// at most `s_max` (at least `min`), and the sagitta they have.
fn chords_for(r: f64, sweep: f64, s_max: f64, min: f64) -> (f64, f64) {
    let sag = |n: f64| r * (1.0 - ryu_math::cos(sweep / (2.0 * n)));
    let mut n = (sweep * ryu_math::sqrt(r / (8.0 * s_max)))
        .ceil()
        .clamp(min, MAX_CHORDS);
    while n < MAX_CHORDS && sag(n) > s_max {
        n += 1.0;
    }
    (n, sag(n))
}

/// An arc's points from its start to its written end, within `s_max + 1`
/// of it both ways, and the deviation they keep.
pub(crate) fn arc_points(a: &Arc, s_max: f64) -> (Vec<Point>, f64) {
    if a.is_degenerate() {
        return (vec![a.start, a.end], 0.0);
    }
    let r = a.radius();
    let sweep = a.sweep();
    let (n, s) = chords_for(r, sweep, s_max, if a.is_full() { 4.0 } else { 1.0 });
    let rho = r + s / 2.0;
    let st = a.start.minus(a.centre);
    let a0 = ryu_math::atan2(st.1 as f64, st.0 as f64);
    let sign = if a.dir == Dir::Ccw { 1.0 } else { -1.0 };
    let (cx, cy) = (a.centre.x as f64, a.centre.y as f64);
    let mut pts = vec![a.start];
    for k in 1..n as usize {
        let t = a0 + sign * sweep * k as f64 / n;
        let (sin, cos) = ryu_math::sin_cos(t);
        pts.push(Point::new(
            round_nm(cx + rho * cos),
            round_nm(cy + rho * sin),
        ));
    }
    if a.is_full() {
        pts.push(a.start);
    } else {
        let e = a.end.minus(a.centre);
        let k = r / ryu_math::sqrt(e.len2() as f64);
        pts.push(Point::new(
            round_nm(cx + e.0 as f64 * k),
            round_nm(cy + e.1 as f64 * k),
        ));
        pts.push(a.end);
    }
    pts.dedup();
    (pts, s + 1.0)
}

/// A contour's points, arcs on chords, and the deviation it keeps.
fn contour_points(c: &Contour, s_max: f64) -> (Vec<Point>, f64) {
    let mut pts: Vec<Point> = Vec::new();
    let mut dev = 0.0f64;
    for e in c.edges() {
        match e {
            Edge::Segment(s) => pts.push(s.a),
            Edge::Arc(a) => {
                let (p, d) = arc_points(a, s_max);
                dev = dev.max(d);
                pts.extend_from_slice(&p[..p.len() - 1]);
            }
        }
    }
    (closed(pts), dev)
}

/// A ring's points with repeats and the closing repeat taken out.
fn closed(mut pts: Vec<Point>) -> Vec<Point> {
    pts.dedup();
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    pts
}

/// Turns a ring to wind `ccw` (or clockwise), by its exact area.
fn wound(mut r: Vec<Point>, ccw: bool) -> Vec<Point> {
    if (area2(&r) > 0) != ccw {
        r.reverse();
    }
    r
}

/// A region's rings: the outer counterclockwise, holes clockwise.
fn region_rings(r: &Region, s_max: f64, out: &mut Vec<Vec<Point>>) -> f64 {
    let (outer, mut dev) = contour_points(&r.outer, s_max);
    if outer.len() < 3 {
        return dev;
    }
    out.push(wound(outer, true));
    for h in &r.holes {
        let (h, d) = contour_points(h, s_max);
        dev = dev.max(d);
        if h.len() >= 3 {
            out.push(wound(h, false));
        }
    }
    dev
}

/// A path's points, arcs on chords, and the deviation it keeps.
fn path_points(path: &[Edge], s_max: f64) -> (Vec<Point>, f64) {
    let mut pts = Vec::new();
    let mut dev = 0.0f64;
    for e in path {
        match e {
            Edge::Segment(s) => {
                pts.push(s.a);
                pts.push(s.b);
            }
            Edge::Arc(a) => {
                let (p, d) = arc_points(a, s_max);
                dev = dev.max(d);
                pts.extend(p);
            }
        }
    }
    pts.dedup();
    (pts, dev)
}

/// A disc of diameter `d` about the origin, counterclockwise, within
/// `s_max / 2 + 1` of it both ways, and that deviation; empty for no
/// width.
pub fn disc(d: Nm, s_max: f64) -> (Vec<Point>, f64) {
    if d <= 0 {
        return (Vec::new(), 0.0);
    }
    let r = d as f64 / 2.0;
    let turn = 2.0 * std::f64::consts::PI;
    let (n, s) = chords_for(r, turn, s_max, 4.0);
    let rho = r + s / 2.0;
    let pts: Vec<Point> = (0..n as usize)
        .map(|k| {
            let (sin, cos) = ryu_math::sin_cos(turn * k as f64 / n);
            Point::new(round_nm(rho * cos), round_nm(rho * sin))
        })
        .collect();
    let pts = closed(pts);
    if pts.len() < 3 {
        return (Vec::new(), 0.0);
    }
    (wound(pts, true), s / 2.0 + 1.0)
}

fn moved(l: &[Point], by: Point) -> Vec<Point> {
    l.iter().map(|p| p.offset(by.x, by.y)).collect()
}

/// What a convex pen covers along each chord of a path: the hull of the
/// pen at both ends of each, counterclockwise.
pub(crate) fn convex_sweep(pen: &[Point], path: &[Point], out: &mut Vec<Vec<Point>>) {
    if pen.len() < 3 {
        return;
    }
    if path.len() == 1 {
        out.push(moved(pen, path[0]));
    }
    for c in path.windows(2) {
        let mut pts = moved(pen, c[0]);
        pts.extend(moved(pen, c[1]));
        let h = hull(pts);
        if h.len() >= 3 {
            out.push(h);
        }
    }
}

/// What a pen of any shape (rings, outer counterclockwise and holes
/// clockwise) covers along each chord of a path: the pen at the chord's
/// start, and the parallelogram each of its edges sweeps, each turned
/// counterclockwise, so every ring adds 0 or 1 to the winding and their
/// nonzero union is what the pen covers.
fn any_sweep(pen: &[Vec<Point>], path: &[Point], out: &mut Vec<Vec<Point>>) {
    if path.len() == 1 {
        out.extend(pen.iter().map(|l| moved(l, path[0])));
        return;
    }
    for c in path.windows(2) {
        out.extend(pen.iter().map(|l| moved(l, c[0])));
        let d = c[1].minus(c[0]);
        for l in pen {
            for k in 0..l.len() {
                let (u, v) = (l[k], l[(k + 1) % l.len()]);
                if v.minus(u).cross(d) == 0 {
                    continue;
                }
                let ring = vec![
                    u.offset(c[0].x, c[0].y),
                    v.offset(c[0].x, c[0].y),
                    v.offset(c[1].x, c[1].y),
                    u.offset(c[1].x, c[1].y),
                ];
                out.push(wound(ring, true));
            }
        }
    }
}

/// A shape's parts as rings, in drawing order, each with its polarity
/// and the deviation it keeps. Parts that flatten to nothing (no width,
/// or smaller than a nanometre) are left out.
pub fn flatten_shape(shape: &Shape, opts: &FlattenOptions) -> Vec<(Polarity, Rings)> {
    let budget = opts.budget();
    let mut parts = Vec::new();
    for part in &shape.parts {
        let mut rings = Vec::new();
        let dev = match &part.piece {
            Piece::Region(r) => region_rings(r, budget - 1.0, &mut rings),
            Piece::Stroke(s) => match s.pen {
                Pen::Round(w) => {
                    let (path, d1) = path_points(&s.path, budget / 2.0 - 1.0);
                    let (pen, d2) = disc(w, budget - 2.0);
                    convex_sweep(&pen, &path, &mut rings);
                    d1 + d2
                }
                Pen::Square(w) => {
                    let mut dev = 0.0f64;
                    for e in &s.path {
                        match e {
                            Edge::Segment(g) => {
                                if let Some(r) = line_outline(g.a, g.b, Pen::Square(w)) {
                                    region_rings(&r, budget - 1.0, &mut rings);
                                }
                            }
                            Edge::Arc(a) => {
                                let (path, d) = arc_points(a, budget - 1.0);
                                dev = dev.max(d);
                                let (lo, hi) = span(w);
                                if lo < hi {
                                    let pen = [
                                        Point::new(lo, lo),
                                        Point::new(hi, lo),
                                        Point::new(hi, hi),
                                        Point::new(lo, hi),
                                    ];
                                    convex_sweep(&pen, &path, &mut rings);
                                }
                            }
                        }
                    }
                    dev
                }
            },
            Piece::Swept { path, pen } => {
                let (path, d1) = path_points(std::slice::from_ref(path), budget / 2.0 - 1.0);
                let mut d2 = 0.0f64;
                for r in pen {
                    let mut l = Vec::new();
                    d2 = d2.max(region_rings(r, budget / 2.0 - 1.0, &mut l));
                    match l.as_slice() {
                        [one] if convex(one) => convex_sweep(one, &path, &mut rings),
                        [] => {}
                        _ => any_sweep(&l, &path, &mut rings),
                    }
                }
                d1 + d2
            }
        };
        if !rings.is_empty() {
            parts.push((part.polarity, Rings::new(rings, dev.ceil() as Nm)));
        }
    }
    parts
}

/// A drilled hole's outline as a ring: a disc, or a slot's two discs'
/// hull.
pub fn flatten_hole(h: &HoleShape, opts: &FlattenOptions) -> Rings {
    let (shape_d, a, b) = match *h {
        HoleShape::Round { at, d } => (d, at, at),
        HoleShape::Slot { a, b, d } => (d, a, b),
    };
    let (pen, dev) = disc(shape_d, opts.budget() - 2.0);
    let mut rings = Vec::new();
    convex_sweep(&pen, &[a, b], &mut rings);
    if a == b {
        rings.truncate(1);
    }
    Rings::new(rings, dev.ceil() as Nm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryu_board::geom::Stroke;
    use ryu_board::shape::{Part, circle};

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    /// The distance from the origin to the segment `a b`.
    fn seg_dist(a: Point, b: Point) -> f64 {
        let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
        let (ux, uy) = (bx - ax, by - ay);
        let l = ux * ux + uy * uy;
        let t = if l == 0.0 {
            0.0
        } else {
            (-(ax * ux + ay * uy) / l).clamp(0.0, 1.0)
        };
        let (x, y) = (ax + ux * t, ay + uy * t);
        ryu_math::sqrt(x * x + y * y)
    }

    #[test]
    fn arcs_lie_within_their_deviation_both_ways() {
        for &(r, s_max) in &[(1_000_000i64, 4.0), (50_000, 49.0), (300, 4.0), (7, 4.0)] {
            let a = Arc {
                start: p(r, 0),
                end: p(0, -r),
                centre: p(0, 0),
                dir: Dir::Ccw,
            };
            let (pts, dev) = arc_points(&a, s_max);
            assert!(dev <= s_max + 1.0, "{r}: {dev}");
            assert_eq!((pts[0], *pts.last().unwrap()), (a.start, a.end));
            for w in pts.windows(2) {
                // Every vertex within `dev` of the circle, and every
                // chord's nearest point to the centre too.
                let vr = ryu_math::sqrt((w[0].x * w[0].x + w[0].y * w[0].y) as f64);
                assert!((vr - r as f64).abs() <= dev, "{r}: vertex {vr}");
                assert!(r as f64 - seg_dist(w[0], w[1]) <= dev, "{r}: chord");
            }
        }
    }

    #[test]
    fn discs_lie_within_their_deviation() {
        for &(d, s_max) in &[(1_000_000i64, 48.0), (201, 3.0), (3, 3.0)] {
            let (pts, dev) = disc(d, s_max);
            assert!(pts.len() >= 3 && dev <= s_max / 2.0 + 1.0, "{d}");
            assert!(area2(&pts) > 0);
            let r = d as f64 / 2.0;
            for k in 0..pts.len() {
                let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                let vr = ryu_math::sqrt((a.x * a.x + a.y * a.y) as f64);
                assert!((vr - r).abs() <= dev, "{d}: {vr}");
                assert!(r - seg_dist(a, b) <= dev, "{d}: chord");
            }
        }
        assert!(disc(0, 4.0).0.is_empty());
    }

    #[test]
    fn options_have_a_floor() {
        assert_eq!(FlattenOptions::default().bound(), 100);
        assert_eq!(FlattenOptions::new(3).bound(), 10);
    }

    #[test]
    fn shapes_flatten_by_part_within_half_the_bound() {
        let opts = FlattenOptions::default();
        let disc_region = Region::solid(circle(p(0, 0), 5_000).unwrap());
        let mut s = Shape::regions(Polarity::Positive, [disc_region]);
        s.parts.push(Part {
            polarity: Polarity::Negative,
            piece: Piece::Stroke(Stroke {
                path: vec![Edge::Arc(Arc {
                    start: p(10_000, 0),
                    end: p(0, 10_000),
                    centre: p(0, 0),
                    dir: Dir::Ccw,
                })],
                pen: Pen::Round(1_000),
            }),
        });
        let parts = flatten_shape(&s, &opts);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, Polarity::Positive);
        assert_eq!(parts[1].0, Polarity::Negative);
        for (_, r) in &parts {
            assert!(r.bound <= 50, "{}", r.bound);
            for ring in &r.rings {
                assert!(area2(ring) > 0);
            }
        }
    }

    #[test]
    fn holes_are_discs_and_slots() {
        let opts = FlattenOptions::default();
        let r = flatten_hole(
            &HoleShape::Round {
                at: p(5, 5),
                d: 300,
            },
            &opts,
        );
        assert_eq!(r.rings.len(), 1);
        let s = flatten_hole(
            &HoleShape::Slot {
                a: p(0, 0),
                b: p(1_000, 0),
                d: 200,
            },
            &opts,
        );
        assert_eq!(s.rings.len(), 1);
        assert!(area2(&s.rings[0]) > 2 * 1_000 * 200);
    }
}
