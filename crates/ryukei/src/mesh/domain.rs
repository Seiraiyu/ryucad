//! A face's region in its scaled parameter plane: a constrained
//! triangulation of its loops with every triangle marked inside or
//! outside.
//!
//! Loops come from the edges' samples, inverted onto the face. In a
//! periodic direction they are unwrapped (each sample moved by whole
//! periods next to the one before), and the plane is cut at the seam into
//! one period, the rectangle the triangulation starts from: a loop is cut
//! at its samples on the seam (the edge stage put one at every crossing),
//! and each piece is shifted into the period. The rectangle's two seam
//! sides are the same curve on the surface, so their points come in pairs
//! that share one position. A side where the surface collapses to a point
//! (a pole, an apex) is a "pole" side whose points all share one position.
//! Any other side lies beyond the loops by a margin and is outside.
//!
//! A loop runs counterclockwise about the face's normal, so the face lies
//! on its left in the parameter plane when the face's normal is the
//! surface's natural one, and on its right otherwise. The triangles each
//! side of every loop segment are marked so, and the marks spread across
//! unfixed edges. A face with no loop of edges is inside everywhere.

use super::cdt::{Cdt, Crossing, NONE};
use super::edge::{EdgeSamples, unwrap};
use super::uv::{FaceGeom, U, V};
use crate::geom::Point2;
use crate::topo::{Body, EdgeId, Sense, VertexId};
use std::collections::{BTreeMap, VecDeque};

/// What a triangulation vertex's position is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Anchor {
    /// A body vertex.
    Vertex(VertexId),
    /// An inner sample of an edge, by index.
    Sample(EdgeId, u32),
    /// The surface at the vertex's parameters.
    Free,
}

/// What one side of the rectangle is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    /// Beyond the loops: no inside triangle touches it.
    Outside,
    /// A seam: the same curve as the opposite side.
    Seam,
    /// A collapsed row: every point on it is one point of the surface.
    Pole,
}

/// The face's region could not be built: its loops cross after
/// unwrapping, wind in a way the cut can't follow, or meet a fixed edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BadDomain;

/// One loop point before unwrapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LoopPoint {
    /// Canonical parameters on the face.
    pub(crate) uv: Point2,
    pub(crate) anchor: Anchor,
    /// The edge and parameters of the segment from this point to the
    /// next.
    pub(crate) seg: (EdgeId, f64, f64),
}

/// A face's loops of edges as points, and its lone vertices.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Loops {
    pub(crate) rings: Vec<Vec<LoopPoint>>,
    pub(crate) lone: Vec<(VertexId, Point2)>,
}

/// Collects a face's loops from its edges' samples.
pub(crate) fn gather(b: &Body, g: &FaceGeom, samples: &[Option<EdgeSamples>]) -> Loops {
    let mut out = Loops::default();
    for l in b.face_loops(g.face) {
        if let Some(v) = b.loops[l].vertex {
            out.lone.push((v, g.invert(b.vertices[v].point).0));
            continue;
        }
        let mut ring = Vec::new();
        for fin in b.loop_fins(l) {
            let e = b.fins[fin].edge;
            let s = samples[e.index()].as_ref().expect("sampled edge");
            let uv = s.uv_on(g.face).expect("edge on face");
            let edge = &b.edges[e];
            let n = s.params.len() - 1;
            let anchor = |k: usize| match k {
                0 => Anchor::Vertex(edge.start),
                k if k == n => Anchor::Vertex(edge.end),
                k => Anchor::Sample(e, k as u32),
            };
            let order: Vec<usize> = match b.fins[fin].sense {
                Sense::Forward => (0..=n).collect(),
                Sense::Reversed => (0..=n).rev().collect(),
            };
            for w in order.windows(2) {
                ring.push(LoopPoint {
                    uv: uv[w[0]],
                    anchor: anchor(w[0]),
                    seg: (e, s.params[w[0]], s.params[w[1]]),
                });
            }
        }
        out.rings.push(ring);
    }
    out
}

/// A union-find over triangulation vertices that must share a position,
/// each class with one anchor.
#[derive(Clone, Debug, Default)]
pub(crate) struct Classes {
    parent: Vec<u32>,
    anchor: Vec<Anchor>,
}

impl Classes {
    fn push(&mut self, a: Anchor) {
        self.parent.push(self.parent.len() as u32);
        self.anchor.push(a);
    }

