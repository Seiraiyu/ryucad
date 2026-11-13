//! Reference bodies (design 364): a file's topology as the file gives it,
//! for display, picking and reading geometry, never for modelling.
//!
//! [`reference`] runs healing's structural steps and none of its
//! certifying ones: import names, each edge's uses (at most two: a sheet
//! may leave one laminar), seams and poles out, loops and faces oriented
//! (kept as the file has them where that can't be done), tolerances
//! sampled at 33 points (grown for what lies between) rather than
//! certified and held to the cap,
//! pcurves exact or the file's but never fitted, and the layout built as
//! a sheet (design 295). Sewing, tightening, certified gaps, collapsing,
//! fitted pcurves, shell judging, self-union and [`crate::check::check`]
//! are left out: the body passes [`crate::check::check_topology`] and
//! nothing more, which is why it is always a sheet, and booleans, blends
//! and mass refuse it as `NotASolid`.
//!
//! Faults degrade rather than refuse. Each connected piece (faces joined
//! by edges) is tried as the file has it. A piece any step or the layout
//! refuses is tried face by face, each face a sheet of its own with its
//! own copies of its edges and vertices, and a face that fails alone is
//! left out. A copy of an edge or vertex in a later part takes a part of
//! its name (`Role::Part`), the part of the least face name keeping it, so
//! names stay unique. Faces keep their import names whatever happens, so
//! a face is named as healing names it.

use super::names::{self, Names};
use super::pcurves::{self, MaybePcurves};
use super::{Loop, Shape, Shell, Use, build, orient, pieces, unseam, validate};
use crate::check::check_topology;
use crate::geom::Curve;
use crate::name::{Name, OpKey, Role};
use crate::tol::{LINEAR_RESOLUTION, TOLERANCE_CAP};
use crate::topo::Body;
use crate::work::{Meter, Work};
use crate::{Error, HealError, TopologyError};
use std::collections::BTreeMap;

/// How far a reference may go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceOptions {
    /// The largest tolerance a reference gives an entity, in meters: a
    /// gap past it is held to it, and the mesher may then leave the face
    /// out.
    pub tolerance_cap: f64,
}

impl Default for ReferenceOptions {
    fn default() -> Self {
        Self {
            tolerance_cap: TOLERANCE_CAP,
        }
    }
}

/// What a reference did, by the shape's import names. Lists are sorted.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ReferenceReport {
    /// Seam edges removed.
    pub seams: Vec<Name>,
    /// Pole vertices dropped with their loops or seams.
    pub poles: Vec<Name>,
    /// Vertices removed between two edges on one curve, which were joined.
    pub joined: Vec<Name>,
    /// The file's edges split where they ran through a pole or apex.
    pub split: Vec<Name>,
    /// Faces some of whose loops ran against the region they bound, and
    /// were reversed.
    pub reversed: Vec<Name>,
    /// Faces turned over to agree with their neighbours.
    pub turned: Vec<Name>,
    /// Faces of parts whose loops or faces couldn't be oriented (loops
    /// that cross, a Möbius band): kept as the file has them.
    pub unoriented: Vec<Name>,
    /// Faces of pieces the layout refused, each laid out alone.
    pub split_faces: Vec<Name>,
    /// Faces that couldn't be laid out even alone, left out.
    pub dropped_faces: Vec<Name>,
    /// Laminar edges: the reference's boundaries.
    pub laminar: Vec<Name>,
    /// Edges and vertices whose tolerance is above
    /// [`LINEAR_RESOLUTION`], with it.
    pub tolerances: Vec<(Name, f64)>,
}

/// The reference body of `shape` named from `key`: a sheet (closed where
/// no edge is laminar), named as [`super::heal`] names the shape, and the
/// report of what it did. Refused only for a malformed shape, geometry
/// outside the size box, or a shape no face of which can be laid out
/// ([`HealError::Malformed`]).
pub fn reference(
    key: OpKey,
    shape: &Shape,
    options: &ReferenceOptions,
) -> Result<(Body, ReferenceReport), Error> {
    reference_with(key, shape, options, &Work::unlimited())
}

/// [`reference`] under `work`: refused with `Cancelled` or
/// `BudgetExceeded` when it stops, building nothing.
pub fn reference_with(
    key: OpKey,
    shape: &Shape,
    options: &ReferenceOptions,
    work: &Work,
) -> Result<(Body, ReferenceReport), Error> {
    crate::work::run(work, |m| reference_in(key, shape, options, m))
}

