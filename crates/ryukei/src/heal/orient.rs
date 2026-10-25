//! Orientation: loops that run the way their face says.
//!
//! **Loops.** A face's region lies to the left of its loops about its
//! normal. Where the loops alone fix the region, their directions can be
//! checked: on a surface that doesn't close up (anything but a sphere, a
//! torus and their kind) with every loop closing in the parameter plane,
//! winding round no period, the face is what its outermost loops enclose,
//! less what the next ones in enclose, and so on. Each loop is read by
//! projecting points along its edges onto the surface (never through a
//! pcurve), unwrapped from one to the next; its depth is how many of the
//! others enclose it (by winding number, decided by exact orientation
//! tests at a point inside one of its edges), and it must run
//! counterclockwise about the face's normal at even depth, clockwise at
//! odd. Where the file's loops already do, nothing is read further; where
//! one doesn't, the loops must not cross (a loop crossing itself or
//! another is [`HealError::SelfCrossingLoop`]) and every loop that runs
//! the wrong way is reversed. A loop whose area or depth rounding can't
//! settle is [`HealError::Uncertain`].

use super::names::Names;
use super::{Loop, Shape, Use};
use crate::geom::{Point2, Surface};
use crate::name::Name;
use crate::predicates::{Sign, orient2d};
use crate::topo::Sense;
use crate::{Error, HealError};
use std::collections::BTreeSet;

/// Samples along each use, its start included and its end left to the
/// next use.
const SAMPLES: usize = 16;

/// Whether a surface closes up, so that loops on it bound a region either
/// way: periodic both ways, or periodic one way with poles at both ends
/// of the other.
fn closes_up(surface: &Surface) -> bool {
    let (pu, pv) = surface.periods();
    let rows = crate::mesh::uv::collapsed_rows(surface);
    match (pu, pv) {
        (Some(_), Some(_)) => true,
        (Some(_), None) => rows[1] == [true, true],
        (None, Some(_)) => rows[0] == [true, true],
        (None, None) => false,
    }
}

fn unwrap(x: f64, near: f64, period: Option<f64>) -> f64 {
    match period {
        Some(p) => x - ((x - near) / p).round() * p,
        None => x,
    }
}

/// A loop as a closed polygon in the face's parameter plane, unwrapped
/// sample by sample, and how many periods it winds each way.
struct Polygon {
    points: Vec<Point2>,
    /// The index of a sample inside each use, for tests at a point off
    /// the loop's vertices.
    inner: Vec<usize>,
    winds: [i64; 2],
}

fn polygon(s: &Shape, surface: &Surface, uses: &[Use]) -> Polygon {
    let (pu, pv) = surface.periods();
    let mut points: Vec<Point2> = Vec::new();
    let mut inner = Vec::new();
    for u in uses {
        let e = &s.edges[u.edge];
        let curve = &s.curves[e.curve];
        let [t0, t1] = e.interval;
        for i in 0..SAMPLES {
            let f = i as f64 / SAMPLES as f64;
            let t = match u.sense {
                Sense::Forward => t0 + (t1 - t0) * f,
                Sense::Reversed => t1 - (t1 - t0) * f,
            };
            if i == SAMPLES / 2 {
                inner.push(points.len());
            }
            let (x, y) = surface.project(curve.point(t)).param;
            let p = match points.last() {
                Some(q) => Point2::new(unwrap(x, q.x, pu), unwrap(y, q.y, pv)),
                None => Point2::new(x, y),
            };
            points.push(p);
        }
    }
    // Back at the start: how far round it came.
    let (first, last) = (points[0], *points.last().expect("samples"));
    let (x, y) = (unwrap(first.x, last.x, pu), unwrap(first.y, last.y, pv));
    let winds = [
        pu.map_or(0, |p| ((x - first.x) / p).round() as i64),
        pv.map_or(0, |p| ((y - first.y) / p).round() as i64),
    ];
    Polygon {
        points,
        inner,
        winds,
    }
}

/// Twice the polygon's signed area, about its first point; its sign, or
/// `None` where it is too small to trust against its extent.
fn area_sign(p: &Polygon) -> Option<bool> {
    let o = p.points[0];
    let n = p.points.len();
    let mut a = 0.0;
    let (mut lo, mut hi) = (o, o);
    for i in 0..n {
        let (x, y) = (p.points[i] - o, p.points[(i + 1) % n] - o);
        a += x.x * y.y - x.y * y.x;
        lo = Point2::new(lo.x.min(p.points[i].x), lo.y.min(p.points[i].y));
        hi = Point2::new(hi.x.max(p.points[i].x), hi.y.max(p.points[i].y));
    }
    let extent = (hi.x - lo.x) * (hi.y - lo.y);
    (a.abs() > 1e-9 * extent && extent > 0.0).then_some(a > 0.0)
}

/// The winding number of polygon `p` about `q`, by exact orientation
/// tests; `None` when `q` lies on it.
fn winding(p: &Polygon, q: Point2) -> Option<i64> {
    let n = p.points.len();
    let mut w = 0;
    for i in 0..n {
        let (a, b) = (p.points[i], p.points[(i + 1) % n]);
        let side = orient2d(a, b, q);
        if a.y <= q.y && b.y > q.y && side == Sign::Positive {
            w += 1;
        } else if b.y <= q.y && a.y > q.y && side == Sign::Negative {
            w -= 1;
        } else if side == Sign::Zero
            && q.x >= a.x.min(b.x)
            && q.x <= a.x.max(b.x)
            && q.y >= a.y.min(b.y)
            && q.y <= a.y.max(b.y)
        {
            return None;
        }
    }
    Some(w)
}