    pub(crate) fn root(&self, mut v: u32) -> u32 {
        while self.parent[v as usize] != v {
            v = self.parent[v as usize];
        }
        v
    }

    /// The anchor of `v`'s class.
    pub(crate) fn anchor(&self, v: u32) -> Anchor {
        self.anchor[self.root(v) as usize]
    }

    /// Gives `v`'s class the anchor `a` too; fails if it already has a
    /// different body anchor.
    fn mark(&mut self, v: u32, a: Anchor) -> Result<(), BadDomain> {
        let r = self.root(v) as usize;
        match (self.anchor[r], a) {
            (_, Anchor::Free) => Ok(()),
            (Anchor::Free, a) => {
                self.anchor[r] = a;
                Ok(())
            }
            (x, a) if x == a => Ok(()),
            _ => Err(BadDomain),
        }
    }

    /// Joins two classes. The lower root stays the root.
    pub(crate) fn join(&mut self, a: u32, b: u32) -> Result<(), BadDomain> {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra == rb {
            return Ok(());
        }
        let (keep, gone) = (ra.min(rb), ra.max(rb));
        let other = self.anchor[gone as usize];
        self.parent[gone as usize] = keep;
        self.mark(keep, other)
    }
}

/// A face's region, triangulated.
#[derive(Clone, Debug)]
pub(crate) struct Domain {
    pub(crate) cdt: Cdt,
    /// Each triangulation vertex's parameters (unscaled, unwrapped into
    /// the rectangle).
    pub(crate) uv: Vec<Point2>,
    pub(crate) classes: Classes,
    /// Each triangle: inside the face.
    pub(crate) inside: Vec<bool>,
    /// Each loop segment, by its triangulation vertices in either order:
    /// its edge and parameters.
    pub(crate) segments: BTreeMap<[u32; 2], (EdgeId, f64, f64)>,
    /// The rectangle in parameters: `[[u0, u1], [v0, v1]]`.
    pub(crate) rect: [[f64; 2]; 2],
    /// Bottom (`v = v0`), right, top, left.
    pub(crate) sides: [Side; 4],
    /// For a vertex on a seam side, its partner on the opposite side:
    /// `[u seam, v seam]`.
    pub(crate) partner: [Vec<u32>; 2],
}

/// A loop point after unwrapping, possibly split at a pole.
#[derive(Clone, Copy, Debug)]
struct Flat {
    x: [f64; 2],
    anchor: Anchor,
    seg: (EdgeId, f64, f64),
    /// On a seam line, by direction: the line's index (0 at the seam).
    line: [Option<i64>; 2],
    /// The segment to the next point only moves along a collapsed row:
    /// no constraint.
    jump: bool,
}

/// Unwraps a ring next to itself, replacing each point on a collapsed row
/// by its neighbours' free parameters. Returns the points and how many
/// periods the ring winds in each direction.
fn unwrap_ring(g: &FaceGeom, ring: &[LoopPoint]) -> Result<(Vec<Flat>, [i64; 2]), BadDomain> {
    // Start at a point not on a pole.
    let start = ring
        .iter()
        .position(|p| g.pole(p.uv).is_none())
        .ok_or(BadDomain)?;
    let n = ring.len();
    let mut out: Vec<Flat> = Vec::with_capacity(n + 2);
    let mut last = [ring[start].uv.x, ring[start].uv.y];
    let line = |k: usize, x: f64| {
        let (period, seam) = (g.periods[k]?, g.seam(k)?);
        Some(((x - seam) / period).round() as i64)
    };
    let mut poles = Vec::new();
    for i in 0..=n {
        let p = &ring[(start + i) % n];
        let raw = [p.uv.x, p.uv.y];
        let mut x = [0.0; 2];
        for k in 0..2 {
            x[k] = unwrap(raw[k], last[k], g.periods[k]);
        }
        if i == n {
            through_poles(g, &mut out, &poles);
            // Back at the start: the winding.
            let wind: [i64; 2] = core::array::from_fn(|k| match g.periods[k] {
                Some(period) => ((x[k] - out[0].x[k]) / period).round() as i64,
                None => 0,
            });
            // Points on poles take their neighbours' free parameters.
            let mut flat = Vec::with_capacity(out.len() + poles.len());
            for (j, f) in out.iter().enumerate() {
                let Some(&(_, k)) = poles.iter().find(|&&(at, _)| at == j) else {
                    flat.push(*f);
                    continue;
                };
                // The segment arriving ends at the first copy; the one
                // leaving starts at the second.
                let free = 1 - k;
                let before = out[(j + out.len() - 1) % out.len()].x[free];
                let after = out[(j + 1) % out.len()].x[free];
                let mut q = *f;
                q.x[free] = before;
                q.line[free] = None;
                q.jump = before != after;
                flat.push(q);
                if q.jump {
                    q.x[free] = after;
                    q.jump = false;
                    flat.push(q);
                }
            }
            return Ok((flat, wind));
        }
        let on_pole = g.pole(p.uv);
        let seam_line = |k: usize| {
            let seam = g.seam(k)?;
            (raw[k] == seam).then(|| line(k, x[k])).flatten()
        };
        out.push(Flat {
            x,
            anchor: p.anchor,
            seg: p.seg,
            line: [seam_line(U), seam_line(V)],
            jump: false,
        });
        match on_pole {
            Some((k, _)) => poles.push((out.len() - 1, k)),
            None => last = x,
        }
    }
    unreachable!()
}

