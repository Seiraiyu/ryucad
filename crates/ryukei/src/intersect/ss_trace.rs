//! Traced surface–surface intersections: certified branch topology.
//!
//! The surface with the simpler parameterization (`p`, by the order plane,
//! cylinder, cone, sphere, torus) is cut to the region and written in its
//! exact NURBS form; on each of its rational Bézier patches the other
//! surface's implicit form is a polynomial `F(s, t)` in the patch's local
//! parameters ([`Implicit::on_patch`]), whose zero set is the
//! intersection. Each patch is subdivided until every cell is
//!
//! - **excluded**: `F` has one certified sign, so no curve;
//! - **regular**: `F_s` or `F_t` has one certified sign, so the curve is a
//!   graph over the other parameter: in an `s`-graph cell each line of
//!   constant `t` meets it at most once, and its crossings of the cell's
//!   sides, sorted by `t`, pair up into the arcs inside;
//! - or **singular**: small, and neither, so it holds a point where both
//!   partials vanish.
//!
//! Crossings of cell sides are the roots of `F` along each side,
//! certified by [`crate::geom::bernstein`]. A side shared by several cells
//! is cut at all their corners and each piece is solved once, so
//! neighbouring cells see the very same crossings; a patch's boundary is
//! solved from its boundary curve, which the patches either side share.
//! Arcs joined at shared crossings make branches. A cluster of singular
//! cells must hold exactly one critical point of `F`, certified by
//! Krawczyk's test, and lie on the curve: there branches meet (a
//! tangency, or a crossing of two branches) or, with none, the surfaces
//! touch at a point. Anything else is [`Failure::Uncertain`]. What lies
//! on a cone's other nappe, which its implicit form also holds, is
//! dropped.

use super::Failure;
use super::implicit::Implicit;
use crate::geom::bernstein::{Bernstein, Root};
use crate::geom::bernstein2::{Bernstein2, krawczyk};
use crate::geom::{BezierCurve, BezierPatch, Box3, NurbsSurface, Point3, Rigid, Surface, Vector3};
use crate::interval::Interval;
use crate::tol::LINEAR_RESOLUTION;
use core::f64::consts::{FRAC_PI_2, TAU};
use std::collections::{BTreeMap, BTreeSet};

/// Where a cell is split: off the dyadic fractions symmetric problems
/// put their curves through.
const SPLIT: f64 = 0.499_023_437_5;

/// The deepest subdivision of a patch: cells down to about 2⁻¹⁶ of it.
/// Deeper, the sides of cells next to a singular point come so close to
/// it that `F`, which vanishes to second order there, sinks below
/// rounding and their crossings can't be certified.
const MAX_DEPTH: u32 = 16;

/// The most cells a trace may leave before it gives up.
const MAX_LEAVES: usize = 1 << 18;

/// The topology of an intersection: its branches as ordered points on
/// both surfaces, and the points where the surfaces touch.
#[derive(Clone, Debug, Default)]
pub(crate) struct Topology {
    pub(crate) branches: Vec<Traced>,
    pub(crate) points: Vec<Point3>,
    /// Curves along which the surfaces touch (only the general tracer
    /// finds them).
    pub(crate) tangents: Vec<TangentTrace>,
}

/// A curve along which two surfaces touch, traced in both surfaces'
/// parameters `[u, v, s, t]`, in order along it.
#[derive(Clone, Debug, Default)]
pub(crate) struct TangentTrace {
    pub(crate) params: Vec<[f64; 4]>,
    pub(crate) closed: bool,
}

/// One branch: points along it in order, the first and last at its ends
/// (where it leaves the region, or a singular point), or a closed loop.
#[derive(Clone, Debug)]
pub(crate) struct Traced {
    pub(crate) points: Vec<Point3>,
    pub(crate) closed: bool,
}

/// The rank pairs are taken in.
pub(crate) fn rank(s: &Surface) -> u8 {
    match s {
        Surface::Plane(_) => 0,
        Surface::Cylinder(_) => 1,
        Surface::Cone(_) => 2,
        Surface::Sphere(_) => 3,
        _ => 4,
    }
}

