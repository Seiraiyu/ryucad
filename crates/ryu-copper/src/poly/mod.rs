//! Polygon booleans and offsets on integer nanometres: shapes flattened
//! within a carried bound, an exact sweep over snap-rounded edges, merged
//! layers, and offsets.
//!
//! Everything a boolean decides is an exact integer predicate over its
//! input's vertices: crossings are found as rationals and snapped to the
//! nanometre grid (Hobby's hot pixels), and the windings either side of
//! every snapped edge come from one sweep. A result is a set of simple
//! polygons, outer rings counterclockwise and holes clockwise, in a
//! canonical order, with the bound it lies within of the exact result.

mod flatten;
mod layer;
mod offset;
mod rings;
mod snap;
mod sweep;

pub use flatten::{FlattenOptions, disc, flatten_hole, flatten_shape};
pub use layer::{merge_layer, merge_layers};
pub use offset::{close, grow, open, shrink};

use ryu_board::geom::{Nm, Point, Rect, orient};

use crate::measure::Verdict;

/// A simple polygon: its outer ring counterclockwise and its holes
/// clockwise, each ring starting at its least vertex, with no vertex
/// repeated and no three consecutive vertices on one line unless another
/// ring touches the middle one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Polygon {
    pub outer: Vec<Point>,
    pub holes: Vec<Vec<Point>>,
}

/// Polygons that meet at most at vertices, and how far they may lie from
/// the exact figure they stand for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Polygons {
    /// Sorted by their outer rings.
    pub polygons: Vec<Polygon>,
    /// Every point of the figure lies within this many nanometres of the
    /// polygons, and every point of the polygons within it of the figure.
    pub bound: Nm,
}

/// Rings to combine, each filled by the winding rule it is given with:
/// a counterclockwise ring winds +1 round what it encloses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rings {
    pub rings: Vec<Vec<Point>>,
    /// How far the rings may lie from what they stand for.
    pub bound: Nm,
}

/// Which windings count as inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FillRule {
    NonZero,
    EvenOdd,
    Positive,
}

impl FillRule {
    pub fn inside(self, w: i32) -> bool {
        match self {
            FillRule::NonZero => w != 0,
            FillRule::EvenOdd => w & 1 == 1,
            FillRule::Positive => w > 0,
        }
    }
}

/// A boolean operation of two operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Union,
    Intersection,
    Difference,
    Xor,
}

impl Op {
    pub fn apply(self, a: bool, b: bool) -> bool {
        match self {
            Op::Union => a || b,
            Op::Intersection => a && b,
            Op::Difference => a && !b,
            Op::Xor => a != b,
        }
    }
}

impl Rings {
    pub fn new(rings: Vec<Vec<Point>>, bound: Nm) -> Rings {
        Rings { rings, bound }
    }
}

impl Polygon {
    /// Its rings, outer first.
    pub fn rings(&self) -> impl Iterator<Item = &Vec<Point>> {
        std::iter::once(&self.outer).chain(&self.holes)
    }

    /// Twice its area, holes taken away, exactly.
    pub fn area2(&self) -> i128 {
        self.rings().map(|r| area2(r)).sum()
    }

    pub fn bounds(&self) -> Rect {
        ring_bounds(&self.outer)
    }
}

impl Polygons {
    /// Every ring of every polygon, as rings to combine again.
    pub fn rings(&self) -> Rings {
        Rings {
            rings: self
                .polygons
                .iter()
                .flat_map(|p| p.rings().cloned())
                .collect(),
            bound: self.bound,
        }
    }

    /// Twice the area, exactly.
    pub fn area2(&self) -> i128 {
        self.polygons.iter().map(Polygon::area2).sum()
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.polygons
            .iter()
            .map(Polygon::bounds)
            .reduce(Rect::union)
    }

    pub fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }

    /// Whether the figure these polygons stand for holds `p`: `Uncertain`
    /// when `p` lies within the bound of an edge, where the figure's own
    /// outline may run either side of it.
    pub fn contains(&self, p: Point) -> Verdict {
        let reach = i128::from(self.bound);
        let mut inside = false;
        for poly in &self.polygons {
            let b = poly.bounds();
            if p.x < b.min.x - self.bound
                || p.x > b.max.x + self.bound
                || p.y < b.min.y - self.bound
                || p.y > b.max.y + self.bound
            {
                continue;
            }
            for r in poly.rings() {
                for k in 0..r.len() {
                    if seg_dist2_within(r[k], r[(k + 1) % r.len()], p, reach) {
                        return Verdict::Uncertain;
                    }
                }
            }
            if winding(&poly.outer, p) != 0 && poly.holes.iter().all(|h| winding(h, p) == 0) {
                inside = true;
            }
        }
        if inside { Verdict::Yes } else { Verdict::No }
    }
}