/// Across a pole a ring's free parameter jumps, so continuity says
/// nothing about which period the stretch after it belongs to: unwrapping
/// takes the nearest, which for a face wider than half a turn between two
/// poles (a lune, such as a sphere's wedge) is its complement. With two
/// poles the stretch between them is moved by whole periods until the
/// ring winds the way the face does: area on its left when the face uses
/// the surface's normal.
fn through_poles(g: &FaceGeom, out: &mut [Flat], poles: &[(usize, usize)]) {
    let [(a, k), (b, k2)] = poles[..] else {
        return;
    };
    let free = 1 - k;
    let Some(period) = g.periods[free].filter(|_| k == k2) else {
        return;
    };
    let area = |pts: &[Flat]| -> f64 {
        let n = pts.len();
        (0..n)
            .map(|i| {
                let (p, q) = (pts[i].x, pts[(i + 1) % n].x);
                p[U] * q[V] - q[U] * p[V]
            })
            .sum::<f64>()
            / 2.0
    };
    let off_pole = |pts: &[Flat]| -> Vec<Flat> {
        pts.iter()
            .enumerate()
            .filter(|&(j, _)| j != a && j != b)
            .map(|(_, p)| *p)
            .collect()
    };
    let shifted = |m: f64| -> Vec<Flat> {
        let mut pts = out.to_vec();
        for p in &mut pts[a + 1..b] {
            p.x[free] += m * period;
        }
        off_pole(&pts)
    };
    let want = if g.forward { 1.0 } else { -1.0 };
    let best = [-1.0, 0.0, 1.0]
        .into_iter()
        .filter(|&m| area(&shifted(m)) * want > 0.0)
        .min_by(|&x, &y| area(&shifted(x)).abs().total_cmp(&area(&shifted(y)).abs()));
    if let Some(m) = best.filter(|&m| m != 0.0) {
        for p in &mut out[a + 1..b] {
            p.x[free] += m * period;
        }
    }
}

/// The strip index of a piece of a ring between seam points, by
/// direction `d`: from an inner point, or for a lone segment from its
/// ends' lines, or for a segment along one line from the side the face
/// is on.
fn strip(g: &FaceGeom, d: usize, piece: &[Flat]) -> Result<i64, BadDomain> {
    let (period, seam) = (
        g.periods[d].expect("periodic"),
        g.seam(d).expect("periodic"),
    );
    let inner = &piece[1..piece.len() - 1];
    let free: Vec<f64> = inner
        .iter()
        .filter(|p| p.line[d].is_none())
        .map(|p| p.x[d])
        .collect();
    if !free.is_empty() {
        // The strip by the middle of the free points' range: one of them
        // may lie exactly on the strip's edge (an apex at the seam).
        let lo = free.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = free.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let m = ((0.5 * (lo + hi) - seam) / period).floor() as i64;
        // Every inner point must be in the same strip.
        let fits = inner.iter().all(|q| {
            let s = (q.x[d] - seam) / period - m as f64;
            (-1e-12..=1.0 + 1e-12).contains(&s)
        });
        return if fits { Ok(m) } else { Err(BadDomain) };
    }
    let (a, b) = (piece[0], piece[piece.len() - 1]);
    let (ma, mb) = (a.line[d].ok_or(BadDomain)?, b.line[d].ok_or(BadDomain)?);
    if ma != mb {
        return Ok(ma.min(mb));
    }
    // Along the line: the face is on the left of the segment when its
    // normal is the surface's.
    let o = 1 - d;
    let rising = b.x[o] > a.x[o];
    // Left of a rising segment is lower u; left of a rightward one is
    // higher v.
    let left_is_lower = if d == U { rising } else { !rising };
    let face_lower = left_is_lower == g.forward;
    Ok(if face_lower { ma - 1 } else { ma })
}