/// A part of the shape that laid out alone: its shape after the steps,
/// its names, its pcurves and what its steps did.
struct Laid {
    shape: Shape,
    names: Names,
    pcurves: MaybePcurves,
    report: ReferenceReport,
}

/// Why a part didn't lay out: whether its orientation had been kept as
/// the file had it on the way.
#[derive(Clone, Copy, Debug)]
struct Failed {
    unoriented: bool,
}

/// A piece's parts, and what trying it said beyond them (faces split,
/// dropped and left unoriented).
struct Tried {
    laid: Vec<Laid>,
    report: ReferenceReport,
}

fn reference_in(
    key: OpKey,
    shape: &Shape,
    options: &ReferenceOptions,
    m: &Meter,
) -> Result<(Body, ReferenceReport), Error> {
    validate(shape)?;
    let names = names::names(key, shape);
    let cap = options.tolerance_cap;
    let groups = pieces::pieces(shape);
    let tried = m.par_map(&groups, |m, faces| piece(key, shape, &names, faces, cap, m));
    if m.halted() {
        // `run` says why.
        return Err(Error::Cancelled);
    }
    let mut report = ReferenceReport::default();
    let mut parts: Vec<Laid> = Vec::new();
    for t in tried {
        merge(&mut report, t.report);
        parts.extend(t.laid);
    }
    if parts.is_empty() {
        return Err(HealError::Malformed {
            what: "no face of the shape could be laid out",
        }
        .into());
    }
    // In the order of their least face, so which copy keeps a name
    // doesn't depend on the order of the file.
    parts.sort_by_key(|p| p.names.faces.iter().min().copied());
    for p in &mut parts {
        merge(&mut report, core::mem::take(&mut p.report));
    }
    let (s, n, pc) = combine(key, parts);
    let body = build::assemble_reference(key, &s, &n, &pc)?;
    if !check_topology(&body).is_empty() {
        return Err(Error::Topology(TopologyError::Inconsistent));
    }
    report.laminar = body
        .edges()
        .filter(|&e| body.is_laminar(e))
        .map(|e| body.edges[e].name)
        .collect();
    for e in body.edges() {
        let t = body.edges[e].tolerance.get();
        if t > LINEAR_RESOLUTION {
            report.tolerances.push((body.edges[e].name, t));
        }
    }
    for v in body.vertices() {
        let t = body.vertices[v].tolerance.get();
        if t > LINEAR_RESOLUTION {
            report.tolerances.push((body.vertices[v].name, t));
        }
    }
    sort(&mut report);
    Ok((body, report))
}

/// One piece tried whole, then face by face.
fn piece(key: OpKey, s: &Shape, names: &Names, faces: &[usize], cap: f64, m: &Meter) -> Tried {
    let mut report = ReferenceReport::default();
    let (sub, n) = part(s, names, faces);
    match attempt(key, sub, n, cap, m) {
        Ok(laid) => Tried {
            laid: vec![laid],
            report,
        },
        Err(failed) => {
            if failed.unoriented {
                report
                    .unoriented
                    .extend(faces.iter().map(|&k| names.faces[k]));
            }
            let mut laid = Vec::new();
            for &k in faces {
                if m.halted() {
                    break;
                }
                let (sub, n) = part(s, names, &[k]);
                match attempt(key, sub, n, cap, m) {
                    Ok(one) => {
                        report.split_faces.push(names.faces[k]);
                        laid.push(one);
                    }
                    Err(_) => report.dropped_faces.push(names.faces[k]),
                }
            }
            Tried { laid, report }
        }
    }
}

