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

use super::cells::{cells, cells_off};
use super::pipe::Cell;
use super::sheet_cs::krawczyk3;
use super::ss_trace::{TangentTrace, Topology, Traced};
use super::{Failure, SLACK};
use crate::geom::bound2::IVec3;
use crate::geom::jet::JVec;
use crate::geom::sheet::Sheet;
use crate::geom::{Box3, Point3, Surface};
use crate::interval::Interval;
use crate::tol::LINEAR_RESOLUTION;
use std::collections::BTreeMap;
use std::rc::Rc;

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

/// How many times a pair is split before one where the surfaces may be
/// tangent is tested for a point of tangency, and kept whole if it holds
/// one: a curve along which they touch would leave singular pairs all
/// along it at [`MAX_DEPTH`].
const TANGENT_DEPTH: u32 = 20;

/// The touch rule's reach: surfaces within it of each other touch.
const REACH: f64 = 100.0 * LINEAR_RESOLUTION;

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

/// A cell of a surface with its enclosures: its jets over the cell and
/// at its middle, the box of its points, and its normal at the middle.
struct Enc {
    cell: Cell,
    over: JVec<Iv>,
    mid: JVec<Iv>,
    bx: Box3,
    normal: Option<[Iv; 3]>,
}

impl Enc {
    fn new(sheet: &Sheet, cell: Cell) -> Option<Self> {
        let over = sheet.jet_box(cell)?;
        let m = cell.map(|[a, b]| (a + b) / 2.0);
        let mid = sheet.jet_box(m.map(|x| [x, x]))?;
        Some(Self {
            cell,
            bx: point_box(&over).grow(SLACK),
            normal: mid_normal(&mid),
            over,
            mid,
        })
    }
}

/// Whether two cells are certified apart: their boxes miss, or a
/// component of `E = A - B`, or `n · E` along either surface's normal at
/// its cell's middle (where its own spread is least), has one certified
/// sign by the mean value form about the cells' middles.
fn excluded(a: &Enc, b: &Enc) -> bool {
    if !a.bx.intersects(&b.bx) {
        return true;
    }
    let w = |c: Cell, k: usize| {
        let m = (c[k][0] + c[k][1]) / 2.0;
        // Rounded outward: the offsets are subtracted in intervals.
        Iv::new(c[k][0], c[k][1]).map(|x| x - Iv::point(m))
    };
    let (Some(du), Some(dv), Some(ds), Some(dt)) =
        (w(a.cell, 0), w(a.cell, 1), w(b.cell, 0), w(b.cell, 1))
    else {
        return false;
    };
    let (pa, pb, qa, qb) = (&a.over, &b.over, &a.mid, &b.mid);
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
    let value = |p: &JVec<Iv>| -> IVec3 { [p[0].v, p[1].v, p[2].v] };
    [a.normal, b.normal].into_iter().flatten().any(|n| {
        let lin = along(n, value(qa)) - along(n, value(qb))
            + along(n, partial(pa, 0)) * du
            + along(n, partial(pa, 1)) * dv
            - along(n, partial(pb, 0)) * ds
            - along(n, partial(pb, 1)) * dt;
        signed(lin)
    })
}

/// The most times [`apart`] splits a pair.
const APART_DEPTH: u32 = 64;

/// The box of a surface's points over a cell: the union of its
/// enclosures over the cell's quarters, quartered again where there is
/// none (a pipe's frame over too long a stretch), up to `depth` times.
fn enclosure_box(sheet: &Sheet, cell: Cell, depth: u32) -> Option<Box3> {
    if let Some(p) = sheet.jet_box(cell) {
        return Some(point_box(&p));
    }
    if depth == 0 {
        return None;
    }
    let mid = |[a, b]: [f64; 2]| (a + b) / 2.0;
    let (mu, mv) = (mid(cell[0]), mid(cell[1]));
    let mut out: Option<Box3> = None;
    for u in [[cell[0][0], mu], [mu, cell[0][1]]] {
        for v in [[cell[1][0], mv], [mv, cell[1][1]]] {
            let b = enclosure_box(sheet, [u, v], depth - 1)?;
            out = Some(match out {
                Some(o) => o.union(&b),
                None => b,
            });
        }
    }
    out
}