/// The topology of where `a` and `b` meet inside `region`.
///
/// Where the curve runs through a corner of a patch or along its side, or
/// a singular point sits on a patch's boundary, the patches' crossings
/// can't be certified; those depend only on where the parameterization
/// puts its seams, so the trace is tried again with `p` turned about its
/// own axis (and a sphere's poles tilted away), which moves the seams and
/// leaves the surface where it is.
pub(crate) fn trace(a: &Surface, b: &Surface, region: &Box3) -> Result<Topology, Failure> {
    let (p, q) = if rank(a) <= rank(b) { (a, b) } else { (b, a) };
    let imp = Implicit::of(q).ok_or(Failure::Unsupported)?;
    for (spin, tilt) in TURNS {
        match attempt(&reparameterized(p, spin, tilt), &imp, region) {
            Err(Failure::Uncertain) => continue,
            done => return done,
        }
    }
    Err(Failure::Uncertain)
}

/// The turns a trace is tried with: about the surface's axis, and about
/// its `x` axis for a sphere.
const TURNS: [(f64, f64); 3] = [(0.0, 0.0), (0.407_1, 0.0), (1.291_3, 0.617_3)];

/// `p` turned by `spin` about its axis and, for a sphere, by `tilt` about
/// its `x` axis, through its origin: the same surface, its parameters
/// moved.
fn reparameterized(p: &Surface, spin: f64, tilt: f64) -> Surface {
    let Some(f) = p.frame() else {
        return p.clone();
    };
    if spin == 0.0 && tilt == 0.0 {
        return p.clone();
    }
    let o = f.origin().to_vector();
    let about = |axis, angle| {
        Rigid::translation(o)
            .then_after(&Rigid::rotation(axis, angle).then_after(&Rigid::translation(-o)))
    };
    let mut r = about(f.z(), spin);
    if matches!(p, Surface::Sphere(_)) {
        r = about(f.x(), tilt).then_after(&r);
    }
    p.transformed(&r)
}

/// One trace with `p` parameterized as it is.
fn attempt(p: &Surface, imp: &Implicit, region: &Box3) -> Result<Topology, Failure> {
    let Some(ranges) = ranges(p, region) else {
        return Ok(Topology::default());
    };
    let (nurbs, _) = p.to_nurbs(ranges).map_err(|_| Failure::Uncertain)?;
    let periodic = [p.periods().0.is_some(), p.periods().1.is_some()];
    let mut topo = Tracer::new(&nurbs, imp, periodic, region).run()?;
    // A cone's implicit form holds its other nappe too. A branch can pass
    // from one nappe to the other only through the apex, a singular
    // point that ends it, so a branch with a point past the apex lies
    // wholly on the other nappe.
    topo.branches
        .retain(|b| !b.points.iter().any(|&x| imp.beyond_apex(x)));
    topo.points.retain(|&x| !imp.beyond_apex(x));
    Ok(topo)
}

/// The parameter box of `p` that covers the region: a plane's and the
/// open directions' extent over the region's corners, closed directions
/// whole. `None` when the region misses a cone's nappe.
fn ranges(p: &Surface, region: &Box3) -> Option<[[f64; 2]; 2]> {
    let f = p.frame()?;
    let corners = region.corners().map(|c| f.to_local(c));
    let span = |k: usize| {
        let lo = corners
            .iter()
            .map(|c| c.to_array()[k])
            .fold(f64::INFINITY, f64::min);
        let hi = corners
            .iter()
            .map(|c| c.to_array()[k])
            .fold(f64::NEG_INFINITY, f64::max);
        [lo - LINEAR_RESOLUTION, hi + LINEAR_RESOLUTION]
    };
    let turn = [0.0, TAU];
    Some(match p {
        Surface::Plane(_) => [span(0), span(1)],
        Surface::Cylinder(_) => [turn, span(2)],
        Surface::Cone(c) => {
            let [lo, hi] = span(2);
            if hi <= c.apex_v() {
                return None;
            }
            [turn, [lo.max(c.apex_v()), hi]]
        }
        Surface::Sphere(_) => [turn, [-FRAC_PI_2, FRAC_PI_2]],
        _ => [turn, turn],
    })
}

/// A line of constant `s` (a side running along `t`) or of constant `t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Dir {
    S,
    T,
}

/// A line cells' sides lie on: a patch boundary, shared by the patches
/// either side and named by its knot and the span of the other direction
/// it crosses, or a line inside one patch at a local parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LineKey {
    Seam { dir: Dir, band: usize, knot: usize },
    Inner { patch: usize, dir: Dir, at: u64 },
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Regular { s_graph: bool },
    Singular,
}