/// A part through the kept steps and laid out alone.
fn attempt(
    key: OpKey,
    mut s: Shape,
    mut names: Names,
    cap: f64,
    m: &Meter,
) -> Result<Laid, Failed> {
    let fail = Failed { unoriented: false };
    // A part tried: its faces.
    m.charge(s.faces.len() as u64).map_err(|_| fail)?;
    build::uses_sheet(&s, &names).map_err(|_| fail)?;
    let mut report = ReferenceReport::default();
    let unseamed = unseam::unseam(key, &mut s, &mut names, cap).map_err(|_| fail)?;
    let at_poles = unseam::split_at_poles(key, &mut s, &mut names);
    report.seams = unseamed.seams;
    report.poles = unseamed.poles;
    report.joined = unseamed.joined;
    report.split = at_poles.split;
    // Each orientation step on a copy: where it refuses, the file's
    // orientation stands.
    let mut unoriented = false;
    let mut t = s.clone();
    match orient::loops(&mut t, &names) {
        Ok(reversed) => {
            report.reversed = reversed;
            s = t;
        }
        Err(_) => unoriented = true,
    }
    let mut t = s.clone();
    match orient::faces(&mut t, &names) {
        Ok((turned, against)) => {
            report.turned = turned;
            report.reversed.extend(against);
            s = t;
        }
        Err(_) => unoriented = true,
    }
    if unoriented {
        report.unoriented = names.faces.clone();
    }
    tolerate(&mut s, cap, m);
    let pcurves = find_pcurves(&s, m);
    tolerate_pcurves(&mut s, &pcurves, cap, m);
    let failed = Failed { unoriented };
    let body = build::assemble_reference(key, &s, &names, &pcurves).map_err(|_| failed)?;
    if !check_topology(&body).is_empty() {
        return Err(failed);
    }
    Ok(Laid {
        shape: s,
        names,
        pcurves,
        report,
    })
}

/// Intervals along an edge for its sampled gap: 33 points, the 17 a gap
/// is usually sampled at and the middles between them. A file's
/// spline edge can pass through the surface at evenly spaced points and
/// stray between them (NIST FTC 10's splines on cylinders lie on them at
/// 17 even samples and 4.9e-7 m off halfway between), so a gap sampled at
/// 17 points can miss what the mesher's samples see. 65 points took
/// twice as long (the projections onto spline faces), past design 364's
/// second for the committed corpus.
const SAMPLES: usize = 32;

/// How much a sampled gap is grown to cover what lies between samples:
/// the sampled maximum of a smooth deviation is short of the true one by
/// about its curvature times the spacing squared over eight.
const UNSAMPLED: f64 = 1.5;

/// Every edge's and vertex's tolerance grown to cover how far it lies
/// from what it bounds: an edge from its faces' surfaces at 33 samples, a
/// vertex from its faces' surfaces and its edges' ends; uncertified, and
/// held to `cap` (a tolerance the file gave stays as it is).
fn tolerate(s: &mut Shape, cap: f64, m: &Meter) {
    let mut edge_faces: Vec<Vec<usize>> = vec![Vec::new(); s.edges.len()];
    let mut vertex_faces: Vec<Vec<usize>> = vec![Vec::new(); s.vertices.len()];
    let mut vertex_ends: Vec<Vec<(usize, bool)>> = vec![Vec::new(); s.vertices.len()];
    for (k, f) in s.faces.iter().enumerate() {
        for l in &f.loops {
            match l {
                Loop::Vertex(v) => vertex_faces[*v].push(k),
                Loop::Edges(uses) => {
                    for u in uses {
                        let e = &s.edges[u.edge];
                        edge_faces[u.edge].push(k);
                        vertex_faces[e.start].push(k);
                        vertex_faces[e.end].push(k);
                        vertex_ends[e.start].push((u.edge, true));
                        vertex_ends[e.end].push((u.edge, false));
                    }
                }
            }
        }
    }
    let edges: Vec<usize> = (0..s.edges.len()).collect();
    let shape: &Shape = s;
    let edge_gaps: Vec<f64> = m.par_map(&edges, |m, &g| {
        // An edge's samples, on each of its faces.
        let _ = m.charge(edge_faces[g].len() as u64);
        let e = &shape.edges[g];
        let curve = &shape.curves[e.curve];
        let at = params(curve, e.interval);
        edge_faces[g]
            .iter()
            .map(|&k| {
                let surface = &shape.surfaces[shape.faces[k].surface];
                worst(&at, |t| surface.distance(curve.point(t)))
            })
            .fold(0.0, f64::max)
    });
    let vertex_gaps: Vec<f64> = (0..shape.vertices.len())
        .map(|v| {
            let p = shape.vertices[v].point;
            let on_faces = vertex_faces[v]
                .iter()
                .map(|&k| shape.surfaces[shape.faces[k].surface].distance(p))
                .fold(0.0, f64::max);
            vertex_ends[v]
                .iter()
                .map(|&(g, start)| {
                    let e = &shape.edges[g];
                    let t = if start { e.interval[0] } else { e.interval[1] };
                    shape.curves[e.curve].point(t).distance(p)
                })
                .fold(on_faces, f64::max)
        })
        .collect();
    // As healing covers a gap, with room for rounding, held to the cap; an
    // edge's grown for what lies between its samples.
    let cover = |gap: f64| (gap * 1.0001 + 1e-12).min(cap);
    for (e, gap) in s.edges.iter_mut().zip(edge_gaps) {
        e.tolerance = e.tolerance.max(cover(gap * UNSAMPLED));
    }
    for (v, gap) in s.vertices.iter_mut().zip(vertex_gaps) {
        v.tolerance = v.tolerance.max(cover(gap));
    }
}

