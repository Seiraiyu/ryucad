//! Orientation: loops that run the way their face says, faces that agree
//! with their neighbours.
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
//!
//! **Faces.** Every edge must then be used once each way. Faces are
//! turned over (sense and every loop reversed together, so the region
//! stays) to make it so: from the least-named face of each connected
//! piece, kept as the file has it, across each edge to the next. A piece
//! that can't be made consistent (a Möbius band) is
//! [`HealError::Misoriented`] at the first edge found against it. A face
//! on a surface that closes up (a torus), turned against most of its
//! piece, bounds a region either way: there the file's sense is kept and
//! its loops alone reversed, since a writer that ran them against its
//! neighbours' (KiCad's tori, issue #364) had the sense right. Which way
//! each shell then faces is decided once it is built (see `shells`).

use super::names::Names;
use super::{Face, Loop, Shape, Use};
use crate::geom::{Curve, Point2, Point3, Surface, Vector2, Vector3};
use crate::name::Name;
use crate::predicates::{Sign, orient2d};
use crate::topo::Sense;
use crate::{Error, HealError};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Samples along each use, its start included and its end left to the
/// next use.
const SAMPLES: usize = 16;

/// The most samples a use takes when its steps are halved (see
/// [`polygon`]).
const MAX_SAMPLES: usize = 1 << 14;
/// The densest samples a use takes when its face's loops seem to cross.
const FINEST_SAMPLES: usize = 1024;

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

