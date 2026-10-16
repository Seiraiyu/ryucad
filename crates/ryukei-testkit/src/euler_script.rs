//! Euler operator sequences played from bytes, for the property tests and
//! the `euler` fuzz target.
//!
//! [`valid_step`] chooses an operator and arguments that keep a body
//! geometrically consistent: new points and curves lie in the plane of the
//! face they are added to, and merges only join coplanar faces that point
//! the same way. An operator whose topological precondition fails is
//! skipped; the kernel checks those itself.
//!
//! Phase 1 has no intersections, so the generator cannot tell locally
//! whether a new straight edge crosses its face's outline. It applies the
//! step and keeps it only if every planar face with straight edges still
//! winds counterclockwise with at least [`MIN_AREA`]; otherwise it rolls
//! the step back. After every step it keeps, [`ryukei::check::check`] must
//! find nothing.
//!
//! [`wild_step`] calls an operator on arbitrary IDs and geometry. It must
//! never panic; an error must leave the body unchanged, and success must
//! leave its topology valid.

use ryukei::Error;
use ryukei::euler::{self, Corner};
use ryukei::geom::{Circle, Curve, Frame, Line, Point3, Rigid, Surface, Vector3};
use ryukei::name::{Journal, OpKey};
use ryukei::predicates::{Sign, winding_sign};
use ryukei::primitive::block;
use ryukei::topo::{Body, CurveId, EdgeId, FaceId, FinId, LoopId, Sense, VertexId};
use std::collections::BTreeSet;

/// Choices read from a byte string; past its end every choice is 0.
pub struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.at >= self.data.len()
    }

    pub fn byte(&mut self) -> u8 {
        let b = self.data.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        b
    }

    /// One of `n` choices, `n > 0`.
    pub fn pick(&mut self, n: usize) -> usize {
        let x = usize::from(self.byte()) << 8 | usize::from(self.byte());
        x % n
    }

    /// One of the items, or `None` when there are none.
    pub fn one<T: Copy>(&mut self, items: &[T]) -> Option<T> {
        (!items.is_empty()).then(|| items[self.pick(items.len())])
    }

    /// A number in `[lo, hi]`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * f64::from(self.byte()) / 255.0
    }
}

/// The body every sequence starts from: a 4 m box at the origin.
pub fn start() -> Body {
    block(OpKey(1), 4.0, 4.0, 4.0, &Rigid::IDENTITY).unwrap().0
}

/// The operators, by the name [`valid_step`] reports.
pub const OPERATORS: [&str; 13] = [
    "mev",
    "mef",
    "mef-closed",
    "mekr",
    "mvr",
    "semv",
    "kev",
    "kef",
    "kemr",
    "kvr",
    "jekv",
    "kfmrh",
    "mfkrh",
];

fn plane_frame(b: &Body, f: FaceId) -> Option<Frame> {
    match b.surface(b.face(f)?.surface)? {
        Surface::Plane(_) => b.surface(b.face(f)?.surface)?.frame().copied(),
        _ => None,
    }
}

fn point(b: &Body, v: VertexId) -> Point3 {
    b.vertex(v).unwrap().point
}

/// The corners of a loop: each fin's start, or its lone vertex.
fn corners(b: &Body, l: LoopId) -> Vec<(Corner, VertexId)> {
    match b.loop_(l).unwrap().vertex {
        Some(v) => vec![(Corner::Lone(l), v)],
        None => b
            .loop_fins(l)
            .map(|f| (Corner::Before(f), b.fin_start(f).unwrap()))
            .collect(),
    }
}

/// A point in the face's plane near `p`.
fn nearby(bytes: &mut Bytes, frame: &Frame, p: Point3) -> Point3 {
    let (a, c) = (bytes.range(-0.5, 0.5), bytes.range(-0.5, 0.5));
    p + frame.x().get() * a + frame.y().get() * c
}

/// A straight edge from `p` to `q`.
fn line(b: &mut Body, p: Point3, q: Point3) -> Option<(CurveId, [f64; 2])> {
    let l = Line::through(p, q).ok()?;
    (p.distance(q) > 1e-6).then(|| (b.add_curve(Curve::Line(l)), [0.0, p.distance(q)]))
}

/// A circle in the face's plane through `p`, starting there.
fn circle(b: &mut Body, bytes: &mut Bytes, frame: &Frame, p: Point3) -> (CurveId, [f64; 2]) {
    let r = bytes.range(0.05, 0.5);
    let x = frame.x().get();
    let f = Frame::new(p + x * r, frame.z().get(), -x).unwrap();
    let c = b.add_curve(Curve::Circle(Circle::new(f, r).unwrap()));
    (c, [0.0, core::f64::consts::TAU])
}