/// A ring that winds round neither period, moved by whole periods into
/// the face's period rectangle, which it must fit without crossing a
/// seam.
fn whole_hole(g: &FaceGeom, mut flat: Vec<Flat>, wind: [i64; 2]) -> Result<Vec<Flat>, BadDomain> {
    if wind != [0, 0] {
        return Err(BadDomain);
    }
    for k in 0..2 {
        fit(g, k, &mut flat)?;
    }
    Ok(flat)
}

/// Moves a ring by whole periods in direction `k` into the face's period
/// there, which it must fit without crossing the seam.
fn fit(g: &FaceGeom, k: usize, flat: &mut [Flat]) -> Result<(), BadDomain> {
    let (Some(p), Some(s)) = (g.periods[k], g.seam(k)) else {
        return Err(BadDomain);
    };
    let lo = flat.iter().map(|q| q.x[k]).fold(f64::INFINITY, f64::min);
    let hi = flat
        .iter()
        .map(|q| q.x[k])
        .fold(f64::NEG_INFINITY, f64::max);
    let shift = ((s - lo) / p).ceil() * p;
    if hi + shift >= s + p {
        return Err(BadDomain);
    }
    for q in flat {
        q.x[k] += shift;
    }
    Ok(())
}

impl Domain {
    /// Builds a face's region from its loops.
    #[cfg(test)]
    pub(crate) fn new(g: &FaceGeom, loops: &Loops) -> Result<Self, BadDomain> {
        Self::with_requests(g, loops, &mut Vec::new())
    }

