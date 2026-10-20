//! Splitting a face into pieces in its parameter plane.
//!
//! A face's arcs are its boundary's global edges (each run one way, as the
//! face's loops run it) and the global edges inside it (run both ways),
//! all as pcurves, from the intersection table. At each vertex the arcs
//! leaving it are ordered by angle about the face's outward normal (the
//! surface's directions, so a pole or an apex, where every arc leaves at
//! one point of the parameter plane, orders them too); arcs leaving within
//! [`SAME_DIRECTION`] of one direction are tangent, and are ordered by
//! where they are a little way out.
//! Walking always turns the same way (arriving at a vertex, take the
//! first arc clockwise from the one arrived by) gives the cycles that
//! bound pieces, each piece on the left of its cycles as the face is on
//! the left of its loops.
//!
//! Which cycles bound one piece is decided by their lifts to the
//! parameter plane: a cycle that closes there (winding zero) is an outer
//! boundary when it runs counterclockwise, else a hole, which belongs to
//! the smallest outer boundary around it (certified winding numbers); a
//! cycle that winds once round a periodic direction is the bottom or top
//! of a band, ordered by where cycles cross a line across the band.
//! Holes and lone vertices outside every outer boundary go to the band
//! they lie in.

use crate::boolean::table::{Boundary, GeId, GvId, Table};
use crate::boolean::{FaceRef, Fail};
use crate::geom::bernstein::{Bernstein, Root};
use crate::geom::chain2::Chain2;
use crate::geom::winding2::winding;
use crate::geom::{Curve2, Point2, Surface, Vector2, Vector3};
use crate::interval::Interval;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, Sense};
use core::cmp::Ordering;
use rayon::prelude::*;

/// A piece of a face: its loops, as global edges.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Piece {
    pub(crate) face: FaceRef,
    pub(crate) loops: Vec<Boundary>,
}

/// Every face of every operand split into pieces, in face order.
pub(crate) fn split_all(ops: &[Body], table: &Table) -> Result<Vec<Piece>, Fail> {
    let faces: Vec<FaceRef> = table.faces.keys().copied().collect();
    let found: Vec<Vec<Piece>> = faces
        .par_iter()
        .map(|&f| split(ops, table, f))
        .collect::<Result<_, _>>()?;
    Ok(found.into_iter().flatten().collect())
}

/// A face's pieces.
pub(crate) fn split(ops: &[Body], table: &Table, f: FaceRef) -> Result<Vec<Piece>, Fail> {
    let work = &table.faces[&f];
    if work.interior.is_empty() && work.nodes.is_empty() {
        return Ok(vec![Piece {
            face: f,
            loops: work.loops.clone(),
        }]);
    }
    let body = &ops[f.op];
    let face = &body.faces[f.face];
    let surface = &body.surfaces[face.surface];
    let sigma = if face.sense == Sense::Forward {
        1.0
    } else {
        -1.0
    };
    let uncertain = || Fail::Uncertain(f, f);
    let mut darts: Vec<Dart> = Vec::new();
    let mut lone: Vec<GvId> = work.nodes.clone();
    for l in &work.loops {
        match l {
            Boundary::Lone(g) => lone.push(*g),
            Boundary::Arcs(arcs) => {
                for &(ge, sense) in arcs {
                    darts.push(Dart::new(table, f, ge, sense, None));
                }
            }
        }
    }
    for &ge in &work.interior {
        let a = darts.len();
        darts.push(Dart::new(table, f, ge, Sense::Forward, Some(a + 1)));
        darts.push(Dart::new(table, f, ge, Sense::Reversed, Some(a)));
    }
    let arr = Arrangement {
        table,
        surface,
        sigma,
        darts: &darts,
    };
    let cycles = arr.walk().ok_or_else(uncertain)?;
    arr.group(f, &cycles, &lone).ok_or_else(uncertain)
}

/// One way along an arc: a global edge, the sense it is run, its end
/// vertices and its pcurve from start to end.
#[derive(Clone, Debug)]
struct Dart {
    ge: GeId,
    sense: Sense,
    from: GvId,
    to: GvId,
    curve: Curve2,
    /// Pcurve parameters at `from` and at `to`.
    s: [f64; 2],
    /// The other way along an arc inside the face.
    twin: Option<usize>,
}

impl Dart {
    fn new(table: &Table, f: FaceRef, ge: GeId, sense: Sense, twin: Option<usize>) -> Self {
        let e = &table.edges[ge];
        let (c, map) = &e.pcurves[&f];
        let s = e.interval.map(|t| map[0] + map[1] * t);
        let (from, to, s) = match sense {
            Sense::Forward => (e.start, e.end, s),
            Sense::Reversed => (e.end, e.start, [s[1], s[0]]),
        };
        Self {
            ge,
            sense,
            from,
            to,
            curve: c.clone(),
            s,
            twin,
        }
    }

