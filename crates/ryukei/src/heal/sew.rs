//! Sewing: edges a file repeats on two faces joined into one, and the
//! vertices at their ends with them.
//!
//! A file may give each face its own edges and vertices (a surface model,
//! or a writer that never shares them), so that every edge is used once.
//! Two such free edges are one edge when they run between the same two
//! places along the same curve: both ends and every one of 17 samples
//! along each lie within the cap of the other edge, and that gap is under
//! half the shorter one's chord, so two short edges side by side are never
//! taken for one. Candidates come from the edges' boxes, grown by the cap
//! and swept along `x`. An edge pairs with its nearest candidate, pairs
//! taken from the nearest gap up (ties by name), each edge once; two edges
//! of one face are never paired, nor edges one of which is closed and the
//! other not. A free edge with two candidates that coincide with it (to
//! ten resolutions) has three faces there: [`HealError::NonManifold`].
//!
//! A pair becomes a new edge on the curve of its least-named edge, named
//! as merged from both ([`Event::Merged`]); the other's use runs it with
//! the sense that keeps its loop's direction, and loses its pcurve unless
//! the two edges are the same curve over the same interval. Vertices
//! paired by the edges' ends are joined into classes, each a new vertex at
//! its least-named member's point, named as merged from all of them.
//! Which faces make a shell is the build's to find out (see `shells`).
//! What the edges' and vertices' new places leave as gaps the tolerances
//! cover later (see `gaps`).
//!
//! A T-junction, where one face splits an edge its neighbour leaves
//! whole, is sewn too. Before pairing, a free edge is split where a free
//! vertex of another face lies within the cap of it, farther than the
//! cap from its ends, and an edge of that face runs from the vertex
//! along it (its other end within the cap of the edge too): the pieces
//! keep its curve, its parameter and its use's pcurve, and are named as
//! parts of it after the vertices it was split at ([`Event::Split`]).
//! After pairing, a free open edge whose two ends were joined (the short
//! piece of a split within the cap of the neighbour's vertex), no longer
//! than the cap, is removed from its loop ([`Event::Deleted`]), unless
//! that would leave the loop with no edges.

use super::names::Names;
use super::{Edge, Loop, Shape, Use, Vertex};
use crate::geom::{Box3, Curve, Point3, Vector3};
use crate::name::{Event, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::Sense;
use crate::{Error, HealError};
use std::collections::{BTreeMap, BTreeSet};

/// Samples along each edge of a pair.
const SAMPLES: usize = 16;

/// What sewing did, by import name.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Sewn {
    /// Edges made by sewing two of the file's.
    pub(super) edges: Vec<Name>,
    /// Vertices made by joining the file's.
    pub(super) vertices: Vec<Name>,
    /// The file's edges split at T-junctions.
    pub(super) split: Vec<Name>,
    /// Free edges whose ends sewing joined, removed.
    pub(super) removed: Vec<Name>,
    pub(super) journal: Journal,
}

/// Where each edge is used: face, loop, position.
fn uses_of(s: &Shape) -> Vec<Vec<(usize, usize, usize)>> {
    let mut out = vec![Vec::new(); s.edges.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for (l, lp) in f.loops.iter().enumerate() {
            if let Loop::Edges(uses) = lp {
                for (i, u) in uses.iter().enumerate() {
                    out[u.edge].push((k, l, i));
                }
            }
        }
    }
    out
}

/// The distance from `p` to edge `e`: to its curve where the foot lies in
/// its interval, else to the nearer end. Never less than the true one.
pub(super) fn to_edge(s: &Shape, e: &Edge, p: Point3) -> f64 {
    let curve = &s.curves[e.curve];
    let [lo, hi] = e.interval;
    let foot = curve.project(p);
    let t = match curve.period() {
        Some(per) => lo + (foot.param - lo).rem_euclid(per),
        None => foot.param,
    };
    if (lo..=hi).contains(&t) {
        foot.distance
    } else {
        curve.point(lo).distance(p).min(curve.point(hi).distance(p))
    }
}

/// Samples along an edge, ends included.
fn samples(s: &Shape, e: &Edge) -> Vec<Point3> {
    let [t0, t1] = e.interval;
    (0..=SAMPLES)
        .map(|i| s.curves[e.curve].point(t0 + (t1 - t0) * i as f64 / SAMPLES as f64))
        .collect()
}

/// The unit tangent of an edge at its start.
fn tangent(s: &Shape, e: &Edge) -> Option<Vector3> {
    let c: &Curve = &s.curves[e.curve];
    c.tangent(e.interval[0]).map(|t| t.get())
}

