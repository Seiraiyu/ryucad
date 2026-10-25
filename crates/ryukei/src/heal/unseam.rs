//! Seams and poles: what files add to periodic faces, taken away.
//!
//! A seam is an edge both of whose uses are on one face, along a
//! parameter line of a direction the face's surface is periodic in: five
//! points along it within the cap of the line through the first. It is
//! removed, so how far off the line it was matters no further: where both uses are in one loop the loop splits in two
//! (a cylinder's side into its two circles), where they are in two
//! loops those join. A piece left empty is a point: kept as a lone
//! vertex at an apex (a cone's tip, where the normal turns round the
//! point), else dropped with the vertex (a sphere's pole, the corner of
//! a torus's two seams); a vertex loop a file put on a face goes the
//! same way. A sphere or torus left with no loops is
//! the whole surface.
//!
//! A writer may put a vertex on a loop where the seam meets it, splitting
//! an edge there, and mark it as the seam's ([`super::Vertex::seam`]).
//! Such a vertex, left between two edges on one curve running on from
//! each other, goes and the two edges are joined: into a closed edge of
//! one period where they meet again at their other ends. The joined
//! edge is named from the names of every edge of the file it joins,
//! sorted, so an edge several seams split is named alike whichever join
//! comes first. A file's own vertices stay, wherever they are.

use super::names::Names;
use super::{Edge, Loop, Shape, Use};
use crate::name::{Event, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::Sense;
use crate::{Error, HealError};
use std::collections::BTreeSet;

/// What unseaming did, by import name.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Unseamed {
    /// Seam edges removed.
    pub(super) seams: Vec<Name>,
    /// Pole vertices dropped.
    pub(super) poles: Vec<Name>,
    /// Vertices removed between two edges joined into one.
    pub(super) joined: Vec<Name>,
    pub(super) journal: Journal,
}

/// The vertex a use starts from and the one it ends at.
fn ends(s: &Shape, u: &Use) -> (usize, usize) {
    let e = &s.edges[u.edge];
    match u.sense {
        Sense::Forward => (e.start, e.end),
        Sense::Reversed => (e.end, e.start),
    }
}

/// Removes seams and poles from `s` in place, and joins what seams split.
/// New edges' names are pushed onto `names.edges`.
pub(super) fn unseam(
    key: OpKey,
    s: &mut Shape,
    names: &mut Names,
    cap: f64,
) -> Result<Unseamed, Error> {
    let mut out = Unseamed::default();
    // Vertices a removed seam ran to, and points left by empty pieces.
    let mut seam_ends: BTreeSet<usize> = BTreeSet::new();
    let mut empty: Vec<(usize, usize)> = Vec::new();
    for k in 0..s.faces.len() {
        while let Some(e) = seam_in(s, k) {
            check_seam(s, k, e, cap).map_err(|()| HealError::BadSeam {
                face: names.faces[k],
            })?;
            let edge = s.edges[e];
            seam_ends.insert(edge.start);
            seam_ends.insert(edge.end);
            out.seams.push(names.edges[e]);
            out.journal.push(Event::Deleted(names.edges[e]));
            remove(s, k, e, &mut empty);
        }
        // Vertex loops the file put at a pole of the face's surface.
        let mut l = 0;
        while l < s.faces[k].loops.len() {
            match s.faces[k].loops[l] {
                Loop::Vertex(v)
                    if kind(&s.surfaces[s.faces[k].surface], s.vertices[v].point, cap)
                        == Point::Pole =>
                {
                    s.faces[k].loops.remove(l);
                    empty.push((k, v));
                }
                _ => l += 1,
            }
        }
    }
    // Every point a seam left alone: an apex keeps its vertex, a pole or
    // anywhere else loses it.
    let used = used_vertices(s);
    let mut kept: BTreeSet<usize> = BTreeSet::new();
    for (k, v) in empty {
        if used.contains(&v) || kept.contains(&v) {
            continue;
        }
        let surface = &s.surfaces[s.faces[k].surface];
        // A vertex a writer added to end a seam marks nothing.
        if !s.vertices[v].seam && kind(surface, s.vertices[v].point, cap) == Point::Apex {
            kept.insert(v);
            s.faces[k].loops.push(Loop::Vertex(v));
        } else if !out.poles.contains(&names.vertices[v]) {
            out.poles.push(names.vertices[v]);
            out.journal.push(Event::Deleted(names.vertices[v]));
        }
    }
    // Vertices in the order of their names; each edge carries the names
    // of the file's edges it joins, so an edge split by several seams is
    // named the same whichever is joined first.
    let mut parts: Vec<Vec<Name>> = names.edges.iter().map(|&n| vec![n]).collect();
    let mut ends: Vec<usize> = seam_ends
        .into_iter()
        .filter(|&v| s.vertices[v].seam)
        .collect();
    ends.sort_by_key(|&v| names.vertices[v]);
    for v in ends {
        join(key, s, names, &mut parts, v, &mut out);
    }
    out.seams.sort();
    out.poles.sort();
    out.joined.sort();
    Ok(out)
}