/// What is wrong with polygons that should be a boolean's result: rings
/// too short, outer rings not counterclockwise or holes not clockwise,
/// rings not starting at their least vertex, polygons or holes out of
/// order, and any two edges meeting other than at a shared end. Empty
/// when they are sound. Edges are paired through a tree of their boxes.
pub fn faults(p: &Polygons) -> Vec<String> {
    let mut out = Vec::new();
    let mut edges: Vec<(Point, Point)> = Vec::new();
    for (k, poly) in p.polygons.iter().enumerate() {
        if area2(&poly.outer) <= 0 {
            out.push(format!("polygon {k}: outer ring not counterclockwise"));
        }
        for h in &poly.holes {
            if area2(h) >= 0 {
                out.push(format!("polygon {k}: a hole not clockwise"));
            }
        }
        if !poly.holes.is_sorted() {
            out.push(format!("polygon {k}: holes out of order"));
        }
        for r in poly.rings() {
            if r.len() < 3 {
                out.push(format!("polygon {k}: a ring of {} vertices", r.len()));
                continue;
            }
            if r.iter().min() != Some(&r[0]) {
                out.push(format!(
                    "polygon {k}: a ring not starting at its least vertex"
                ));
            }
            for i in 0..r.len() {
                edges.push((r[i], r[(i + 1) % r.len()]));
            }
        }
    }
    if !p.polygons.windows(2).all(|w| w[0].outer < w[1].outer) {
        out.push("polygons out of order".into());
    }
    let tree = crate::tree::Packed::new(
        edges
            .iter()
            .enumerate()
            .map(|(k, &(a, b))| (Rect::new(a, b), k as u32))
            .collect(),
    );
    let mut near = Vec::new();
    for (i, &(a, b)) in edges.iter().enumerate() {
        near.clear();
        tree.query(&Rect::new(a, b), &mut near);
        for &j in &near {
            let (c, d) = edges[j as usize];
            if (j as usize) > i && bad_meeting(a, b, c, d) {
                out.push(format!("edges {a:?}-{b:?} and {c:?}-{d:?} meet"));
            }
        }
    }
    out
}

/// Whether two segments meet anywhere but at one shared end.
fn bad_meeting(a: Point, b: Point, c: Point, d: Point) -> bool {
    let inside = |p: Point, q: Point, x: Point| {
        orient(p, q, x) == 0
            && x != p
            && x != q
            && x.minus(p).dot(q.minus(p)) > 0
            && x.minus(q).dot(p.minus(q)) > 0
    };
    if (a == c && b == d) || (a == d && b == c) {
        return true;
    }
    if inside(a, b, c) || inside(a, b, d) || inside(c, d, a) || inside(c, d, b) {
        return true;
    }
    let (d1, d2) = (orient(a, b, c), orient(a, b, d));
    let (d3, d4) = (orient(c, d, a), orient(c, d, b));
    d1 != 0 && d2 != 0 && d3 != 0 && d4 != 0 && (d1 > 0) != (d2 > 0) && (d3 > 0) != (d4 > 0)
}

/// Twice a ring's signed area, exactly: positive counterclockwise.
pub fn area2(r: &[Point]) -> i128 {
    let n = r.len();
    (0..n)
        .map(|k| {
            let (a, b) = (r[k], r[(k + 1) % n]);
            i128::from(a.x) * i128::from(b.y) - i128::from(b.x) * i128::from(a.y)
        })
        .sum()
}

pub(crate) fn ring_bounds(r: &[Point]) -> Rect {
    r[1..].iter().fold(Rect::of_point(r[0]), |b, &p| b.with(p))
}

/// The winding number of a ring round `p`, which must not lie on it:
/// crossings of the ray from `p` along +x, edges counted half-open in y.
pub fn winding(r: &[Point], p: Point) -> i32 {
    let n = r.len();
    let mut w = 0;
    for k in 0..n {
        let (a, b) = (r[k], r[(k + 1) % n]);
        if a.y <= p.y {
            if b.y > p.y && orient(a, b, p) > 0 {
                w += 1;
            }
        } else if b.y <= p.y && orient(a, b, p) < 0 {
            w -= 1;
        }
    }
    w
}