fn polygon(s: &Shape, surface: &Surface, uses: &[Use], samples: usize) -> Polygon {
    let (pu, pv) = surface.periods();
    let mut points: Vec<Point2> = Vec::new();
    let mut inner = Vec::new();
    for u in uses {
        let e = &s.edges[u.edge];
        let curve = &s.curves[e.curve];
        let [t0, t1] = e.interval;
        let at = |f: f64| match u.sense {
            Sense::Forward => t0 + (t1 - t0) * f,
            Sense::Reversed => t1 - (t1 - t0) * f,
        };
        // Each sample is unwrapped next to the one before, so a step that
        // turns a periodic parameter by a quarter period or more (by its
        // ends, or by its parameter velocity at either: a helix of many
        // turns, issue #473) is halved first, the use's end (the next
        // use's start) included. A step of a turn and a bit, unwrapped as
        // a small one backwards, sent the loop round the wrong way, and
        // healing reversed a loop that was right. Every other loop keeps
        // its 16 samples.
        let mut last: Option<(f64, Step)> = None;
        let mut taken = 0;
        let mut ahead: Vec<f64> = (0..=samples)
            .rev()
            .map(|i| i as f64 / samples as f64)
            .collect();
        while let Some(f) = ahead.pop() {
            let t = at(f);
            let (x, y) = surface.project(curve.point(t)).param;
            let p = match points.last() {
                Some(q) => Point2::new(unwrap(x, q.x, pu), unwrap(y, q.y, pv)),
                None => Point2::new(x, y),
            };
            let here = (t, p, rate(surface, curve, p, t));
            if let Some((f0, there)) = last
                && taken < MAX_SAMPLES
                && f - f0 > 1e-9
                && !small(pu, pv, there, here)
            {
                ahead.push(f);
                ahead.push(0.5 * (f0 + f));
                continue;
            }
            if f == 1.0 {
                break;
            }
            if f == 0.5 {
                inner.push(points.len());
            }
            points.push(p);
            last = Some((f, here));
            taken += 1;
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

/// The parameter velocity on `surface` at `uv` of `curve` at `t`: the
/// least-squares solution of `S_u du + S_v dv = C'`. `None` where the
/// partials are dependent (a pole, an apex).
fn rate(surface: &Surface, curve: &Curve, uv: Point2, t: f64) -> Option<Vector2> {
    let e = surface.eval(uv.x, uv.y);
    let d = curve.eval(t).d1;
    let (a, b, c) = (e.du.dot(e.du), e.du.dot(e.dv), e.dv.dot(e.dv));
    let det = a * c - b * b;
    let scale = a.max(c).max(1e-300);
    if det.is_nan() || det <= 1e-24 * scale * scale {
        return None;
    }
    let (x, y) = (e.du.dot(d), e.dv.dot(d));
    Some(Vector2::new((c * x - b * y) / det, (a * y - b * x) / det))
}

/// A sample of a use: its curve parameter, its unwrapped surface
/// parameters and their velocity.
type Step = (f64, Point2, Option<Vector2>);

/// Whether a step from `a` to `b` turns each periodic parameter by less
/// than a quarter period, by its ends and by either velocity.
fn small(pu: Option<f64>, pv: Option<f64>, a: Step, b: Step) -> bool {
    let h = (b.0 - a.0).abs();
    let under = |p: f64, d: f64, da: Option<f64>, db: Option<f64>| {
        d.abs() < p / 4.0
            && da.is_none_or(|x| (x * h).abs() < p / 4.0)
            && db.is_none_or(|x| (x * h).abs() < p / 4.0)
    };
    pu.is_none_or(|p| under(p, b.1.x - a.1.x, a.2.map(|r| r.x), b.2.map(|r| r.x)))
        && pv.is_none_or(|p| under(p, b.1.y - a.1.y, a.2.map(|r| r.y), b.2.map(|r| r.y)))
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

/// Whether a face's two loops, each once round the one period of a
/// surface periodic only that way, both run against the face's sense: the
/// region between them must lie to the left of each about the normal, so
/// the lower loop (by the other parameter) runs up the period on a
/// forward face periodic in `u`, down it on one periodic in `v`, and the
/// upper the other way. Loops that overlap in the other parameter, or
/// that disagree with each other, are left as the file has them.
fn band_runs_against(surface: &Surface, polygons: &[Polygon], sense: Sense) -> bool {
    let d = match surface.periods() {
        (Some(_), None) => 0,
        (None, Some(_)) => 1,
        _ => return false,
    };
    let [a, b] = polygons else {
        return false;
    };
    let o = 1 - d;
    let winds = |p: &Polygon| (p.winds[o] == 0 && p.winds[d].abs() == 1).then_some(p.winds[d]);
    let (Some(wa), Some(wb)) = (winds(a), winds(b)) else {
        return false;
    };
    let range = |p: &Polygon| {
        p.points
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                let x = if o == 0 { q.x } else { q.y };
                (lo.min(x), hi.max(x))
            })
    };
    let (ra, rb) = (range(a), range(b));
    let lower = if ra.1 < rb.0 {
        wa
    } else if rb.1 < ra.0 {
        wb
    } else {
        return false;
    };
    if wa != -wb {
        return false;
    }
    let up = if d == 0 { 1 } else { -1 };
    let want = if sense == Sense::Forward { up } else { -up };
    lower == -want
}

/// The loops (by index) running against the region the face's sense and
/// their nesting say they bound; `None` where a nesting or an area can't
/// be signed.
fn wrong_loops(polygons: &[Polygon], surface: &Surface, sense: Sense) -> Option<BTreeSet<usize>> {
    let periods = surface.periods();
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
                .find_map(|&x| encloses(q, p.points[x], periods))?;
            depth += usize::from(inside);
        }
        let ccw = area_sign(p)?;
        let want = (depth % 2 == 0) == (sense == Sense::Forward);
        if ccw != want {
            wrong.insert(i);
        }
    }
    Some(wrong)
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
        let polygons: Vec<Polygon> = lists
            .iter()
            .map(|u| polygon(s, &surface, u, SAMPLES))
            .collect();
        if polygons.iter().any(|p| p.winds != [0, 0]) {
            // A band between two loops round the one period of a surface
            // that doesn't close up: they fix its region too, and a file
            // whose band runs against its sense (KiCad's horn tori, read as
            // a revolution of an open arc: issue #364) is turned back.
            if band_runs_against(&surface, &polygons, face.sense) {
                for l in &mut s.faces[k].loops {
                    if let Loop::Edges(uses) = l {
                        reverse(uses);
                    }
                }
                changed.push(names.faces[k]);
            }
            continue;
        }
        // A lone loop of no area to speak of (two edges along one line, a
        // sliver a file wrote as a face) fixes no region: it stays as the
        // file has it, and collapsing takes the face out (issue #449).
        if let [p] = &polygons[..]
            && area_sign(p).is_none()
        {
            continue;
        }
        let uncertain = || -> Error { HealError::Uncertain { at: names.faces[k] }.into() };
        // Loops whose samples cross may only lie closer together than the
        // samples' sagitta (a ring 39 µm wide round 2.8 mm circles): they
        // are judged again on samples four times as dense, up to 1,024 a
        // use, before they are taken to cross. Faces whose loops don't
        // cross at 16 samples are judged as they always were.
        let mut samples = SAMPLES;
        let mut polygons = polygons;
        let wrong = loop {
            let wrong = wrong_loops(&polygons, &surface, face.sense).ok_or_else(uncertain)?;
            if wrong.is_empty() {
                break wrong;
            }
            if !crossing(&polygons, surface.periods()) {
                break wrong;
            }
            if samples >= FINEST_SAMPLES {
                return Err(HealError::SelfCrossingLoop {
                    face: names.faces[k],
                }
                .into());
            }
            samples *= 4;
            polygons = lists
                .iter()
                .map(|u| polygon(s, &surface, u, samples))
                .collect();
        };
        if wrong.is_empty() {
            continue;
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

/// Whether face `k`'s region, as its sense and loops have it, folds back
/// over a neighbour's at each edge where their surfaces are tangent (at
/// least one): at the edge's middle, the directions into each face from
/// the edge (its normal across the edge's direction as it runs) point the
/// same way. Faces that continue each other smoothly point opposite ways
/// whatever their senses, so this reads only the regions.
fn folds(s: &Shape, k: usize, uses: &[Vec<(usize, Sense)>]) -> bool {
    let into = |f: usize, e: usize, sense: Sense, p: Point3| -> Option<(Vector3, Vector3)> {
        let face = &s.faces[f];
        let surface = &s.surfaces[face.surface];
        let (u, v) = surface.project(p).param;
        let n = surface.normal(u, v)?.get();
        let n = if face.sense == Sense::Forward { n } else { -n };
        let edge = &s.edges[e];
        let t = (edge.interval[0] + edge.interval[1]) / 2.0;
        let d = s.curves[edge.curve].tangent(t)?.get();
        let d = if sense == Sense::Forward { d } else { -d };
        Some((n, n.cross(d)))
    };
    let mut smooth = 0;
    for (e, list) in uses.iter().enumerate() {
        let [(a, sa), (b, sb)] = list[..] else {
            continue;
        };
        if (a == k) == (b == k) {
            continue;
        }
        let ((mine, sm), (other, so)) = if a == k {
            ((a, sa), (b, sb))
        } else {
            ((b, sb), (a, sa))
        };
        let edge = &s.edges[e];
        let p = s.curves[edge.curve].point((edge.interval[0] + edge.interval[1]) / 2.0);
        let (Some((n1, t1)), Some((n2, t2))) = (into(mine, e, sm, p), into(other, e, so, p)) else {
            continue;
        };
        if n1.cross(n2).length() > 1e-3 {
            continue;
        }
        smooth += 1;
        if t1.dot(t2) <= 0.0 {
            return false;
        }
    }
    smooth > 0
}

/// Turns faces over until every edge is used once each way. Returns the
/// faces turned and those whose loops alone were reversed.
pub(super) fn faces(s: &mut Shape, names: &Names) -> Result<(Vec<Name>, Vec<Name>), Error> {
    // Each edge's uses: face and sense.
    let mut uses: Vec<Vec<(usize, Sense)>> = vec![Vec::new(); s.edges.len()];
    let mut edges_of: Vec<Vec<usize>> = vec![Vec::new(); s.faces.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            if let Loop::Edges(list) = l {
                for u in list {
                    uses[u.edge].push((k, u.sense));
                    edges_of[k].push(u.edge);
                }
            }
        }
    }
    // Faces from the least-named, each piece from its least-named face.
    let mut order: Vec<usize> = (0..s.faces.len()).collect();
    order.sort_by_key(|&k| names.faces[k]);
    let mut turn: Vec<Option<bool>> = vec![None; s.faces.len()];
    let mut piece: Vec<usize> = vec![0; s.faces.len()];
    let mut against: Option<Name> = None;
    for &root in &order {
        if turn[root].is_some() {
            continue;
        }
        turn[root] = Some(false);
        piece[root] = root;
        let mut queue = VecDeque::from([root]);
        while let Some(k) = queue.pop_front() {
            let mut list = edges_of[k].clone();
            list.sort_by_key(|&e| names.edges[e]);
            for e in list {
                let [(a, sa), (b, sb)] = uses[e][..] else {
                    continue;
                };
                let (other, mine, theirs) = if a == k { (b, sa, sb) } else { (a, sb, sa) };
                // The other face is turned when its use runs the same way
                // as this face's, once this face's turn is applied.
                let want = turn[k].expect("visited") ^ (mine == theirs);
                match turn[other] {
                    None => {
                        turn[other] = Some(want);
                        piece[other] = root;
                        queue.push_back(other);
                    }
                    Some(t) if t != want => {
                        let n = names.edges[e];
                        if against.is_none_or(|x| n < x) {
                            against = Some(n);
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }
    if let Some(at) = against {
        return Err(HealError::Misoriented { at }.into());
    }
    // How most of each piece's faces are turned.
    let mut count: BTreeMap<usize, [usize; 2]> = BTreeMap::new();
    for (k, t) in turn.iter().enumerate() {
        count.entry(piece[k]).or_default()[usize::from(*t == Some(true))] += 1;
    }
    let most = |k: usize| {
        let [kept, turned] = count[&piece[k]];
        (kept != turned).then_some(turned > kept)
    };
    // A face on a surface that closes up, turned against most of its
    // piece: its loops bound a region either way. Where the region they
    // bound folds back over its neighbours at every smooth edge, the file
    // wrote its loops against theirs and its sense right (KiCad's tori,
    // issue #364): turning it over would keep that region (three quarters
    // of a fillet's tube), so its loops alone are reversed, or its sense
    // alone turned with the rest. Judged on the faces as the file has
    // them, before any is changed.
    let loops_only: Vec<bool> = (0..s.faces.len())
        .map(|k| {
            let t = turn[k] == Some(true);
            most(k).is_some_and(|m| m != t)
                && closes_up(&s.surfaces[s.faces[k].surface])
                && folds(s, k, &uses)
        })
        .collect();
    let (mut turned, mut reversed) = (Vec::new(), Vec::new());
    for (k, t) in turn.iter().enumerate() {
        let t = *t == Some(true);
        match (loops_only[k], t) {
            (true, true) => {
                for l in &mut s.faces[k].loops {
                    if let Loop::Edges(list) = l {
                        reverse(list);
                    }
                }
                reversed.push(names.faces[k]);
            }
            (true, false) => {
                s.faces[k].sense = s.faces[k].sense.flipped();
                turned.push(names.faces[k]);
            }
            (false, true) => {
                self::turn(&mut s.faces[k]);
                turned.push(names.faces[k]);
            }
            (false, false) => {}
        }
    }
    turned.sort();
    reversed.sort();
    Ok((turned, reversed))
}

/// Turns a face over: its sense and every loop reversed together, so its
/// region stays.
pub(super) fn turn(f: &mut Face) {
    f.sense = f.sense.flipped();
    for l in &mut f.loops {
        if let Loop::Edges(list) = l {
            reverse(list);
        }
    }
}
