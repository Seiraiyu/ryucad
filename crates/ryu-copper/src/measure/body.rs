//! Bodies from shapes: a region as it is, a round pen as thick pieces of
//! its path, a square or symbol pen along a segment as the polygons it
//! sweeps (exact), and along an arc, or with arcs in its outline, on
//! chords within the bound.

use ryu_board::geom::{Arc, Contour, Edge, Nm, Pen, Point, Region, orient, round_nm};
use ryu_board::model::Polarity;
use ryu_board::shape::{Piece, Shape, line_outline};

use super::core::{Core, SegP, edge_cores};
use super::{Body, Shell, Solid};

/// How bodies are made from shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BodyOptions {
    bound: Nm,
}

impl BodyOptions {
    /// The default bound for pens swept along arcs, in nanometres.
    pub const DEFAULT_BOUND: Nm = 100;
    /// The finest bound a caller may ask for.
    pub const MIN_BOUND: Nm = 10;

    /// Options with this bound, raised to [`Self::MIN_BOUND`] if finer.
    pub fn new(bound: Nm) -> BodyOptions {
        BodyOptions {
            bound: bound.max(Self::MIN_BOUND),
        }
    }

    pub fn bound(&self) -> Nm {
        self.bound
    }
}

impl Default for BodyOptions {
    fn default() -> BodyOptions {
        BodyOptions::new(Self::DEFAULT_BOUND)
    }
}

/// The most chords one arc is cut into.
const MAX_CHORDS: f64 = 65_536.0;