/// Whether `p` lies within `reach` of the segment `a b`, exactly.
fn seg_dist2_within(a: Point, b: Point, p: Point, reach: i128) -> bool {
    let u = b.minus(a);
    let v = p.minus(a);
    let r2 = reach * reach;
    let t = u.dot(v);
    let l = u.len2();
    if t <= 0 || l == 0 {
        return v.len2() <= r2;
    }
    if t >= l {
        return p.minus(b).len2() <= r2;
    }
    // dist² = cross² / l, compared without dividing; cross < 2^83, so
    // cross² needs the wide integers.
    let c = u.cross(v);
    let lhs = crate::wide::Int::from(c) * crate::wide::Int::from(c);
    let rhs = crate::wide::Int::from(r2) * crate::wide::Int::from(l);
    lhs <= rhs
}

/// Combines two operands: `op` of `a` filled by `rule_a` and `b` filled by
/// `rule_b`. The result's bound is the larger operand's plus a nanometre
/// for the snapping.
pub fn boolean(a: &Rings, rule_a: FillRule, b: &Rings, rule_b: FillRule, op: Op) -> Polygons {
    let mut edges = Vec::new();
    for (operand, rings) in [(0u8, a), (1u8, b)] {
        for r in &rings.rings {
            snap::ring_edges(r, operand, &mut edges);
        }
    }
    let frags = snap::fragments(edges);
    let sides = sweep::windings(&frags);
    let inside = |w: [i32; 2]| op.apply(rule_a.inside(w[0]), rule_b.inside(w[1]));
    let polygons = rings::assemble(&frags, &sides, inside);
    Polygons {
        polygons,
        bound: a.bound.max(b.bound) + 1,
    }
}

/// What `rings` cover by `rule`, as simple polygons.
pub fn simplify(rings: &Rings, rule: FillRule) -> Polygons {
    boolean(rings, rule, &Rings::default(), FillRule::NonZero, Op::Union)
}

/// The union of polygon sets.
pub fn union(a: &Polygons, b: &Polygons) -> Polygons {
    boolean(
        &a.rings(),
        FillRule::NonZero,
        &b.rings(),
        FillRule::NonZero,
        Op::Union,
    )
}

/// What `a` and `b` share.
pub fn intersection(a: &Polygons, b: &Polygons) -> Polygons {
    boolean(
        &a.rings(),
        FillRule::NonZero,
        &b.rings(),
        FillRule::NonZero,
        Op::Intersection,
    )
}

/// `a` less `b`.
pub fn difference(a: &Polygons, b: &Polygons) -> Polygons {
    boolean(
        &a.rings(),
        FillRule::NonZero,
        &b.rings(),
        FillRule::NonZero,
        Op::Difference,
    )
}