    fn uv(&self, end: usize) -> Point2 {
        self.curve.point(self.s[end])
    }

    /// The direction of travel in the parameter plane at an end.
    fn tangent(&self, end: usize) -> Vector2 {
        let dir = if self.s[1] >= self.s[0] { 1.0 } else { -1.0 };
        let d = self.curve.eval(self.s[end]).d1 * dir;
        if d.length() > 0.0 {
            return d;
        }
        // A pcurve that stops at an end: the chord a little way in.
        let k = if end == 0 { 1e-6 } else { 1.0 - 1e-6 };
        let q = self.curve.point(self.s[0] + (self.s[1] - self.s[0]) * k);
        if end == 0 {
            q - self.uv(0)
        } else {
            self.uv(1) - q
        }
    }

    /// A point a fraction `k` of the way along the dart.
    fn at(&self, k: f64) -> Point2 {
        self.curve.point(self.s[0] + (self.s[1] - self.s[0]) * k)
    }

    fn chain(&self) -> Chain2 {
        Chain2::new(&self.curve, self.s[0], self.s[1])
    }
}

/// One end of an arc at a vertex: the dart leaving the vertex along it
/// and the dart arriving along it, either of which may be missing (a
/// boundary arc runs one way only).
#[derive(Clone, Copy, Debug)]
struct Slot {
    out: Option<usize>,
    arrive: Option<usize>,
}

/// The slots at each vertex in order, and each dart's slot at its start
/// and at its end.
type Slots = (Vec<Vec<Slot>>, Vec<[usize; 2]>);

struct Arrangement<'a> {
    table: &'a Table,
    surface: &'a Surface,
    sigma: f64,
    darts: &'a [Dart],
}

/// How a vertex is reached in the parameter plane: at a point, or (at a
/// sphere's pole or a cone's apex) anywhere along a line of `u`, the
/// `sign` saying which way along it clockwise turns about the face's
/// normal go.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Node {
    Point,
    Pole { sign: f64 },
}

