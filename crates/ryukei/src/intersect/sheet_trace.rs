//! Traced intersections of any surface with an analytic one: certified
//! branch topology in the first surface's parameter plane.
//!
//! The method is [`super::ss_trace`]'s, on the surface's own parameters
//! rather than a patch's, with `F = f ∘ P` (see [`super::pipe`]) in place
//! of a polynomial: `P` is the surface over jets ([`Sheet`]), `f` the
//! analytic surface's implicit form. The plane is cut into a grid of
//! cells (a pipe's spine pieces, at most a 32nd of its domain, by
//! sixteenths of a turn; another surface's pieces one way and the other
//! likewise, a periodic direction in sixteenths, an unbounded one over
//! the range the region reaches), cells whose points can't reach the
//! region are dropped, and each cell is subdivided until it is excluded
//! (`F` of one certified sign), regular (`F_s` or `F_θ` of one sign, so
//! the curve is a graph) or singular (small, and neither). Crossings of
//! cell sides are solved once per piece of a line, the lines where a
//! periodic direction wraps counting as one, and paired into arcs in
//! each regular cell; arcs joined at shared crossings make branches. A
//! cluster of singular cells must hold exactly one critical point of `F`
//! (Krawczyk's test on its gradient, with the Hessian's enclosure) on the
//! curve: there branches meet, or with none, the surfaces touch at a
//! point. Where a crossing falls on a cell's corner the trace is tried
//! again with the grid shifted.

use super::Failure;
use super::implicit::Implicit;
use super::pipe::{Cell, Field, Line, signed};
use super::ss_trace::{Topology, Traced};
use crate::geom::sheet::Sheet;
use crate::geom::{Box3, Curve, Point3, Surface, Vector3};
use crate::interval::Interval;
use crate::tol::LINEAR_RESOLUTION;
use core::f64::consts::{PI, TAU};
use std::collections::{BTreeMap, BTreeSet};

/// Where a cell is split: off the dyadic fractions symmetric problems put
/// their curves through.
const SPLIT: f64 = 0.499_023_437_5;

/// The deepest subdivision of a grid cell.
const MAX_DEPTH: u32 = 16;

/// The most cells a trace may leave before it gives up.
const MAX_LEAVES: usize = 1 << 18;

/// The most singular cells a trace may leave: past this the surfaces
/// touch along a curve (singular all along it), which no shift of the
/// grid settles, so the trace gives up without trying again.
const MAX_SINGULAR: usize = 1 << 12;

/// The shifts of the grid a trace is tried with: a turn in `θ`, and a
/// fraction of a cell along `s` (the whole grid about a closed spine, the
/// cuts between its pieces' ends about a clamped one).
const SHIFTS: [(f64, f64); 3] = [(0.0, 0.0), (0.071_3, 0.137), (0.183_9, 0.311)];

/// The topology of where `surface` (any but an analytic one: a pipe, a
/// NURBS, a swept surface) meets the analytic `other` inside `region`.
/// Fails with `Unsupported` for an `other` with no implicit form, and
/// `Uncertain` where a crossing, a singular point or a pairing can't be
/// certified (a tangency along a curve among them).
pub(crate) fn trace(
    surface: &Surface,
    other: &Surface,
    region: &Box3,
) -> Result<Topology, Failure> {
    let imp = Implicit::of(other).ok_or(Failure::Unsupported)?;
    for shift in SHIFTS {
        let mut tracer = Tracer::new(surface, &imp, region, shift);
        let result = tracer.run();
        if tracer.singular > MAX_SINGULAR {
            return Err(Failure::Uncertain);
        }
        match result {
            Err(Failure::Uncertain) => continue,
            Err(e) => return Err(e),
            Ok(mut topo) => {
                // What lies on a cone's other nappe is no intersection.
                topo.branches
                    .retain(|b| !b.points.iter().any(|&x| imp.beyond_apex(x)));
                topo.points.retain(|&x| !imp.beyond_apex(x));
                return Ok(topo);
            }
        }
    }
    Err(Failure::Uncertain)
}