/// Golden-section steps a sampled gap's largest sample is refined by:
/// its bracket shrinks to under 1% of the samples' spacing either side.
/// A gap under a hundredth of resolution at every sample isn't refined.
const REFINE: usize = 10;

/// The parameters an edge's gap is sampled at: [`SAMPLES`] intervals,
/// and the middle of each knot span of a spline edge that no sample falls
/// inside. A file's spline edge can stray from its faces inside one short
/// span only (#795: 6.5e-7 m off within the last of 16 spans, a sixtieth
/// of the edge, where the 33 even samples find 3.5e-9 m), which the
/// mesher's samples, closer there, see.
fn params(curve: &Curve, [t0, t1]: [f64; 2]) -> Vec<f64> {
    let mut at: Vec<f64> = (0..=SAMPLES)
        .map(|i| t0 + (t1 - t0) * i as f64 / SAMPLES as f64)
        .collect();
    if let Curve::Nurbs(n) = curve {
        let step = (t1 - t0) / SAMPLES as f64;
        for w in n.knots().knots().windows(2) {
            let (a, b) = (w[0].max(t0), w[1].min(t1));
            // A span inside the interval and between two samples.
            let next = t0 + (((a - t0) / step).floor() + 1.0) * step;
            if b > a && next >= b {
                at.push(0.5 * (a + b));
            }
        }
        at.sort_by(f64::total_cmp);
    }
    at
}

/// The largest of `gap` at the sorted parameters `at`, refined by a
/// golden-section search between the largest sample's neighbours: the
/// sampled maximum of a narrow stray is short of the true one by more
/// than [`UNSAMPLED`] covers (#795's spikes, a third of their peak at the
/// nearest sample).
fn worst(at: &[f64], gap: impl Fn(f64) -> f64) -> f64 {
    let gaps: Vec<f64> = at.iter().map(|&t| gap(t)).collect();
    let Some((i, &most)) = gaps
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)))
    else {
        return 0.0;
    };
    // An edge on its face at every sample (an exact one, mostly) has no
    // stray worth the projections a search costs.
    if most < 1e-2 * LINEAR_RESOLUTION {
        return most;
    }
    let (mut a, mut b) = (at[i.saturating_sub(1)], at[(i + 1).min(at.len() - 1)]);
    let mut best = most;
    const PHI: f64 = 0.618_033_988_749_894_8;
    let (mut c, mut d) = (b - PHI * (b - a), a + PHI * (b - a));
    let (mut gc, mut gd) = (gap(c), gap(d));
    for _ in 0..REFINE {
        best = best.max(gc).max(gd);
        if gc >= gd {
            b = d;
            d = c;
            gd = gc;
            c = b - PHI * (b - a);
            gc = gap(c);
        } else {
            a = c;
            c = d;
            gc = gd;
            d = a + PHI * (b - a);
            gd = gap(d);
        }
    }
    best.max(gc).max(gd)
}

/// Every edge's tolerance grown to cover how far its pcurves' images lie
/// from it, as the mesher samples an edge on a face through its pcurve
/// (#797: a file's pcurve on a spline face accepted at the samples, up to
/// 1.2 times the projected gap off between them); held to `cap`.
fn tolerate_pcurves(s: &mut Shape, pcurves: &MaybePcurves, cap: f64, m: &Meter) {
    let mut grown = vec![0.0_f64; s.edges.len()];
    for (k, f) in s.faces.iter().enumerate() {
        let surface = &s.surfaces[f.surface];
        for (l, lp) in f.loops.iter().enumerate() {
            let Loop::Edges(uses) = lp else { continue };
            for (j, u) in uses.iter().enumerate() {
                let Some((c2, map, _)) = &pcurves[k][l][j] else {
                    continue;
                };
                let _ = m.charge(1);
                let e = &s.edges[u.edge];
                let curve = &s.curves[e.curve];
                let at = params(curve, e.interval);
                let gap = worst(&at, |t| {
                    let uv = c2.point(map[0] + map[1] * t);
                    surface.point(uv.x, uv.y).distance(curve.point(t))
                });
                grown[u.edge] = grown[u.edge].max(gap);
            }
        }
    }
    let cover = |gap: f64| (gap * 1.0001 + 1e-12).min(cap);
    for (e, gap) in s.edges.iter_mut().zip(grown) {
        e.tolerance = e.tolerance.max(cover(gap * UNSAMPLED));
    }
}