/// What a point of a surface is to a lone vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    /// Where a row of the parameters collapses (see
    /// [`crate::mesh::uv::collapsed_rows`]) and the normal turns as the
    /// free parameter does: a cone's tip.
    Apex,
    /// Where a row collapses and the normal passes smoothly: a sphere's
    /// pole.
    Pole,
    /// Anywhere else.
    Regular,
}

/// What `p` is, as a point within `reach` of a collapsed row counts as
/// on it.
fn kind(surface: &crate::geom::Surface, p: crate::geom::Point3, reach: f64) -> Point {
    let domain = surface.domain();
    let collapsed = crate::mesh::uv::collapsed_rows(surface);
    for d in 0..2 {
        for end in 0..2 {
            let edge = domain[d][end];
            let free = domain[1 - d];
            let mid = (free[0] + free[1]) / 2.0;
            let row = if d == 0 {
                surface.point(edge, mid)
            } else {
                surface.point(mid, edge)
            };
            if !collapsed[d][end] || row.distance(p) > reach {
                continue;
            }
            // The normal at four places round the free parameter.
            let normals: Vec<Option<crate::geom::UnitVector3>> = (0..4)
                .map(|i| {
                    let f = free[0] + (free[1] - free[0]) * (i as f64 + 0.5) / 4.0;
                    let q = if d == 0 { (edge, f) } else { (f, edge) };
                    surface.normal(q.0, q.1)
                })
                .collect();
            let turns = match normals[0] {
                None => true,
                Some(n0) => normals
                    .iter()
                    .any(|n| n.is_none_or(|n| n.dot(n0) < 1.0 - 1e-12)),
            };
            return if turns { Point::Apex } else { Point::Pole };
        }
    }
    Point::Regular
}

/// The first edge face `k` uses twice, if any.
fn seam_in(s: &Shape, k: usize) -> Option<usize> {
    let mut seen = BTreeSet::new();
    for l in &s.faces[k].loops {
        if let Loop::Edges(uses) = l {
            for u in uses {
                if !seen.insert(u.edge) {
                    return Some(u.edge);
                }
            }
        }
    }
    None
}

/// Whether edge `e` lies along a parameter line of a direction face `k`'s
/// surface is periodic in: its points inside, projected, share that
/// parameter.
fn check_seam(s: &Shape, k: usize, e: usize, cap: f64) -> Result<(), ()> {
    let surface = &s.surfaces[s.faces[k].surface];
    let edge = &s.edges[e];
    let curve = &s.curves[edge.curve];
    let (pu, pv) = surface.periods();
    let [t0, t1] = edge.interval;
    let points: Vec<_> = (1..6)
        .map(|i| curve.point(t0 + (t1 - t0) * i as f64 / 6.0))
        .collect();
    let params: Vec<[f64; 2]> = points
        .iter()
        .map(|&q| {
            let (u, v) = surface.project(q).param;
            [u, v]
        })
        .collect();
    // Each point within the cap of the line through the first, in
    // direction `d` held fixed.
    let along = |d: usize| {
        params.iter().zip(&points).all(|(x, &q)| {
            let mut on = *x;
            on[d] = params[0][d];
            surface.point(on[0], on[1]).distance(q) <= cap
        })
    };
    let ok = pu.is_some() && along(0) || pv.is_some() && along(1);
    if ok { Ok(()) } else { Err(()) }
}

