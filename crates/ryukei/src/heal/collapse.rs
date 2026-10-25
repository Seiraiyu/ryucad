//! Collapsing: what is smaller than its own tolerance taken out, so
//! nothing downstream trips on it, and nothing moves by more than a
//! tolerance.
//!
//! An open edge whose length is within its tolerance (bounded from
//! above: a line's or an arc's length, a spline's hull) is removed and
//! its vertices joined, unless that would close another edge or leave a
//! loop with no edges. Edges are taken in name order, chains of short
//! edges joined into one vertex; each class of joined vertices becomes a
//! new vertex at its least-named member's point, named as merged from
//! all of them ([`Event::Merged`]), its tolerance covering every member.
//!
//! Then a face whose only loop is two open edges between the same two
//! vertices, each within the larger of their tolerances of the other (17
//! samples along each), is a sliver narrower than its tolerance: it is
//! removed, and its two edges become one on the least-named one's curve,
//! named as merged from both, used by the faces either side; its
//! tolerance covers the sliver's width. Slivers are taken in name order,
//! none sharing an edge, nor two whose other faces would be one face.
//! What the joins leave as gaps the tolerances cover (see `gaps`).

use super::names::Names;
use super::sew::{find, sorted, to_edge};
use super::{Edge, Loop, Shape, Vertex};
use crate::geom::{Curve, Point3};
use crate::name::{Event, Journal, Name, OpKey, Role};
use crate::topo::Sense;
use std::collections::{BTreeMap, BTreeSet};

/// Samples along each edge of a sliver.
const SAMPLES: usize = 16;

/// What collapsing did, by import name.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Collapsed {
    /// Edges and faces removed.
    pub(super) removed: Vec<Name>,
    /// Vertices made by joining the ends of removed edges.
    pub(super) vertices: Vec<Name>,
    /// Edges made by joining a sliver's two.
    pub(super) edges: Vec<Name>,
    pub(super) journal: Journal,
}

/// At least the length of edge `e`.
fn length(s: &Shape, e: &Edge) -> f64 {
    let [t0, t1] = e.interval;
    match &s.curves[e.curve] {
        Curve::Line(_) => t1 - t0,
        Curve::Circle(c) => c.radius() * (t1 - t0),
        Curve::Ellipse(x) => x.major() * (t1 - t0),
        c => {
            let b = c.bounds(e.interval);
            (b.max() - b.min()).length()
        }
    }
}

/// The faces using each edge.
fn faces_of(s: &Shape) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); s.edges.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            if let Loop::Edges(uses) = l {
                for u in uses {
                    out[u.edge].push(k);
                }
            }
        }
    }
    out
}

/// Collapses short edges and slivers of `s` in place. New edges' and
/// vertices' names are pushed onto `names`; removed faces go from both.
pub(super) fn collapse(key: OpKey, s: &mut Shape, names: &mut Names) -> Collapsed {
    let mut out = Collapsed::default();
    edges(key, s, names, &mut out);
    slivers(key, s, names, &mut out);
    out.removed.sort();
    out.vertices.sort();
    out.edges.sort();
    out
}