/// Fan triangles of a run of corner points.
fn fan(points: &[Point3]) -> Vec<[Point3; 3]> {
    (1..points.len().saturating_sub(1))
        .map(|i| [points[0], points[i], points[i + 1]])
        .collect()
}

/// Whether a fin's edge is straight.
fn straight_fin(b: &Body, f: FinId) -> bool {
    let e = b.edge(b.fin(f).unwrap().edge).unwrap();
    matches!(b.curve(e.curve), Some(Curve::Line(_)))
}

/// The corners of a run of fins and the corner after them, or `None` when
/// one of the fins is curved: the checker leaves such faces alone.
fn run(b: &Body, fins: &[FinId], then: FinId) -> Option<Vec<Point3>> {
    let mut pts = Vec::new();
    for &f in fins {
        if !straight_fin(b, f) {
            return None;
        }
        pts.push(point(b, b.fin_start(f).unwrap()));
    }
    pts.push(point(b, b.fin_start(then).unwrap()));
    Some(pts)
}

/// A whole loop's corners, or `None` when it is curved.
fn loop_points(b: &Body, l: LoopId) -> Option<Vec<Point3>> {
    let fins: Vec<FinId> = b.loop_fins(l).collect();
    match fins.split_last() {
        None => Some(Vec::new()),
        Some((&last, rest)) => {
            let mut pts = run(b, rest, last)?;
            straight_fin(b, last).then_some(())?;
            pts.dedup();
            Some(pts)
        }
    }
}

/// How a face made of these loops winds about `n`, as the checker decides
/// it; `None` when any loop is curved.
fn winding(loops: &[Option<Vec<Point3>>], n: Vector3) -> Option<Sign> {
    let mut triangles = Vec::new();
    for l in loops {
        triangles.extend(fan(l.as_ref()?));
    }
    Some(winding_sign(&triangles, n))
}

/// A face the generator makes must have at least this area (m²), so that
/// the rounding in a later SEMV point (about 1e-16 m off its line) can
/// never tip its winding.
const MIN_AREA: f64 = 1e-6;

/// Whether a face made of these loops is curved (the checker leaves it
/// alone) or winds counterclockwise about `n` with room to spare.
fn robust(loops: &[Option<Vec<Point3>>], n: Vector3) -> bool {
    if loops.iter().any(Option::is_none) {
        return true;
    }
    let area: f64 = loops
        .iter()
        .flatten()
        .flat_map(|l| fan(l))
        .map(|[a, b, c]| (b - a).cross(c - a).dot(n) / 2.0)
        .sum();
    winding(loops, n) == Some(Sign::Positive) && area >= MIN_AREA
}

/// The face's outward normal.
fn normal(b: &Body, f: FaceId, frame: &Frame) -> Vector3 {
    let n = frame.z().get();
    if b.face(f).unwrap().sense == Sense::Forward {
        n
    } else {
        -n
    }
}

/// Whether every planar face the checker can judge (all edges straight)
/// winds counterclockwise about its normal with room to spare. Choices
/// made inside a curved face go unjudged until an operator removes its
/// curves, so this is checked over every face after every step.
fn faces_robust(b: &Body) -> bool {
    b.faces().all(|f| {
        let Some(frame) = plane_frame(b, f) else {
            return true;
        };
        let loops: Vec<_> = b.face_loops(f).map(|l| loop_points(b, l)).collect();
        robust(&loops, normal(b, f, &frame))
    })
}

/// Points a planar face the way its loops wind, as a caller must after
/// MFKRH.
fn orient(b: &mut Body, f: FaceId, journal: &mut Journal) {
    let Some(frame) = plane_frame(b, f) else {
        return;
    };
    let loops: Vec<_> = b.face_loops(f).map(|l| loop_points(b, l)).collect();
    if winding(&loops, normal(b, f, &frame)) == Some(Sign::Negative) {
        let face = b.face(f).unwrap();
        let (s, sense) = (face.surface, face.sense.flipped());
        journal.append(euler::set_surface(b, f, s, sense).unwrap());
    }
}

/// The exact inverse of a step that made something, by the IDs it made.
#[derive(Clone, Copy, Debug)]
pub enum Undo {
    Kev(EdgeId, VertexId),
    Kef(EdgeId),
    Jekv(VertexId),
    Kvr(LoopId),
}

impl Undo {
    /// Applies the inverse operator.
    pub fn apply(self, b: &mut Body, key: OpKey) -> Result<(), Error> {
        match self {
            Self::Kev(e, v) => euler::kev(b, e, v).map(drop),
            Self::Kef(e) => euler::kef(b, key, e).map(drop),
            Self::Jekv(v) => euler::jekv(b, key, v).map(drop),
            Self::Kvr(l) => euler::kvr(b, l).map(drop),
        }
    }
}