/// A direction of the grid: its cuts, and where it wraps, if it does.
struct Axis {
    cuts: Vec<f64>,
    wrap: Option<(f64, f64)>,
}

/// One period from `start`, cut into `n` equal parts.
fn turn(start: f64, period: f64, n: u32) -> Axis {
    let cuts: Vec<f64> = (0..=n)
        .map(|k| {
            if k == n {
                start + period
            } else {
                start + period * f64::from(k) / f64::from(n)
            }
        })
        .collect();
    Axis {
        wrap: Some((cuts[0], cuts[cuts.len() - 1])),
        cuts,
    }
}

/// `breaks` (increasing, first and last the ends) with each stretch between
/// two cut into parts at most `most` long, at least two, the inner cuts
/// moved by `along` of a part.
fn spans(breaks: &[f64], most: f64, along: f64) -> Axis {
    let mut cuts = vec![breaks[0]];
    for w in breaks.windows(2) {
        let n = ((w[1] - w[0]) / most).ceil().max(2.0) as usize;
        for i in 1..n {
            cuts.push(w[0] + (w[1] - w[0]) * (i as f64 - 0.5 + along) / n as f64);
        }
        cuts.push(w[1]);
    }
    Axis { cuts, wrap: None }
}

/// Where a curve's pieces meet in its domain, ends included.
fn breaks(c: &Curve) -> Vec<f64> {
    let [d0, d1] = c.domain();
    let mut out: Vec<f64> = match c {
        Curve::Nurbs(n) => n.pieces().iter().map(|p| p.range[0]).chain([d1]).collect(),
        _ => vec![d0, d1],
    };
    out.dedup();
    out
}

/// The least and greatest of `p · w` over a box's corners.
fn reach(b: &Box3, w: Vector3) -> [f64; 2] {
    b.corners()
        .iter()
        .fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], p| {
            let x = p.to_vector().dot(w);
            [lo.min(x), hi.max(x)]
        })
}

/// One direction of a non-pipe surface's grid: a periodic one in
/// sixteenths (at least one cut per piece) from a start moved by
/// `shift`, a bounded one by its pieces, cut as a pipe's spine is.
fn axis(period: Option<f64>, domain: [f64; 2], pieces: &[f64], (shift, along): (f64, f64)) -> Axis {
    match period {
        Some(p) => {
            let n = (pieces.len().saturating_sub(1) as u32).max(16);
            turn(domain[0] + p * shift / TAU, p, n)
        }
        None => spans(pieces, (domain[1] - domain[0]) / 32.0, along),
    }
}

/// The grid of a surface's parameter plane near `region`, moved by
/// `shift`; `None` where a direction can't be bounded.
fn grid(s: &Surface, region: &Box3, (shift, along): (f64, f64)) -> Option<[Axis; 2]> {
    match s {
        Surface::Pipe(pipe) => {
            let [d0, d1] = pipe.spine().domain();
            let most = (d1 - d0) / 32.0;
            let su = if pipe.spine().period().is_some() {
                // The whole grid moved along the closed spine.
                turn(d0 + most * along, d1 - d0, 32)
            } else {
                spans(&breaks(pipe.spine()), most, along)
            };
            Some([su, turn(-PI + shift, TAU, 16)])
        }
        Surface::Nurbs(n) => {
            let [du, dv] = n.domain();
            let (pu, pv) = n.periods();
            let mut us: Vec<f64> = n
                .patches()
                .iter()
                .map(|p| p.range[0][0])
                .chain([du[1]])
                .collect();
            let mut vs: Vec<f64> = n
                .patches()
                .iter()
                .map(|p| p.range[1][0])
                .chain([dv[1]])
                .collect();
            for x in [&mut us, &mut vs] {
                x.sort_by(f64::total_cmp);
                x.dedup();
            }
            Some([
                axis(pu, du, &us, (shift, along)),
                axis(pv, dv, &vs, (shift, along)),
            ])
        }
        Surface::Extrusion(e) => {
            let c = e.curve();
            let d = e.dir().get();
            let (cb, rb) = (reach(&c.bounds(c.domain()), d), reach(region, d));
            let v = [rb[0] - cb[1], rb[1] - cb[0]];
            if v[0] >= v[1] || v[0].is_nan() || v[1].is_nan() {
                return None;
            }
            Some([
                axis(c.period(), c.domain(), &breaks(c), (shift, along)),
                spans(&v, (v[1] - v[0]) / 32.0, along),
            ])
        }
        Surface::Revolution(r) => {
            let c = r.curve();
            Some([
                turn(shift, TAU, 16),
                axis(c.period(), c.domain(), &breaks(c), (shift, along)),
            ])
        }
        _ => None,
    }
}