/// Removes both uses of edge `e` from face `k`'s loops, recording the
/// vertex of each piece left empty.
fn remove(s: &mut Shape, k: usize, e: usize, empty: &mut Vec<(usize, usize)>) {
    let at: Vec<(usize, usize)> = s.faces[k]
        .loops
        .iter()
        .enumerate()
        .flat_map(|(l, lp)| match lp {
            Loop::Edges(uses) => uses
                .iter()
                .enumerate()
                .filter(|(_, u)| u.edge == e)
                .map(|(i, _)| (l, i))
                .collect::<Vec<_>>(),
            Loop::Vertex(_) => Vec::new(),
        })
        .collect();
    let uses_of = |s: &Shape, l: usize| match &s.faces[k].loops[l] {
        Loop::Edges(u) => u.clone(),
        Loop::Vertex(_) => unreachable!("a loop of edges"),
    };
    let ((la, a), (lb, b)) = (at[0], at[1]);
    let mut pieces: Vec<Vec<Use>> = Vec::new();
    // The uses strictly between positions `from` and `to`, cyclically.
    let between = |list: &[Use], from: usize, to: usize| -> Vec<Use> {
        let n = list.len();
        let mut out = Vec::new();
        let mut i = (from + 1) % n;
        while i != to {
            out.push(list[i]);
            i = (i + 1) % n;
        }
        out
    };
    if la == lb {
        let list = uses_of(s, la);
        // After a up to b, and after b up to a: each empty one is a point
        // where the two uses meet.
        for (from, to) in [(a, b), (b, a)] {
            let piece = between(&list, from, to);
            if piece.is_empty() {
                empty.push((k, ends(s, &list[from]).1));
            } else {
                pieces.push(piece);
            }
        }
        s.faces[k].loops.remove(la);
    } else {
        let (x, y) = (uses_of(s, la), uses_of(s, lb));
        let mut joined = between(&x, a, a);
        joined.extend(between(&y, b, b));
        if joined.is_empty() {
            empty.push((k, ends(s, &x[a]).1));
        } else {
            pieces.push(joined);
        }
        let (hi, lo) = (la.max(lb), la.min(lb));
        s.faces[k].loops.remove(hi);
        s.faces[k].loops.remove(lo);
    }
    for p in pieces {
        s.faces[k].loops.push(Loop::Edges(p));
    }
}

/// Every vertex an edge still in use ends at.
fn used_vertices(s: &Shape) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for f in &s.faces {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    let e = &s.edges[u.edge];
                    out.insert(e.start);
                    out.insert(e.end);
                }
            }
        }
    }
    out
}

/// Two curves the same: one entry, or equal values.
fn same_curve(s: &Shape, a: usize, b: usize) -> bool {
    a == b || s.curves[a] == s.curves[b]
}