/// What [`valid_step`] did.
pub struct Step {
    pub op: &'static str,
    pub journal: Journal,
    pub undo: Option<Undo>,
}

/// Picks one operator and applies it where it keeps the body valid.
/// `None` when nothing changed: no target, or a precondition the kernel
/// refused.
pub fn valid_step(b: &mut Body, key: OpKey, bytes: &mut Bytes) -> Option<Step> {
    let before = b.clone();
    let step = try_step(b, key, bytes)?;
    if faces_robust(b) {
        Some(step)
    } else {
        *b = before;
        None
    }
}

fn try_step(b: &mut Body, key: OpKey, bytes: &mut Bytes) -> Option<Step> {
    let op = OPERATORS[bytes.pick(OPERATORS.len())];
    let faces: Vec<FaceId> = b.faces().filter(|&f| plane_frame(b, f).is_some()).collect();
    let edges: Vec<EdgeId> = b.edges().collect();
    let vertices: Vec<VertexId> = b.vertices().collect();
    let loops: Vec<LoopId> = b.loops().collect();
    let journal = match op {
        "mev" => {
            let f = bytes.one(&faces)?;
            let frame = plane_frame(b, f)?;
            let l = bytes.one(&b.face_loops(f).collect::<Vec<_>>())?;
            let (at, v) = bytes.one(&corners(b, l))?;
            let q = nearby(bytes, &frame, point(b, v));
            let (c, t) = line(b, point(b, v), q)?;
            let (m, j) = euler::mev(b, key, at, q, c, t).ok()?;
            (j, Some(Undo::Kev(m.edge, m.vertex)))
        }
        "mef" => {
            let f = bytes.one(&faces)?;
            let l = bytes.one(&b.face_loops(f).collect::<Vec<_>>())?;
            let cs = corners(b, l);
            let ((from, va), (to, vb)) = (bytes.one(&cs)?, bytes.one(&cs)?);
            let (Corner::Before(_), Corner::Before(_)) = (from, to) else {
                return None;
            };
            if va == vb {
                return None;
            }
            let (curve, t) = line(b, point(b, va), point(b, vb))?;
            let (m, j) = euler::mef(b, key, from, to, curve, t).ok()?;
            (j, Some(Undo::Kef(m.edge)))
        }
        "mef-closed" => {
            let f = bytes.one(&faces)?;
            let frame = plane_frame(b, f)?;
            let l = bytes.one(&b.face_loops(f).collect::<Vec<_>>())?;
            let (at, v) = bytes.one(&corners(b, l))?;
            let (c, t) = circle(b, bytes, &frame, point(b, v));
            let (m, j) = euler::mef(b, key, at, at, c, t).ok()?;
            (j, Some(Undo::Kef(m.edge)))
        }
        "mekr" => {
            let f = bytes.one(&faces)?;
            let ls: Vec<LoopId> = b.face_loops(f).collect();
            if ls.len() < 2 {
                return None;
            }
            let i = bytes.pick(ls.len());
            let j = (i + 1 + bytes.pick(ls.len() - 1)) % ls.len();
            let (from, va) = bytes.one(&corners(b, ls[i]))?;
            let (to, vb) = bytes.one(&corners(b, ls[j]))?;
            let (c, t) = line(b, point(b, va), point(b, vb))?;
            (euler::mekr(b, key, from, to, c, t).ok()?.1, None)
        }
        "mvr" => {
            let f = bytes.one(&faces)?;
            let frame = plane_frame(b, f)?;
            let q = nearby(bytes, &frame, frame.origin());
            let (m, j) = euler::mvr(b, key, f, q).ok()?;
            (j, Some(Undo::Kvr(m.loop_)))
        }
        "semv" => {
            let e = bytes.one(&edges)?;
            let edge = b.edge(e)?;
            let [t0, t1] = edge.interval;
            let t = t0 + (t1 - t0) * bytes.range(0.1, 0.9);
            let q = b.curve(edge.curve)?.point(t);
            let (m, j) = euler::semv(b, key, e, t, q).ok()?;
            (j, Some(Undo::Jekv(m.vertex)))
        }
        "kev" => {
            let e = bytes.one(&edges)?;
            let edge = b.edge(e)?;
            let v = if bytes.byte().is_multiple_of(2) {
                edge.start
            } else {
                edge.end
            };
            (euler::kev(b, e, v).ok()?, None)
        }
        "kef" => {
            let e = bytes.one(&edges)?;
            let fins: Vec<FinId> = b.edge_fins(e).collect();
            let [f1, f2] = fins[..] else { return None };
            let face = |f: FinId| {
                b.face(b.loop_(b.fin(f).unwrap().loop_).unwrap().face)
                    .unwrap()
            };
            let (x, y) = (face(f1), face(f2));
            if (x.surface, x.sense) != (y.surface, y.sense) {
                return None;
            }
            (euler::kef(b, key, e).ok()?.1, None)
        }
        "kemr" => (euler::kemr(b, bytes.one(&edges)?).ok()?.1, None),
        "kvr" => (euler::kvr(b, bytes.one(&loops)?).ok()?, None),
        "jekv" => (euler::jekv(b, key, bytes.one(&vertices)?).ok()?.1, None),
        "kfmrh" => {
            let (keep, kill) = (bytes.one(&faces)?, bytes.one(&faces)?);
            let (x, y) = (b.face(keep)?, b.face(kill)?);
            if (x.surface, x.sense) != (y.surface, y.sense) {
                return None;
            }
            (euler::kfmrh(b, key, keep, kill).ok()?.1, None)
        }
        "mfkrh" => {
            let l = bytes.one(&loops)?;
            let old = b.loop_(l)?.face;
            let (m, mut journal) = euler::mfkrh(b, key, l).ok()?;
            orient(b, old, &mut journal);
            orient(b, m.face, &mut journal);
            (journal, None)
        }
        _ => unreachable!(),
    };
    let (journal, undo) = journal;
    Some(Step { op, journal, undo })
}