fn edges(key: OpKey, s: &mut Shape, names: &mut Names, out: &mut Collapsed) {
    let faces = faces_of(s);
    let used: Vec<usize> = (0..s.edges.len())
        .filter(|&e| !faces[e].is_empty())
        .collect();
    let mut short: Vec<usize> = used
        .iter()
        .copied()
        .filter(|&e| {
            let x = &s.edges[e];
            x.start != x.end && length(s, x) <= x.tolerance
        })
        .collect();
    if short.is_empty() {
        return;
    }
    short.sort_by_key(|&e| names.edges[e]);
    let mut parent: Vec<usize> = (0..s.vertices.len()).collect();
    let mut gone: BTreeSet<usize> = BTreeSet::new();
    for e in short {
        let mut trial = parent.clone();
        let (a, b) = (
            find(&mut trial, s.edges[e].start),
            find(&mut trial, s.edges[e].end),
        );
        if a == b {
            continue;
        }
        trial[a.max(b)] = a.min(b);
        gone.insert(e);
        // No other edge closed, no loop left without edges.
        let closes = used.iter().any(|&g| {
            !gone.contains(&g) && {
                let x = &s.edges[g];
                x.start != x.end && find(&mut trial, x.start) == find(&mut trial, x.end)
            }
        });
        let empties = s.faces.iter().any(|f| {
            f.loops.iter().any(|l| match l {
                Loop::Edges(uses) => uses.iter().all(|u| gone.contains(&u.edge)),
                Loop::Vertex(_) => false,
            })
        });
        if closes || empties {
            gone.remove(&e);
            continue;
        }
        parent = trial;
    }
    if gone.is_empty() {
        return;
    }
    let mut classes: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for v in 0..s.vertices.len() {
        let r = find(&mut parent, v);
        classes.entry(r).or_default().push(v);
    }
    let mut vertex_to: Vec<usize> = (0..s.vertices.len()).collect();
    let mut joined: Vec<(Name, Vec<usize>)> = classes
        .into_values()
        .filter(|c| c.len() > 1)
        .map(|c| {
            (
                c.iter().map(|&v| names.vertices[v]).min().expect("members"),
                c,
            )
        })
        .collect();
    joined.sort_by_key(|x| x.0);
    for (least, class) in joined {
        let first = *class
            .iter()
            .find(|&&v| names.vertices[v] == least)
            .expect("its least");
        let point: Point3 = s.vertices[first].point;
        let new = s.vertices.len();
        s.vertices.push(Vertex {
            point,
            tolerance: class
                .iter()
                .map(|&v| s.vertices[v].tolerance + s.vertices[v].point.distance(point))
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
    for &e in &gone {
        out.removed.push(names.edges[e]);
        out.journal.push(Event::Deleted(names.edges[e]));
    }
    for f in &mut s.faces {
        for l in &mut f.loops {
            match l {
                Loop::Edges(uses) => uses.retain(|u| !gone.contains(&u.edge)),
                Loop::Vertex(v) => *v = vertex_to[*v],
            }
        }
    }
    for x in &mut s.edges {
        x.start = vertex_to[x.start];
        x.end = vertex_to[x.end];
    }
}

/// The vertex a use of edge `e` starts at.
fn start_of(e: &Edge, sense: Sense) -> usize {
    match sense {
        Sense::Forward => e.start,
        Sense::Reversed => e.end,
    }
}

/// How far apart edges `a` and `b` are, both ways, by samples.
fn width(s: &Shape, a: &Edge, b: &Edge) -> f64 {
    let along = |x: &Edge, y: &Edge| {
        let [t0, t1] = x.interval;
        (0..=SAMPLES)
            .map(|i| {
                let p = s.curves[x.curve].point(t0 + (t1 - t0) * i as f64 / SAMPLES as f64);
                to_edge(s, y, p)
            })
            .fold(0.0, f64::max)
    };
    along(a, b).max(along(b, a))
}

fn slivers(key: OpKey, s: &mut Shape, names: &mut Names, out: &mut Collapsed) {
    let faces = faces_of(s);
    let mut order: Vec<usize> = (0..s.faces.len()).collect();
    order.sort_by_key(|&k| names.faces[k]);
    let mut taken_edges: BTreeSet<usize> = BTreeSet::new();
    let mut taken_faces: BTreeSet<usize> = BTreeSet::new();
    let mut found: Vec<(usize, usize, usize, f64)> = Vec::new();
    for k in order {
        let [Loop::Edges(uses)] = &s.faces[k].loops[..] else {
            continue;
        };
        let [ua, ub] = uses[..] else { continue };
        let (a, b) = (ua.edge, ub.edge);
        let (ea, eb) = (&s.edges[a], &s.edges[b]);
        if a == b || ea.start == ea.end || eb.start == eb.end {
            continue;
        }
        if BTreeSet::from([ea.start, ea.end]) != BTreeSet::from([eb.start, eb.end]) {
            continue;
        }
        let other = |e: usize| -> Option<usize> {
            match faces[e][..] {
                [x, y] if x == k => Some(y),
                [x, y] if y == k => Some(x),
                _ => None,
            }
        };
        let (Some(fa), Some(fb)) = (other(a), other(b)) else {
            continue;
        };
        if fa == fb
            || taken_edges.contains(&a)
            || taken_edges.contains(&b)
            || taken_faces.contains(&fa)
            || taken_faces.contains(&fb)
        {
            continue;
        }
        let w = width(s, ea, eb);
        if w > ea.tolerance.max(eb.tolerance) {
            continue;
        }
        taken_edges.extend([a, b]);
        taken_faces.extend([fa, fb]);
        let (a, b) = if names.edges[a] <= names.edges[b] {
            (a, b)
        } else {
            (b, a)
        };
        found.push((k, a, b, w));
    }
    let mut removed_faces: Vec<usize> = Vec::new();
    for (k, a, b, w) in found {
        let (ea, eb) = (s.edges[a], s.edges[b]);
        let new = s.edges.len();
        s.edges.push(Edge {
            tolerance: ea.tolerance.max(eb.tolerance).max(w * 1.0001 + 1e-12),
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
        out.removed.push(names.faces[k]);
        out.journal.push(Event::Deleted(names.faces[k]));
        for (j, f) in s.faces.iter_mut().enumerate() {
            if j == k {
                continue;
            }
            for l in &mut f.loops {
                let Loop::Edges(uses) = l else { continue };
                for u in uses.iter_mut() {
                    if u.edge == a {
                        u.edge = new;
                    } else if u.edge == b {
                        // Run the new edge from where this use of b started.
                        let from = start_of(&eb, u.sense);
                        u.sense = if ea.start == from {
                            Sense::Forward
                        } else {
                            Sense::Reversed
                        };
                        u.edge = new;
                        u.pcurve = None;
                    }
                }
            }
        }
        removed_faces.push(k);
    }
    removed_faces.sort_unstable();
    for &k in removed_faces.iter().rev() {
        s.faces.remove(k);
        names.faces.remove(k);
        for sh in &mut s.shells {
            sh.faces.retain(|&f| f != k);
            for f in &mut sh.faces {
                if *f > k {
                    *f -= 1;
                }
            }
        }
    }
}