#[derive(Clone, Copy, Debug)]
struct Leaf {
    patch: usize,
    s: [f64; 2],
    t: [f64; 2],
    kind: Kind,
}

/// A crossing of the curve with a piece of a line.
#[derive(Clone, Copy, Debug)]
struct Crossing {
    point: Point3,
    /// Where along the line, in the local parameter it runs by.
    along: f64,
}

/// One side of a leaf: its line, where it sits across the line, and the
/// stretch along it.
#[derive(Clone, Copy, Debug)]
struct Side {
    line: LineKey,
    across: f64,
    along: [f64; 2],
}

struct Tracer<'a> {
    nurbs: &'a NurbsSurface,
    imp: &'a Implicit,
    patches: &'a [BezierPatch],
    /// Patch spans along `u` and `v`.
    na: usize,
    nb: usize,
    periodic: [bool; 2],
    region: Box3,
    polys: Vec<Option<Bernstein2>>,
    leaves: Vec<Leaf>,
    /// Crossings found so far, and, per piece of a line, the ones on it.
    crossings: Vec<Crossing>,
    pieces: BTreeMap<(LineKey, u64, u64), Vec<usize>>,
}

impl<'a> Tracer<'a> {
    fn new(nurbs: &'a NurbsSurface, imp: &'a Implicit, periodic: [bool; 2], region: &Box3) -> Self {
        let patches = nurbs.patches();
        let nb = patches
            .iter()
            .take_while(|p| p.range[0] == patches[0].range[0])
            .count();
        Self {
            nurbs,
            imp,
            patches,
            na: patches.len() / nb,
            nb,
            periodic,
            region: region.grow(LINEAR_RESOLUTION),
            polys: vec![None; patches.len()],
            leaves: Vec::new(),
            crossings: Vec::new(),
            pieces: BTreeMap::new(),
        }
    }

    fn run(mut self) -> Result<Topology, Failure> {
        for k in 0..self.patches.len() {
            if !self.patches[k].bounds().intersects(&self.region) {
                continue;
            }
            let (f, _) = self.imp.on_patch(&self.patches[k]);
            self.subdivide(k, &f, [0.0, 1.0], [0.0, 1.0], 0)?;
            self.polys[k] = Some(f);
        }
        let arcs = self.arcs()?;
        self.link(arcs)
    }

    fn subdivide(
        &mut self,
        patch: usize,
        f: &Bernstein2,
        s: [f64; 2],
        t: [f64; 2],
        depth: u32,
    ) -> Result<(), Failure> {
        let signed = |x: Interval| x.lo() > 0.0 || x.hi() < 0.0;
        if signed(f.range()) {
            return Ok(());
        }
        if self.leaves.len() >= MAX_LEAVES {
            return Err(Failure::Uncertain);
        }
        let s_graph = signed(f.deriv_s().range());
        let t_graph = signed(f.deriv_t().range());
        if s_graph || t_graph {
            self.leaves.push(Leaf {
                patch,
                s,
                t,
                kind: Kind::Regular { s_graph },
            });
            return Ok(());
        }
        if depth == MAX_DEPTH {
            self.leaves.push(Leaf {
                patch,
                s,
                t,
                kind: Kind::Singular,
            });
            return Ok(());
        }
        let ms = s[0] + (s[1] - s[0]) * SPLIT;
        let mt = t[0] + (t[1] - t[0]) * SPLIT;
        let (left, right) = f.split_s(SPLIT);
        for (g, sr) in [(left, [s[0], ms]), (right, [ms, s[1]])] {
            let (lo, hi) = g.split_t(SPLIT);
            self.subdivide(patch, &lo, sr, [t[0], mt], depth + 1)?;
            self.subdivide(patch, &hi, sr, [mt, t[1]], depth + 1)?;
        }
        Ok(())
    }

    /// Patch `k`'s place in the grid of spans.
    fn grid(&self, k: usize) -> (usize, usize) {
        (k / self.nb, k % self.nb)
    }

    /// The knot index `i` of a direction with `n` spans, a periodic end
    /// named as its start.
    fn knot(&self, dir: usize, i: usize, n: usize) -> usize {
        if self.periodic[dir] && i == n { 0 } else { i }
    }