/// Every ID a wild sequence has seen, live or since killed.
#[derive(Default)]
pub struct Pool {
    faces: BTreeSet<FaceId>,
    loops: BTreeSet<LoopId>,
    fins: BTreeSet<FinId>,
    edges: BTreeSet<EdgeId>,
    vertices: BTreeSet<VertexId>,
}

impl Pool {
    fn absorb(&mut self, b: &Body) {
        self.faces.extend(b.faces());
        self.loops.extend(b.loops());
        self.fins.extend(b.fins());
        self.edges.extend(b.edges());
        self.vertices.extend(b.vertices());
    }
}

/// Any member of a set, or the first of `fallback` when it is empty.
fn any<T: Copy>(bytes: &mut Bytes, set: &BTreeSet<T>, fallback: T) -> T {
    let items: Vec<T> = set.iter().copied().collect();
    bytes.one(&items).unwrap_or(fallback)
}

/// Calls one operator on IDs from `pool` (dead ones included) and
/// arbitrary geometry. Errors are expected.
pub fn wild_step(
    b: &mut Body,
    key: OpKey,
    bytes: &mut Bytes,
    pool: &mut Pool,
) -> (&'static str, Result<(), Error>) {
    pool.absorb(b);
    let op = OPERATORS[bytes.pick(OPERATORS.len())];
    let p = &*pool;
    let (f0, l0) = (*p.faces.first().unwrap(), *p.loops.first().unwrap());
    let (n0, e0) = (*p.fins.first().unwrap(), *p.edges.first().unwrap());
    let v0 = *p.vertices.first().unwrap();
    let corner = |bytes: &mut Bytes| {
        if bytes.byte().is_multiple_of(3) {
            Corner::Lone(any(bytes, &p.loops, l0))
        } else {
            Corner::Before(any(bytes, &p.fins, n0))
        }
    };
    let (a, c) = (corner(bytes), corner(bytes));
    let q = Point3::new(
        bytes.range(-1.0, 5.0),
        bytes.range(-1.0, 5.0),
        bytes.range(-1.0, 5.0),
    );
    let t = [bytes.range(-2.0, 2.0), bytes.range(-2.0, 2.0)];
    let (face, other) = (any(bytes, &p.faces, f0), any(bytes, &p.faces, f0));
    let (lp, edge, vertex) = (
        any(bytes, &p.loops, l0),
        any(bytes, &p.edges, e0),
        any(bytes, &p.vertices, v0),
    );
    let curve = b.add_curve(Curve::Line(
        Line::through(Point3::ORIGIN, Point3::new(1.0, 2.0, 3.0)).unwrap(),
    ));
    let r = match op {
        "mev" => euler::mev(b, key, a, q, curve, t).map(drop),
        "mef" | "mef-closed" => euler::mef(b, key, a, c, curve, t).map(drop),
        "mekr" => euler::mekr(b, key, a, c, curve, t).map(drop),
        "mvr" => euler::mvr(b, key, face, q).map(drop),
        "semv" => euler::semv(b, key, edge, t[0], q).map(drop),
        "kev" => euler::kev(b, edge, vertex).map(drop),
        "kef" => euler::kef(b, key, edge).map(drop),
        "kemr" => euler::kemr(b, edge).map(drop),
        "kvr" => euler::kvr(b, lp).map(drop),
        "jekv" => euler::jekv(b, key, vertex).map(drop),
        "kfmrh" => euler::kfmrh(b, key, face, other).map(drop),
        "mfkrh" => euler::mfkrh(b, key, lp).map(drop),
        _ => unreachable!(),
    };
    (op, r)
}