/// Joins the two edges at vertex `v` into one, if they are all there is
/// at it, on one curve and run on from each other, and every face runs
/// them one after the other.
fn join(
    key: OpKey,
    s: &mut Shape,
    names: &mut Names,
    parts: &mut Vec<Vec<Name>>,
    v: usize,
    out: &mut Unseamed,
) {
    let mut at: BTreeSet<usize> = BTreeSet::new();
    for f in &s.faces {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    let e = &s.edges[u.edge];
                    if e.start == v || e.end == v {
                        at.insert(u.edge);
                    }
                }
            }
        }
    }
    let list: Vec<usize> = at.into_iter().collect();
    let [a, b] = list[..] else {
        return;
    };
    let (ea, eb) = (s.edges[a], s.edges[b]);
    if ea.start == ea.end || eb.start == eb.end || !same_curve(s, ea.curve, eb.curve) {
        return;
    }
    // First the edge that ends at v, then the one that starts there.
    let (first, second) = if ea.end == v && eb.start == v {
        (a, b)
    } else if eb.end == v && ea.start == v {
        (b, a)
    } else {
        return;
    };
    let (e1, e2) = (s.edges[first], s.edges[second]);
    let curve = &s.curves[e1.curve];
    let period = curve.period();
    let gap = match period {
        Some(p) => {
            let d = (e2.interval[0] - e1.interval[1]).rem_euclid(p);
            d.min(p - d)
        }
        None => (e2.interval[0] - e1.interval[1]).abs(),
    };
    let scale = (e1.interval[1] - e1.interval[0]).abs().max(1.0);
    if gap > 1e-9 * scale
        && curve
            .point(e2.interval[0])
            .distance(curve.point(e1.interval[1]))
            > LINEAR_RESOLUTION
    {
        return;
    }
    let start = e1.interval[0];
    let length = (e1.interval[1] - e1.interval[0]) + (e2.interval[1] - e2.interval[0]);
    let closed = e1.start == e2.end;
    let interval = match (closed, period) {
        (true, Some(p)) => [start, start + p],
        // A clamped curve whose ends meet: the closed edge is its whole
        // domain.
        (true, None) => {
            let domain = curve.domain();
            let near = |a: f64, b: f64| (a - b).abs() <= 1e-9 * scale;
            if !(near(start, domain[0]) && near(e2.interval[1], domain[1])) {
                return;
            }
            domain
        }
        // The second piece's own end, moved by whole periods to run on
        // from the first: its bits, not a sum's.
        (false, _) => {
            let shift = period.map_or(0.0, |p| ((e1.interval[1] - e2.interval[0]) / p).round() * p);
            [start, e2.interval[1] + shift]
        }
    };
    if let Some(p) = period
        && !closed
        && length >= p
    {
        return;
    }
    // Every face must run the two one after the other.
    let mut replaced: Vec<(usize, usize, Vec<Use>)> = Vec::new();
    let new = s.edges.len();
    for (k, f) in s.faces.iter().enumerate() {
        for (l, lp) in f.loops.iter().enumerate() {
            let Loop::Edges(uses) = lp else {
                continue;
            };
            if !uses.iter().any(|u| u.edge == first || u.edge == second) {
                continue;
            }
            let n = uses.len();
            let Some(i) = (0..n).find(|&i| {
                let (x, y) = (uses[i], uses[(i + 1) % n]);
                match x.sense {
                    Sense::Forward => x.edge == first && y.edge == second && y.sense == x.sense,
                    Sense::Reversed => x.edge == second && y.edge == first && y.sense == x.sense,
                }
            }) else {
                return;
            };
            let mut list: Vec<Use> = Vec::with_capacity(n - 1);
            for m in 0..n {
                if m == (i + 1) % n {
                    continue;
                }
                if m == i {
                    // A seam's pieces share their edge's pcurve, written
                    // once or once a piece: the joined edge keeps it.
                    let (x, y) = (uses[i].pcurve, uses[(i + 1) % n].pcurve);
                    let pcurve = match (x, y) {
                        (Some(p), Some(q)) if p == q || s.curves2[p] == s.curves2[q] => Some(p),
                        _ => None,
                    };
                    list.push(Use {
                        edge: new,
                        sense: uses[i].sense,
                        pcurve,
                    });
                } else {
                    list.push(uses[m]);
                }
            }
            replaced.push((k, l, list));
        }
    }
    s.edges.push(Edge {
        curve: e1.curve,
        interval,
        start: e1.start,
        end: e2.end,
        tolerance: e1.tolerance.max(e2.tolerance),
    });
    for (k, l, list) in replaced {
        s.faces[k].loops[l] = Loop::Edges(list);
    }
    let mut all: Vec<Name> = parts[first].iter().chain(&parts[second]).copied().collect();
    all.sort();
    let child = Name::derive(key, Role::Merged, &all);
    let mut parents = [names.edges[first], names.edges[second]];
    parents.sort();
    names.edges.push(child);
    parts.push(all);
    out.joined.push(names.vertices[v]);
    out.journal.push(Event::Deleted(names.vertices[v]));
    out.journal
        .push(Event::merged(parents[0], parents[1], child));
}