/// Every use's pcurve where one is found without fitting.
fn find_pcurves(s: &Shape, m: &Meter) -> MaybePcurves {
    let faces: Vec<usize> = (0..s.faces.len()).collect();
    m.par_map(&faces, |m, &k| {
        let f = &s.faces[k];
        f.loops
            .iter()
            .map(|l| match l {
                Loop::Vertex(_) => Vec::new(),
                Loop::Edges(uses) => uses
                    .iter()
                    .map(|u| {
                        // A use given its pcurve.
                        let _ = m.charge(1);
                        pcurves::unfitted(s, f, u)
                    })
                    .collect(),
            })
            .collect()
    })
}

/// Faces `faces` of `s` as a shape of their own (one shell, a sheet), with
/// their names: only the geometry, vertices and edges they use, each in
/// the order first met.
fn part(s: &Shape, names: &Names, faces: &[usize]) -> (Shape, Names) {
    let mut c = Copier {
        s,
        names,
        out: Shape::default(),
        n: Names {
            vertices: Vec::new(),
            edges: Vec::new(),
            faces: Vec::new(),
        },
        surfaces: BTreeMap::new(),
        curves: BTreeMap::new(),
        curves2: BTreeMap::new(),
        vertices: BTreeMap::new(),
        edges: BTreeMap::new(),
    };
    for &k in faces {
        c.face(k);
    }
    let mut out = c.out;
    out.shells = vec![Shell {
        faces: (0..out.faces.len()).collect(),
        closed: false,
    }];
    out.sheets = vec![0];
    (out, c.n)
}

/// Copies entities of a shape into another, each once.
struct Copier<'a> {
    s: &'a Shape,
    names: &'a Names,
    out: Shape,
    n: Names,
    surfaces: BTreeMap<usize, usize>,
    curves: BTreeMap<usize, usize>,
    curves2: BTreeMap<usize, usize>,
    vertices: BTreeMap<usize, usize>,
    edges: BTreeMap<usize, usize>,
}

impl Copier<'_> {
    fn surface(&mut self, i: usize) -> usize {
        *self.surfaces.entry(i).or_insert_with(|| {
            self.out.surfaces.push(self.s.surfaces[i].clone());
            self.out.surfaces.len() - 1
        })
    }

    fn curve(&mut self, i: usize) -> usize {
        *self.curves.entry(i).or_insert_with(|| {
            self.out.curves.push(self.s.curves[i].clone());
            self.out.curves.len() - 1
        })
    }

    fn curve2(&mut self, i: usize) -> usize {
        *self.curves2.entry(i).or_insert_with(|| {
            self.out.curves2.push(self.s.curves2[i].clone());
            self.out.curves2.len() - 1
        })
    }

    fn vertex(&mut self, v: usize) -> usize {
        if let Some(&x) = self.vertices.get(&v) {
            return x;
        }
        self.out.vertices.push(self.s.vertices[v]);
        self.n.vertices.push(self.names.vertices[v]);
        let x = self.out.vertices.len() - 1;
        self.vertices.insert(v, x);
        x
    }

    fn edge(&mut self, g: usize) -> usize {
        if let Some(&x) = self.edges.get(&g) {
            return x;
        }
        let e = self.s.edges[g];
        let edge = super::Edge {
            curve: self.curve(e.curve),
            start: self.vertex(e.start),
            end: self.vertex(e.end),
            ..e
        };
        self.out.edges.push(edge);
        self.n.edges.push(self.names.edges[g]);
        let x = self.out.edges.len() - 1;
        self.edges.insert(g, x);
        x
    }

    fn face(&mut self, k: usize) {
        let f = &self.s.faces[k];
        let surface = self.surface(f.surface);
        let loops = f
            .loops
            .iter()
            .map(|l| match l {
                Loop::Vertex(v) => Loop::Vertex(self.vertex(*v)),
                Loop::Edges(uses) => Loop::Edges(
                    uses.iter()
                        .map(|u| Use {
                            edge: self.edge(u.edge),
                            sense: u.sense,
                            pcurve: u.pcurve.map(|p| self.curve2(p)),
                        })
                        .collect(),
                ),
            })
            .collect();
        self.out.faces.push(super::Face {
            surface,
            sense: f.sense,
            loops,
            tolerance: f.tolerance,
        });
        self.n.faces.push(self.names.faces[k]);
    }
}