/// A part of a cell in [`apart`], how many times it was halved, and the
/// other surface's cells it isn't yet certified apart from, each with how
/// many times it was halved.
type Part = (Cell, u32, Vec<(Rc<Enc>, u32)>);

/// Whether surface `a` over `cell` and surface `b` are certified apart,
/// leaving out parts of `a`'s cell that `skip` rules out. Each part of
/// `a`'s cell carries the cells of `b` it isn't yet certified apart from
/// (first, `b`'s cells near the whole cell), which its halves inherit:
/// a part with none left is certified, one skipped is dropped, and
/// otherwise whichever spreads further is halved, the part or its cells of
/// `b`, so `b` is split only where some part of `a` needs it.
/// `Some(false)` when a part or a cell has been halved [`APART_DEPTH`]
/// times, `None` when `budget` (pairs tested) runs out.
pub(crate) fn apart(
    a: &Surface,
    cell: Cell,
    b: &Surface,
    skip: &dyn Fn(Cell) -> bool,
    budget: usize,
) -> Option<bool> {
    apart_within(a, cell, b, None, skip, &|_| false, budget)
}

/// [`apart`] against `b` over its cell `within` only, when given,
/// leaving out cells of `b` that `skip_b` rules out.
pub(crate) fn apart_within(
    a: &Surface,
    cell: Cell,
    b: &Surface,
    within: Option<Cell>,
    skip: &dyn Fn(Cell) -> bool,
    skip_b: &dyn Fn(Cell) -> bool,
    mut budget: usize,
) -> Option<bool> {
    let sheets = [Sheet::of(a), Sheet::of(b)];
    let Some(near) = enclosure_box(&sheets[0], cell, 12) else {
        return Some(false);
    };
    let near = near.grow(SLACK);
    let spread = |p: &JVec<Iv>, c: Cell| -> (usize, f64) {
        (0..2)
            .map(|d| {
                let w = c[d][1] - c[d][0];
                (d, crate::intersect::sheet_cs::norm_hi(partial(p, d)) * w)
            })
            .fold((0, -1.0), |best, x| if x.1 > best.1 { x } else { best })
    };
    let halves = |c: Cell, d: usize| -> Option<[Cell; 2]> {
        let [lo, hi] = c[d];
        let m = (lo + hi) / 2.0;
        if m <= lo || m >= hi {
            return None;
        }
        let (mut x, mut y) = (c, c);
        x[d] = [lo, m];
        y[d] = [m, hi];
        Some([x, y])
    };
    let enclose = |k: usize, c: Cell| Enc::new(&sheets[k], c);
    let clip = |c: Cell| -> Option<Cell> {
        let Some(w) = within else { return Some(c) };
        let out: Cell = core::array::from_fn(|k| [c[k][0].max(w[k][0]), c[k][1].min(w[k][1])]);
        (out[0][0] < out[0][1] && out[1][0] < out[1][1]).then_some(out)
    };
    let Some(firsts) = cells(b, &near)
        .into_iter()
        .filter_map(clip)
        .filter(|&c| !skip_b(c))
        .map(|c| enclose(1, c).map(|e| (Rc::new(e), 0)))
        .collect::<Option<Vec<(Rc<Enc>, u32)>>>()
    else {
        return Some(false);
    };
    let mut stack: Vec<Part> = vec![(cell, 0, firsts)];
    while let Some((part, depth, others)) = stack.pop() {
        if skip(part) {
            continue;
        }
        let Some(ea) = enclose(0, part) else {
            // No enclosure over the part (a pipe's frame over too long a
            // stretch): halve it along its wider parameter.
            if depth >= APART_DEPTH {
                return Some(false);
            }
            let d = usize::from(part[1][1] - part[1][0] > part[0][1] - part[0][0]);
            let [x, y] = halves(part, d)?;
            stack.push((x, depth + 1, others.clone()));
            stack.push((y, depth + 1, others));
            continue;
        };
        // The cells of `b` this part isn't certified apart from.
        let mut left: Vec<(Rc<Enc>, u32)> = Vec::new();
        for (eb, db) in others {
            budget = budget.checked_sub(1)?;
            if !excluded(&ea, &eb) {
                left.push((eb, db));
            }
        }
        if left.is_empty() {
            continue;
        }
        let (da, sa) = spread(&ea.over, part);
        let widest = left
            .iter()
            .map(|(e, _)| spread(&e.over, e.cell).1)
            .fold(0.0, f64::max);
        if sa >= widest {
            if depth >= APART_DEPTH {
                return Some(false);
            }
            let [x, y] = halves(part, da)?;
            stack.push((x, depth + 1, left.clone()));
            stack.push((y, depth + 1, left));
        } else {
            let mut split = Vec::with_capacity(2 * left.len());
            for (e, d) in left {
                if d >= APART_DEPTH {
                    return Some(false);
                }
                let (dd, _) = spread(&e.over, e.cell);
                let Some(hs) = halves(e.cell, dd) else {
                    return Some(false);
                };
                for h in hs {
                    if skip_b(h) {
                        continue;
                    }
                    // A cell with no enclosure can't be excluded.
                    let Some(e) = enclose(1, h) else {
                        return Some(false);
                    };
                    split.push((Rc::new(e), d + 1));
                }
            }
            stack.push((part, depth, split));
        }
    }
    Some(true)
}