    /// Builds a face's region from its loops. Where two loop segments
    /// cross (edges of a sliver face too close for their samples), the
    /// samples in the middle of both are pushed to `requests`: with them
    /// the polylines draw apart.
    pub(crate) fn with_requests(
        g: &FaceGeom,
        loops: &Loops,
        requests: &mut Vec<(EdgeId, f64)>,
    ) -> Result<Self, BadDomain> {
        let periodic: Vec<usize> = (0..2).filter(|&k| g.periods[k].is_some()).collect();
        // Pieces: open polylines between seam points (or whole rings),
        // shifted into the period.
        let mut pieces: Vec<(Vec<Flat>, bool)> = Vec::new();
        for ring in &loops.rings {
            let (flat, wind) = unwrap_ring(g, ring)?;
            match periodic[..] {
                [] => pieces.push((flat, true)),
                [d] => pieces.extend(cut(g, d, flat, wind[d])?),
                // Periodic both ways: a hole that fits between the seams
                // (which the face geometry puts between holes where it
                // can) stays whole; a ring that fits between one way's
                // seams (a band round the surface the other way, or a
                // hole the other way's seam can't miss) is cut at the
                // seam it crosses.
                _ => {
                    let fits = |k: usize| wind[k] == 0 && fit(g, k, &mut flat.clone()).is_ok();
                    match (fits(U), fits(V)) {
                        (true, true) => pieces.push((whole_hole(g, flat, wind)?, true)),
                        (true, false) | (false, true) => {
                            let k = if fits(U) { U } else { V };
                            let mut flat = flat;
                            fit(g, k, &mut flat)?;
                            pieces.extend(cut(g, 1 - k, flat, wind[1 - k])?);
                        }
                        (false, false) => {
                            return Err(BadDomain);
                        }
                    }
                }
            }
        }
        let lone: Vec<(VertexId, Point2)> = loops
            .lone
            .iter()
            .map(|&(v, uv)| {
                let mut x = [uv.x, uv.y];
                for (k, x) in x.iter_mut().enumerate() {
                    if let (Some(p), Some(s)) = (g.periods[k], g.seam(k)) {
                        *x = s + (*x - s).rem_euclid(p);
                    }
                }
                (v, Point2::new(x[0], x[1]))
            })
            .collect();
        // The rectangle.
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        let mut reach = |x: [f64; 2]| {
            for k in 0..2 {
                lo[k] = lo[k].min(x[k]);
                hi[k] = hi[k].max(x[k]);
            }
        };
        for (piece, _) in &pieces {
            for p in piece {
                reach(p.x);
            }
        }
        for (_, uv) in &lone {
            reach([uv.x, uv.y]);
        }
        let mut rect = [[0.0; 2]; 2];
        for k in 0..2 {
            if let (Some(p), Some(s)) = (g.periods[k], g.seam(k)) {
                rect[k] = [s, s + p];
                continue;
            }
            for (e, end) in rect[k].iter_mut().enumerate() {
                *end = if g.collapsed[k][e] {
                    g.domain[k][e]
                } else if lo[k] <= hi[k] {
                    let margin =
                        0.05 * (hi[k] - lo[k]) + 1e-6 * (1.0 + lo[k].abs().max(hi[k].abs()));
                    if e == 0 {
                        lo[k] - margin
                    } else {
                        hi[k] + margin
                    }
                } else {
                    return Err(BadDomain);
                };
            }
            if rect[k][0] >= rect[k][1] {
                return Err(BadDomain);
            }
        }
        let side = |k: usize, e: usize| {
            if g.periods[k].is_some() {
                Side::Seam
            } else if g.collapsed[k][e] {
                Side::Pole
            } else {
                Side::Outside
            }
        };
        let sides = [side(V, 0), side(U, 1), side(V, 1), side(U, 0)];
        let m = g.metric;
        let corner_uv = [
            Point2::new(rect[U][0], rect[V][0]),
            Point2::new(rect[U][1], rect[V][0]),
            Point2::new(rect[U][1], rect[V][1]),
            Point2::new(rect[U][0], rect[V][1]),
        ];
        let mut d = Self {
            cdt: Cdt::new(m.scale(corner_uv[0]), m.scale(corner_uv[2])),
            uv: corner_uv.to_vec(),
            classes: Classes::default(),
            inside: Vec::new(),
            segments: BTreeMap::new(),
            rect,
            sides,
            partner: [vec![NONE; 4], vec![NONE; 4]],
        };
        for _ in 0..4 {
            d.classes.push(Anchor::Free);
        }
        // Corners on a seam side pair across it; corners on a pole side
        // join it.
        if sides[3] == Side::Seam {
            d.pair(U, 0, 1)?;
            d.pair(U, 3, 2)?;
        }
        if sides[0] == Side::Seam {
            d.pair(V, 0, 3)?;
            d.pair(V, 1, 2)?;
        }
        for (s, (a, b)) in [(0, 1), (1, 2), (2, 3), (3, 0)].into_iter().enumerate() {
            if sides[s] == Side::Pole {
                d.classes.join(a, b)?;
            }
        }
        // Points, in loop order, then lone vertices. A loop through one
        // vertex twice (a pinch) reaches it from two edges, whose samples
        // put it a rounding apart: both visits take the first's parameters,
        // so they are one point of the triangulation.
        let mut ids: Vec<Vec<u32>> = Vec::new();
        let mut seen: BTreeMap<VertexId, Vec<Point2>> = BTreeMap::new();
        let near = 1e-9 * (rect[U][1] - rect[U][0]).max(rect[V][1] - rect[V][0]);
        for (piece, _) in &pieces {
            let mut row = Vec::with_capacity(piece.len());
            for p in piece {
                let mut uv = Point2::new(p.x[0], p.x[1]);
                if let Anchor::Vertex(v) = p.anchor {
                    let at = seen.entry(v).or_default();
                    match at.iter().find(|q| q.distance(uv) <= near) {
                        Some(&q) => uv = q,
                        None => at.push(uv),
                    }
                }
                row.push(d.add(g, uv, p.anchor)?);
            }
            ids.push(row);
        }
        for &(v, uv) in &lone {
            d.add(g, uv, Anchor::Vertex(v))?;
        }
        // Segments, and the marks each side of them.
        let mut marks: Vec<(u32, u32)> = Vec::new();
        for ((piece, closed), row) in pieces.iter().zip(&ids) {
            let n = piece.len();
            let count = if *closed { n } else { n - 1 };
            for i in 0..count {
                if piece[i].jump {
                    continue;
                }
                let (a, b) = (row[i], row[(i + 1) % n]);
                let seg = piece[i].seg;
                let parts = match d.cdt.insert_segment(a, b) {
                    Ok(parts) => parts,
                    Err(Crossing(hit)) => {
                        let other = hit.and_then(|[x, y]| d.segments.get(&[x.min(y), x.max(y)]));
                        for &(e, t0, t1) in [Some(&seg), other].into_iter().flatten() {
                            requests.push((e, 0.5 * (t0 + t1)));
                        }
                        return Err(BadDomain);
                    }
                };
                for [x, y] in parts {
                    d.segments.insert([x.min(y), x.max(y)], seg);
                    marks.push((x, y));
                }
            }
        }
        d.mark(g, &marks, !loops.rings.is_empty())?;
        Ok(d)
    }

