//! Traced intersections of any two surfaces: certified subdivision of
//! their four parameters.
//!
//! Where neither surface has an implicit form (two NURBS, two pipes, a
//! swept surface and a spline), the curve where they meet is the zero set
//! of `E(u, v, s, t) = A(u, v) - B(s, t)` in the four-dimensional space of
//! both surfaces' parameters. Pairs of cells, one of each surface's
//! parameter plane (from their first cells near the region,
//! [`super::cells`]), are enclosed over jets ([`Sheet`]) and split, the
//! cell whose points spread further split along its wider parameter,
//! until each pair is
//!
//! - **excluded**: the two cells' enclosures are apart, or a component of
//!   `E` is of one certified sign by the mean value form;
//! - **simple**: a component of `N_A × N_B` (each `N` the cell's
//!   enclosure of `S_u × S_v`) is of one certified sign. The normals are
//!   then never parallel, so the curve is regular all through the pair,
//!   and its tangent, along `N_A × N_B`, moves one way along that
//!   coordinate: each arc is monotone in it, so no closed loop can hide
//!   in the pair and every arc crosses the pair's boundary twice;
//! - or **singular**: small, and neither.
//!
//! A simple pair's boundary is its eight faces, each one parameter held
//! at a side of its cell: there the curve crosses three equations in three
//! unknowns, solved by subdivision and Krawczyk's test, every crossing
//! certified unique in its box and strictly inside the face (a crossing
//! on a face's edge is uncertain, and the trace is tried again with other
//! split fractions). A crossing found from both sides of a face is the
//! same one when either's parameters lie in the other's certified box. A
//! simple pair with no crossings holds no curve; with two, one arc; with
//! more it is split again. Arcs joined at shared crossings make
//! branches, which end at a bounded surface's domain edge, outside the
//! region, or at a cluster of singular pairs. Each cluster must hold
//! exactly one point where the surfaces are tangent (Krawczyk's test on
//! `((A - B) · B_s, (A - B) · B_t, N_B · A_u, N_B · A_v) = 0`, which is
//! regular at a touch where the squared distance's stationary system is
//! not), on both surfaces: there branches meet, or with none, the surfaces
//! touch at a point. Anything else is [`Failure::Uncertain`].

use super::cells::cells;
use super::pipe::Cell;
use super::sheet_cs::krawczyk3;
use super::ss_trace::{Topology, Traced};
use super::{Failure, SLACK};
use crate::geom::bound2::IVec3;
use crate::geom::jet::JVec;
use crate::geom::sheet::Sheet;
use crate::geom::{Box3, Point3, Surface};
use crate::interval::Interval;
use crate::tol::LINEAR_RESOLUTION;
use std::collections::BTreeMap;

type Iv = Interval;

/// The fractions a cell is split at, one per attempt: off the dyadic
/// fractions symmetric problems put their curves through, and different
/// from each other so a crossing on a face's edge moves off it.
const SPLITS: [f64; 3] = [0.499_023_437_5, 0.471_191_406_25, 0.523_681_640_625];

/// The most times a pair may be split.
const MAX_DEPTH: u32 = 48;

/// The most pairs a trace may leave.
const MAX_LEAVES: usize = 1 << 17;

/// The most singular pairs a trace may leave: past this the surfaces
/// touch along a curve.
const MAX_SINGULAR: usize = 1 << 12;

/// The deepest a face's box is halved while isolating crossings.
const FACE_DEPTH: u32 = 96;

/// The most boxes one face's search may examine.
const FACE_BOXES: usize = 1 << 14;

/// How many rounds of splitting pairs with too many crossings a trace
/// may take.
const MAX_ROUNDS: usize = 12;

fn signed(x: Iv) -> bool {
    x.lo() > 0.0 || x.hi() < 0.0
}

fn meet(a: Iv, b: Iv) -> Iv {
    Iv::new(a.lo().max(b.lo()), a.hi().min(b.hi())).unwrap_or(a)
}

