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
//! Shells joined by a sewn edge are one shell, and their solids one solid.
//! What the edges' and vertices' new places leave as gaps the tolerances
//! cover later (see `gaps`).

use super::names::Names;
use super::{Edge, Loop, Shape, Shell, Solid, Vertex};
use crate::geom::{Box3, Curve, Point3, Vector3};
use crate::name::{Event, Journal, Name, OpKey, Role};
use crate::tol::LINEAR_RESOLUTION;
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
fn to_edge(s: &Shape, e: &Edge, p: Point3) -> f64 {
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

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Sews the free edges of `s` in place. New edges' and vertices' names
/// are pushed onto `names`.
pub(super) fn sew(key: OpKey, s: &mut Shape, names: &mut Names, cap: f64) -> Result<Sewn, Error> {
    let uses = uses_of(s);
    let free: Vec<usize> = (0..s.edges.len()).filter(|&e| uses[e].len() == 1).collect();
    if free.is_empty() {
        return Ok(Sewn::default());
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
    let mut out = Sewn::default();
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
    regroup(s);
    out.edges.sort();
    out.vertices.sort();
    Ok(out)
}

fn sorted(mut x: Vec<Name>) -> Vec<Name> {
    x.sort();
    x
}

/// Shells sharing an edge made one, in the order of their first faces;
/// their solids with them.
fn regroup(s: &mut Shape) {
    let mut shell_of = vec![0; s.faces.len()];
    for (k, sh) in s.shells.iter().enumerate() {
        for &f in &sh.faces {
            shell_of[f] = k;
        }
    }
    let mut parent: Vec<usize> = (0..s.shells.len()).collect();
    for list in uses_of(s) {
        if let [a, b, ..] = list[..] {
            let (x, y) = (
                find(&mut parent, shell_of[a.0]),
                find(&mut parent, shell_of[b.0]),
            );
            if x != y {
                parent[x.max(y)] = x.min(y);
            }
        }
    }
    let roots: Vec<usize> = (0..s.shells.len()).map(|k| find(&mut parent, k)).collect();
    if roots.iter().enumerate().all(|(k, &r)| r == k) {
        return;
    }
    let mut index: BTreeMap<usize, usize> = BTreeMap::new();
    let mut shells: Vec<Shell> = Vec::new();
    for (k, &r) in roots.iter().enumerate() {
        let n = *index.entry(r).or_insert_with(|| {
            shells.push(Shell {
                faces: Vec::new(),
                closed: true,
            });
            shells.len() - 1
        });
        shells[n].faces.extend(s.shells[k].faces.iter().copied());
        shells[n].closed &= s.shells[k].closed;
    }
    // Solids whose shells were joined are one: its outer shells' union,
    // and every void that isn't one of them.
    let mut solids: Vec<Solid> = Vec::new();
    let mut solid_of: BTreeMap<usize, usize> = BTreeMap::new();
    for so in &s.solids {
        let outer = index[&roots[so.outer]];
        let n = *solid_of.entry(outer).or_insert_with(|| {
            solids.push(Solid {
                outer,
                voids: Vec::new(),
            });
            solids.len() - 1
        });
        for &v in &so.voids {
            let v = index[&roots[v]];
            if v != outer && !solids[n].voids.contains(&v) {
                solids[n].voids.push(v);
            }
        }
    }
    s.shells = shells;
    s.solids = solids;
}