    /// Pairs two vertices across a seam in direction `k`: they share a
    /// position.
    pub(crate) fn pair(&mut self, k: usize, a: u32, b: u32) -> Result<(), BadDomain> {
        self.partner[k][a as usize] = b;
        self.partner[k][b as usize] = a;
        self.classes.join(a, b)
    }

    /// Inserts a vertex at parameters `uv` with an anchor (or finds the
    /// one already there), and its partners across seams and its pole.
    fn add(&mut self, g: &FaceGeom, uv: Point2, anchor: Anchor) -> Result<u32, BadDomain> {
        let v = self.insert(g, uv)?;
        self.classes.mark(v, anchor)?;
        for k in 0..2 {
            if g.periods[k].is_none() {
                continue;
            }
            let x = [uv.x, uv.y][k];
            let [r0, r1] = self.rect[k];
            let other = if x == r0 {
                r1
            } else if x == r1 {
                r0
            } else {
                continue;
            };
            let mut q = [uv.x, uv.y];
            q[k] = other;
            let w = self.insert(g, Point2::new(q[0], q[1]))?;
            self.pair(k, v, w)?;
        }
        self.join_poles(v)?;
        Ok(v)
    }

    /// Joins a vertex on a pole side to that side's class.
    pub(crate) fn join_poles(&mut self, v: u32) -> Result<(), BadDomain> {
        let p = self.uv[v as usize];
        let corner = [(V, 0, 0), (U, 1, 1), (V, 1, 2), (U, 0, 3)];
        for (s, &(k, e, c)) in corner.iter().enumerate() {
            if self.sides[s] == Side::Pole && [p.x, p.y][k] == self.rect[k][e] {
                self.classes.join(c, v)?;
            }
        }
        Ok(())
    }

    /// Inserts a point (or finds the vertex there).
    fn insert(&mut self, g: &FaceGeom, uv: Point2) -> Result<u32, BadDomain> {
        let v = self.cdt.insert(g.metric.scale(uv)).ok_or(BadDomain)?;
        self.grow(v, uv);
        Ok(v)
    }

    /// Records a new vertex's parameters and class.
    pub(crate) fn grow(&mut self, v: u32, uv: Point2) {
        if v as usize == self.uv.len() {
            self.uv.push(uv);
            self.classes.push(Anchor::Free);
            self.partner[0].push(NONE);
            self.partner[1].push(NONE);
        }
    }

    /// Marks each triangle inside or outside: the triangles each side of
    /// each directed loop segment, then across unfixed edges.
    fn mark(
        &mut self,
        g: &FaceGeom,
        marks: &[(u32, u32)],
        has_rings: bool,
    ) -> Result<(), BadDomain> {
        let n = self.cdt.triangle_count();
        let mut label: Vec<Option<bool>> = vec![None; n];
        let mut queue = VecDeque::new();
        let set = |label: &mut Vec<Option<bool>>, t: u32, inside: bool| match label[t as usize] {
            Some(x) if x != inside => Err(BadDomain),
            Some(_) => Ok(false),
            None => {
                label[t as usize] = Some(inside);
                Ok(true)
            }
        };
        for &(a, b) in marks {
            // The triangle left of a → b holds the directed edge a → b.
            for (x, y, inside) in [(a, b, g.forward), (b, a, !g.forward)] {
                if let Some((t, _)) = self.cdt.find_edge(x, y)
                    && set(&mut label, t, inside)?
                {
                    queue.push_back(t);
                }
            }
        }
        while let Some(t) = queue.pop_front() {
            let inside = label[t as usize].expect("labelled");
            for i in 0..3 {
                if self.cdt.is_fixed(t, i) {
                    continue;
                }
                let u = self.cdt.neighbour(t, i);
                if set(&mut label, u, inside)? {
                    queue.push_back(u);
                }
            }
        }
        self.inside = label.into_iter().map(|l| l.unwrap_or(!has_rings)).collect();
        Ok(())
    }
}