/// Whether `q`, moved by any whole number of periods, lies inside `p`.
fn encloses(p: &Polygon, q: Point2, periods: (Option<f64>, Option<f64>)) -> Option<bool> {
    let shifts = |x: Option<f64>| match x {
        Some(per) => vec![-2.0 * per, -per, 0.0, per, 2.0 * per],
        None => vec![0.0],
    };
    for du in shifts(periods.0) {
        for dv in shifts(periods.1) {
            if winding(p, Point2::new(q.x + du, q.y + dv))? != 0 {
                return Some(true);
            }
        }
    }
    Some(false)
}

/// Whether segments `ab` and `cd` cross properly.
fn cross(a: Point2, b: Point2, c: Point2, d: Point2) -> bool {
    let opposite = |x: Sign, y: Sign| {
        (x == Sign::Positive && y == Sign::Negative) || (x == Sign::Negative && y == Sign::Positive)
    };
    opposite(orient2d(a, b, c), orient2d(a, b, d)) && opposite(orient2d(c, d, a), orient2d(c, d, b))
}

/// Whether any two segments of the polygons cross, other than
/// neighbours along one polygon; on a periodic surface, against the
/// others moved by a period either way.
fn crossing(polygons: &[Polygon], periods: (Option<f64>, Option<f64>)) -> bool {
    // Every segment: its polygon, its index and its ends.
    let mut segments: Vec<(usize, usize, Point2, Point2)> = Vec::new();
    for (k, p) in polygons.iter().enumerate() {
        let n = p.points.len();
        for i in 0..n {
            segments.push((k, i, p.points[i], p.points[(i + 1) % n]));
        }
    }
    let shifts = |x: Option<f64>| match x {
        Some(per) => vec![-per, 0.0, per],
        None => vec![0.0],
    };
    let mut moved: Vec<(usize, usize, Point2, Point2)> = Vec::new();
    for du in shifts(periods.0) {
        for dv in shifts(periods.1) {
            for &(k, i, a, b) in &segments {
                let d = crate::geom::Vector2::new(du, dv);
                moved.push((k, i, a + d, b + d));
            }
        }
    }
    let lo = |s: &(usize, usize, Point2, Point2)| s.2.x.min(s.3.x);
    let hi = |s: &(usize, usize, Point2, Point2)| s.2.x.max(s.3.x);
    segments.sort_by(|x, y| lo(x).total_cmp(&lo(y)));
    moved.sort_by(|x, y| lo(x).total_cmp(&lo(y)));
    let mut start = 0;
    for s in &segments {
        while start < moved.len() && hi(&moved[start]) < lo(s) {
            start += 1;
        }
        for m in &moved[start..] {
            if lo(m) > hi(s) {
                break;
            }
            if s.0 == m.0 {
                let n = polygons[s.0].points.len();
                if (s.1 + 1) % n == m.1 || (m.1 + 1) % n == s.1 || s.1 == m.1 {
                    continue;
                }
            }
            if cross(s.2, s.3, m.2, m.3) {
                return true;
            }
        }
    }
    false
}

/// Reverses a loop: its uses in the other order, each the other way.
fn reverse(uses: &mut [Use]) {
    uses.reverse();
    for u in uses {
        u.sense = u.sense.flipped();
    }
}

/// Makes the loops of every face whose loops fix its region run the way
/// they must. Returns the faces changed.
pub(super) fn loops(s: &mut Shape, names: &Names) -> Result<Vec<Name>, Error> {
    let mut changed = Vec::new();
    for k in 0..s.faces.len() {
        let surface = s.surfaces[s.faces[k].surface].clone();
        if closes_up(&surface) {
            continue;
        }
        let face = &s.faces[k];
        let lists: Vec<&Vec<Use>> = face
            .loops
            .iter()
            .filter_map(|l| match l {
                Loop::Edges(u) => Some(u),
                Loop::Vertex(_) => None,
            })
            .collect();
        if lists.is_empty() {
            continue;
        }
        let polygons: Vec<Polygon> = lists.iter().map(|u| polygon(s, &surface, u)).collect();
        if polygons.iter().any(|p| p.winds != [0, 0]) {
            continue;
        }
        let periods = surface.periods();
        let uncertain = || -> Error { HealError::Uncertain { at: names.faces[k] }.into() };
        let mut wrong: BTreeSet<usize> = BTreeSet::new();
        for (i, p) in polygons.iter().enumerate() {
            let mut depth = 0;
            for (j, q) in polygons.iter().enumerate() {
                if i == j {
                    continue;
                }
                // At a point inside one of its edges, off the others.
                let inside = p
                    .inner
                    .iter()
                    .find_map(|&x| encloses(q, p.points[x], periods))
                    .ok_or_else(uncertain)?;
                depth += usize::from(inside);
            }
            let ccw = area_sign(p).ok_or_else(uncertain)?;
            let want = (depth % 2 == 0) == (face.sense == Sense::Forward);
            if ccw != want {
                wrong.insert(i);
            }
        }
        if wrong.is_empty() {
            continue;
        }
        if crossing(&polygons, periods) {
            return Err(HealError::SelfCrossingLoop {
                face: names.faces[k],
            }
            .into());
        }
        let mut i = 0;
        for l in &mut s.faces[k].loops {
            if let Loop::Edges(uses) = l {
                if wrong.contains(&i) {
                    reverse(uses);
                }
                i += 1;
            }
        }
        changed.push(names.faces[k]);
    }
    changed.sort();
    Ok(changed)
}