impl Arrangement<'_> {
    /// Whether a vertex is a pole of the surface.
    fn node(&self, g: GvId) -> Node {
        let v = &self.table.vertices[g];
        let reach = v.tolerance.max(LINEAR_RESOLUTION) * 4.0;
        match self.surface {
            Surface::Sphere(s) => {
                let f = self.surface.frame().expect("analytic");
                for sign in [1.0, -1.0] {
                    let pole = f.origin() + f.z().get() * (s.radius() * sign);
                    if pole.distance(v.point) <= reach {
                        // The outward normal at the pole is along `sign z`.
                        return Node::Pole {
                            sign: -sign * self.sigma,
                        };
                    }
                }
                Node::Point
            }
            Surface::Cone(c) => {
                let f = self.surface.frame().expect("analytic");
                let apex = f.origin() + f.z().get() * c.apex_v();
                if apex.distance(v.point) <= reach {
                    // The outward normal at the apex is along `-z`.
                    Node::Pole { sign: self.sigma }
                } else {
                    Node::Point
                }
            }
            _ => Node::Point,
        }
    }

    /// The normal turns are measured about at a dart's end: the face's
    /// outward normal, or at an apex the cone's axis outwards.
    fn normal(&self, d: &Dart, end: usize) -> Option<Vector3> {
        let g = if end == 0 { d.from } else { d.to };
        let uv = d.uv(end);
        if let Some(n) = self.surface.normal(uv.x, uv.y) {
            return Some(n.get() * self.sigma);
        }
        match (self.node(g), self.surface) {
            (Node::Pole { .. }, Surface::Cone(_)) => {
                Some(self.surface.frame()?.z().get() * -self.sigma)
            }
            _ => None,
        }
    }

    /// The direction a dart leaves (`end` 0) or arrives back along (`end`
    /// 1, reversed) its vertex, in the tangent plane about `n`.
    fn direction(&self, d: &Dart, end: usize, n: Vector3) -> Vector2 {
        let uv = d.uv(end);
        let e = self.surface.eval(uv.x, uv.y);
        let t = d.tangent(end);
        let t = if end == 0 { t } else { -t };
        let mut v = e.du * t.x + e.dv * t.y;
        if v.length() == 0.0 {
            // A pole: the chord a little way along.
            let k = if end == 0 { 1e-6 } else { 1.0 - 1e-6 };
            let q = d.at(k);
            v = self.surface.point(q.x, q.y) - e.point;
        }
        project(v, n)
    }

    /// The direction of a point a fraction `k` along a dart from the end
    /// at the vertex, for breaking ties.
    fn chord(&self, d: &Dart, end: usize, k: f64, n: Vector3) -> Vector2 {
        let uv = d.uv(end);
        let at = if end == 0 { d.at(k) } else { d.at(1.0 - k) };
        let v = self.surface.point(at.x, at.y) - self.surface.point(uv.x, uv.y);
        project(v, n)
    }

    /// The slots at every vertex, sorted counterclockwise about the
    /// normal, with each dart's slot at its end.
    fn slots(&self) -> Option<Slots> {
        let nv = self.table.vertices.len();
        let mut at: Vec<Vec<(Slot, usize, usize)>> = vec![Vec::new(); nv];
        let mut placed = vec![[usize::MAX; 2]; self.darts.len()];
        // Each arc end once: a dart's start is a slot with it leaving; its
        // twin (if any) arrives there.
        for (i, d) in self.darts.iter().enumerate() {
            at[d.from].push((
                Slot {
                    out: Some(i),
                    arrive: d.twin,
                },
                i,
                0,
            ));
            if d.twin.is_none() {
                at[d.to].push((
                    Slot {
                        out: None,
                        arrive: Some(i),
                    },
                    i,
                    1,
                ));
            }
        }
        let mut sorted: Vec<Vec<Slot>> = vec![Vec::new(); nv];
        for (g, list) in at.into_iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let (d0, e0) = (list[0].1, list[0].2);
            let n = self.normal(&self.darts[d0], e0)?;
            let dirs: Vec<Vector2> = list
                .iter()
                .map(|&(_, d, end)| self.direction(&self.darts[d], end, n))
                .collect();
            // Directions within SAME_DIRECTION of each other are tied at
            // first order: groups of them, in order of angle, each group
            // ordered a little way out.
            let angle =
                |v: Vector2| crate::math::atan2(v.y, v.x).rem_euclid(core::f64::consts::TAU);
            let mut by_angle: Vec<(f64, usize)> = dirs
                .iter()
                .enumerate()
                .map(|(i, &d)| (angle(d), i))
                .collect();
            by_angle.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            let m = by_angle.len();
            let close = |a: f64, b: f64| {
                let d = (a - b).abs();
                d.min(core::f64::consts::TAU - d) <= SAME_DIRECTION
            };
            // Start the groups after a gap, so no group straddles the wrap.
            let start = (0..m)
                .find(|&i| !close(by_angle[(i + m - 1) % m].0, by_angle[i].0))
                .unwrap_or(0);
            let mut groups: Vec<Vec<usize>> = Vec::new();
            for step in 0..m {
                let (a, i) = by_angle[(start + step) % m];
                match groups.last_mut() {
                    Some(last)
                        if step > 0 && close(angle(dirs[*last.last().expect("a member")]), a) =>
                    {
                        last.push(i)
                    }
                    _ => groups.push(vec![i]),
                }
            }
            let mut failed = false;
            for group in &mut groups {
                group.sort_by(|&a, &b| {
                    for k in [1e-4, 1e-3, 1e-2, 1e-1, 0.3] {
                        let (da, ea) = (list[a].1, list[a].2);
                        let (db, eb) = (list[b].1, list[b].2);
                        let ca = self.chord(&self.darts[da], ea, k, n);
                        let cb = self.chord(&self.darts[db], eb, k, n);
                        // Both near one direction: compare by the turn
                        // from one to the other.
                        let c = ca.cross(cb);
                        if c.abs() > SAME_DIRECTION * ca.length() * cb.length() {
                            return if c > 0.0 {
                                Ordering::Less
                            } else {
                                Ordering::Greater
                            };
                        }
                    }
                    failed |= a != b;
                    a.cmp(&b)
                });
            }
            if failed {
                return None;
            }
            let order: Vec<usize> = groups.into_iter().flatten().collect();
            for (k, &o) in order.iter().enumerate() {
                let (slot, d, end) = list[o];
                sorted[g].push(slot);
                placed[d][end] = k;
                if let (0, Some(t)) = (end, slot.arrive) {
                    placed[t][1] = k;
                }
            }
        }
        Some((sorted, placed))
    }

    /// The cycles of darts, each piece on their left.
    fn walk(&self) -> Option<Vec<Vec<usize>>> {
        let (slots, placed) = self.slots()?;
        let mut used = vec![false; self.darts.len()];
        let mut cycles = Vec::new();
        for start in 0..self.darts.len() {
            if used[start] {
                continue;
            }
            let mut cycle = Vec::new();
            let mut d = start;
            loop {
                if used[d] {
                    return None;
                }
                used[d] = true;
                cycle.push(d);
                let v = self.darts[d].to;
                let list = &slots[v];
                let k = placed[d][1];
                let n = list.len();
                // The first slot clockwise that leaves, else turn back.
                let mut next = None;
                for step in 1..n {
                    if let Some(o) = list[(k + n - step) % n].out {
                        next = Some(o);
                        break;
                    }
                }
                let next = next.or(list[k].out)?;
                if next == start {
                    break;
                }
                d = next;
                if cycle.len() > self.darts.len() {
                    return None;
                }
            }
            cycles.push(cycle);
        }
        Some(cycles)
    }

    /// The parameter-plane shift that carries dart `next`'s start on from
    /// where dart `prev` ended, passing along a pole's line by the turn.
    fn carry(&self, prev: &Dart, end: Point2, next: &Dart) -> Vector2 {
        let start = next.uv(0);
        let (pu, pv) = self.surface.periods();
        let snap = |gap: f64, p: Option<f64>| match p {
            Some(p) => (gap / p).round() * p,
            None => 0.0,
        };
        match self.node(prev.to) {
            Node::Point => Vector2::new(snap(end.x - start.x, pu), snap(end.y - start.y, pv)),
            Node::Pole { sign } => {
                let n = self.normal(prev, 1).unwrap_or(Vector3::new(0.0, 0.0, 1.0));
                let back = self.direction(prev, 1, n);
                let out = self.direction(next, 0, n);
                // The clockwise turn from the way back to the way out.
                let a = crate::math::atan2(back.y, back.x);
                let b = crate::math::atan2(out.y, out.x);
                let mut turn = (a - b).rem_euclid(core::f64::consts::TAU);
                if turn == 0.0 {
                    turn = core::f64::consts::TAU;
                }
                let gap = sign * turn;
                Vector2::new(end.x + gap - start.x, snap(end.y - start.y, pv))
            }
        }
    }

    /// A cycle's darts lifted into one continuous path: their chains,
    /// moved, and the shift that closes it (whole periods it winds, plus
    /// nothing else).
    fn lift(&self, cycle: &[usize]) -> (Vec<Chain2>, Vector2) {
        let mut chains = Vec::with_capacity(cycle.len());
        let first = self.darts[cycle[0]].uv(0);
        let mut end = first;
        for (k, &d) in cycle.iter().enumerate() {
            let dart = &self.darts[d];
            let offset = if k == 0 {
                Vector2::ZERO
            } else {
                self.carry(&self.darts[cycle[k - 1]], end, dart)
            };
            chains.push(moved(&dart.chain(), offset));
            end = dart.uv(1) + offset;
        }
        let last = &self.darts[cycle[cycle.len() - 1]];
        let close = self.carry(last, end, &self.darts[cycle[0]]);
        (chains, close)
    }

    /// Groups the cycles into pieces, and places the lone vertices.
    fn group(&self, f: FaceRef, cycles: &[Vec<usize>], lone: &[GvId]) -> Option<Vec<Piece>> {
        let (pu, pv) = self.surface.periods();
        let mut lifted = Vec::new();
        for c in cycles {
            let (chains, shift) = self.lift(c);
            let wind = [
                pu.map_or(0, |p| (shift.x / p).round() as i64),
                pv.map_or(0, |p| (shift.y / p).round() as i64),
            ];
            let area = signed_area(&chains) * self.sigma;
            lifted.push(Lifted { chains, wind, area });
        }
        let arcs_of = |c: &[usize]| -> Boundary {
            Boundary::Arcs(
                c.iter()
                    .map(|&d| (self.darts[d].ge, self.darts[d].sense))
                    .collect(),
            )
        };
        // The direction of the bands: u for cycles winding round u.
        let winding: Vec<usize> = (0..lifted.len())
            .filter(|&i| lifted[i].wind != [0, 0])
            .collect();
        let along_u = winding.iter().all(|&i| lifted[i].wind[1] == 0);
        let along_v = winding.iter().all(|&i| lifted[i].wind[0] == 0);
        if !winding.is_empty() && !along_u && !along_v {
            return None;
        }
        let swap = !winding.is_empty() && !along_u;
        let mut pieces: Vec<Piece> = Vec::new();
        // Outer boundaries: counterclockwise cycles that close.
        let outers: Vec<usize> = (0..lifted.len())
            .filter(|&i| lifted[i].wind == [0, 0] && lifted[i].area > 0.0)
            .collect();
        let holes: Vec<usize> = (0..lifted.len())
            .filter(|&i| lifted[i].wind == [0, 0] && lifted[i].area <= 0.0)
            .collect();
        for &o in &outers {
            if lifted[o].area.abs() < 1e-24 {
                return None;
            }
            pieces.push(Piece {
                face: f,
                loops: vec![arcs_of(&cycles[o])],
            });
        }
        // Bands between winding cycles, sorted by height.
        let bands = self.bands(&lifted, &winding, swap)?;
        let band_base = pieces.len();
        for b in &bands {
            pieces.push(Piece {
                face: f,
                loops: b.iter().map(|&i| arcs_of(&cycles[i])).collect(),
            });
        }
        // A face with no winding cycles on a closed surface has one region
        // outside every outer boundary.
        let whole = winding.is_empty() && self.surface.closed_genus().is_some();
        let whole_index = pieces.len();
        if whole {
            pieces.push(Piece {
                face: f,
                loops: Vec::new(),
            });
        }
        // Holes and lone vertices go to the smallest outer boundary around
        // them, else to their band.
        let ges = |c: &[usize]| -> Vec<GeId> { c.iter().map(|&d| self.darts[d].ge).collect() };
        // A hole that shares an arc with an outer boundary is across that
        // arc from it: outside it.
        let place = |p: Point2, shared: &[GeId]| -> Option<usize> {
            let mut best: Option<(f64, usize)> = None;
            for (k, &o) in outers.iter().enumerate() {
                if ges(&cycles[o]).iter().any(|g| shared.contains(g)) {
                    continue;
                }
                if self.inside(p, &lifted[o].chains)? {
                    let a = lifted[o].area;
                    if best.is_none_or(|(b, _)| a < b) {
                        best = Some((a, k));
                    }
                }
            }
            if let Some((_, k)) = best {
                return Some(k);
            }
            if whole {
                return Some(whole_index);
            }
            let b = self.band_of(p, &lifted, &bands, swap)?;
            Some(band_base + b)
        };
        for &h in &holes {
            let p = self.sample(&lifted[h])?;
            let k = place(p, &ges(&cycles[h]))?;
            pieces[k].loops.push(arcs_of(&cycles[h]));
        }
        for &g in lone {
            let v = &self.table.vertices[g];
            let (u, w) = self.surface.project(v.point).param;
            let mut p = Point2::new(u, w);
            if matches!(self.node(g), Node::Pole { .. }) {
                // A pole or an apex: a point just inside the face's side,
                // on a line of u clear of every vertex.
                let [_, [v0, v1]] = self.surface.domain();
                let all: Vec<usize> = (0..lifted.len()).collect();
                let u = match self.surface.periods().0 {
                    Some(per) => self.across(&lifted, &all, false, per).unwrap_or(u),
                    None => u,
                };
                p = Point2::new(
                    u,
                    if (w - v0).abs() < (w - v1).abs() {
                        w + 1e-6
                    } else {
                        w - 1e-6
                    },
                );
            }
            let k = place(p, &[])?;
            pieces[k].loops.push(Boundary::Lone(g));
        }
        // The region outside every outer boundary may be no region at all.
        if whole && pieces[whole_index].loops.is_empty() {
            pieces.remove(whole_index);
        }
        Some(pieces)
    }

    /// The bands: for each winding cycle with the band on its left above
    /// it, that cycle and the next one up, which must have the band on its
    /// left below it. On a sphere (and at a cone's apex) the band past the
    /// last cycle reaches the pole; on a torus the bands go round.
    fn bands(&self, lifted: &[Lifted], winding: &[usize], swap: bool) -> Option<Vec<Vec<usize>>> {
        if winding.is_empty() {
            return Some(Vec::new());
        }
        let (pu, pv) = self.surface.periods();
        let period = if swap { pv } else { pu }?;
        // A line across the bands, away from every vertex.
        let across = self.across(lifted, winding, swap, period)?;
        let mut keyed: Vec<(f64, bool, usize)> = Vec::new();
        for &i in winding {
            let hits = crossings(&lifted[i].chains, across, swap, period)?;
            let lowest = hits.into_iter().fold(f64::INFINITY, f64::min);
            if !lowest.is_finite() {
                return None;
            }
            keyed.push((lowest, self.up(&lifted[i], swap), i));
        }
        // Lowest first; at one height a band's top before the next
        // band's bottom.
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let n = keyed.len();
        let mut out = Vec::new();
        if (if swap { pu } else { pv }).is_some() {
            // Round a torus: bands pair a bottom with the next top.
            let start = keyed.iter().position(|x| x.1)?;
            if n % 2 == 1 {
                return None;
            }
            for k in (0..n).step_by(2) {
                let (_, up, a) = keyed[(start + k) % n];
                let (_, up2, b) = keyed[(start + k + 1) % n];
                if !up || up2 {
                    return None;
                }
                out.push(vec![a, b]);
            }
            return Some(out);
        }
        let mut i = 0;
        while i < n {
            let (_, up, c) = keyed[i];
            match (up, keyed.get(i + 1)) {
                (true, Some(&(_, false, t))) => {
                    out.push(vec![c, t]);
                    i += 2;
                }
                (true, Some(_)) => return None,
                (true, None) => {
                    // Up to the top end, if the surface closes there.
                    if !self.closes_at(true) {
                        return None;
                    }
                    out.push(vec![c]);
                    i += 1;
                }
                (false, _) => {
                    // A top with nothing below it: down to the bottom end.
                    if !self.closes_at(false) {
                        return None;
                    }
                    out.push(vec![c]);
                    i += 1;
                }
            }
        }
        Some(out)
    }

    /// Whether a winding cycle has its band above it: on its left, so
    /// above when it winds forwards (seen from outside).
    fn up(&self, l: &Lifted, swap: bool) -> bool {
        let w = if swap { -l.wind[1] } else { l.wind[0] };
        (w as f64) * self.sigma > 0.0
    }

    /// Whether the surface closes up at its top (`true`) or bottom end of
    /// `v`: a sphere at both poles, a cone at its apex.
    fn closes_at(&self, top: bool) -> bool {
        match self.surface {
            Surface::Sphere(_) => true,
            Surface::Cone(_) => !top,
            _ => false,
        }
    }

    /// A line `u = const` (or `v = const` when swapped) away from every
    /// vertex of the winding cycles.
    fn across(&self, lifted: &[Lifted], winding: &[usize], swap: bool, period: f64) -> Option<f64> {
        // Every vertex, and every joint between Bézier pieces.
        let mut at: Vec<f64> = Vec::new();
        for &i in winding {
            for c in &lifted[i].chains {
                for piece in &c.pieces {
                    let p = piece.pts[0].p;
                    let x = if swap { p.y } else { p.x };
                    at.push(x.rem_euclid(period));
                }
            }
        }
        at.sort_by(f64::total_cmp);
        at.dedup();
        if at.is_empty() {
            return Some(0.37 * period);
        }
        // The middle of the widest gap between vertices.
        let mut best = (0.0, 0.0);
        for k in 0..at.len() {
            let a = at[k];
            let b = if k + 1 < at.len() {
                at[k + 1]
            } else {
                at[0] + period
            };
            if b - a > best.0 {
                best = (b - a, (a + b) / 2.0);
            }
        }
        (best.0 > 1e-9 * period).then_some(best.1)
    }

    /// The band a point lies in: above its bottom and below its top.
    fn band_of(
        &self,
        p: Point2,
        lifted: &[Lifted],
        bands: &[Vec<usize>],
        swap: bool,
    ) -> Option<usize> {
        let (pu, pv) = self.surface.periods();
        let period = if swap { pv } else { pu }?;
        let (x0, y) = if swap { (p.y, p.x) } else { (p.x, p.y) };
        // A line through a vertex can't be counted across: one a little
        // either side of it serves, still in the point's band.
        let x = [0.0, 1e-7, -1e-7, 1e-5, -1e-5]
            .into_iter()
            .map(|d| x0 + d * period)
            .find(|&x| {
                bands
                    .iter()
                    .flatten()
                    .all(|&c| crossings(&lifted[c].chains, x, swap, period).is_some())
            })?;
        let above = |c: usize| -> Option<bool> {
            let hits = crossings(&lifted[c].chains, x, swap, period)?;
            Some(hits.iter().filter(|&&h| h < y).count() % 2 == 1)
        };
        let mut found = None;
        for (k, b) in bands.iter().enumerate() {
            let mut inside = true;
            for &c in b {
                // The band is above a bottom cycle and below a top one.
                if above(c)? != self.up(&lifted[c], swap) {
                    inside = false;
                }
            }
            if inside {
                if found.is_some() {
                    return None;
                }
                found = Some(k);
            }
        }
        found
    }

    /// Whether `p` (at any shift by periods) lies inside a closed lifted
    /// cycle.
    fn inside(&self, p: Point2, chains: &[Chain2]) -> Option<bool> {
        let (lo, hi) = bounds(chains);
        let (pu, pv) = self.surface.periods();
        let shifts = |x: f64, lo: f64, hi: f64, per: Option<f64>| -> Vec<f64> {
            match per {
                None => vec![x],
                Some(q) => {
                    let first = ((lo - x) / q).ceil() as i64;
                    let last = ((hi - x) / q).floor() as i64;
                    (first..=last).map(|k| x + k as f64 * q).collect()
                }
            }
        };
        for x in shifts(p.x, lo.x, hi.x, pu) {
            for y in shifts(p.y, lo.y, hi.y, pv) {
                if winding(Vector2::new(x, y), chains)? != 0 {
                    return Some(true);
                }
            }
        }
        Some(false)
    }

    /// A point of a cycle away from its vertices: the middle of its
    /// longest dart.
    fn sample(&self, l: &Lifted) -> Option<Point2> {
        let c = l.chains.iter().max_by(|a, b| {
            let len = |c: &Chain2| (c.hi - c.lo).length();
            len(a).total_cmp(&len(b))
        })?;
        let piece = &c.pieces[c.pieces.len() / 2];
        Some(Point2::ORIGIN + piece.split(0.5)[1].pts[0].p)
    }
}