/// The zones of `surface`'s collapsed rows that the implicit surface
/// passes within [`REACH`] of, inside the region, each with its side cut
/// into the grid. A zone starts as a quarter of the first span beside its
/// row and is halved (60 times at most) until the surface's enclosure over
/// it lies within half the reach of the pole.
fn zones(surface: &Surface, imp: &Implicit, region: &Box3, cuts: [&mut Vec<f64>; 2]) -> Vec<Zone> {
    let rows = crate::mesh::uv::collapsed_rows(surface);
    let domain = surface.domain();
    let sheet = Sheet::of(surface);
    let mut out = Vec::new();
    let mut cuts = cuts;
    for k in 0..2 {
        for e in 0..2 {
            if !rows[k][e] || cuts[k].len() < 2 {
                continue;
            }
            let edge = domain[k][e];
            let free = domain[1 - k];
            let at = |x: f64| {
                if k == 0 {
                    surface.point(edge, x)
                } else {
                    surface.point(x, edge)
                }
            };
            let pole = at(free[0]);
            if !region.grow(REACH).intersects(&Box3::new(pole, pole)) {
                continue;
            }
            let f = imp.value_at(pole).mag();
            if imp.distance_bound(f, imp.local(pole.to_vector())) > REACH {
                continue;
            }
            let c = &cuts[k];
            let first = if e == 0 {
                c[1] - c[0]
            } else {
                c[c.len() - 1] - c[c.len() - 2]
            };
            let mut w = first / 4.0;
            let mut found = None;
            for _ in 0..60 {
                let strip = if e == 0 {
                    [edge, edge + w]
                } else {
                    [edge - w, edge]
                };
                let cell = if k == 0 { [strip, free] } else { [free, strip] };
                if let Some(p) = sheet.jet_box(cell) {
                    let lo = Point3::new(p[0].v.lo(), p[1].v.lo(), p[2].v.lo());
                    let hi = Point3::new(p[0].v.hi(), p[1].v.hi(), p[2].v.hi());
                    let far = Box3::new(lo, hi)
                        .corners()
                        .iter()
                        .map(|q| q.distance(pole))
                        .fold(0.0, f64::max);
                    if far <= REACH / 2.0 {
                        found = Some(strip);
                        break;
                    }
                }
                w /= 2.0;
            }
            let Some(range) = found else {
                continue;
            };
            let side = if e == 0 { range[1] } else { range[0] };
            let c = &mut cuts[k];
            if !c.contains(&side) {
                c.push(side);
                c.sort_by(f64::total_cmp);
            }
            out.push(Zone {
                dir: k,
                range,
                pole,
            });
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Dir {
    /// `s` held, `θ` runs.
    S,
    /// `θ` held, `s` runs.
    T,
}

/// A line of the grid: its direction and where it sits, as bits.
type LineKey = (Dir, u64);

#[derive(Clone, Copy, Debug)]
enum Kind {
    Regular {
        s_graph: bool,
    },
    Singular,
    /// Inside a pole's zone (see [`Zone`]).
    Pole(usize),
}

/// The touch rule's reach: points this close to each other are one.
const REACH: f64 = 100.0 * LINEAR_RESOLUTION;

/// The strip of the parameter plane beside a collapsed row (a pole or an
/// apex) the other surface passes within [`REACH`] of: every point of the
/// surface over it lies within half the reach of the pole, so whatever of
/// the curve enters it reaches the pole by the touch rule. Its cells are
/// never subdivided (`F` vanishes all along the row, where no crossing
/// can be certified); a crossing on its side is where a branch ends, at
/// the pole.
#[derive(Clone, Copy, Debug)]
struct Zone {
    /// The direction the row collapses: 0 for an end of `s`, 1 of `θ`.
    dir: usize,
    range: [f64; 2],
    pole: Point3,
}

#[derive(Clone, Copy, Debug)]
struct Leaf {
    cell: Cell,
    kind: Kind,
}

/// A crossing of the curve with a piece of a line.
#[derive(Clone, Copy, Debug)]
struct Crossing {
    point: Point3,
    /// Where along the line, in the parameter that runs along it.
    along: f64,
}

/// One side of a leaf: its line, where it sits across, and its stretch
/// along.
#[derive(Clone, Copy, Debug)]
struct Side {
    line: LineKey,
    across: f64,
    along: [f64; 2],
}

struct Tracer<'a> {
    field: Field<'a>,
    region: Box3,
    /// Where each direction's grid wraps: its first line and the last,
    /// which is the same line a period on.
    s_wrap: Option<(f64, f64)>,
    t_wrap: Option<(f64, f64)>,
    s_cuts: Vec<f64>,
    t_cuts: Vec<f64>,
    leaves: Vec<Leaf>,
    /// How many of the leaves are singular.
    singular: usize,
    crossings: Vec<Crossing>,
    pieces: BTreeMap<(LineKey, u64, u64), Vec<usize>>,
    /// No grid could be laid over the surface near the region.
    ungridded: bool,
    zones: Vec<Zone>,
}

impl<'a> Tracer<'a> {
    fn new(surface: &'a Surface, imp: &'a Implicit, region: &Box3, shift: (f64, f64)) -> Self {
        let (axes, ungridded) = match grid(surface, region, shift) {
            Some(a) => (a, false),
            None => (
                [
                    Axis {
                        cuts: Vec::new(),
                        wrap: None,
                    },
                    Axis {
                        cuts: Vec::new(),
                        wrap: None,
                    },
                ],
                true,
            ),
        };
        let [mut su, mut tu] = axes;
        let zones = if ungridded {
            Vec::new()
        } else {
            zones(surface, imp, region, [&mut su.cuts, &mut tu.cuts])
        };
        Self {
            field: Field {
                sheet: Sheet::of(surface),
                imp,
            },
            region: region.grow(LINEAR_RESOLUTION),
            s_wrap: su.wrap,
            t_wrap: tu.wrap,
            s_cuts: su.cuts,
            t_cuts: tu.cuts,
            leaves: Vec::new(),
            singular: 0,
            crossings: Vec::new(),
            pieces: BTreeMap::new(),
            ungridded,
            zones,
        }
    }

    /// A line's key, the wrapping lines folded onto the first.
    fn key(&self, dir: Dir, x: f64) -> LineKey {
        let x = match dir {
            Dir::S => match self.s_wrap {
                Some((a, b)) if x == b => a,
                _ => x,
            },
            Dir::T => match self.t_wrap {
                Some((a, b)) if x == b => a,
                _ => x,
            },
        };
        (dir, x.to_bits())
    }

    fn run(&mut self) -> Result<Topology, Failure> {
        if self.ungridded {
            return Err(Failure::Uncertain);
        }
        let (sc, tc) = (self.s_cuts.clone(), self.t_cuts.clone());
        for s in sc.windows(2) {
            for t in tc.windows(2) {
                let cell = [[s[0], s[1]], [t[0], t[1]]];
                if self.reaches(cell) {
                    self.subdivide(cell, 0)?;
                }
            }
        }
        let arcs = self.arcs()?;
        self.link(arcs)
    }

    /// Whether the surface over a cell may reach the region.
    fn reaches(&self, cell: Cell) -> bool {
        let Some(p) = self.field.sheet.jet_box(cell) else {
            return true;
        };
        let b = Box3::new(
            Point3::new(p[0].v.lo(), p[1].v.lo(), p[2].v.lo()),
            Point3::new(p[0].v.hi(), p[1].v.hi(), p[2].v.hi()),
        );
        b.intersects(&self.region)
    }

    fn subdivide(&mut self, cell: Cell, depth: u32) -> Result<(), Failure> {
        if let Some(z) = self
            .zones
            .iter()
            .position(|z| cell[z.dir][0] >= z.range[0] && cell[z.dir][1] <= z.range[1])
        {
            self.leaves.push(Leaf {
                cell,
                kind: Kind::Pole(z),
            });
            return Ok(());
        }
        let [[s0, s1], [t0, t1]] = cell;
        let split = |me: &mut Self| -> Result<(), Failure> {
            let ms = s0 + (s1 - s0) * SPLIT;
            let mt = t0 + (t1 - t0) * SPLIT;
            for s in [[s0, ms], [ms, s1]] {
                for t in [[t0, mt], [mt, t1]] {
                    me.subdivide([s, t], depth + 1)?;
                }
            }
            Ok(())
        };
        let Some(e) = self.field.enclose(cell) else {
            if depth == MAX_DEPTH {
                return Err(Failure::Uncertain);
            }
            return split(self);
        };
        if signed(e.v) {
            return Ok(());
        }
        if self.leaves.len() >= MAX_LEAVES {
            return Err(Failure::Uncertain);
        }
        let (s_graph, t_graph) = (signed(e.d[0]), signed(e.d[1]));
        if s_graph || t_graph {
            self.leaves.push(Leaf {
                cell,
                kind: Kind::Regular { s_graph },
            });
            return Ok(());
        }
        if depth == MAX_DEPTH {
            self.singular += 1;
            if self.singular > MAX_SINGULAR {
                return Err(Failure::Uncertain);
            }
            self.leaves.push(Leaf {
                cell,
                kind: Kind::Singular,
            });
            return Ok(());
        }
        split(self)
    }

    /// A leaf's sides: `s = s0`, `s = s1`, `θ = t0`, `θ = t1`.
    fn sides(&self, l: &Leaf) -> [Side; 4] {
        let [[s0, s1], [t0, t1]] = l.cell;
        [
            Side {
                line: self.key(Dir::S, s0),
                across: s0,
                along: [t0, t1],
            },
            Side {
                line: self.key(Dir::S, s1),
                across: s1,
                along: [t0, t1],
            },
            Side {
                line: self.key(Dir::T, t0),
                across: t0,
                along: [s0, s1],
            },
            Side {
                line: self.key(Dir::T, t1),
                across: t1,
                along: [s0, s1],
            },
        ]
    }

    /// Cuts every line at all the corners on it, solves each piece a
    /// regular leaf needs, and pairs each regular leaf's crossings into
    /// arcs.
    fn arcs(&mut self) -> Result<Vec<[usize; 2]>, Failure> {
        let mut breaks: BTreeMap<LineKey, BTreeSet<u64>> = BTreeMap::new();
        for l in &self.leaves {
            for side in self.sides(l) {
                let set = breaks.entry(side.line).or_default();
                set.insert(side.along[0].to_bits());
                set.insert(side.along[1].to_bits());
            }
        }
        let leaves = self.leaves.clone();
        let mut arcs = Vec::new();
        for l in &leaves {
            let Kind::Regular { s_graph } = l.kind else {
                continue;
            };
            // Each crossing on the leaf's boundary, with its place along
            // the parameter the curve is a graph over (θ in an s-graph).
            let mut found: Vec<(f64, usize)> = Vec::new();
            for (n, side) in self.sides(l).into_iter().enumerate() {
                // Bits don't sort negative parameters: sort the values.
                let mut cuts: Vec<f64> = breaks[&side.line]
                    .iter()
                    .map(|&b| f64::from_bits(b))
                    .filter(|&x| x >= side.along[0] && x <= side.along[1])
                    .collect();
                cuts.sort_by(f64::total_cmp);
                for w in cuts.windows(2) {
                    for c in self.solve(&side, [w[0], w[1]])? {
                        let along = self.crossings[c].along;
                        let key = match (s_graph, n) {
                            (true, 0 | 1) | (false, 2 | 3) => along,
                            _ => side.across,
                        };
                        found.push((key, c));
                    }
                }
            }
            found.sort_by(|x, y| x.0.total_cmp(&y.0));
            if found.len() % 2 == 1 {
                return Err(Failure::Uncertain);
            }
            arcs.extend(found.chunks(2).map(|w| [w[0].1, w[1].1]));
        }
        Ok(arcs)
    }

    /// The crossings on the piece `[a, b]` of a side's line, solved once.
    fn solve(&mut self, side: &Side, [a, b]: [f64; 2]) -> Result<Vec<usize>, Failure> {
        let key = (side.line, a.to_bits(), b.to_bits());
        if let Some(found) = self.pieces.get(&key) {
            return Ok(found.clone());
        }
        let at = f64::from_bits(side.line.1);
        let line = match side.line.0 {
            Dir::S => Line::S(at),
            Dir::T => Line::T(at),
        };
        let mut found = Vec::new();
        for x in self.field.roots(line, [a, b])? {
            let (s, t) = line.at(x);
            self.crossings.push(Crossing {
                point: self.field.point(s, t),
                along: x,
            });
            found.push(self.crossings.len() - 1);
        }
        self.pieces.insert(key, found.clone());
        Ok(found)
    }

    /// Joins arcs at shared crossings into branches, and settles the
    /// singular clusters they end at.
    fn link(&self, arcs: Vec<[usize; 2]>) -> Result<Topology, Failure> {
        let n = self.crossings.len();
        let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (k, &[a, b]) in arcs.iter().enumerate() {
            next[a].push(k);
            next[b].push(k);
        }
        if next.iter().any(|v| v.len() > 2) {
            return Err(Failure::Uncertain);
        }
        let clusters = self.clusters()?;
        let mut at_singular: BTreeMap<usize, usize> = BTreeMap::new();
        for (ci, (members, _)) in clusters.iter().enumerate() {
            for &li in members {
                for side in self.sides(&self.leaves[li]) {
                    for (&(line, lo, hi), found) in &self.pieces {
                        let inside = f64::from_bits(lo) >= side.along[0]
                            && f64::from_bits(hi) <= side.along[1];
                        if line == side.line && inside {
                            for &c in found {
                                at_singular.insert(c, ci);
                            }
                        }
                    }
                }
            }
        }
        let mut used = vec![false; arcs.len()];
        let mut out = Topology::default();
        let mut touched = vec![false; clusters.len()];
        let walk = |start: usize, first: usize, used: &mut Vec<bool>| -> (Vec<usize>, bool) {
            let mut chain = vec![start];
            let (mut at, mut arc) = (start, first);
            loop {
                used[arc] = true;
                let [a, b] = arcs[arc];
                at = if a == at { b } else { a };
                if at == start {
                    return (chain, true);
                }
                chain.push(at);
                match next[at].iter().find(|&&k| !used[k]) {
                    Some(&k) => arc = k,
                    None => return (chain, false),
                }
            }
        };
        let ends: Vec<usize> = (0..n).filter(|&c| next[c].len() == 1).collect();
        for c in ends {
            let k = next[c][0];
            if used[k] {
                continue;
            }
            let (chain, _) = walk(c, k, &mut used);
            let mut points: Vec<Point3> = chain.iter().map(|&i| self.crossings[i].point).collect();
            if let Some(&ci) = at_singular.get(&chain[0]) {
                points.insert(0, clusters[ci].1);
                touched[ci] = true;
            }
            if let Some(&ci) = at_singular.get(chain.last().expect("a chain has a start")) {
                points.push(clusters[ci].1);
                touched[ci] = true;
            }
            out.branches.push(Traced {
                points,
                closed: false,
            });
        }
        for k in 0..arcs.len() {
            if used[k] {
                continue;
            }
            let (chain, closed) = walk(arcs[k][0], k, &mut used);
            debug_assert!(closed);
            let points = chain.iter().map(|&i| self.crossings[i].point).collect();
            out.branches.push(Traced { points, closed });
        }
        for (ci, (_, p)) in clusters.iter().enumerate() {
            if !touched[ci] {
                out.points.push(*p);
            }
        }
        Ok(out)
    }

    /// The singular leaves grouped by touching, each group with its one
    /// certified critical point of `F`, which must lie on the curve.
    fn clusters(&self) -> Result<Vec<(Vec<usize>, Point3)>, Failure> {
        let singular: Vec<usize> = (0..self.leaves.len())
            .filter(|&i| matches!(self.leaves[i].kind, Kind::Singular))
            .collect();
        let mut group: Vec<usize> = (0..singular.len()).collect();
        fn root(g: &mut [usize], mut i: usize) -> usize {
            while g[i] != i {
                g[i] = g[g[i]];
                i = g[i];
            }
            i
        }
        for x in 0..singular.len() {
            for y in x + 1..singular.len() {
                let ([a, b], [c, d]) =
                    (self.leaves[singular[x]].cell, self.leaves[singular[y]].cell);
                let touch = a[0] <= c[1] && c[0] <= a[1] && b[0] <= d[1] && d[0] <= b[1];
                if touch {
                    let (rx, ry) = (root(&mut group, x), root(&mut group, y));
                    group[rx.max(ry)] = rx.min(ry);
                }
            }
        }
        let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (x, &leaf) in singular.iter().enumerate() {
            let r = root(&mut group, x);
            members.entry(r).or_default().push(leaf);
        }
        let mut out: Vec<(Vec<usize>, Point3)> = members
            .into_values()
            .map(|m| {
                let p = self.critical(&m)?;
                Ok((m, p))
            })
            .collect::<Result<_, Failure>>()?;
        // Each pole's zone is settled at the pole.
        for (zi, z) in self.zones.iter().enumerate() {
            let m: Vec<usize> = (0..self.leaves.len())
                .filter(|&i| matches!(self.leaves[i].kind, Kind::Pole(k) if k == zi))
                .collect();
            if !m.is_empty() {
                out.push((m, z.pole));
            }
        }
        Ok(out)
    }

    /// The one critical point of `F` in a cluster of leaves, on the curve.
    fn critical(&self, members: &[usize]) -> Result<Point3, Failure> {
        let [mut s, mut t] = self.leaves[members[0]].cell;
        for &i in members {
            let [a, b] = self.leaves[i].cell;
            s = [s[0].min(a[0]), s[1].max(a[1])];
            t = [t[0].min(b[0]), t[1].max(b[1])];
        }
        // Newton's method on the gradient, from the middle.
        let (mut x, mut y) = ((s[0] + s[1]) / 2.0, (t[0] + t[1]) / 2.0);
        for _ in 0..50 {
            let j = self.field.at(x, y);
            let [a, b, d] = j.h;
            let det = a * d - b * b;
            if det == 0.0 || !det.is_finite() {
                break;
            }
            let (dx, dy) = (
                (d * j.d[0] - b * j.d[1]) / det,
                (a * j.d[1] - b * j.d[0]) / det,
            );
            x -= dx;
            y -= dy;
            if dx.abs() + dy.abs() < 1e-17 {
                break;
            }
        }
        let size = (s[1] - s[0]).max(t[1] - t[0]);
        let by = |v: f64, r: [f64; 2]| v >= r[0] - size && v <= r[1] + size;
        if !(by(x, s) && by(y, t)) {
            return Err(Failure::Uncertain);
        }
        for half in [2.0 * size, size, size / 4.0, size / 16.0, 1e-6, 1e-8, 1e-10] {
            let around = [[x - half, x + half], [y - half, y + half]];
            if self.krawczyk(around) != Some(true) {
                continue;
            }
            let cluster = [
                [s[0] - size / 8.0, s[1] + size / 8.0],
                [t[0] - size / 8.0, t[1] + size / 8.0],
            ];
            if !self.no_critical(cluster, around, 12) {
                return Err(Failure::Uncertain);
            }
            let p = self.field.point(x, y);
            let imp = self.field.imp;
            let fp = imp.value_at(p);
            if imp.distance_bound(fp.mag(), imp.local(p.to_vector())) <= LINEAR_RESOLUTION {
                return Ok(p);
            }
            return Err(Failure::Uncertain);
        }
        Err(Failure::Uncertain)
    }

    /// Krawczyk's test for a zero of `∇F` in a cell: `Some(true)` for
    /// exactly one, `Some(false)` for none, `None` undecided.
    fn krawczyk(&self, cell: Cell) -> Option<bool> {
        let p = Interval::point;
        let mid = |x: Interval| (x.lo() + x.hi()) / 2.0;
        let [[s0, s1], [t0, t1]] = cell;
        let c = [(s0 + s1) / 2.0, (t0 + t1) / 2.0];
        let h = self.field.enclose(cell)?.h;
        let jac = [[h[0], h[1]], [h[1], h[2]]];
        let m = jac.map(|r| r.map(mid));
        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let y = [
            [m[1][1] / det, -m[0][1] / det],
            [-m[1][0] / det, m[0][0] / det],
        ];
        let g = self.field.enclose([[c[0], c[0]], [c[1], c[1]]])?.d;
        let boxes = [
            Interval::new(s0, s1)? - p(c[0]),
            Interval::new(t0, t1)? - p(c[1]),
        ];
        let bounds = [[s0, s1], [t0, t1]];
        let (mut inside, mut apart) = (true, false);
        for row in 0..2 {
            let mut k = p(c[row]) - (p(y[row][0]) * g[0] + p(y[row][1]) * g[1]);
            for (col, &b) in boxes.iter().enumerate() {
                let ident = if row == col { 1.0 } else { 0.0 };
                let e = p(ident) - (p(y[row][0]) * jac[0][col] + p(y[row][1]) * jac[1][col]);
                k = k + e * b;
            }
            let [lo, hi] = bounds[row];
            inside &= k.lo() > lo && k.hi() < hi;
            apart |= k.hi() < lo || k.lo() > hi;
        }
        if apart {
            Some(false)
        } else if inside {
            Some(true)
        } else {
            None
        }
    }

    /// True when a cell holds no zero of `∇F` outside the cell `avoid`.
    fn no_critical(&self, cell: Cell, avoid: Cell, depth: u32) -> bool {
        let [[s0, s1], [t0, t1]] = cell;
        let [a, b] = avoid;
        if s0 >= a[0] && s1 <= a[1] && t0 >= b[0] && t1 <= b[1] {
            return true;
        }
        if let Some(e) = self.field.enclose(cell)
            && (signed(e.d[0]) || signed(e.d[1]))
        {
            return true;
        }
        if self.krawczyk(cell) == Some(false) {
            return true;
        }
        if depth == 0 {
            return false;
        }
        let (ms, mt) = ((s0 + s1) / 2.0, (t0 + t1) / 2.0);
        [[s0, ms], [ms, s1]].iter().all(|&ss| {
            [[t0, mt], [mt, t1]]
                .iter()
                .all(|&tt| self.no_critical([ss, tt], avoid, depth - 1))
        })
    }
}

#[cfg(test)]
mod tests;