/// The topology of where `a` and `b` meet inside `region`, traced in
/// their four parameters. Fails with `Uncertain` where a crossing, a
/// tangency or a pairing can't be certified (a tangency along a curve
/// among them).
pub(crate) fn trace(a: &Surface, b: &Surface, region: &Box3) -> Result<Topology, Failure> {
    for (attempt, split) in SPLITS.into_iter().enumerate() {
        let mut tracer = Tracer::new([a, b], region, split);
        tracer.shift = CELL_SHIFTS[attempt];
        match tracer.run() {
            Err(Failure::Uncertain) if tracer.singular <= MAX_SINGULAR => continue,
            other => return other,
        }
    }
    Err(Failure::Uncertain)
}

/// How a cluster of singular pairs is settled.
#[derive(Clone, Debug)]
enum Settled {
    /// One point where the surfaces touch (or where branches meet).
    Point(Point3),
    /// A curve along which they touch.
    Curve(TangentTrace),
}

/// The most steps a march along a tangent curve may take.
const MARCH_STEPS: usize = 1 << 16;

/// A polyline's segments by the cubes of a grid they reach, for finding
/// those near a point.
struct Segments {
    size: f64,
    cubes: BTreeMap<[i64; 3], Vec<[Point3; 2]>>,
}

impl Segments {
    /// The segments of `line` in cubes of side `size`: each in every cube
    /// its box meets.
    fn new(line: &[Point3], size: f64) -> Self {
        let mut cubes: BTreeMap<[i64; 3], Vec<[Point3; 2]>> = BTreeMap::new();
        let cube = |p: Point3| [p.x, p.y, p.z].map(|x| (x / size).floor() as i64);
        let pairs: Vec<[Point3; 2]> = if line.len() == 1 {
            vec![[line[0], line[0]]]
        } else {
            line.windows(2).map(|w| [w[0], w[1]]).collect()
        };
        for seg in pairs {
            let (a, b) = (cube(seg[0]), cube(seg[1]));
            for x in a[0].min(b[0])..=a[0].max(b[0]) {
                for y in a[1].min(b[1])..=a[1].max(b[1]) {
                    for z in a[2].min(b[2])..=a[2].max(b[2]) {
                        cubes.entry([x, y, z]).or_default().push(seg);
                    }
                }
            }
        }
        Self { size, cubes }
    }