    /// A leaf's four sides: left, right, bottom, top.
    fn sides(&self, l: &Leaf) -> [Side; 4] {
        let (a, b) = self.grid(l.patch);
        let s_line = |x: f64| match x {
            0.0 => LineKey::Seam {
                dir: Dir::S,
                band: b,
                knot: self.knot(0, a, self.na),
            },
            1.0 => LineKey::Seam {
                dir: Dir::S,
                band: b,
                knot: self.knot(0, a + 1, self.na),
            },
            _ => LineKey::Inner {
                patch: l.patch,
                dir: Dir::S,
                at: x.to_bits(),
            },
        };
        let t_line = |x: f64| match x {
            0.0 => LineKey::Seam {
                dir: Dir::T,
                band: a,
                knot: self.knot(1, b, self.nb),
            },
            1.0 => LineKey::Seam {
                dir: Dir::T,
                band: a,
                knot: self.knot(1, b + 1, self.nb),
            },
            _ => LineKey::Inner {
                patch: l.patch,
                dir: Dir::T,
                at: x.to_bits(),
            },
        };
        [
            Side {
                line: s_line(l.s[0]),
                across: l.s[0],
                along: l.t,
            },
            Side {
                line: s_line(l.s[1]),
                across: l.s[1],
                along: l.t,
            },
            Side {
                line: t_line(l.t[0]),
                across: l.t[0],
                along: l.s,
            },
            Side {
                line: t_line(l.t[1]),
                across: l.t[1],
                along: l.s,
            },
        ]
    }

    /// The polynomial along a whole line, in the local parameter it runs
    /// by: a patch boundary from the boundary curve of a patch it bounds,
    /// an inner line from its patch's polynomial.
    fn line_poly(&self, line: LineKey) -> Bernstein {
        match line {
            LineKey::Inner { patch, dir, at } => {
                let f = self.polys[patch].as_ref().expect("a traced patch");
                let x = f64::from_bits(at);
                match dir {
                    Dir::S => f.at_s(x),
                    Dir::T => f.at_t(x),
                }
            }
            LineKey::Seam { dir, band, knot } => {
                let curve = match dir {
                    Dir::S => {
                        let (a, end) = if knot < self.na {
                            (knot, 0)
                        } else {
                            (knot - 1, 1)
                        };
                        self.edge(a * self.nb + band, Dir::S, end)
                    }
                    Dir::T => {
                        let (b, end) = if knot < self.nb {
                            (knot, 0)
                        } else {
                            (knot - 1, 1)
                        };
                        self.edge(band * self.nb + b, Dir::T, end)
                    }
                };
                self.imp.on_piece(&curve).0
            }
        }
    }

    /// A patch's boundary curve at `s = end` (running along `t`) or
    /// `t = end`.
    fn edge(&self, k: usize, dir: Dir, end: usize) -> BezierCurve<Vector3> {
        let p = &self.patches[k];
        let [m, n] = p.degrees;
        let pts = match dir {
            Dir::S => {
                let i = end * m;
                (0..=n).map(|j| p.pts[i * (n + 1) + j]).collect()
            }
            Dir::T => {
                let j = end * n;
                (0..=m).map(|i| p.pts[i * (n + 1) + j]).collect()
            }
        };
        BezierCurve {
            range: [0.0, 1.0],
            pts,
        }
    }

