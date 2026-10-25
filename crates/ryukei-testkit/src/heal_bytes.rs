//! Shapes to heal from bytes, for the `heal` fuzz target and for
//! replaying what it has found: a body (as [`crate::blend_bytes::body`]
//! makes them) written as a file must describe it
//! ([`ryukei::heal::seamed`]), then up to four of the faults files have:
//! a face turned over, an edge given to one face as a copy of its own,
//! an edge's curve or a vertex moved off by a small amount, a short piece
//! split off an edge with a tolerance longer than it, a loop run the
//! other way, and every shell called a solid of its own.
//!
//! [`HealCase::mild`] says whether every fault is one healing must undo
//! whatever the body: turned faces, copies, moves under a micrometre,
//! short pieces and forgotten cavities. A loop run backwards on its own
//! and larger moves may be refused.

use crate::blend_bytes::body;
use crate::euler_script::Bytes;
use ryukei::geom::{Curve, Rigid, Vector3};
use ryukei::heal::{Edge, Loop, Shape, Solid, Use, Vertex, seamed};
use ryukei::topo::{Body, Sense};

/// A shape to heal, the body it was written from, and whether its faults
/// are all ones healing must undo.
pub struct HealCase {
    pub body: Body,
    pub shape: Shape,
    pub mild: bool,
    /// What was done to the shape, in order, for reports.
    pub faults: Vec<String>,
}

/// Turns face `k` over: its sense and every loop reversed.
fn turn(s: &mut Shape, k: usize) {
    let f = &mut s.faces[k];
    f.sense = f.sense.flipped();
    for l in &mut f.loops {
        if let Loop::Edges(list) = l {
            reverse(list);
        }
    }
}

fn reverse(list: &mut [Use]) {
    list.reverse();
    for u in list {
        u.sense = u.sense.flipped();
    }
}

/// Each use of an edge: face, loop, position.
fn uses_of(s: &Shape, e: usize) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for (k, f) in s.faces.iter().enumerate() {
        for (l, lp) in f.loops.iter().enumerate() {
            if let Loop::Edges(list) = lp {
                for (i, u) in list.iter().enumerate() {
                    if u.edge == e {
                        out.push((k, l, i));
                    }
                }
            }
        }
    }
    out
}

/// A direction from bytes, never zero.
fn direction(b: &mut Bytes) -> Vector3 {
    let mut c = || f64::from(b.pick(5) as u8) - 2.0;
    let v = Vector3::new(c(), c(), c());
    if v.length() == 0.0 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        v * (1.0 / v.length())
    }
}

/// The parameter step along `c` that covers about `length` of it, where
/// the curve's speed is known.
fn step(c: &Curve, length: f64) -> Option<f64> {
    match c {
        Curve::Line(_) => Some(length),
        Curve::Circle(x) => Some(length / x.radius()),
        Curve::Ellipse(x) => Some(length / x.major()),
        _ => None,
    }
}

/// A shape from bytes, or `None` when they give no body or it can't be
/// written.
pub fn heal_case(data: &[u8]) -> Option<HealCase> {
    let (body, rest) = body(data)?;
    let mut shape = seamed(&body, 1e-7).ok()?;
    if shape.faces.is_empty() {
        return None;
    }
    let mut b = Bytes::new(rest);
    let mut mild = true;
    let mut faults = Vec::new();
    let s = &mut shape;
    for _ in 0..b.pick(5) {
        let kind = b.pick(7);
        // A sphere or a torus has no edges or vertices to pick.
        if (1..=4).contains(&kind) && s.edges.is_empty() || kind == 3 && s.vertices.is_empty() {
            continue;
        }
        match kind {
            0 => {
                let k = b.pick(s.faces.len());
                turn(s, k);
                faults.push(format!("turned face {k}"));
            }
            1 => {
                // Edge `e` as a copy of its own to its second use.
                let e = b.pick(s.edges.len());
                let uses = uses_of(s, e);
                // A seam's two uses are one face's: files share seams.
                let [(k0, _, _), (k, l, i)] = uses[..] else {
                    continue;
                };
                if k0 == k {
                    continue;
                }
                s.edges.push(s.edges[e]);
                let copy = s.edges.len() - 1;
                if let Loop::Edges(list) = &mut s.faces[k].loops[l] {
                    list[i].edge = copy;
                }
                faults.push(format!("copied edge {e} for face {k}"));
            }
            2 => {
                let e = b.pick(s.edges.len());
                let by = [1e-8, 1e-7, 1e-6, 1e-5, 5e-5][b.pick(5)];
                mild &= by <= 1e-6;
                let moved = s.curves[s.edges[e].curve]
                    .transformed(&Rigid::translation(direction(&mut b) * by));
                s.curves.push(moved);
                s.edges[e].curve = s.curves.len() - 1;
                faults.push(format!("moved edge {e} by {by:e}"));
            }
            3 => {
                let v = b.pick(s.vertices.len());
                let by = [1e-8, 1e-7, 1e-6, 1e-5][b.pick(4)];
                mild &= by <= 1e-6;
                s.vertices[v].point += direction(&mut b) * by;
                faults.push(format!("moved vertex {v} by {by:e}"));
            }
            4 => {
                // A piece 50 nm long split off an edge's end, with a
                // tolerance of 100 nm.
                let e = b.pick(s.edges.len());
                let x = s.edges[e];
                let Some(d) = step(&s.curves[x.curve], 5e-8) else {
                    continue;
                };
                let faces: Vec<usize> = uses_of(s, e).iter().map(|u| u.0).collect();
                // An edge copied for its second face is used once: split,
                // its copy would end at a vertex the copy doesn't have (a
                // T-junction, which sewing doesn't undo).
                if faces.len() != 2
                    || x.start == x.end
                    || x.interval[1] - x.interval[0] <= 4.0 * d
                    || faces.windows(2).any(|w| w[0] == w[1])
                {
                    continue;
                }
                let cut = x.interval[1] - d;
                let vn = s.vertices.len();
                s.vertices.push(Vertex {
                    point: s.curves[x.curve].point(cut),
                    tolerance: 0.0,
                    seam: false,
                });
                s.edges[e].interval[1] = cut;
                s.edges[e].end = vn;
                let piece = s.edges.len();
                faults.push(format!("split edge {e}"));
                s.edges.push(Edge {
                    interval: [cut, x.interval[1]],
                    start: vn,
                    tolerance: 1e-7,
                    ..x
                });
                for (k, l, i) in uses_of(s, e) {
                    let Loop::Edges(list) = &mut s.faces[k].loops[l] else {
                        continue;
                    };
                    let u = list[i];
                    // The file's pcurve was for the whole edge.
                    list[i].pcurve = None;
                    let p = Use {
                        edge: piece,
                        sense: u.sense,
                        pcurve: None,
                    };
                    match u.sense {
                        Sense::Forward => list.insert(i + 1, p),
                        Sense::Reversed => list.insert(i, p),
                    }
                }
            }
            5 => {
                let k = b.pick(s.faces.len());
                let l = b.pick(s.faces[k].loops.len().max(1));
                if let Some(Loop::Edges(list)) = s.faces[k].loops.get_mut(l) {
                    reverse(list);
                    mild = false;
                    faults.push(format!("reversed loop {l} of face {k}"));
                }
            }
            _ => {
                s.solids = (0..s.shells.len())
                    .map(|outer| Solid {
                        outer,
                        voids: Vec::new(),
                    })
                    .collect();
                faults.push("forgot cavities".into());
            }
        }
    }
    Some(HealCase {
        body,
        shape,
        mild,
        faults,
    })
}