    /// Whether a segment passes within `reach` (at most the cubes' side)
    /// of `p`: those in the cubes round its own.
    fn within(&self, p: Point3, reach: f64) -> bool {
        let c = [p.x, p.y, p.z].map(|x| (x / self.size).floor() as i64);
        for x in c[0] - 1..=c[0] + 1 {
            for y in c[1] - 1..=c[1] + 1 {
                for z in c[2] - 1..=c[2] + 1 {
                    let Some(segs) = self.cubes.get(&[x, y, z]) else {
                        continue;
                    };
                    for &[a, b] in segs {
                        let d = b - a;
                        let len2 = d.length_squared();
                        let t = if len2 > 0.0 {
                            ((p - a).dot(d) / len2).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        if (a + d * t).distance(p) <= reach {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
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
    /// How the first cells are cut ([`cells_off`]).
    shift: f64,
    leaves: Vec<Leaf>,
    singular: usize,
    crossings: Vec<Crossing>,
    planes: BTreeMap<Plane, Vec<usize>>,
}

/// The first cells' shift on each attempt: the surfaces' own pieces
/// twice (with two of [`SPLITS`]), then cut off them.
const CELL_SHIFTS: [f64; 3] = [0.0, 0.0, 0.293];

impl<'a> Tracer<'a> {
    fn new(surfaces: [&'a Surface; 2], region: &Box3, split: f64) -> Self {
        Self {
            surfaces,
            sheets: surfaces.map(Sheet::of),
            region: region.grow(LINEAR_RESOLUTION),
            split,
            shift: 0.0,
            leaves: Vec::new(),
            singular: 0,
            crossings: Vec::new(),
            planes: BTreeMap::new(),
        }
    }

    fn run(&mut self) -> Result<Topology, Failure> {
        let firsts = [
            cells_off(self.surfaces[0], &self.region, self.shift),
            // The second moved differently, as it is split differently.
            cells_off(
                self.surfaces[1],
                &self.region,
                if self.shift > 0.0 {
                    1.0 - self.shift
                } else {
                    0.0
                },
            ),
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
        // On the attempt off the surfaces' pieces the second surface is
        // split elsewhere too: surfaces sharing a parameter (sides of one
        // sweep, along its path) would otherwise put their cells' sides
        // together where the curve crosses.
        let fraction = if k == 1 && self.shift > 0.0 {
            1.0 - self.split
        } else {
            self.split
        };
        let m = lo + (hi - lo) * fraction;
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
        if depth >= MAX_DEPTH || (depth >= TANGENT_DEPTH && self.near_tangency(cells)) {
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

    /// Whether a pair holds a point where the surfaces are tangent and
    /// within the touch rule's reach of each other: Gauss-Newton steps on
    /// the tangency system from its middle come to rest inside it (grown
    /// by a quarter). Only the place to stop splitting depends on it.
    fn near_tangency(&self, cells: [Cell; 2]) -> bool {
        let tan = Tangency {
            sheets: self.sheets,
        };
        let x0 = [
            (cells[0][0][0] + cells[0][0][1]) / 2.0,
            (cells[0][1][0] + cells[0][1][1]) / 2.0,
            (cells[1][0][0] + cells[1][0][1]) / 2.0,
            (cells[1][1][0] + cells[1][1][1]) / 2.0,
        ];
        let Some(x) = tan.settle(x0) else {
            return false;
        };
        let inside = (0..4).all(|k| {
            let [lo, hi] = cells[k / 2][k % 2];
            let q = (hi - lo) / 4.0;
            x[k] >= lo - q && x[k] <= hi + q
        });
        inside && tan.gap(x) <= REACH
    }

    fn excluded(&self, cells: [Cell; 2], pa: &JVec<Iv>, pb: &JVec<Iv>) -> bool {
        let enc = |k: usize, p: &JVec<Iv>| {
            let m = cells[k].map(|[a, b]| (a + b) / 2.0);
            let mid = self.sheets[k].jet_box(m.map(|x| [x, x]))?;
            Some(Enc {
                cell: cells[k],
                bx: point_box(p).grow(SLACK),
                normal: mid_normal(&mid),
                over: *p,
                mid,
            })
        };
        match (enc(0, pa), enc(1, pb)) {
            (Some(a), Some(b)) => excluded(&a, &b),
            _ => false,
        }
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
        on_edge || self.leaves(c.point)
    }

    /// Whether a point is outside the region or on its boundary (within
    /// [`SLACK`]): an unbounded direction's first cells end exactly where
    /// the region does, so a branch leaving through such a cell's end
    /// crosses it on the region's face, not strictly beyond it.
    fn leaves(&self, p: Point3) -> bool {
        let (lo, hi) = (self.region.min(), self.region.max());
        p.x < lo.x + SLACK
            || p.x > hi.x - SLACK
            || p.y < lo.y + SLACK
            || p.y > hi.y - SLACK
            || p.z < lo.z + SLACK
            || p.z > hi.z - SLACK
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
                // A transversal branch can't be told from the curve it
                // would leave.
                Some(k) if matches!(clusters[k].1, Settled::Curve(_)) => {
                    return Err(Failure::Uncertain);
                }
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
            if let Some(&k) = at_cluster.get(&chain[0])
                && let Settled::Point(p) = clusters[k].1
            {
                points.insert(0, p);
                touched[k] = true;
            }
            if let Some(&k) = at_cluster.get(chain.last().expect("a chain has a start"))
                && let Settled::Point(p) = clusters[k].1
            {
                points.push(p);
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
        for (k, (_, settled)) in clusters.into_iter().enumerate() {
            match settled {
                Settled::Point(p) if !touched[k] => out.points.push(p),
                Settled::Point(_) => {}
                Settled::Curve(c) => out.tangents.push(c),
            }
        }
        Ok(out)
    }

    /// The singular leaves grouped by touching (a periodic direction's
    /// cells a period apart touching too), each group settled: one
    /// certified point of tangency where the surfaces touch, else a curve
    /// along which they touch.
    fn clusters(&self) -> Result<Vec<(Vec<usize>, Settled)>, Failure> {
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
        let periods = self.surfaces.map(|s| s.periods());
        let touch = |x: &Leaf, y: &Leaf| {
            (0..2).all(|k| {
                (0..2).all(|d| {
                    let (a, b) = (x.cells[k][d], y.cells[k][d]);
                    let p = if d == 0 { periods[k].0 } else { periods[k].1 };
                    let shifts: &[f64] = match p {
                        Some(p) => &[-p, 0.0, p],
                        None => &[0.0],
                    };
                    shifts.iter().any(|&o| a[0] <= b[1] + o && b[0] + o <= a[1])
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
                let settled = match self.tangency(&m) {
                    Ok(p) => Settled::Point(p),
                    Err(_) => Settled::Curve(self.tangent_curve(&m)?),
                };
                Ok((m, settled))
            })
            .collect()
    }

    /// The curve along which the surfaces touch through a cluster: from a
    /// point of tangency in it, marched both ways along the null direction
    /// of the tangency system ([`Self::march`]) until it closes, leaves a
    /// domain's edge or the region; every point within the touch rule's
    /// reach. Every part of the cluster's cells of the first surface
    /// outside a strip round the curve, `sqrt(2 × size × reach)` wide, must
    /// be certified apart from the second surface ([`apart`]), so all
    /// they share there is the curve.
    fn tangent_curve(&self, members: &[usize]) -> Result<TangentTrace, Failure> {
        let tan = Tangency {
            sheets: self.sheets,
        };
        let inside = |x: [f64; 4]| members.iter().any(|&i| self.holds(&self.leaves[i], x));
        let seed = members
            .iter()
            .find_map(|&i| {
                let c = self.leaves[i].cells;
                let mid = |k: usize, d: usize| (c[k][d][0] + c[k][d][1]) / 2.0;
                tan.settle([mid(0, 0), mid(0, 1), mid(1, 0), mid(1, 1)])
                    .filter(|&x| inside(x) && tan.gap(x) <= REACH)
            })
            .ok_or(Failure::Uncertain)?;
        // The size of the cluster's part of the first surface.
        let mut size_box: Option<Box3> = None;
        for &i in members {
            let b = enclosure_box(&self.sheets[0], self.leaves[i].cells[0], 8)
                .ok_or(Failure::Uncertain)?;
            size_box = Some(match size_box {
                Some(o) => o.union(&b),
                None => b,
            });
        }
        let size_box = size_box.ok_or(Failure::Uncertain)?;
        let size = (size_box.max() - size_box.min()).length();
        let strip = crate::math::sqrt(2.0 * size * REACH);
        let step = strip / 2.0;
        let (forward, closed) = self.march(&tan, seed, 1.0, step, &inside)?;
        let params = if closed {
            forward
        } else {
            let (mut back, _) = self.march(&tan, seed, -1.0, step, &inside)?;
            back.reverse();
            back.pop();
            back.extend(forward);
            back
        };
        if params.len() < 2 {
            return Err(Failure::Uncertain);
        }
        // Every part of the cluster's cells of the first surface outside
        // the strip apart from the second surface.
        let polyline: Vec<Point3> = params
            .iter()
            .map(|x| self.sheets[0].point(x[0], x[1]))
            .collect();
        let index = Segments::new(&polyline, strip);
        let cells: Vec<Cell> = members.iter().map(|&i| self.leaves[i].cells[0]).collect();
        let (pu, pv) = self.surfaces[0].periods();
        let meets = |part: Cell, c: &Cell| {
            let hit = |a: [f64; 2], b: [f64; 2], p: Option<f64>| {
                let shifts: &[f64] = match p {
                    Some(p) => &[-p, 0.0, p],
                    None => &[0.0],
                };
                shifts.iter().any(|&o| a[0] <= b[1] + o && b[0] + o <= a[1])
            };
            hit(part[0], c[0], pu) && hit(part[1], c[1], pv)
        };
        // Parts within the strip, or meeting none of the cluster's cells
        // (their pairs were settled apart or simple), are left out.
        let skip = |part: Cell| -> bool {
            if !cells.iter().any(|c| meets(part, c)) {
                return true;
            }
            let Some(p) = self.sheets[0].jet_box(part) else {
                return false;
            };
            let b = point_box(&p);
            let c = b.min() + (b.max() - b.min()) * 0.5;
            let half = (b.max() - b.min()).length() / 2.0;
            half <= strip && index.within(c, strip - half)
        };
        let mut hull = cells[0];
        for c in &cells {
            for d in 0..2 {
                hull[d] = [hull[d][0].min(c[d][0]), hull[d][1].max(c[d][1])];
            }
        }
        if apart(self.surfaces[0], hull, self.surfaces[1], &skip, 1 << 22) != Some(true) {
            return Err(Failure::Uncertain);
        }
        Ok(TangentTrace { params, closed })
    }

    /// Marches from `x0` along the null direction of the tangency system,
    /// `sign` choosing the way, in steps of about `step` in model space:
    /// each predicted point corrected by Newton's method on three of the
    /// four equations with the parameter the curve moves fastest in held,
    /// and within the touch rule's reach. Stops where the curve closes
    /// (`true`), or leaves the cluster (`inside`) at a bounded surface's
    /// domain edge (the last point put on it) or outside the region;
    /// leaving the cluster anywhere else is `Uncertain`.
    fn march(
        &self,
        tan: &Tangency,
        x0: [f64; 4],
        sign: f64,
        step: f64,
        inside: &dyn Fn([f64; 4]) -> bool,
    ) -> Result<(Vec<[f64; 4]>, bool), Failure> {
        let mut pts = vec![x0];
        let mut x = x0;
        let mut way: Option<[f64; 4]> = None;
        let start = self.sheets[0].point(x0[0], x0[1]);
        for _ in 0..MARCH_STEPS {
            let mut n = tan.null(x).ok_or(Failure::Uncertain)?;
            let flip = match way {
                None => sign < 0.0,
                Some(w) => (0..4).map(|k| n[k] * w[k]).sum::<f64>() < 0.0,
            };
            if flip {
                n = n.map(|v| -v);
            }
            way = Some(n);
            let held = (0..4)
                .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
                .expect("four parameters");
            let here = self.sheets[0].point(x[0], x[1]);
            let mut h = step;
            let y = loop {
                let pred: [f64; 4] = core::array::from_fn(|k| x[k] + h * n[k]);
                if let Some(y) = tan.correct(pred, held)
                    && tan.gap(y) <= REACH
                    && self.sheets[0].point(y[0], y[1]).distance(here) <= 2.0 * h
                {
                    break y;
                }
                h /= 2.0;
                if h < step / 1024.0 {
                    return Err(Failure::Uncertain);
                }
            };
            if !inside(y) {
                if let Some(end) = self.edge_end(tan, x, y) {
                    pts.push(end);
                    return Ok((pts, false));
                }
                if self.leaves(self.sheets[0].point(y[0], y[1])) {
                    pts.push(y);
                    return Ok((pts, false));
                }
                return Err(Failure::Uncertain);
            }
            if pts.len() > 3 && self.sheets[0].point(y[0], y[1]).distance(start) <= 1.5 * h {
                return Ok((pts, true));
            }
            pts.push(y);
            x = y;
        }
        Err(Failure::Uncertain)
    }

    /// Where the curve from `x` (inside both domains) to `y` leaves a
    /// bounded surface's domain: the parameter that runs out held at the
    /// domain's edge and the point corrected onto the curve there. `None`
    /// when `y` is inside both domains.
    fn edge_end(&self, tan: &Tangency, x: [f64; 4], y: [f64; 4]) -> Option<[f64; 4]> {
        for k in 0..4 {
            let s = self.surfaces[k / 2];
            let d = s.domain()[k % 2];
            let p = if k % 2 == 0 {
                s.periods().0
            } else {
                s.periods().1
            };
            if p.is_some() {
                continue;
            }
            let edge = if y[k] < d[0] {
                d[0]
            } else if y[k] > d[1] {
                d[1]
            } else {
                continue;
            };
            let f = (edge - x[k]) / (y[k] - x[k]);
            let mut pred: [f64; 4] = core::array::from_fn(|j| x[j] + f * (y[j] - x[j]));
            pred[k] = edge;
            return tan.correct(pred, k);
        }
        None
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
                .map(|k| Iv::new(x[k][0], x[k][1]).map(|v| v - Iv::point(m[k])))
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

    /// The system and its Jacobian at a point, in floating point.
    fn at(&self, x: [f64; 4]) -> Option<([f64; 4], [[f64; 4]; 4])> {
        let (f, jac) = self.enclose(x.map(|v| [v, v]))?;
        let mid = |v: Iv| (v.lo() + v.hi()) / 2.0;
        Some((f.map(mid), jac.map(|r| r.map(mid))))
    }

    /// The surfaces' distance at a point of their parameters.
    fn gap(&self, x: [f64; 4]) -> f64 {
        self.sheets[0]
            .point(x[0], x[1])
            .distance(self.sheets[1].point(x[2], x[3]))
    }

    /// Damped Gauss-Newton steps on the system from `x`, which may have
    /// no isolated root (a curve of tangency): `None` where they don't
    /// settle.
    fn settle(&self, mut x: [f64; 4]) -> Option<[f64; 4]> {
        for _ in 0..60 {
            let (f, j) = self.at(x)?;
            // (JᵀJ + μ I) dx = -Jᵀ f.
            let mut m = [[0.0; 4]; 4];
            let mut r = [0.0; 4];
            for a in 0..4 {
                for b in 0..4 {
                    m[a][b] = (0..4).map(|k| j[k][a] * j[k][b]).sum();
                }
                r[a] = -(0..4).map(|k| j[k][a] * f[k]).sum::<f64>();
            }
            let mu = 1e-12 * (0..4).map(|a| m[a][a]).fold(0.0, f64::max);
            for (a, row) in m.iter_mut().enumerate() {
                row[a] += mu;
            }
            let dx = solve4(m, r)?;
            let next: [f64; 4] = core::array::from_fn(|k| x[k] + dx[k]);
            if next.iter().any(|v| !v.is_finite()) {
                return None;
            }
            let small = (0..4).all(|k| dx[k].abs() <= 1e-14 * (1.0 + x[k].abs()));
            x = next;
            if small {
                return Some(x);
            }
        }
        Some(x)
    }

    /// The direction the curve of tangency runs at `x`: the null vector of
    /// the system's Jacobian (by inverse iteration on `JᵀJ`), scaled to
    /// unit speed of the first surface's point.
    fn null(&self, x: [f64; 4]) -> Option<[f64; 4]> {
        let (_, j) = self.at(x)?;
        let mut m = [[0.0; 4]; 4];
        for a in 0..4 {
            for b in 0..4 {
                m[a][b] = (0..4).map(|k| j[k][a] * j[k][b]).sum();
            }
        }
        let scale = (0..4).map(|a| m[a][a]).fold(0.0, f64::max);
        for (a, row) in m.iter_mut().enumerate() {
            row[a] += 1e-12 * scale;
        }
        let mut v = [0.5; 4];
        for _ in 0..4 {
            let w = solve4(m, v)?;
            let len = crate::math::sqrt(w.iter().map(|x| x * x).sum());
            if !(len > 0.0 && len.is_finite()) {
                return None;
            }
            v = w.map(|x| x / len);
        }
        let ja = self.sheets[0].jets_at(x[0], x[1]);
        let speed = |k: usize| ja[k].d[0] * v[0] + ja[k].d[1] * v[1];
        let s = crate::math::sqrt((0..3).map(|k| speed(k) * speed(k)).sum());
        (s > 0.0 && s.is_finite()).then(|| v.map(|x| x / s))
    }

    /// Newton's method from `x` on three of the system's four equations
    /// (those whose Jacobian without column `held` is best conditioned)
    /// with parameter `held` fixed: `None` where it doesn't converge.
    fn correct(&self, mut x: [f64; 4], held: usize) -> Option<[f64; 4]> {
        let free: Vec<usize> = (0..4).filter(|&k| k != held).collect();
        let det3 = |a: [[f64; 3]; 3]| {
            a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
                - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
        };
        for _ in 0..30 {
            let (f, j) = self.at(x)?;
            let rows = (0..4)
                .map(|drop| {
                    let r: Vec<usize> = (0..4).filter(|&k| k != drop).collect();
                    let m: [[f64; 3]; 3] =
                        core::array::from_fn(|a| core::array::from_fn(|b| j[r[a]][free[b]]));
                    (r, m)
                })
                .max_by(|a, b| det3(a.1).abs().total_cmp(&det3(b.1).abs()))?;
            let rhs: [f64; 3] = core::array::from_fn(|a| f[rows.0[a]]);
            let dx = super::sheet_cs::solve3(rows.1, rhs)?;
            for (a, &k) in free.iter().enumerate() {
                x[k] -= dx[a];
            }
            if x.iter().any(|v| !v.is_finite()) {
                return None;
            }
            if (0..3).all(|a| dx[a].abs() <= 1e-14 * (1.0 + x[free[a]].abs())) {
                return Some(x);
            }
        }
        None
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