    /// The model point at a patch's local parameters.
    fn point(&self, k: usize, s: f64, t: f64) -> Point3 {
        let [[u0, u1], [v0, v1]] = self.patches[k].range;
        let at = |a: f64, b: f64, x: f64| match x {
            0.0 => a,
            1.0 => b,
            _ => a + (b - a) * x,
        };
        self.nurbs.point(at(u0, u1, s), at(v0, v1, t))
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
            // the graph's parameter.
            let mut found: Vec<(f64, usize)> = Vec::new();
            for (n, side) in self.sides(l).into_iter().enumerate() {
                let cuts: Vec<f64> = breaks[&side.line]
                    .range(side.along[0].to_bits()..=side.along[1].to_bits())
                    .map(|&b| f64::from_bits(b))
                    .collect();
                for w in cuts.windows(2) {
                    for c in self.solve(l, &side, [w[0], w[1]])? {
                        let along = self.crossings[c].along;
                        let key = match (s_graph, n) {
                            (true, 0 | 1) | (false, 2 | 3) => along,
                            (true, _) | (false, _) => side.across,
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

    /// The crossings on the piece `[a, b]` of a leaf's side, solved once.
    fn solve(&mut self, l: &Leaf, side: &Side, [a, b]: [f64; 2]) -> Result<Vec<usize>, Failure> {
        let key = (side.line, a.to_bits(), b.to_bits());
        if let Some(found) = self.pieces.get(&key) {
            return Ok(found.clone());
        }
        let poly = self.line_poly(side.line).part(a, b);
        let mut found = Vec::new();
        for r in poly.roots(1e-13) {
            let Root::Simple { lo, hi, .. } = r else {
                return Err(Failure::Uncertain);
            };
            let along = a + (b - a) * (lo + hi) / 2.0;
            let point = match side.line {
                LineKey::Seam { dir, .. } | LineKey::Inner { dir, .. } => match dir {
                    Dir::S => self.point(l.patch, side.across, along),
                    Dir::T => self.point(l.patch, along, side.across),
                },
            };
            self.crossings.push(Crossing { point, along });
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
        // Which singular point each crossing on a singular leaf's side
        // belongs to.
        let mut at_singular: BTreeMap<usize, usize> = BTreeMap::new();
        for (ci, (members, _)) in clusters.iter().enumerate() {
            for &li in members {
                let l = self.leaves[li];
                for side in self.sides(&l) {
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
        // Leaves touch on one patch where their boxes overlap: swept in
        // order of their least `s`, each against those still open (a box
        // whose `s` reaches it). Which pairs join doesn't depend on the
        // order, and each group's root is its least index, so the groups
        // are the all-pairs ones (thousands of leaves made it quadratic).
        let mut order: Vec<usize> = (0..singular.len()).collect();
        order.sort_by(|&x, &y| {
            let (a, b) = (&self.leaves[singular[x]], &self.leaves[singular[y]]);
            a.patch
                .cmp(&b.patch)
                .then(a.s[0].total_cmp(&b.s[0]))
                .then(x.cmp(&y))
        });
        let mut open: Vec<usize> = Vec::new();
        for &y in &order {
            let b = &self.leaves[singular[y]];
            open.retain(|&x| {
                let a = &self.leaves[singular[x]];
                a.patch == b.patch && b.s[0] <= a.s[1]
            });
            for &x in &open {
                let a = &self.leaves[singular[x]];
                if a.t[0] <= b.t[1] && b.t[0] <= a.t[1] {
                    let (rx, ry) = (root(&mut group, x), root(&mut group, y));
                    group[rx.max(ry)] = rx.min(ry);
                }
            }
            open.push(y);
        }
        let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (x, &leaf) in singular.iter().enumerate() {
            let r = root(&mut group, x);
            members.entry(r).or_default().push(leaf);
        }
        members
            .into_values()
            .map(|m| {
                let p = self.critical(&m)?;
                Ok((m, p))
            })
            .collect()
    }

    /// The one critical point of `F` in a cluster of leaves, on the curve.
    fn critical(&self, members: &[usize]) -> Result<Point3, Failure> {
        let first = self.leaves[members[0]];
        let f = self.polys[first.patch].as_ref().expect("a traced patch");
        let mut s = first.s;
        let mut t = first.t;
        for &i in members {
            let l = &self.leaves[i];
            s = [s[0].min(l.s[0]), s[1].max(l.s[1])];
            t = [t[0].min(l.t[0]), t[1].max(l.t[1])];
        }
        let (fs, ft) = (f.deriv_s(), f.deriv_t());
        let (fss, fst, ftt) = (fs.deriv_s(), fs.deriv_t(), ft.deriv_t());
        let mid = |x: Interval| (x.lo() + x.hi()) / 2.0;
        // Newton's method on the gradient, from the middle.
        let (mut x, mut y) = ((s[0] + s[1]) / 2.0, (t[0] + t[1]) / 2.0);
        for _ in 0..50 {
            let (gx, gy) = (mid(fs.eval(x, y)), mid(ft.eval(x, y)));
            let (a, b, d) = (
                mid(fss.eval(x, y)),
                mid(fst.eval(x, y)),
                mid(ftt.eval(x, y)),
            );
            let det = a * d - b * b;
            if det == 0.0 {
                break;
            }
            let (dx, dy) = ((d * gx - b * gy) / det, (a * gy - b * gx) / det);
            x -= dx;
            y -= dy;
            if dx.abs() + dy.abs() < 1e-17 {
                break;
            }
        }
        // Newton must have stayed by the cluster.
        let size = (s[1] - s[0]).max(t[1] - t[0]);
        let by = |v: f64, r: [f64; 2]| v >= r[0] - size && v <= r[1] + size;
        if !(by(x, s) && by(y, t)) {
            return Err(Failure::Uncertain);
        }
        // Krawczyk's test on the largest box about it that passes, from
        // one holding the whole cluster down to one so small that rounding
        // in the gradient is all it must cover; then no other critical
        // point in the rest of the cluster. The boxes may reach past the
        // patch: a critical point on its boundary is the polynomial's all
        // the same.
        for half in [2.0 * size, size, size / 4.0, size / 16.0, 1e-6, 1e-8, 1e-10] {
            let sx = [x - half, x + half];
            let ty = [y - half, y + half];
            if krawczyk(&fs, &ft, sx, ty) != Some(true) {
                continue;
            }
            let cluster = (
                [s[0] - size / 8.0, s[1] + size / 8.0],
                [t[0] - size / 8.0, t[1] + size / 8.0],
            );
            if !no_critical(&fs, &ft, cluster, (sx, ty), 12) {
                return Err(Failure::Uncertain);
            }
            let p = self.point(first.patch, x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
            let fp = self.imp.value_at(p);
            if self
                .imp
                .distance_bound(fp.mag(), self.imp.local(p.to_vector()))
                <= LINEAR_RESOLUTION
            {
                return Ok(p);
            }
            return Err(Failure::Uncertain);
        }
        Err(Failure::Uncertain)
    }
}

/// True when the box `(s, t)` holds no zero of `(F_s, F_t)` outside the
/// box `avoid`: each part is inside `avoid`, has a partial of one
/// certified sign, or is excluded by Krawczyk's test, halving to `depth`.
fn no_critical(
    fs: &Bernstein2,
    ft: &Bernstein2,
    (s, t): ([f64; 2], [f64; 2]),
    (a, b): ([f64; 2], [f64; 2]),
    depth: u32,
) -> bool {
    if s[0] >= a[0] && s[1] <= a[1] && t[0] >= b[0] && t[1] <= b[1] {
        return true;
    }
    let signed = |x: Interval| x.lo() > 0.0 || x.hi() < 0.0;
    if signed(fs.part(s, t).range()) || signed(ft.part(s, t).range()) {
        return true;
    }
    if krawczyk(fs, ft, s, t) == Some(false) {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let (ms, mt) = ((s[0] + s[1]) / 2.0, (t[0] + t[1]) / 2.0);
    [[s[0], ms], [ms, s[1]]].iter().all(|&ss| {
        [[t[0], mt], [mt, t[1]]]
            .iter()
            .all(|&tt| no_critical(fs, ft, (ss, tt), (a, b), depth - 1))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Cone, Cylinder, Frame, Plane, Sphere, Torus};

    fn region() -> Box3 {
        Box3::new(Point3::new(-6.0, -6.0, -6.0), Point3::new(6.0, 6.0, 6.0))
    }

    fn frame(o: (f64, f64, f64), z: (f64, f64, f64)) -> Frame {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(1.0, 0.2, 0.1),
        )
        .unwrap()
    }

    /// Every traced point lies on both surfaces; returns the branch
    /// shapes as (closed, point count).
    fn traced(a: &Surface, b: &Surface) -> Topology {
        let topo = trace(a, b, &region()).unwrap();
        for br in &topo.branches {
            assert!(br.points.len() >= 2);
            for &p in &br.points {
                assert!(a.distance(p) <= 1e-9 && b.distance(p) <= 1e-9, "{p:?}");
            }
        }
        topo
    }

    #[test]
    fn crossing_cylinders_of_unequal_radii() {
        // Two closed loops, one round each side of the thinner cylinder.
        let a =
            Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 2.0).unwrap());
        let b =
            Surface::Cylinder(Cylinder::new(frame((0.0, 0.3, 0.0), (1.0, 0.0, 0.0)), 1.0).unwrap());
        let t = traced(&a, &b);
        assert_eq!(t.branches.len(), 2, "{t:?}");
        assert!(t.branches.iter().all(|b| b.closed));
        assert!(t.points.is_empty());
    }

    #[test]
    fn a_sphere_off_a_cylinder_axis_and_a_torus_cut_by_a_tilted_plane() {
        let c =
            Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap());
        let s = Surface::Sphere(Sphere::new(frame((1.2, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap());
        let t = traced(&c, &s);
        assert_eq!(t.branches.len(), 1);
        assert!(t.branches[0].closed);
        let torus =
            Surface::Torus(Torus::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 3.0, 1.0).unwrap());
        let plane = Surface::Plane(Plane::new(frame((0.0, 0.0, 0.3), (0.2, 0.1, 1.0))));
        let t = traced(&plane, &torus);
        assert_eq!(t.branches.len(), 2, "{t:?}");
        assert!(t.branches.iter().all(|b| b.closed));
    }

    #[test]
    fn a_cone_through_a_sphere_leaves_the_region() {
        let k =
            Surface::Cone(Cone::new(frame((0.5, 0.0, 0.0), (1.0, 0.0, 0.2)), 0.5, 0.3).unwrap());
        let s = Surface::Sphere(Sphere::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 2.0).unwrap());
        let t = traced(&k, &s);
        assert!(!t.branches.is_empty());
    }

    #[test]
    fn closed_forms_trace_the_same_branches() {
        use super::super::ss_exact::exact;
        let z = (0.0, 0.0, 1.0);
        let pairs = [
            (
                Surface::Plane(Plane::new(frame((0.0, 0.0, 0.5), (0.0, 0.6, 1.0)))),
                Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), z), 1.5).unwrap()),
            ),
            (
                Surface::Sphere(Sphere::new(frame((0.0, 0.0, 0.0), z), 2.0).unwrap()),
                Surface::Sphere(Sphere::new(frame((1.0, 1.0, 0.5), z), 2.0).unwrap()),
            ),
            (
                Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), z), 1.0).unwrap()),
                Surface::Cylinder(
                    Cylinder::new(frame((0.0, 0.0, 0.0), (1.0, 0.0, 0.0)), 1.0).unwrap(),
                ),
            ),
            (
                Surface::Cone(Cone::new(frame((0.0, 0.0, 0.0), z), 1.0, 0.5).unwrap()),
                Surface::Plane(Plane::new(frame((0.0, 0.0, 1.0), (0.0, 0.3, 1.0)))),
            ),
        ];
        for (a, b) in pairs {
            let Some((branches, _)) = exact(&a, &b, &region()) else {
                panic!("a closed form")
            };
            let t = traced(&a, &b);
            if branches.len() == 2 && t.branches.len() == 4 {
                // Two ellipses crossing twice: four branches between the
                // two crossings, which are singular points.
                assert!(t.branches.iter().all(|b| !b.closed));
                continue;
            }
            assert_eq!(t.branches.len(), branches.len(), "{a:?} {b:?}");
            // Every traced point is on a closed-form branch.
            for br in &t.branches {
                for &p in &br.points {
                    assert!(branches.iter().any(|c| c.curve.distance(p) < 1e-9));
                }
            }
        }
    }

    #[test]
    fn equal_tori_side_by_side_cross_on_a_seam() {
        // Offset across their axes, two equal tori meet in curves that
        // cross where the tori touch, on the tubes' tops: the seams of
        // the torus patches, which no turn about the axis moves.
        let z = (0.0, 1.0, 0.0);
        let a = Surface::Torus(Torus::new(frame((0.0, 0.0, 0.5), z), 2.5, 1.0).unwrap());
        let b = Surface::Torus(Torus::new(frame((0.0, 0.0, -0.5), z), 2.5, 1.0).unwrap());
        let t = traced(&a, &b);
        assert!(!t.branches.is_empty());
        assert!(t.branches.iter().all(|b| !b.closed));
    }

    #[test]
    fn a_tangency_is_a_singular_point() {
        // A sphere touching a cylinder from outside, off its axis: one
        // point.
        let c =
            Surface::Cylinder(Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap());
        let s = Surface::Sphere(Sphere::new(frame((2.5, 0.0, 0.7), (0.0, 0.0, 1.0)), 1.5).unwrap());
        let t = traced(&c, &s);
        assert!(t.branches.is_empty(), "{t:?}");
        assert_eq!(t.points.len(), 1);
        assert!(t.points[0].distance(Point3::new(1.0, 0.0, 0.7)) < 1e-6);
    }
}