/// Cuts an unwrapped ring at its points on the seam lines of direction
/// `d` and shifts each piece into the period. A ring that never touches a
/// seam line stays whole (and closed).
fn cut(
    g: &FaceGeom,
    d: usize,
    flat: Vec<Flat>,
    wind: i64,
) -> Result<Vec<(Vec<Flat>, bool)>, BadDomain> {
    let period = g.periods[d].expect("periodic");
    let shift = |piece: &mut Vec<Flat>, m: i64| {
        for p in piece.iter_mut() {
            p.x[d] -= m as f64 * period;
            if let Some(l) = p.line[d] {
                p.line[d] = Some(l - m);
            }
        }
    };
    let Some(first) = flat.iter().position(|p| p.line[d].is_some()) else {
        if wind != 0 {
            return Err(BadDomain);
        }
        let mut ring = flat;
        let seam = g.seam(d).expect("periodic");
        let m = ((ring[0].x[d] - seam) / period).floor() as i64;
        shift(&mut ring, m);
        let inside = ring.iter().all(|p| p.x[d] > seam && p.x[d] < seam + period);
        return if inside {
            Ok(vec![(ring, true)])
        } else {
            Err(BadDomain)
        };
    };
    // Start at a seam point and close the ring there. Points past the
    // ring's end come round again, moved by its winding.
    let n = flat.len();
    let rotated: Vec<Flat> = (0..=n)
        .map(|i| {
            let mut p = flat[(first + i) % n];
            if first + i >= n {
                p.x[d] += wind as f64 * period;
                p.line[d] = p.line[d].map(|l| l + wind);
            }
            p
        })
        .collect();
    let cuts: Vec<usize> = (0..rotated.len())
        .filter(|&i| rotated[i].line[d].is_some())
        .collect();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let mut piece = rotated[w[0]..=w[1]].to_vec();
        let m = strip(g, d, &piece)?;
        shift(&mut piece, m);
        if !piece.iter().all(|p| {
            let line = p.line[d];
            line.is_none() || line == Some(0) || line == Some(1)
        }) {
            return Err(BadDomain);
        }
        // The seam points are now exactly on the rectangle's sides.
        let seam = g.seam(d).expect("periodic");
        for p in piece.iter_mut() {
            if let Some(l) = p.line[d] {
                p.x[d] = if l == 0 { seam } else { seam + period };
            }
        }
        out.push((piece, false));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rigid;
    use crate::mesh::edge::discretize;
    use crate::mesh::edge::tests::{geoms, targets, turned_seam};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::topo::FaceId;
    use core::f64::consts::{PI, TAU};

    /// The domains of every face of a body, with the area each covers in
    /// its parameters.
    fn domains(b: &Body, chord: f64) -> Vec<(FaceId, Result<Domain, BadDomain>, f64)> {
        let g = geoms(b);
        let t = targets(chord, 0.5);
        let mut samples = vec![None; b.edges.slot_count()];
        for e in b.edges() {
            samples[e.index()] = Some(discretize(b, e, &g, &t, &[]));
        }
        b.faces()
            .map(|f| {
                let geom = g[f.index()].as_ref().unwrap();
                let d = Domain::new(geom, &gather(b, geom, &samples));
                let area = d.as_ref().map_or(0.0, |d| d.area());
                (f, d, area)
            })
            .collect()
    }

    impl Domain {
        /// The parameter area of the inside triangles.
        fn area(&self) -> f64 {
            (0..self.cdt.triangle_count() as u32)
                .filter(|&t| self.inside[t as usize])
                .map(|t| {
                    let [a, b, c] = self.cdt.corners(t).map(|v| self.uv[v as usize]);
                    (b - a).cross(c - a) / 2.0
                })
                .sum()
        }
    }

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn a_block() {
        let (b, _) = primitive::block(OpKey(1), 1.0, 2.0, 3.0, &Rigid::IDENTITY).unwrap();
        let mut total = 0.0;
        for (_, d, area) in domains(&b, 1e-3) {
            let d = d.unwrap();
            d.cdt.validate();
            assert_eq!(d.sides, [Side::Outside; 4]);
            total += area;
        }
        // 2 (1·2 + 2·3 + 1·3) in plane coordinates, which are meters.
        assert!(near(total, 22.0, 1e-12), "{total}");
    }

    #[test]
    fn a_cylinder() {
        let (b, _) = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let ds = domains(&b, 1e-3);
        let side = &ds[0];
        let d = side.1.as_ref().unwrap();
        d.cdt.validate();
        assert_eq!(
            d.sides,
            [Side::Outside, Side::Seam, Side::Outside, Side::Seam]
        );
        assert_eq!(d.rect[U], [0.0, TAU]);
        // The whole band between the two circles.
        assert!(near(side.2, TAU * 2.0, 1e-12), "{}", side.2);
        // The disks: polygons inscribed in a unit circle.
        for (_, _, area) in &ds[1..] {
            assert!(near(*area, PI, 2e-3) && *area < PI, "{area}");
        }
    }

    #[test]
    fn a_cylinder_whose_seam_is_turned() {
        let b = turned_seam(1.0);
        let ds = domains(&b, 1e-3);
        let d = ds[0].1.as_ref().unwrap();
        d.cdt.validate();
        assert!(near(ds[0].2, TAU * 2.0, 1e-12), "{}", ds[0].2);
    }

    #[test]
    fn a_sphere_and_a_torus() {
        let (b, _) = primitive::sphere(OpKey(1), 2.0, &Rigid::IDENTITY).unwrap();
        let ds = domains(&b, 1e-3);
        let d = ds[0].1.as_ref().unwrap();
        assert_eq!(d.sides, [Side::Pole, Side::Seam, Side::Pole, Side::Seam]);
        assert!(near(ds[0].2, TAU * PI, 1e-12));
        // The corners: two poles.
        assert_eq!(d.classes.root(1), 0);
        assert_eq!(d.classes.root(2), 2);
        assert_eq!(d.classes.root(3), 2);
        let (b, _) = primitive::torus(OpKey(1), 3.0, 1.0, &Rigid::IDENTITY).unwrap();
        let ds = domains(&b, 1e-3);
        let d = ds[0].1.as_ref().unwrap();
        assert_eq!(d.sides, [Side::Seam; 4]);
        assert!(near(ds[0].2, TAU * TAU, 1e-12));
        // All four corners are one point.
        assert!((0..4).all(|c| d.classes.root(c) == 0));
        assert_eq!(d.partner[U][0], 1);
        assert_eq!(d.partner[V][0], 3);
    }

    #[test]
    fn a_cone_reaches_its_apex() {
        let (b, _) = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let ds = domains(&b, 1e-3);
        let d = ds[0].1.as_ref().unwrap();
        d.cdt.validate();
        assert_eq!(d.sides, [Side::Pole, Side::Seam, Side::Outside, Side::Seam]);
        // From the apex at v = -2 to the base at v = 0, all the way round.
        assert_eq!(d.rect[V][0], -2.0);
        assert!(near(ds[0].2, TAU * 2.0, 1e-12), "{}", ds[0].2);
        // The apex is the lone vertex.
        let apex = b
            .vertices()
            .find(|&v| b.vertices[v].edge.is_none())
            .unwrap();
        assert_eq!(d.classes.anchor(0), Anchor::Vertex(apex));
        assert_eq!(d.classes.anchor(1), Anchor::Vertex(apex));
    }

    #[test]
    fn a_nurbs_cone_reaches_its_collapsed_row() {
        use crate::geom::Surface;
        use std::sync::Arc;
        let (mut b, _) = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let f = b.faces().next().unwrap();
        let s = b.faces[f].surface;
        let (n, _) = b.surfaces[s].to_nurbs([[0.0, 0.0], [-2.0, 0.0]]).unwrap();
        let id = b.add_surface(Surface::Nurbs(Arc::new(n)));
        crate::euler::set_surface(&mut b, f, id, Sense::Forward).unwrap();
        let ds = domains(&b, 1e-3);
        let d = ds[0].1.as_ref().unwrap();
        d.cdt.validate();
        assert_eq!(d.sides, [Side::Pole, Side::Seam, Side::Outside, Side::Seam]);
        assert!(near(ds[0].2, TAU * 2.0, 1e-9), "{}", ds[0].2);
    }

    #[test]
    fn frustums_either_way() {
        for (r0, r1) in [(1.0, 0.5), (0.5, 1.0)] {
            let (b, _) = primitive::frustum(OpKey(1), r0, r1, 2.0, &Rigid::IDENTITY).unwrap();
            for (_, d, area) in domains(&b, 1e-3) {
                d.unwrap().cdt.validate();
                assert!(area > 0.0);
            }
        }
    }
}