/// How far apart edges `a` and `b` are, and whether they run the same
/// way, if they may be one edge within `cap`.
fn fit(s: &Shape, a: usize, b: usize, cap: f64) -> Option<(f64, bool)> {
    let (ea, eb) = (&s.edges[a], &s.edges[b]);
    let closed = ea.start == ea.end;
    if closed != (eb.start == eb.end) {
        return None;
    }
    let p = |v: usize| s.vertices[v].point;
    let same = if closed {
        let (ta, tb) = (tangent(s, ea)?, tangent(s, eb)?);
        ta.dot(tb) > 0.0
    } else {
        let along = p(ea.start)
            .distance(p(eb.start))
            .max(p(ea.end).distance(p(eb.end)));
        let across = p(ea.start)
            .distance(p(eb.end))
            .max(p(ea.end).distance(p(eb.start)));
        along <= across
    };
    let (sa, sb) = (samples(s, ea), samples(s, eb));
    let ends = if same {
        sa[0].distance(sb[0]).max(sa[SAMPLES].distance(sb[SAMPLES]))
    } else {
        sa[0].distance(sb[SAMPLES]).max(sa[SAMPLES].distance(sb[0]))
    };
    if ends > cap {
        return None;
    }
    let mut gap = ends;
    for &q in &sa {
        gap = gap.max(to_edge(s, eb, q));
    }
    for &q in &sb {
        gap = gap.max(to_edge(s, ea, q));
    }
    // Under half the shorter chord: a closed edge measures across.
    let chord = |x: &[Point3]| {
        if closed {
            x[SAMPLES / 2].distance(x[0])
        } else {
            x[SAMPLES].distance(x[0])
        }
    };
    (gap <= cap && gap < chord(&sa).min(chord(&sb)) / 2.0).then_some((gap, same))
}

pub(super) fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The parameter of the foot of `p` on edge `e` and its distance, where
/// the foot lies inside the edge's interval.
fn foot(s: &Shape, e: &Edge, p: Point3) -> Option<(f64, f64)> {
    let curve = &s.curves[e.curve];
    let [lo, hi] = e.interval;
    let f = curve.project(p);
    let t = match curve.period() {
        Some(per) => lo + (f.param - lo).rem_euclid(per),
        None => f.param,
    };
    (lo < t && t < hi).then_some((t, f.distance))
}

/// Splits free edges at T-junctions (see the module's notes), recording
/// what it did in `out`.
fn split_at_ts(key: OpKey, s: &mut Shape, names: &mut Names, cap: f64, out: &mut Sewn) {
    let uses = uses_of(s);
    let free: Vec<usize> = (0..s.edges.len()).filter(|&e| uses[e].len() == 1).collect();
    if free.len() < 2 {
        return;
    }
    // Each free vertex and the free open edges ending there.
    let mut at: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &e in &free {
        let x = &s.edges[e];
        if x.start != x.end {
            at.entry(x.start).or_default().push(e);
            at.entry(x.end).or_default().push(e);
        }
    }
    let mut by_x: Vec<(f64, usize)> = at.keys().map(|&v| (s.vertices[v].point.x, v)).collect();
    by_x.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut cuts: Vec<(usize, Vec<(f64, usize)>)> = Vec::new();
    for &e in &free {
        let x = s.edges[e];
        if x.start == x.end {
            continue;
        }
        let face = uses[e][0].0;
        let reach = s.curves[x.curve].bounds(x.interval).grow(cap);
        let first = by_x.partition_point(|&(px, _)| px < reach.min().x);
        let ends = [s.vertices[x.start].point, s.vertices[x.end].point];
        let mut here: Vec<(f64, usize)> = Vec::new();
        for &(px, v) in &by_x[first..] {
            if px > reach.max().x {
                break;
            }
            let p = s.vertices[v].point;
            if v == x.start || v == x.end || !reach.intersects(&Box3::from_point(p)) {
                continue;
            }
            if ends.iter().any(|q| q.distance(p) <= cap) {
                continue;
            }
            let Some((t, d)) = foot(s, &x, p) else {
                continue;
            };
            if d > cap {
                continue;
            }
            // An edge of another face runs from the vertex along this one.
            let along = at[&v].iter().any(|&g| {
                let y = &s.edges[g];
                let w = if y.start == v { y.end } else { y.start };
                uses[g][0].0 != face && to_edge(s, &x, s.vertices[w].point) <= cap
            });
            if along {
                here.push((t, v));
            }
        }
        if here.is_empty() {
            continue;
        }
        here.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        here.dedup_by_key(|c| c.1);
        cuts.push((e, here));
    }
    for (e, here) in cuts {
        let x = s.edges[e];
        let mut parents = vec![names.edges[e]];
        parents.extend(here.iter().map(|c| names.vertices[c.1]));
        let mut stops = vec![(x.interval[0], x.start)];
        stops.extend(here.iter().copied());
        stops.push((x.interval[1], x.end));
        let mut pieces: Vec<usize> = Vec::new();
        for (i, w) in stops.windows(2).enumerate() {
            s.edges.push(Edge {
                interval: [w[0].0, w[1].0],
                start: w[0].1,
                end: w[1].1,
                ..x
            });
            pieces.push(s.edges.len() - 1);
            let part = u8::try_from(i).unwrap_or(u8::MAX);
            names
                .edges
                .push(Name::derive(key, Role::Part(part), &parents));
        }
        let children: Vec<Name> = pieces.iter().map(|&p| names.edges[p]).collect();
        out.split.push(names.edges[e]);
        out.journal.push(Event::Split {
            parent: names.edges[e],
            children,
        });
        let (k, l, i) = uses[e][0];
        let Loop::Edges(list) = &mut s.faces[k].loops[l] else {
            unreachable!("an edge's use is in a loop of edges")
        };
        let u = list[i];
        let mut run: Vec<Use> = pieces.iter().map(|&p| Use { edge: p, ..u }).collect();
        if u.sense == Sense::Reversed {
            run.reverse();
        }
        list.splice(i..=i, run);
    }
    out.split.sort();
}