/// The parts as one shape, their names and pcurves, a later copy of an
/// edge or vertex renamed (see [`distinct`]).
fn combine(key: OpKey, parts: Vec<Laid>) -> (Shape, Names, MaybePcurves) {
    let mut s = Shape::default();
    let mut n = Names {
        vertices: Vec::new(),
        edges: Vec::new(),
        faces: Vec::new(),
    };
    let mut pc: MaybePcurves = Vec::new();
    for p in parts {
        let (sv, se, ss, sc, sc2) = (
            s.vertices.len(),
            s.edges.len(),
            s.surfaces.len(),
            s.curves.len(),
            s.curves2.len(),
        );
        let part = p.shape;
        s.surfaces.extend(part.surfaces);
        s.curves.extend(part.curves);
        s.curves2.extend(part.curves2);
        s.vertices.extend(part.vertices);
        s.edges.extend(part.edges.into_iter().map(|e| super::Edge {
            curve: e.curve + sc,
            start: e.start + sv,
            end: e.end + sv,
            ..e
        }));
        s.faces.extend(part.faces.into_iter().map(|f| {
            super::Face {
                surface: f.surface + ss,
                sense: f.sense,
                tolerance: f.tolerance,
                loops: f
                    .loops
                    .into_iter()
                    .map(|l| match l {
                        Loop::Vertex(v) => Loop::Vertex(v + sv),
                        Loop::Edges(uses) => Loop::Edges(
                            uses.into_iter()
                                .map(|u| Use {
                                    edge: u.edge + se,
                                    sense: u.sense,
                                    pcurve: u.pcurve.map(|c| c + sc2),
                                })
                                .collect(),
                        ),
                    })
                    .collect(),
            }
        }));
        n.vertices.extend(p.names.vertices);
        n.edges.extend(p.names.edges);
        n.faces.extend(p.names.faces);
        pc.extend(p.pcurves);
    }
    s.shells = vec![Shell {
        faces: (0..s.faces.len()).collect(),
        closed: false,
    }];
    s.sheets = vec![0];
    distinct(key, &mut n);
    (s, n, pc)
}

/// Copies of one vertex or edge in several parts: the first (in the order
/// the parts come) keeps its name, each later one takes a part of it.
fn distinct(key: OpKey, n: &mut Names) {
    for list in [&mut n.vertices, &mut n.edges] {
        let mut seen: BTreeMap<Name, u128> = BTreeMap::new();
        for name in list.iter_mut() {
            let parent = *name;
            let rank = seen.entry(parent).or_insert(0);
            if *rank > 0 {
                let part = u8::try_from(*rank).unwrap_or(u8::MAX);
                *name = Name::derive(key, Role::Part(part), &[parent, Name::from_u128(*rank)]);
            }
            *rank += 1;
        }
    }
}

/// `more` added to `report`.
fn merge(report: &mut ReferenceReport, more: ReferenceReport) {
    report.seams.extend(more.seams);
    report.poles.extend(more.poles);
    report.joined.extend(more.joined);
    report.split.extend(more.split);
    report.reversed.extend(more.reversed);
    report.turned.extend(more.turned);
    report.unoriented.extend(more.unoriented);
    report.split_faces.extend(more.split_faces);
    report.dropped_faces.extend(more.dropped_faces);
}

/// Every list sorted, each name once.
fn sort(r: &mut ReferenceReport) {
    for list in [
        &mut r.seams,
        &mut r.poles,
        &mut r.joined,
        &mut r.split,
        &mut r.reversed,
        &mut r.turned,
        &mut r.unoriented,
        &mut r.split_faces,
        &mut r.dropped_faces,
        &mut r.laminar,
    ] {
        list.sort();
        list.dedup();
    }
    r.tolerances.sort_by_key(|x| x.0);
}

#[cfg(test)]
mod tests;