/// A cycle lifted to the parameter plane.
struct Lifted {
    chains: Vec<Chain2>,
    /// Whole periods it winds round in `u` and `v`.
    wind: [i64; 2],
    /// Its signed area, positive counterclockwise seen from outside.
    area: f64,
}

/// `v` in the tangent plane about unit normal `n`, in a fixed frame.
fn project(v: Vector3, n: Vector3) -> Vector2 {
    let a = if n.x.abs() < 0.6 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let e1 = (a - n * a.dot(n)).normalize().map(|u| u.get()).unwrap_or(a);
    let e2 = n.cross(e1);
    Vector2::new(v.dot(e1), v.dot(e2))
}

/// Directions this close (in angle, or the sine of the angle between
/// chords) are the same: tangents are computed in floating point, so an
/// exact test on them would order tangent arcs by rounding.
const SAME_DIRECTION: f64 = 1e-9;

/// A chain moved by `d`.
fn moved(c: &Chain2, d: Vector2) -> Chain2 {
    let mut c = c.clone();
    if d == Vector2::ZERO {
        return c;
    }
    for p in &mut c.pieces {
        for h in &mut p.pts {
            h.p += d;
        }
    }
    c.lo += d;
    c.hi += d;
    c
}

fn bounds(chains: &[Chain2]) -> (Vector2, Vector2) {
    chains.iter().fold(
        (
            Vector2::new(f64::INFINITY, f64::INFINITY),
            Vector2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        ),
        |(lo, hi), c| {
            (
                Vector2::new(lo.x.min(c.lo.x), lo.y.min(c.lo.y)),
                Vector2::new(hi.x.max(c.hi.x), hi.y.max(c.hi.y)),
            )
        },
    )
}