/// Sews the free edges of `s` in place. New edges' and vertices' names
/// are pushed onto `names`.
pub(super) fn sew(key: OpKey, s: &mut Shape, names: &mut Names, cap: f64) -> Result<Sewn, Error> {
    let mut out = Sewn::default();
    split_at_ts(key, s, names, cap, &mut out);
    let uses = uses_of(s);
    let free: Vec<usize> = (0..s.edges.len()).filter(|&e| uses[e].len() == 1).collect();
    if free.is_empty() {
        return Ok(out);
    }
    // Candidate pairs: boxes grown by the cap, swept along x.
    let boxes: Vec<Box3> = free
        .iter()
        .map(|&e| {
            let x = &s.edges[e];
            s.curves[x.curve].bounds(x.interval).grow(cap)
        })
        .collect();
    let mut order: Vec<usize> = (0..free.len()).collect();
    order.sort_by(|&i, &j| {
        boxes[i]
            .min()
            .x
            .total_cmp(&boxes[j].min().x)
            .then(i.cmp(&j))
    });
    let mut pairs: Vec<(f64, Name, Name, usize, usize, bool)> = Vec::new();
    for (n, &i) in order.iter().enumerate() {
        for &j in &order[n + 1..] {
            if boxes[j].min().x > boxes[i].max().x {
                break;
            }
            if !boxes[i].intersects(&boxes[j]) {
                continue;
            }
            let (a, b) = (free[i], free[j]);
            if uses[a][0].0 == uses[b][0].0 {
                continue;
            }
            if let Some((gap, same)) = fit(s, a, b, cap) {
                let (na, nb) = (names.edges[a], names.edges[b]);
                let (a, b, na, nb) = if na <= nb {
                    (a, b, na, nb)
                } else {
                    (b, a, nb, na)
                };
                pairs.push((gap, na, nb, a, b, same));
            }
        }
    }
    // Three faces at one edge: two candidates coinciding with one edge.
    let mut close: BTreeMap<usize, usize> = BTreeMap::new();
    for p in &pairs {
        if p.0 <= 10.0 * LINEAR_RESOLUTION {
            *close.entry(p.3).or_default() += 1;
            *close.entry(p.4).or_default() += 1;
        }
    }
    if let Some(at) = close
        .iter()
        .filter(|x| *x.1 > 1)
        .map(|x| names.edges[*x.0])
        .min()
    {
        return Err(HealError::NonManifold { at }.into());
    }
    pairs.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    let mut chosen: Vec<(usize, usize, bool)> = Vec::new();
    for &(_, _, _, a, b, same) in &pairs {
        if taken.contains(&a) || taken.contains(&b) {
            continue;
        }
        taken.insert(a);
        taken.insert(b);
        chosen.push((a, b, same));
    }
    if chosen.is_empty() {
        return Ok(out);
    }
    // Vertices joined by the pairs' ends.
    let mut parent: Vec<usize> = (0..s.vertices.len()).collect();
    let mut join = |x: usize, y: usize| {
        let (rx, ry) = (find(&mut parent, x), find(&mut parent, y));
        if rx != ry {
            parent[rx.max(ry)] = rx.min(ry);
        }
    };
    for &(a, b, same) in &chosen {
        let (ea, eb) = (s.edges[a], s.edges[b]);
        let (b0, b1) = if same {
            (eb.start, eb.end)
        } else {
            (eb.end, eb.start)
        };
        join(ea.start, b0);
        join(ea.end, b1);
    }
    let mut classes: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for v in 0..s.vertices.len() {
        let r = find(&mut parent, v);
        classes.entry(r).or_default().push(v);
    }
    // Each class of more than one: a new vertex at its least-named
    // member's point.
    let mut vertex_to: Vec<usize> = (0..s.vertices.len()).collect();
    let mut joined: Vec<(Name, Vec<usize>)> = classes
        .into_values()
        .filter(|c| c.len() > 1)
        .map(|c| {
            let least = c.iter().map(|&v| names.vertices[v]).min().expect("members");
            (least, c)
        })
        .collect();
    joined.sort_by_key(|x| x.0);
    for (_, class) in joined {
        let first = *class
            .iter()
            .min_by_key(|&&v| names.vertices[v])
            .expect("members");
        let new = s.vertices.len();
        s.vertices.push(Vertex {
            point: s.vertices[first].point,
            tolerance: class
                .iter()
                .map(|&v| s.vertices[v].tolerance)
                .fold(0.0, f64::max),
            seam: class.iter().all(|&v| s.vertices[v].seam),
        });
        let parents: Vec<Name> = class.iter().map(|&v| names.vertices[v]).collect();
        let child = Name::derive(key, Role::Merged, &sorted(parents.clone()));
        names.vertices.push(child);
        out.vertices.push(child);
        out.journal.push(Event::merged_all(parents, child));
        for v in class {
            vertex_to[v] = new;
        }
    }
    // Each pair: a new edge on the least-named one's curve.
    for (a, b, same) in chosen {
        let (ea, eb) = (s.edges[a], s.edges[b]);
        let new = s.edges.len();
        s.edges.push(Edge {
            tolerance: ea.tolerance.max(eb.tolerance),
            ..ea
        });
        let child = Name::derive(
            key,
            Role::Merged,
            &sorted(vec![names.edges[a], names.edges[b]]),
        );
        names.edges.push(child);
        out.edges.push(child);
        out.journal
            .push(Event::merged(names.edges[a], names.edges[b], child));
        let alike = ea.curve == eb.curve && ea.interval == eb.interval;
        for (e, own) in [(a, true), (b, false)] {
            let (k, l, i) = uses[e][0];
            let Loop::Edges(list) = &mut s.faces[k].loops[l] else {
                unreachable!("an edge's use is in a loop of edges")
            };
            let u = &mut list[i];
            u.edge = new;
            if !own {
                if !same {
                    u.sense = u.sense.flipped();
                }
                if !alike {
                    u.pcurve = None;
                }
            }
        }
    }
    // Free open edges whose ends were joined, short enough to go.
    let gone: Vec<usize> = free
        .iter()
        .copied()
        .filter(|&e| {
            let x = &s.edges[e];
            !taken.contains(&e)
                && x.start != x.end
                && vertex_to[x.start] == vertex_to[x.end]
                && super::collapse::length(s, x) <= cap
        })
        .collect();
    for e in gone {
        let (k, l, _) = uses[e][0];
        let Loop::Edges(list) = &mut s.faces[k].loops[l] else {
            unreachable!("an edge's use is in a loop of edges")
        };
        if list.len() > 1 {
            list.retain(|u| u.edge != e);
            out.removed.push(names.edges[e]);
            out.journal.push(Event::Deleted(names.edges[e]));
        }
    }
    for e in &mut s.edges {
        e.start = vertex_to[e.start];
        e.end = vertex_to[e.end];
    }
    for f in &mut s.faces {
        for l in &mut f.loops {
            if let Loop::Vertex(v) = l {
                *v = vertex_to[*v];
            }
        }
    }
    out.edges.sort();
    out.vertices.sort();
    out.removed.sort();
    Ok(out)
}

pub(super) fn sorted(mut x: Vec<Name>) -> Vec<Name> {
    x.sort();
    x
}