/// What lies in exactly one of `a` and `b`.
pub fn xor(a: &Polygons, b: &Polygons) -> Polygons {
    boolean(
        &a.rings(),
        FillRule::NonZero,
        &b.rings(),
        FillRule::NonZero,
        Op::Xor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64) -> Point {
        Point::new(x, y)
    }

    fn sq(x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<Point> {
        vec![p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]
    }

    #[test]
    fn areas_and_windings() {
        let r = sq(0, 0, 10, 10);
        assert_eq!(area2(&r), 200);
        let mut cw = r.clone();
        cw.reverse();
        assert_eq!(area2(&cw), -200);
        assert_eq!(winding(&r, p(5, 5)), 1);
        assert_eq!(winding(&cw, p(5, 5)), -1);
        assert_eq!(winding(&r, p(15, 5)), 0);
        let poly = Polygon {
            outer: r,
            holes: vec![vec![p(2, 2), p(2, 4), p(4, 4), p(4, 2)]],
        };
        assert_eq!(poly.area2(), 200 - 8);
        assert_eq!(poly.bounds(), Rect::new(p(0, 0), p(10, 10)));
    }

    #[test]
    fn faults_find_what_is_wrong() {
        let good = Polygons {
            polygons: vec![
                Polygon {
                    outer: sq(0, 0, 10, 10),
                    holes: vec![],
                },
                Polygon {
                    outer: sq(10, 10, 20, 20),
                    holes: vec![],
                },
            ],
            bound: 0,
        };
        assert!(faults(&good).is_empty(), "{:?}", faults(&good));
        let mut bad = good.clone();
        bad.polygons.swap(0, 1);
        assert_eq!(faults(&bad), vec!["polygons out of order".to_string()]);
        let mut bad = good.clone();
        bad.polygons[1].outer = sq(5, 5, 20, 20);
        assert_eq!(faults(&bad).len(), 2, "{:?}", faults(&bad));
        let mut bad = good;
        bad.polygons[0].outer.reverse();
        assert!(
            faults(&bad)
                .iter()
                .any(|f| f.contains("not counterclockwise"))
        );
        assert!(faults(&bad).iter().any(|f| f.contains("least vertex")));
    }

    #[test]
    fn contains_is_uncertain_near_edges() {
        let mut s = Polygons {
            polygons: vec![Polygon {
                outer: sq(0, 0, 100, 100),
                holes: vec![],
            }],
            bound: 1,
        };
        assert_eq!(s.contains(p(50, 50)), Verdict::Yes);
        assert_eq!(s.contains(p(50, 101)), Verdict::Uncertain);
        assert_eq!(s.contains(p(50, 102)), Verdict::No);
        s.bound = 10;
        assert_eq!(s.contains(p(50, 110)), Verdict::Uncertain);
        assert_eq!(s.contains(p(50, 89)), Verdict::Yes);
        assert_eq!(s.contains(p(50, 90)), Verdict::Uncertain);
    }

    fn rings(r: Vec<Vec<Point>>) -> Rings {
        Rings::new(r, 0)
    }

    fn op(a: Vec<Vec<Point>>, b: Vec<Vec<Point>>, o: Op) -> Polygons {
        boolean(
            &rings(a),
            FillRule::NonZero,
            &rings(b),
            FillRule::NonZero,
            o,
        )
    }

    #[test]
    fn overlapping_squares() {
        let (a, b) = (sq(0, 0, 10, 10), sq(5, 5, 15, 15));
        let u = op(vec![a.clone()], vec![b.clone()], Op::Union);
        assert_eq!(u.polygons.len(), 1);
        assert_eq!(u.area2(), 2 * 175);
        assert_eq!(
            u.polygons[0].outer,
            vec![
                p(0, 0),
                p(10, 0),
                p(10, 5),
                p(15, 5),
                p(15, 15),
                p(5, 15),
                p(5, 10),
                p(0, 10)
            ]
        );
        assert_eq!(u.bound, 1);
        let i = op(vec![a.clone()], vec![b.clone()], Op::Intersection);
        assert_eq!(
            i.polygons,
            vec![Polygon {
                outer: sq(5, 5, 10, 10),
                holes: vec![]
            }]
        );
        let d = op(vec![a.clone()], vec![b.clone()], Op::Difference);
        assert_eq!(d.area2(), 2 * 75);
        let x = op(vec![a], vec![b], Op::Xor);
        assert_eq!((x.polygons.len(), x.area2()), (2, 2 * 150));
    }

    #[test]
    fn a_hole_and_its_owner() {
        let d = op(
            vec![sq(0, 0, 30, 30)],
            vec![sq(10, 10, 20, 20)],
            Op::Difference,
        );
        assert_eq!(d.polygons.len(), 1);
        let poly = &d.polygons[0];
        assert_eq!(poly.outer, sq(0, 0, 30, 30));
        // Clockwise, from its least vertex.
        assert_eq!(
            poly.holes,
            vec![vec![p(10, 10), p(10, 20), p(20, 20), p(20, 10)]]
        );
        // A ring inside the hole is a polygon of its own.
        let mut a = vec![sq(0, 0, 30, 30)];
        a.push(sq(12, 12, 18, 18));
        let d = op(a, vec![sq(10, 10, 20, 20)], Op::Difference);
        assert_eq!(d.polygons.len(), 1);
        // Even-odd makes the inner ring a hole of the first operand, which
        // the exclusive or turns into an island.
        let d = boolean(
            &rings(vec![sq(0, 0, 30, 30), sq(12, 12, 18, 18)]),
            FillRule::EvenOdd,
            &rings(vec![sq(10, 10, 20, 20)]),
            FillRule::NonZero,
            Op::Xor,
        );
        assert_eq!(d.polygons.len(), 2);
        assert_eq!(d.polygons[1].outer, sq(12, 12, 18, 18));
        assert_eq!(d.polygons[0].holes.len(), 1);
    }

    #[test]
    fn corners_that_touch_stay_apart() {
        let u = op(vec![sq(0, 0, 10, 10)], vec![sq(10, 10, 20, 20)], Op::Union);
        assert_eq!(u.polygons.len(), 2);
        assert_eq!(u.polygons[0].outer, sq(0, 0, 10, 10));
        // A hole touching its outer ring at a vertex.
        let d = op(
            vec![sq(0, 0, 20, 20)],
            vec![vec![p(0, 10), p(10, 5), p(10, 15)]],
            Op::Difference,
        );
        assert_eq!(d.polygons.len(), 1);
        assert_eq!(d.area2(), 2 * 400 - 100);
    }

    #[test]
    fn many_edges_through_one_point() {
        // Eight thin triangles meeting at the origin.
        let mut r = Vec::new();
        for k in 0..8 {
            let (s, c) = ryu_math::sin_cos(k as f64 * std::f64::consts::PI / 4.0);
            let at = |t: f64| {
                let (s2, c2) = ryu_math::sin_cos(t);
                p(
                    ryu_board::geom::round_nm(1000.0 * c2),
                    ryu_board::geom::round_nm(1000.0 * s2),
                )
            };
            let a0 = ryu_math::atan2(s, c);
            r.push(vec![p(0, 0), at(a0 - 0.1), at(a0 + 0.1)]);
        }
        let u = simplify(&rings(r), FillRule::NonZero);
        assert_eq!(u.polygons.len(), 8);
        assert!(
            u.polygons
                .iter()
                .all(|q| q.outer[0] == p(0, 0) || q.outer.contains(&p(0, 0)))
        );
    }

    #[test]
    fn fill_rules() {
        // Two squares wound the same way, overlapping.
        let r = rings(vec![sq(0, 0, 10, 10), sq(5, 0, 15, 10)]);
        assert_eq!(simplify(&r, FillRule::NonZero).area2(), 2 * 150);
        assert_eq!(simplify(&r, FillRule::EvenOdd).area2(), 2 * 100);
        // A bowtie: one lobe each way.
        let bow = rings(vec![vec![p(0, 0), p(10, 10), p(10, 0), p(0, 10)]]);
        assert_eq!(simplify(&bow, FillRule::NonZero).polygons.len(), 2);
        assert_eq!(simplify(&bow, FillRule::Positive).polygons.len(), 1);
    }

    #[test]
    fn spikes_and_slivers_go() {
        // A square with a spike out and back along one line.
        let r = rings(vec![vec![
            p(0, 0),
            p(10, 0),
            p(20, 0),
            p(10, 0),
            p(10, 10),
            p(0, 10),
        ]]);
        let s = simplify(&r, FillRule::NonZero);
        assert_eq!(
            s.polygons,
            vec![Polygon {
                outer: sq(0, 0, 10, 10),
                holes: vec![]
            }]
        );
        // Two squares sharing a side merge, and the side goes.
        let u = op(vec![sq(0, 0, 10, 10)], vec![sq(10, 0, 20, 10)], Op::Union);
        assert_eq!(u.polygons[0].outer, sq(0, 0, 20, 10));
    }

    #[test]
    fn crossings_snap_to_the_grid() {
        // Two thin slanted strips crossing off the grid.
        let a = vec![p(0, 0), p(1000, 333), p(1000, 334), p(0, 1)];
        let b = vec![p(0, 333), p(1000, 0), p(1000, 1), p(0, 334)];
        let u = op(vec![a], vec![b], Op::Union);
        assert_eq!(u.polygons.len(), 1);
        for q in &u.polygons {
            for r in q.rings() {
                assert!(r.len() >= 3);
            }
        }
        assert_eq!(u.contains(p(500, 166)), Verdict::Uncertain);
        assert_eq!(u.contains(p(100, 100)), Verdict::No);
        // A strip a nanometre wide is within the bound of its edges all
        // across; 1.3 nm below it isn't.
        assert_eq!(u.contains(p(10, 4)), Verdict::Uncertain);
        assert_eq!(u.contains(p(10, 2)), Verdict::No);
    }
}