/// Points along an arc from its start to its written end, chords between
/// them within `bound` of the arc both ways, and the bound they keep.
///
/// A chord across `φ` lies within `r φ² / 8` of its arc; each point is
/// rounded to the nanometre, within `√2 / 2`. So `n` chords with
/// `r (sweep / n)² / 8 <= bound - 1` keep the bound. The true end is
/// rounded too, and the join from it to the written end added.
pub fn chords(a: &Arc, bound: Nm) -> (Vec<Point>, Nm) {
    if a.is_degenerate() {
        return (vec![a.start, a.end], 0);
    }
    let r = a.radius();
    let sweep = a.sweep();
    let target = (bound - 1).max(1) as f64;
    let n = (sweep * ryu_math::sqrt(r / (8.0 * target)))
        .ceil()
        .clamp(1.0, MAX_CHORDS);
    let phi = sweep / n;
    let kept = (r * phi * phi / 8.0).min(r).ceil() as Nm + 1;
    let s = a.start.minus(a.centre);
    let a0 = ryu_math::atan2(s.1 as f64, s.0 as f64);
    let sign = if a.dir == ryu_board::geom::Dir::Ccw {
        1.0
    } else {
        -1.0
    };
    let (cx, cy) = (a.centre.x as f64, a.centre.y as f64);
    let mut pts = vec![a.start];
    for k in 1..n as usize {
        let t = a0 + sign * phi * k as f64;
        let (sin, cos) = ryu_math::sin_cos(t);
        pts.push(Point::new(round_nm(cx + r * cos), round_nm(cy + r * sin)));
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
    (pts, kept)
}

/// A shell of a region as it is.
pub fn shell(r: &Region) -> Shell {
    let mut cores = Vec::new();
    for c in std::iter::once(&r.outer).chain(&r.holes) {
        for e in c.edges() {
            edge_cores(e, &mut cores);
        }
    }
    Shell::new(cores, r.outer.edges()[0].start())
}

/// A shell of closed point loops.
fn loops(ls: &[Vec<Point>]) -> Shell {
    let mut cores = Vec::new();
    for l in ls {
        for k in 0..l.len() {
            cores.push(Core::Seg(SegP::new(l[k], l[(k + 1) % l.len()])));
        }
    }
    Shell::new(cores, ls[0][0])
}

/// A contour's points with every arc on chords within `bound`.
fn flat(c: &Contour, bound: Nm, kept: &mut Nm) -> Vec<Point> {
    let mut pts: Vec<Point> = Vec::new();
    for e in c.edges() {
        match e {
            Edge::Segment(s) => pts.push(s.a),
            Edge::Arc(a) => {
                let (p, k) = chords(a, bound);
                *kept = (*kept).max(k);
                // The last point is the written end, which the next edge
                // starts at.
                pts.extend_from_slice(&p[..p.len() - 1]);
            }
        }
    }
    pts.dedup();
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    pts
}

/// A path's points, arcs on chords within `bound`.
fn flat_path(path: &[Edge], bound: Nm, kept: &mut Nm) -> Vec<Point> {
    let mut pts = Vec::new();
    for e in path {
        match e {
            Edge::Segment(s) => {
                pts.push(s.a);
                pts.push(s.b);
            }
            Edge::Arc(a) => {
                let (p, k) = chords(a, bound);
                *kept = (*kept).max(k);
                pts.extend(p);
            }
        }
    }
    pts.dedup();
    pts
}

fn moved(l: &[Point], by: Point) -> Vec<Point> {
    l.iter().map(|p| p.offset(by.x, by.y)).collect()
}

/// What a pen of point loops covers moving along a path of points.
///
/// A convex pen of one loop covers the hull of itself at both ends of
/// each chord. Any other:
///
/// A pen `P` moving along a path `C` covers `P` at the path's start and
/// what each of its edges `e` sweeps, `e ⊕ C`: a point of `P` moved along
/// the path either stays inside `P` (where it started) or crosses an
/// edge. Along a run of the path's chords that all cross `e`'s direction
/// the same way, the parallelograms `e` sweeps all turn one way, so the
/// one loop round them (the run moved by `e`'s start, then back moved by
/// its end) has a nonzero winding number exactly where they are: one
/// region per edge per run. A chord parallel to `e` sweeps a segment.
fn sweep(pen: &[Vec<Point>], path: &[Point], out: &mut Vec<Solid>) {
    if let [l] = pen
        && convex(l)
    {
        // A convex pen covers, along each chord, the hull of itself at
        // both ends: one small region per chord.
        if path.len() == 1 {
            out.push(Solid::Region(loops(&[moved(l, path[0])])));
        }
        for c in path.windows(2) {
            let mut pts = moved(l, c[0]);
            pts.extend(moved(l, c[1]));
            out.push(Solid::Region(loops(&[hull(pts)])));
        }
        return;
    }
    out.push(Solid::Region(loops(
        &pen.iter().map(|l| moved(l, path[0])).collect::<Vec<_>>(),
    )));
    for l in pen {
        for k in 0..l.len() {
            let (u, v) = (l[k], l[(k + 1) % l.len()]);
            if u == v {
                continue;
            }
            let e = v.minus(u);
            let mut run: Vec<Point> = Vec::new();
            let mut side = 0i128;
            let close = |run: &mut Vec<Point>, out: &mut Vec<Solid>| {
                if run.len() > 1 {
                    let mut ring = moved(run, u);
                    ring.extend(moved(run, v).into_iter().rev());
                    out.push(Solid::Region(loops(&[ring])));
                }
                run.clear();
            };
            for c in path.windows(2) {
                let turn = c[1].minus(c[0]).cross(e).signum();
                if turn == 0 {
                    close(&mut run, out);
                    for (a, b) in [
                        (c[0].offset(u.x, u.y), c[1].offset(v.x, v.y)),
                        (c[1].offset(u.x, u.y), c[0].offset(v.x, v.y)),
                    ] {
                        out.push(Solid::Thick {
                            core: Core::Seg(SegP::new(a, b)),
                            w: 0,
                        });
                    }
                    side = 0;
                    continue;
                }
                if turn != side {
                    close(&mut run, out);
                    run.push(c[0]);
                    side = turn;
                }
                run.push(c[1]);
            }
            close(&mut run, out);
        }
    }
}

/// Whether a loop of points turns one way only: a convex polygon, for the
/// simple loops symbols' outlines give.
fn convex(l: &[Point]) -> bool {
    let n = l.len();
    if n < 3 {
        return false;
    }
    let turns: Vec<i128> = (0..n)
        .map(|k| orient(l[k], l[(k + 1) % n], l[(k + 2) % n]).signum())
        .collect();
    turns.iter().all(|&t| t >= 0) || turns.iter().all(|&t| t <= 0)
}

/// The convex hull of points, counterclockwise, by exact orientation
/// (Andrew's monotone chain), with no collinear points.
fn hull(mut pts: Vec<Point>) -> Vec<Point> {
    pts.sort();
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<Point> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && orient(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && orient(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A square pen along a segment: the hull of the pen at both ends.
fn square(a: Point, b: Point, w: Nm, out: &mut Vec<Solid>) {
    match line_outline(a, b, Pen::Square(w)) {
        Some(r) => out.push(Solid::Region(shell(&r))),
        None => out.push(Solid::Thick {
            core: Core::Seg(SegP::new(a, b)),
            w: 0,
        }),
    }
}

impl Body {
    /// What a shape's positive parts cover.
    pub fn of_shape(shape: &Shape, opts: &BodyOptions) -> Body {
        let mut solids = Vec::new();
        let mut kept = 0;
        let mut negatives = 0;
        for part in &shape.parts {
            if part.polarity == Polarity::Negative {
                negatives += 1;
                continue;
            }
            match &part.piece {
                Piece::Region(r) => solids.push(Solid::Region(shell(r))),
                Piece::Stroke(s) => match s.pen {
                    Pen::Round(w) => {
                        let mut cores = Vec::new();
                        for e in &s.path {
                            edge_cores(e, &mut cores);
                        }
                        solids.extend(cores.into_iter().map(|core| Solid::Thick { core, w }));
                    }
                    Pen::Square(w) => {
                        for e in &s.path {
                            match e {
                                Edge::Segment(g) => square(g.a, g.b, w, &mut solids),
                                Edge::Arc(a) => {
                                    let (p, k) = chords(a, opts.bound);
                                    kept = kept.max(k);
                                    let (lo, hi) = ryu_board::geom::span(w);
                                    if lo >= hi {
                                        for c in p.windows(2) {
                                            square(c[0], c[1], w, &mut solids);
                                        }
                                        continue;
                                    }
                                    let pen = vec![vec![
                                        Point::new(lo, lo),
                                        Point::new(hi, lo),
                                        Point::new(hi, hi),
                                        Point::new(lo, hi),
                                    ]];
                                    sweep(&pen, &p, &mut solids);
                                }
                            }
                        }
                    }
                },
                Piece::Swept { path, pen } => {
                    let pts = flat_path(std::slice::from_ref(path), opts.bound, &mut kept);
                    for r in pen {
                        let pen: Vec<Vec<Point>> = std::iter::once(&r.outer)
                            .chain(&r.holes)
                            .map(|c| flat(c, opts.bound, &mut kept))
                            .filter(|l| !l.is_empty())
                            .collect();
                        if !pen.is_empty() {
                            sweep(&pen, &pts, &mut solids);
                        }
                    }
                }
            }
        }
        let mut b = Body::new(solids, kept);
        b.negatives = negatives;
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::{Verdict, contains, within};
    use ryu_board::geom::{Dir, Stroke};
    use ryu_board::shape::{Part, circle};

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn part(piece: Piece) -> Shape {
        Shape {
            parts: vec![Part {
                polarity: Polarity::Positive,
                piece,
            }],
            ..Shape::default()
        }
    }

    #[test]
    fn chords_keep_their_bound() {
        let a = Arc {
            start: p(1_000_000, 0),
            end: p(-3, 1_000_001),
            centre: p(0, 0),
            dir: Dir::Ccw,
        };
        let (pts, kept) = chords(&a, 20);
        assert!(kept <= 20, "{kept}");
        // Every point within a nanometre of the circle; the last two the
        // rounded true end and the written end.
        for q in &pts[..pts.len() - 1] {
            let r = ((q.x * q.x + q.y * q.y) as f64).sqrt();
            assert!((r - 1_000_000.0).abs() < 1.0, "{q:?}");
        }
        assert_eq!(pts[0], a.start);
        assert_eq!(pts[pts.len() - 1], a.end);
        // Chords no longer than the bound allows: r φ² / 8 <= 19.
        let longest = pts
            .windows(2)
            .map(|w| (w[1].minus(w[0]).len2() as f64).sqrt())
            .fold(0.0, f64::max);
        assert!(longest * longest / (8.0 * 1_000_000.0) <= 19.0 + 1e-6);
        // Degenerate: the two ends.
        let d = Arc {
            centre: a.start,
            ..a
        };
        assert_eq!(chords(&d, 20), (vec![d.start, d.end], 0));
    }

    #[test]
    fn options_have_a_floor() {
        assert_eq!(BodyOptions::default().bound(), 100);
        assert_eq!(BodyOptions::new(3).bound(), 10);
        assert_eq!(BodyOptions::new(50).bound(), 50);
    }

    #[test]
    fn regions_and_round_pens_are_exact() {
        let r = Region::solid(circle(p(0, 0), 500).unwrap());
        let b = Body::of_shape(
            &Shape::regions(Polarity::Positive, [r]),
            &BodyOptions::default(),
        );
        assert_eq!((b.solids.len(), b.bound), (1, 0));
        assert_eq!(contains(&b, p(0, 500)), Verdict::Yes);
        assert_eq!(contains(&b, p(354, 354)), Verdict::No);
        let s = part(Piece::Stroke(Stroke {
            path: vec![
                Edge::line(p(0, 0), p(1000, 0)),
                Edge::Arc(Arc {
                    start: p(1000, 0),
                    end: p(1000, 1000),
                    centre: p(1000, 500),
                    dir: Dir::Ccw,
                }),
            ],
            pen: Pen::Round(100),
        }));
        let b = Body::of_shape(&s, &BodyOptions::default());
        assert_eq!((b.solids.len(), b.bound), (2, 0));
        // The far side of the half turn, 550 right of its centre.
        assert_eq!(within(&b, &Body::point(p(1550, 500)), 0), Verdict::Yes);
        assert_eq!(within(&b, &Body::point(p(1551, 500)), 0), Verdict::No);
    }

    #[test]
    fn square_pens_on_arcs_are_bounded() {
        let arc = Arc {
            start: p(10_000, 0),
            end: p(0, 10_000),
            centre: p(0, 0),
            dir: Dir::Ccw,
        };
        let s = part(Piece::Stroke(Stroke {
            path: vec![Edge::Arc(arc)],
            pen: Pen::Square(1000),
        }));
        let b = Body::of_shape(&s, &BodyOptions::new(20));
        assert!(b.bound > 0 && b.bound <= 20, "{}", b.bound);
        // |(7600, 7600)| = 10748; less 10000 and the square's half
        // diagonal 707: 41 from the band.
        let far = Body::point(p(7_600, 7_600));
        assert_eq!(within(&b, &far, 10), Verdict::No);
        assert_eq!(within(&b, &far, 41), Verdict::Uncertain);
        assert_eq!(within(&b, &far, 80), Verdict::Yes);
        // Along a segment it stays exact.
        let l = part(Piece::Stroke(Stroke {
            path: vec![Edge::line(p(0, 0), p(1000, 0))],
            pen: Pen::Square(200),
        }));
        let l = Body::of_shape(&l, &BodyOptions::default());
        assert_eq!(l.bound, 0);
        assert_eq!(contains(&l, p(1100, 100)), Verdict::Yes);
        assert_eq!(contains(&l, p(1100, 101)), Verdict::No);
    }

    #[test]
    fn swept_pens_cover_the_path() {
        // A triangle swept from (0, 0) to (1000, 0).
        let tri =
            Region::solid(ryu_board::shape::polygon(&[p(-50, -50), p(50, -50), p(0, 50)]).unwrap());
        let s = part(Piece::Swept {
            path: Edge::line(p(0, 0), p(1000, 0)),
            pen: vec![tri.clone()],
        });
        let b = Body::of_shape(&s, &BodyOptions::default());
        // A triangle is convex: one hull.
        assert_eq!((b.bound, b.solids.len()), (0, 1));
        for (x, y, inside) in [
            (500, 49, true),
            (500, -50, true),
            (500, 51, false),
            (1050, -50, true),
            (1051, -50, false),
        ] {
            assert_eq!(contains(&b, p(x, y)).is_yes(), inside, "({x}, {y})");
        }
        // Along a quarter turn the arc is on chords: bounded, and the far
        // side of the turn is covered.
        let s = part(Piece::Swept {
            path: Edge::Arc(Arc {
                start: p(10_000, 0),
                end: p(0, 10_000),
                centre: p(0, 0),
                dir: Dir::Ccw,
            }),
            pen: vec![tri],
        });
        let b = Body::of_shape(&s, &BodyOptions::default());
        assert!(b.bound > 0);
        // A bound can't settle touching (distance zero), even on the path
        // itself; distances past the bound it can.
        assert_eq!(contains(&b, p(7071, 7071)), Verdict::Uncertain);
        assert_eq!(
            within(&b, &Body::point(p(7071, 7071)), b.bound),
            Verdict::Yes
        );
        assert_eq!(contains(&b, p(7300, 7300)), Verdict::No);
    }

    #[test]
    fn concave_pens_sweep_their_edges() {
        // An L-shaped pen swept from (0, 0) to (1000, 0): the pen at the
        // start, a region per upright edge, and two segments for each edge
        // along the path.
        let l = Region::solid(
            ryu_board::shape::polygon(&[
                p(0, 0),
                p(100, 0),
                p(100, 30),
                p(30, 30),
                p(30, 100),
                p(0, 100),
            ])
            .unwrap(),
        );
        let s = part(Piece::Swept {
            path: Edge::line(p(0, 0), p(1000, 0)),
            pen: vec![l],
        });
        let b = Body::of_shape(&s, &BodyOptions::default());
        assert_eq!((b.bound, b.solids.len()), (0, 1 + 3 + 3 * 2));
        // The L's upright reaches x = 1030 at the end, its foot 1100.
        for (x, y, inside) in [
            (500, 50, true),
            (1020, 99, true),
            (1030, 100, true),
            (1031, 99, false),
            (1050, 10, true),
            (1101, 10, false),
            (-1, 50, false),
        ] {
            assert_eq!(contains(&b, p(x, y)).is_yes(), inside, "({x}, {y})");
        }
    }

    #[test]
    fn negatives_are_left_out() {
        let r = Region::solid(circle(p(0, 0), 10).unwrap());
        let mut s = Shape::regions(Polarity::Positive, [r.clone()]);
        s.parts.push(Part {
            polarity: Polarity::Negative,
            piece: Piece::Region(r),
        });
        let b = Body::of_shape(&s, &BodyOptions::default());
        assert_eq!((b.solids.len(), b.negatives), (1, 1));
    }
}