/// The signed area of a closed path of chains (the gaps between them
/// closed by straight lines), from points along each piece.
fn signed_area(chains: &[Chain2]) -> f64 {
    let mut pts: Vec<Vector2> = Vec::new();
    for c in chains {
        for piece in &c.pieces {
            for k in 0..16 {
                let s = f64::from(k) / 16.0;
                let q = if k == 0 {
                    piece.pts[0].p
                } else {
                    piece.split(s)[1].pts[0].p
                };
                pts.push(q);
            }
        }
        pts.push(c.pieces[c.pieces.len() - 1].pts[c.pieces[c.pieces.len() - 1].pts.len() - 1].p);
    }
    let n = pts.len();
    (0..n).map(|i| pts[i].cross(pts[(i + 1) % n]) / 2.0).sum()
}

/// Where a lifted winding cycle crosses the line `u = x` (or `v = x`
/// when swapped), at any shift by the period: the other parameter at each
/// crossing. `None` when a crossing can't be certified.
fn crossings(chains: &[Chain2], x: f64, swap: bool, period: f64) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    let coord = |v: Vector2| if swap { (v.y, v.x) } else { (v.x, v.y) };
    for c in chains {
        let (lo, hi) = (coord(c.lo).0, coord(c.hi).0);
        let first = ((lo - x) / period).floor() as i64;
        let last = ((hi - x) / period).ceil() as i64;
        for k in first..=last {
            let at = x + k as f64 * period;
            if at < lo || at > hi {
                continue;
            }
            for piece in &c.pieces {
                let (plo, phi) = piece.hull();
                let (a, b) = (coord(plo).0, coord(phi).0);
                if at < a || at > b {
                    continue;
                }
                let coeffs: Vec<Interval> = piece
                    .pts
                    .iter()
                    .map(|h| {
                        (Interval::point(coord(h.p).0) - Interval::point(at)) * Interval::point(h.w)
                    })
                    .collect();
                for r in Bernstein::new(coeffs).roots(1e-13) {
                    let Root::Simple { .. } = r else {
                        return None;
                    };
                    let [s0, s1] = r.range();
                    let s = (s0 + s1) / 2.0;
                    // At a joint both pieces find it: one crossing.
                    let q = if s <= 1e-9 {
                        piece.pts[0].p
                    } else if s >= 1.0 - 1e-9 {
                        piece.pts[piece.pts.len() - 1].p
                    } else {
                        piece.split(s)[1].pts[0].p
                    };
                    let y = coord(q).1;
                    if !out
                        .iter()
                        .any(|&o: &f64| (o - y).abs() <= 1e-9 * (1.0 + y.abs()))
                    {
                        out.push(y);
                    }
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean::table::build;
    use crate::geom::{Point3, Rigid};
    use crate::name::OpKey;
    use crate::primitive;

    fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid::translation(Vector3::new(x, y, z))
    }

    /// The pieces of the first face of operand `op` whose surface passes
    /// `test`, with each piece's loop count.
    fn pieces_of(ops: &[Body], op: usize, test: impl Fn(&Surface) -> bool) -> Vec<usize> {
        let t = build(ops).unwrap();
        let b = &ops[op];
        let face = b
            .faces()
            .find(|&f| test(&b.surfaces[b.faces[f].surface]))
            .expect("a face");
        let mut out: Vec<usize> = split(ops, &t, FaceRef { op, face })
            .unwrap()
            .iter()
            .map(|p| p.loops.len())
            .collect();
        out.sort_unstable();
        out
    }

    fn plane_at(p: Point3, n: Vector3) -> impl Fn(&Surface) -> bool {
        move |s: &Surface| {
            matches!(s, Surface::Plane(_))
                && s.distance(p) < 1e-12
                && s.frame().unwrap().z().get().cross(n).length() < 1e-12
        }
    }

    #[test]
    fn a_rod_through_a_plate() {
        let plate = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let rod = primitive::cylinder(OpKey(2), 0.5, 3.0, &at(2.0, 2.0, -1.0))
            .unwrap()
            .0;
        let ops = [plate, rod];
        let up = Vector3::new(0.0, 0.0, 1.0);
        // The top: a disc, and the square with a round hole.
        assert_eq!(
            pieces_of(&ops, 0, plane_at(Point3::new(0.0, 0.0, 1.0), up)),
            [1, 2]
        );
        assert_eq!(pieces_of(&ops, 0, plane_at(Point3::ORIGIN, up)), [1, 2]);
        // The rod's side: three bands.
        assert_eq!(
            pieces_of(&ops, 1, |s| matches!(s, Surface::Cylinder(_))),
            [2, 2, 2]
        );
        // A side of the plate is whole.
        assert_eq!(
            pieces_of(
                &ops,
                0,
                plane_at(Point3::ORIGIN, Vector3::new(1.0, 0.0, 0.0))
            ),
            [1]
        );
    }

    #[test]
    fn crossing_blocks() {
        let a = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let b = primitive::block(OpKey(2), 2.0, 1.0, 1.0, &at(1.0, 0.5, 0.5))
            .unwrap()
            .0;
        let ops = [a, b];
        let x = Vector3::new(1.0, 0.0, 0.0);
        assert_eq!(
            pieces_of(&ops, 0, plane_at(Point3::new(2.0, 0.0, 0.0), x)),
            [1, 2]
        );
        let y = Vector3::new(0.0, 1.0, 0.0);
        assert_eq!(
            pieces_of(&ops, 1, plane_at(Point3::new(0.0, 0.5, 0.0), y)),
            [1, 1]
        );
    }

    #[test]
    fn spheres_cut_across_and_through_their_poles() {
        let sphere = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let is_sphere = |s: &Surface| matches!(s, Surface::Sphere(_));
        // A slab's top across the sphere: two caps.
        let slab = primitive::block(OpKey(2), 4.0, 4.0, 2.0, &at(-2.0, -2.0, -1.7))
            .unwrap()
            .0;
        assert_eq!(pieces_of(&[sphere.clone(), slab], 0, is_sphere), [1, 1]);
        // A block's side through both poles: two halves.
        let half = primitive::block(OpKey(3), 2.0, 4.0, 4.0, &at(0.0, -2.0, -2.0))
            .unwrap()
            .0;
        assert_eq!(pieces_of(&[sphere.clone(), half], 0, is_sphere), [1, 1]);
        // A small box poking into the side: a disc and the rest.
        let poke = primitive::block(OpKey(4), 1.0, 0.4, 0.4, &at(0.7, -0.2, -0.2))
            .unwrap()
            .0;
        let got = pieces_of(&[sphere, poke], 0, is_sphere);
        assert_eq!(got, [1, 1]);
    }

    #[test]
    fn bands_of_cylinders_cones_and_tori() {
        // A cylinder cut by a tilted slab: three bands.
        let cyl = primitive::cylinder(OpKey(1), 1.0, 4.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let tilt = Rigid::rotation(Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(), 0.3);
        let slab = primitive::block(
            OpKey(2),
            6.0,
            6.0,
            1.0,
            &tilt.then_after(&at(-3.0, -3.0, 1.5)),
        )
        .unwrap()
        .0;
        assert_eq!(
            pieces_of(&[cyl, slab], 0, |s| matches!(s, Surface::Cylinder(_))),
            [2, 2, 2]
        );
        // A cone's tip cut off: the tip keeps the apex.
        let cone = primitive::cone(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let top = primitive::block(OpKey(2), 4.0, 4.0, 2.0, &at(-2.0, -2.0, 1.0))
            .unwrap()
            .0;
        assert_eq!(
            pieces_of(&[cone, top], 0, |s| matches!(s, Surface::Cone(_))),
            [2, 2]
        );
        // A torus cut by a slab through its middle: four bands round the
        // tube.
        let torus = primitive::torus(OpKey(1), 3.0, 1.0, &Rigid::IDENTITY)
            .unwrap()
            .0;
        let slab = primitive::block(OpKey(2), 10.0, 10.0, 0.6, &at(-5.0, -5.0, -0.3))
            .unwrap()
            .0;
        assert_eq!(
            pieces_of(&[torus, slab], 0, |s| matches!(s, Surface::Torus(_))),
            [2, 2, 2, 2]
        );
    }
}