fn cross(a: IVec3, b: IVec3) -> IVec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: IVec3, b: IVec3) -> Iv {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The box of a surface point's enclosure.
fn point_box(p: &JVec<Iv>) -> Box3 {
    Box3::new(
        Point3::new(p[0].v.lo(), p[1].v.lo(), p[2].v.lo()),
        Point3::new(p[0].v.hi(), p[1].v.hi(), p[2].v.hi()),
    )
}

fn partial(p: &JVec<Iv>, k: usize) -> IVec3 {
    [p[0].d[k], p[1].d[k], p[2].d[k]]
}

fn second(p: &JVec<Iv>, k: usize) -> IVec3 {
    [p[0].h[k], p[1].h[k], p[2].h[k]]
}

/// The unit normal of a surface at a point, from its jets there (their
/// intervals' middles); `None` where it vanishes. Used only to choose a
/// direction for exclusion, which holds whatever direction is chosen.
fn mid_normal(q: &JVec<Iv>) -> Option<[Iv; 3]> {
    let m = |x: Iv| (x.lo() + x.hi()) / 2.0;
    let (a, b) = (partial(q, 0).map(m), partial(q, 1).map(m));
    let n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let len = crate::math::sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
    (len > 0.0 && len.is_finite()).then(|| n.map(|x| Iv::point(x / len)))
}

/// `n · v` for a direction and an interval vector.
fn along(n: [Iv; 3], v: IVec3) -> Iv {
    n[0] * v[0] + n[1] * v[1] + n[2] * v[2]
}

/// The topology of where `a` and `b` meet inside `region`, traced in
/// their four parameters. Fails with `Uncertain` where a crossing, a
/// tangency or a pairing can't be certified (a tangency along a curve
/// among them).
pub(crate) fn trace(a: &Surface, b: &Surface, region: &Box3) -> Result<Topology, Failure> {
    for split in SPLITS {
        let mut tracer = Tracer::new([a, b], region, split);
        match tracer.run() {
            Err(Failure::Uncertain) if tracer.singular <= MAX_SINGULAR => continue,
            other => return other,
        }
    }
    Err(Failure::Uncertain)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Simple,
    Singular,
}

#[derive(Clone, Copy, Debug)]
struct Leaf {
    cells: [Cell; 2],
    depth: u32,
    kind: Kind,
}

/// A hyperplane of the four parameters: which surface, which of its
/// parameters is held, and at what (as bits, a periodic direction's end
/// folded onto its start).
type Plane = (usize, usize, u64);

/// A crossing of the curve with a face.
#[derive(Clone, Debug)]
struct Crossing {
    plane: Plane,
    /// The three free parameters, and the box they are certified unique
    /// in.
    free: [f64; 3],
    unique: [[f64; 2]; 3],
    /// The four parameters, and the point.
    params: [f64; 4],
    point: Point3,
    /// The simple leaves that found it.
    found_by: Vec<usize>,
}

struct Tracer<'a> {
    surfaces: [&'a Surface; 2],
    sheets: [Sheet<'a>; 2],
    region: Box3,
    split: f64,
    leaves: Vec<Leaf>,
    singular: usize,
    crossings: Vec<Crossing>,
    planes: BTreeMap<Plane, Vec<usize>>,
}

impl<'a> Tracer<'a> {
    fn new(surfaces: [&'a Surface; 2], region: &Box3, split: f64) -> Self {
        Self {
            surfaces,
            sheets: surfaces.map(Sheet::of),
            region: region.grow(LINEAR_RESOLUTION),
            split,
            leaves: Vec::new(),
            singular: 0,
            crossings: Vec::new(),
            planes: BTreeMap::new(),
        }
    }

    fn run(&mut self) -> Result<Topology, Failure> {
        let firsts = [
            cells(self.surfaces[0], &self.region),
            cells(self.surfaces[1], &self.region),
        ];
        // The first pairs: cells whose enclosures meet.
        let boxes: Vec<Vec<Option<Box3>>> = (0..2)
            .map(|k| {
                firsts[k]
                    .iter()
                    .map(|&c| self.sheets[k].jet_box(c).map(|p| point_box(&p).grow(SLACK)))
                    .collect()
            })
            .collect();
        for (i, &ca) in firsts[0].iter().enumerate() {
            for (j, &cb) in firsts[1].iter().enumerate() {
                let meets = match (boxes[0][i], boxes[1][j]) {
                    (Some(x), Some(y)) => x.intersects(&y),
                    _ => true,
                };
                if meets {
                    self.subdivide([ca, cb], 0)?;
                }
            }
        }
        // Crossings, splitting again the simple leaves holding more than
        // two, until none does.
        let mut todo: Vec<usize> = (0..self.leaves.len())
            .filter(|&i| self.leaves[i].kind == Kind::Simple)
            .collect();
        for round in 0.. {
            let mut again = Vec::new();
            for &li in &todo {
                let found = self.leaf_crossings(li)?;
                if found.len() > 2 || found.len() % 2 == 1 {
                    again.push(li);
                }
            }
            if again.is_empty() {
                break;
            }
            if round == MAX_ROUNDS {
                return Err(Failure::Uncertain);
            }
            let first_new = self.leaves.len();
            for &li in &again {
                let leaf = self.leaves[li];
                // The leaf is retired: it finds nothing from now on.
                self.leaves[li].kind = Kind::Singular;
                self.leaves[li].cells = [[[f64::NAN; 2]; 2]; 2];
                for c in &mut self.crossings {
                    c.found_by.retain(|&x| x != li);
                }
                self.split_pair(leaf.cells, leaf.depth)?;
            }
            todo = (first_new..self.leaves.len())
                .filter(|&i| self.leaves[i].kind == Kind::Simple)
                .collect();
        }
        self.link()
    }

    /// Splits the pair's cell whose points spread further, along its
    /// wider parameter, and subdivides both halves.
    fn split_pair(&mut self, cells: [Cell; 2], depth: u32) -> Result<(), Failure> {
        if depth >= MAX_DEPTH {
            return Err(Failure::Uncertain);
        }
        let mut best = (0, 0, -1.0);
        for (k, (sheet, cell)) in self.sheets.iter().zip(cells).enumerate() {
            let p = sheet.jet_box(cell);
            for (d, [lo, hi]) in cell.into_iter().enumerate() {
                let w = hi - lo;
                let spread = match &p {
                    Some(p) => crate::intersect::sheet_cs::norm_hi(partial(p, d)) * w,
                    None => w,
                };
                if spread > best.2 {
                    best = (k, d, spread);
                }
            }
        }
        let (k, d, _) = best;
        let [lo, hi] = cells[k][d];
        let m = lo + (hi - lo) * self.split;
        if m <= lo || m >= hi {
            return Err(Failure::Uncertain);
        }
        for half in [[lo, m], [m, hi]] {
            let mut c = cells;
            c[k][d] = half;
            self.subdivide(c, depth + 1)?;
        }
        Ok(())
    }

    fn subdivide(&mut self, cells: [Cell; 2], depth: u32) -> Result<(), Failure> {
        let (Some(pa), Some(pb)) = (
            self.sheets[0].jet_box(cells[0]),
            self.sheets[1].jet_box(cells[1]),
        ) else {
            return self.split_pair(cells, depth);
        };
        let (ba, bb) = (point_box(&pa).grow(SLACK), point_box(&pb).grow(SLACK));
        if !ba.intersects(&bb) || !ba.intersects(&self.region) {
            return Ok(());
        }
        if self.excluded(cells, &pa, &pb) {
            return Ok(());
        }
        if self.leaves.len() >= MAX_LEAVES {
            return Err(Failure::Uncertain);
        }
        let t = cross(
            cross(partial(&pa, 0), partial(&pa, 1)),
            cross(partial(&pb, 0), partial(&pb, 1)),
        );
        if t.iter().any(|&x| signed(x)) {
            self.leaves.push(Leaf {
                cells,
                depth,
                kind: Kind::Simple,
            });
            return Ok(());
        }
        if depth >= MAX_DEPTH {
            self.singular += 1;
            if self.singular > MAX_SINGULAR {
                return Err(Failure::Uncertain);
            }
            self.leaves.push(Leaf {
                cells,
                depth,
                kind: Kind::Singular,
            });
            return Ok(());
        }
        self.split_pair(cells, depth)
    }

    /// Whether a component of `E` has one certified sign over the pair,
    /// by the mean value form about its middle.
    fn excluded(&self, cells: [Cell; 2], pa: &JVec<Iv>, pb: &JVec<Iv>) -> bool {
        let mid = |c: Cell| c.map(|[a, b]| (a + b) / 2.0);
        let (ma, mb) = (mid(cells[0]), mid(cells[1]));
        let (Some(qa), Some(qb)) = (
            self.sheets[0].jet_box(ma.map(|x| [x, x])),
            self.sheets[1].jet_box(mb.map(|x| [x, x])),
        ) else {
            return false;
        };
        let w = |c: Cell, m: [f64; 2], k: usize| Iv::new(c[k][0] - m[k], c[k][1] - m[k]);
        let (Some(du), Some(dv), Some(ds), Some(dt)) = (
            w(cells[0], ma, 0),
            w(cells[0], ma, 1),
            w(cells[1], mb, 0),
            w(cells[1], mb, 1),
        ) else {
            return false;
        };
        let along_axes = (0..3).any(|k| {
            let direct = pa[k].v - pb[k].v;
            let lin = (qa[k].v - qb[k].v) + pa[k].d[0] * du + pa[k].d[1] * dv
                - pb[k].d[0] * ds
                - pb[k].d[1] * dt;
            signed(meet(direct, lin))
        });
        if along_axes {
            return true;
        }
        // Along each surface's normal at its cell's middle, where its own
        // spread is least: `n · (A - B)` by the mean value form.
        let value = |p: &JVec<Iv>| -> IVec3 { [p[0].v, p[1].v, p[2].v] };
        [mid_normal(&qa), mid_normal(&qb)]
            .into_iter()
            .flatten()
            .any(|n| {
                let lin = along(n, value(&qa)) - along(n, value(&qb))
                    + along(n, partial(pa, 0)) * du
                    + along(n, partial(pa, 1)) * dv
                    - along(n, partial(pb, 0)) * ds
                    - along(n, partial(pb, 1)) * dt;
                signed(lin)
            })
    }

    /// The crossings on a simple leaf's faces, recorded and matched with
    /// those already found.
    fn leaf_crossings(&mut self, li: usize) -> Result<Vec<usize>, Failure> {
        let cells = self.leaves[li].cells;
        let mut found = Vec::new();
        for side in 0..2 {
            for param in 0..2 {
                for end in 0..2 {
                    let at = cells[side][param][end];
                    for c in self.face(cells, side, param, at)? {
                        if !self.crossings[c].found_by.contains(&li) {
                            self.crossings[c].found_by.push(li);
                        }
                        if !found.contains(&c) {
                            found.push(c);
                        }
                    }
                }
            }
        }
        Ok(found)
    }

    /// The key of a hyperplane, a periodic direction's end folded onto
    /// its start.
    fn plane(&self, side: usize, param: usize, at: f64) -> Plane {
        let [du, dv] = self.surfaces[side].domain();
        let (pu, pv) = self.surfaces[side].periods();
        let (d, p) = if param == 0 { (du, pu) } else { (dv, pv) };
        let at = match p {
            Some(p) if at == d[0] + p => d[0],
            _ => at,
        };
        (side, param, at.to_bits())
    }

    /// The crossings of the curve with the face of the pair where
    /// `side`'s parameter `param` is held at `at`.
    fn face(
        &mut self,
        cells: [Cell; 2],
        side: usize,
        param: usize,
        at: f64,
    ) -> Result<Vec<usize>, Failure> {
        // The free parameters: the held surface's other one, then the
        // other surface's two.
        let other = 1 - side;
        let free = [cells[side][1 - param], cells[other][0], cells[other][1]];
        let face = Face {
            sheets: self.sheets,
            side,
            param,
            at,
        };
        let mut roots = Vec::new();
        let mut budget = FACE_BOXES;
        face.search(free, 0, &mut budget, &mut roots)?;
        let plane = self.plane(side, param, at);
        let mut out = Vec::with_capacity(roots.len());
        for (x, unique) in roots {
            let params = face.params(x);
            let inside =
                |p: [f64; 3], b: [[f64; 2]; 3]| (0..3).all(|k| b[k][0] <= p[k] && p[k] <= b[k][1]);
            let same = self.planes.get(&plane).and_then(|list| {
                list.iter().copied().find(|&c| {
                    let o = &self.crossings[c];
                    inside(x, o.unique) || inside(o.free, unique)
                })
            });
            let index = match same {
                Some(c) => c,
                None => {
                    let point = self.sheets[0].point(params[0], params[1]);
                    self.crossings.push(Crossing {
                        plane,
                        free: x,
                        unique,
                        params,
                        point,
                        found_by: Vec::new(),
                    });
                    let c = self.crossings.len() - 1;
                    self.planes.entry(plane).or_default().push(c);
                    c
                }
            };
            out.push(index);
        }
        Ok(out)
    }

    /// Whether a crossing lies where a branch may end: on an edge of a
    /// bounded surface's domain, or outside the region (where a pair was
    /// left out for not reaching it).
    fn end_allowed(&self, c: &Crossing) -> bool {
        let (side, param, bits) = c.plane;
        let at = f64::from_bits(bits);
        let s = self.surfaces[side];
        let d = s.domain()[param];
        let periodic = if param == 0 {
            s.periods().0
        } else {
            s.periods().1
        };
        let on_edge = periodic.is_none() && (at == d[0] || at == d[1]);
        let (lo, hi) = (self.region.min(), self.region.max());
        let p = c.point;
        let outside =
            p.x < lo.x || p.x > hi.x || p.y < lo.y || p.y > hi.y || p.z < lo.z || p.z > hi.z;
        on_edge || outside
    }

    /// Whether the closed box of a leaf holds the four parameters `x`, a
    /// periodic one at either representative.
    fn holds(&self, leaf: &Leaf, x: [f64; 4]) -> bool {
        (0..4).all(|k| {
            let (side, param) = (k / 2, k % 2);
            let [lo, hi] = leaf.cells[side][param];
            let s = self.surfaces[side];
            let period = if param == 0 {
                s.periods().0
            } else {
                s.periods().1
            };
            let v = x[k];
            (lo <= v && v <= hi)
                || period
                    .is_some_and(|p| (lo <= v + p && v + p <= hi) || (lo <= v - p && v - p <= hi))
        })
    }

    /// Joins arcs at shared crossings into branches, and settles the
    /// singular clusters they end at.
    fn link(&self) -> Result<Topology, Failure> {
        let n = self.crossings.len();
        // Each simple leaf with two crossings is an arc.
        let mut arcs: Vec<[usize; 2]> = Vec::new();
        let mut per_leaf: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (c, x) in self.crossings.iter().enumerate() {
            if x.found_by.len() > 2 {
                return Err(Failure::Uncertain);
            }
            for &l in &x.found_by {
                per_leaf.entry(l).or_default().push(c);
            }
        }
        for found in per_leaf.values() {
            match found.len() {
                0 => {}
                2 => arcs.push([found[0], found[1]]),
                _ => return Err(Failure::Uncertain),
            }
        }
        let clusters = self.clusters()?;
        // A crossing found by one leaf only is an end: of the domain,
        // outside the region, or at a singular cluster.
        let mut at_cluster: BTreeMap<usize, usize> = BTreeMap::new();
        for (c, x) in self.crossings.iter().enumerate() {
            if x.found_by.len() != 1 || self.end_allowed(x) {
                continue;
            }
            let cl = clusters.iter().position(|(members, _)| {
                members
                    .iter()
                    .any(|&l| self.holds(&self.leaves[l], x.params))
            });
            match cl {
                Some(k) => {
                    at_cluster.insert(c, k);
                }
                None => return Err(Failure::Uncertain),
            }
        }
        let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (k, &[a, b]) in arcs.iter().enumerate() {
            next[a].push(k);
            next[b].push(k);
        }
        let mut used = vec![false; arcs.len()];
        let mut touched = vec![false; clusters.len()];
        let mut out = Topology::default();
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
        for c in 0..n {
            if next[c].len() != 1 || used[next[c][0]] {
                continue;
            }
            let (chain, _) = walk(c, next[c][0], &mut used);
            let mut points: Vec<Point3> = chain.iter().map(|&i| self.crossings[i].point).collect();
            if let Some(&k) = at_cluster.get(&chain[0]) {
                points.insert(0, clusters[k].1);
                touched[k] = true;
            }
            if let Some(&k) = at_cluster.get(chain.last().expect("a chain has a start")) {
                points.push(clusters[k].1);
                touched[k] = true;
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
            if !closed {
                return Err(Failure::Uncertain);
            }
            let points = chain.iter().map(|&i| self.crossings[i].point).collect();
            out.branches.push(Traced { points, closed });
        }
        for (k, (_, p)) in clusters.iter().enumerate() {
            if !touched[k] {
                out.points.push(*p);
            }
        }
        Ok(out)
    }

    /// The singular leaves grouped by touching, each group with its one
    /// certified point of tangency on both surfaces.
    fn clusters(&self) -> Result<Vec<(Vec<usize>, Point3)>, Failure> {
        let singular: Vec<usize> = (0..self.leaves.len())
            .filter(|&i| {
                self.leaves[i].kind == Kind::Singular && !self.leaves[i].cells[0][0][0].is_nan()
            })
            .collect();
        let mut group: Vec<usize> = (0..singular.len()).collect();
        fn root(g: &mut [usize], mut i: usize) -> usize {
            while g[i] != i {
                g[i] = g[g[i]];
                i = g[i];
            }
            i
        }
        let touch = |x: &Leaf, y: &Leaf| {
            (0..2).all(|k| {
                (0..2).all(|d| {
                    let (a, b) = (x.cells[k][d], y.cells[k][d]);
                    a[0] <= b[1] && b[0] <= a[1]
                })
            })
        };
        for x in 0..singular.len() {
            for y in x + 1..singular.len() {
                if touch(&self.leaves[singular[x]], &self.leaves[singular[y]]) {
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
        members
            .into_values()
            .map(|m| {
                let p = self.tangency(&m)?;
                Ok((m, p))
            })
            .collect()
    }

    /// The one point of tangency in a cluster of leaves, where the
    /// surfaces touch.
    fn tangency(&self, members: &[usize]) -> Result<Point3, Failure> {
        let mut hull = self.leaves[members[0]].cells;
        for &i in members {
            let c = self.leaves[i].cells;
            for k in 0..2 {
                for d in 0..2 {
                    hull[k][d] = [hull[k][d][0].min(c[k][d][0]), hull[k][d][1].max(c[k][d][1])];
                }
            }
        }
        let tan = Tangency {
            sheets: self.sheets,
        };
        let mut x = [
            (hull[0][0][0] + hull[0][0][1]) / 2.0,
            (hull[0][1][0] + hull[0][1][1]) / 2.0,
            (hull[1][0][0] + hull[1][0][1]) / 2.0,
            (hull[1][1][0] + hull[1][1][1]) / 2.0,
        ];
        x = tan.newton(x);
        let size = (0..4)
            .map(|k| hull[k / 2][k % 2][1] - hull[k / 2][k % 2][0])
            .fold(0.0, f64::max);
        let near = (0..4).all(|k| {
            let [lo, hi] = hull[k / 2][k % 2];
            x[k] >= lo - size && x[k] <= hi + size
        });
        if !near {
            return Err(Failure::Uncertain);
        }
        for half in [2.0 * size, size, size / 4.0, size / 16.0, 1e-8, 1e-10] {
            let around: [[f64; 2]; 4] = x.map(|v| [v - half, v + half]);
            if tan.krawczyk(around) != Some(true) {
                continue;
            }
            // No other point of tangency anywhere in the cluster.
            for &i in members {
                let c = self.leaves[i].cells;
                let b = [c[0][0], c[0][1], c[1][0], c[1][1]];
                if !tan.none_but(b, around, 12) {
                    return Err(Failure::Uncertain);
                }
            }
            let p = self.sheets[0].point(x[0], x[1]);
            let q = self.sheets[1].point(x[2], x[3]);
            if p.distance(q) <= LINEAR_RESOLUTION {
                return Ok(p);
            }
            return Err(Failure::Uncertain);
        }
        Err(Failure::Uncertain)
    }
}

/// A face of a pair: one surface's parameter held, the other three free.
struct Face<'a> {
    sheets: [Sheet<'a>; 2],
    side: usize,
    param: usize,
    at: f64,
}

impl Face<'_> {
    /// The two cells of a box of the free parameters.
    fn cells(&self, x: [[f64; 2]; 3]) -> [Cell; 2] {
        let held = if self.param == 0 {
            [[self.at, self.at], x[0]]
        } else {
            [x[0], [self.at, self.at]]
        };
        if self.side == 0 {
            [held, [x[1], x[2]]]
        } else {
            [[x[1], x[2]], held]
        }
    }

    /// The four parameters of a point of the free ones.
    fn params(&self, x: [f64; 3]) -> [f64; 4] {
        let c = self.cells(x.map(|v| [v, v]));
        [c[0][0][0], c[0][1][0], c[1][0][0], c[1][1][0]]
    }

    /// `E` and its Jacobian in the free parameters over a box.
    fn enclose(&self, x: [[f64; 2]; 3]) -> Option<([Iv; 3], [[Iv; 3]; 3])> {
        let [ca, cb] = self.cells(x);
        let pa = self.sheets[0].jet_box(ca)?;
        let pb = self.sheets[1].jet_box(cb)?;
        let e = core::array::from_fn(|k| pa[k].v - pb[k].v);
        let free = 1 - self.param;
        let jac = core::array::from_fn(|k| {
            if self.side == 0 {
                [pa[k].d[free], -pb[k].d[0], -pb[k].d[1]]
            } else {
                [-pb[k].d[free], pa[k].d[0], pa[k].d[1]]
            }
        });
        Some((e, jac))
    }

    fn search(
        &self,
        x: [[f64; 2]; 3],
        depth: u32,
        budget: &mut usize,
        out: &mut Vec<([f64; 3], [[f64; 2]; 3])>,
    ) -> Result<(), Failure> {
        *budget = budget.checked_sub(1).ok_or(Failure::Uncertain)?;
        let m: [f64; 3] = x.map(|[a, b]| (a + b) / 2.0);
        let enc = self.enclose(x);
        let at = self.enclose(m.map(|v| [v, v]));
        if let (Some((e0, jac)), Some((em, _))) = (&enc, &at) {
            let mut e = *e0;
            let d: Option<Vec<Iv>> = (0..3)
                .map(|k| Iv::new(x[k][0] - m[k], x[k][1] - m[k]))
                .collect();
            if let Some(d) = d {
                for k in 0..3 {
                    e[k] = meet(
                        e[k],
                        em[k] + jac[k][0] * d[0] + jac[k][1] * d[1] + jac[k][2] * d[2],
                    );
                }
            }
            if e.iter().any(|&v| signed(v)) {
                return Ok(());
            }
            match krawczyk3(*em, *jac, x) {
                Some(false) => return Ok(()),
                Some(true) => {
                    out.push((self.root(x), x));
                    return Ok(());
                }
                None => {}
            }
        }
        if depth == FACE_DEPTH {
            return Err(Failure::Uncertain);
        }
        let spread = |k: usize| {
            let w = x[k][1] - x[k][0];
            match &enc {
                Some((_, jac)) => (0..3).map(|r| jac[r][k].mag()).fold(0.0, f64::max) * w,
                None => w,
            }
        };
        let k = (0..3)
            .max_by(|&a, &b| spread(a).total_cmp(&spread(b)))
            .expect("three parameters");
        let [lo, hi] = x[k];
        let mid = (lo + hi) / 2.0;
        if mid <= lo || mid >= hi {
            return Err(Failure::Uncertain);
        }
        for half in [[lo, mid], [mid, hi]] {
            let mut y = x;
            y[k] = half;
            self.search(y, depth + 1, budget, out)?;
        }
        Ok(())
    }

    /// The crossing in a box certified to hold one, by Newton's method
    /// from its middle, kept inside the box.
    fn root(&self, x: [[f64; 2]; 3]) -> [f64; 3] {
        let mut p: [f64; 3] = x.map(|[a, b]| (a + b) / 2.0);
        for _ in 0..40 {
            let [ca, cb] = self.cells(p.map(|v| [v, v]));
            let ja = self.sheets[0].jets_at(ca[0][0], ca[1][0]);
            let jb = self.sheets[1].jets_at(cb[0][0], cb[1][0]);
            let free = 1 - self.param;
            let e: [f64; 3] = core::array::from_fn(|k| ja[k].v - jb[k].v);
            let m: [[f64; 3]; 3] = core::array::from_fn(|k| {
                if self.side == 0 {
                    [ja[k].d[free], -jb[k].d[0], -jb[k].d[1]]
                } else {
                    [-jb[k].d[free], ja[k].d[0], ja[k].d[1]]
                }
            });
            let Some(dx) = super::sheet_cs::solve3(m, e) else {
                break;
            };
            let next: [f64; 3] = core::array::from_fn(|k| (p[k] - dx[k]).clamp(x[k][0], x[k][1]));
            if next == p {
                break;
            }
            p = next;
        }
        p
    }
}

/// The system whose roots are where the surfaces are tangent:
/// `((A - B) · B_s, (A - B) · B_t, N_B · A_u, N_B · A_v)`, `N_B = B_s ×
/// B_t`.
struct Tangency<'a> {
    sheets: [Sheet<'a>; 2],
}

impl Tangency<'_> {
    /// The system and its Jacobian in `(u, v, s, t)` over a box.
    fn enclose(&self, x: [[f64; 2]; 4]) -> Option<([Iv; 4], [[Iv; 4]; 4])> {
        let pa = self.sheets[0].jet_box([x[0], x[1]])?;
        let pb = self.sheets[1].jet_box([x[2], x[3]])?;
        let w: IVec3 = core::array::from_fn(|k| pa[k].v - pb[k].v);
        let (au, av) = (partial(&pa, 0), partial(&pa, 1));
        let (bs, bt) = (partial(&pb, 0), partial(&pb, 1));
        let (auu, auv, avv) = (second(&pa, 0), second(&pa, 1), second(&pa, 2));
        let (bss, bst, btt) = (second(&pb, 0), second(&pb, 1), second(&pb, 2));
        let n = cross(bs, bt);
        let add = |a: IVec3, b: IVec3| -> IVec3 { core::array::from_fn(|k| a[k] + b[k]) };
        let ns = add(cross(bss, bt), cross(bs, bst));
        let nt = add(cross(bst, bt), cross(bs, btt));
        let f = [dot(w, bs), dot(w, bt), dot(n, au), dot(n, av)];
        let jac = [
            [
                dot(au, bs),
                dot(av, bs),
                dot(w, bss) - dot(bs, bs),
                dot(w, bst) - dot(bt, bs),
            ],
            [
                dot(au, bt),
                dot(av, bt),
                dot(w, bst) - dot(bs, bt),
                dot(w, btt) - dot(bt, bt),
            ],
            [dot(n, auu), dot(n, auv), dot(ns, au), dot(nt, au)],
            [dot(n, auv), dot(n, avv), dot(ns, av), dot(nt, av)],
        ];
        Some((f, jac))
    }

    /// True when a box holds no root of the system outside `avoid`.
    fn none_but(&self, x: [[f64; 2]; 4], avoid: [[f64; 2]; 4], depth: u32) -> bool {
        if (0..4).all(|k| x[k][0] >= avoid[k][0] && x[k][1] <= avoid[k][1]) {
            return true;
        }
        if let Some((f, _)) = self.enclose(x)
            && f.iter().any(|&v| signed(v))
        {
            return true;
        }
        if self.krawczyk(x) == Some(false) {
            return true;
        }
        if depth == 0 {
            return false;
        }
        let k = (0..4)
            .max_by(|&a, &b| (x[a][1] - x[a][0]).total_cmp(&(x[b][1] - x[b][0])))
            .expect("four parameters");
        let mid = (x[k][0] + x[k][1]) / 2.0;
        [[x[k][0], mid], [mid, x[k][1]]].iter().all(|&half| {
            let mut y = x;
            y[k] = half;
            self.none_but(y, avoid, depth - 1)
        })
    }

    /// Newton's method on the system from `x`.
    fn newton(&self, mut x: [f64; 4]) -> [f64; 4] {
        for _ in 0..50 {
            let Some((f, jac)) = self.enclose(x.map(|v| [v, v])) else {
                break;
            };
            let mid = |v: Iv| (v.lo() + v.hi()) / 2.0;
            let m = jac.map(|r| r.map(mid));
            let Some(dx) = solve4(m, f.map(mid)) else {
                break;
            };
            let next: [f64; 4] = core::array::from_fn(|k| x[k] - dx[k]);
            if next.iter().any(|v| !v.is_finite()) || next == x {
                break;
            }
            x = next;
        }
        x
    }

    /// Krawczyk's test for a root of the system in a box: `Some(true)`
    /// for exactly one, `Some(false)` for none, `None` undecided.
    fn krawczyk(&self, x: [[f64; 2]; 4]) -> Option<bool> {
        let (_, jac) = self.enclose(x)?;
        let c: [f64; 4] = x.map(|[a, b]| (a + b) / 2.0);
        let (f, _) = self.enclose(c.map(|v| [v, v]))?;
        krawczyk4(f, jac, x)
    }
}

/// The solution of `m x = r` by Gaussian elimination with partial
/// pivoting; `None` when `m` is singular.
fn solve4(mut m: [[f64; 4]; 4], mut r: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let piv = (col..4).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[piv][col] == 0.0 || !m[piv][col].is_finite() {
            return None;
        }
        m.swap(col, piv);
        r.swap(col, piv);
        for row in col + 1..4 {
            let f = m[row][col] / m[col][col];
            let pivot = m[col];
            for (x, p) in m[row].iter_mut().zip(pivot).skip(col) {
                *x -= f * p;
            }
            r[row] -= f * r[col];
        }
    }
    let mut x = [0.0; 4];
    for row in (0..4).rev() {
        let mut s = r[row];
        for k in row + 1..4 {
            s -= m[row][k] * x[k];
        }
        x[row] = s / m[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The inverse of `m`, column by column; `None` when it is singular.
fn inverse4(m: [[f64; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    let mut inv = [[0.0; 4]; 4];
    for col in 0..4 {
        let mut e = [0.0; 4];
        e[col] = 1.0;
        let x = solve4(m, e)?;
        for row in 0..4 {
            inv[row][col] = x[row];
        }
    }
    Some(inv)
}

/// Krawczyk's test for four functions over a box, from their values at
/// its middle and their Jacobian over it.
fn krawczyk4(at: [Iv; 4], jac: [[Iv; 4]; 4], cell: [[f64; 2]; 4]) -> Option<bool> {
    let p = Iv::point;
    let mid = |x: Iv| (x.lo() + x.hi()) / 2.0;
    let y = inverse4(jac.map(|r| r.map(mid)))?;
    let x: [f64; 4] = cell.map(|[a, b]| (a + b) / 2.0);
    let boxes: Vec<Iv> = (0..4)
        .map(|k| Iv::new(cell[k][0], cell[k][1]).map(|b| b - p(x[k])))
        .collect::<Option<_>>()?;
    let (mut inside, mut apart) = (true, false);
    for row in 0..4 {
        let mut k = p(x[row]);
        for (j, &a) in at.iter().enumerate() {
            k = k - p(y[row][j]) * a;
        }
        for (col, &b) in boxes.iter().enumerate() {
            let mut e = p(if row == col { 1.0 } else { 0.0 });
            for (j, r) in jac.iter().enumerate() {
                e = e - p(y[row][j]) * r[col];
            }
            k = k + e * b;
        }
        let [lo, hi] = cell[row];
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

#[cfg(test)]
mod tests;
