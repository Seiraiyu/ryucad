//! A body as files describe it: every periodic face cut open along a
//! seam, so each face is a region of its surface's parameter plane
//! bounded like a disk with holes.
//!
//! ryukei's periodic faces have no seams: a cylinder's side is one face
//! between two circles, a sphere one face with no loops. STEP and the
//! readers of it want a seam edge where a face wraps, used twice by the
//! face, and the poles a seam runs to as vertices. [`seamed`] gives a
//! body as a [`Shape`] with them added; the body itself is never
//! changed, and [`super::heal`] takes them away again.
//!
//! A face wraps in a periodic direction when one of its loops winds
//! round it (a cylinder's circles), or when no loop winds and every loop
//! is a hole (a sphere with dents, a torus with none). The seam is the
//! parameter line at one value of that direction (the parameter of a
//! vertex on the loops it crosses where one keeps clear of every other
//! loop, else the surface's own seam, else the middle of the widest gap
//! between the other loops), running the other way from the loop below
//! the face to the one above it, or to a pole (a sphere's, a cone's
//! apex) where there is none; a loop crossed inside an edge has the edge
//! split there. A torus face with no loops that wind gets two seams, one
//! each way, through one vertex.
//!
//! A pipe face becomes a NURBS within `pipe_tolerance`
//! ([`crate::geom::Pipe::to_nurbs_within`], periodic along a closed spine
//! it wraps), its bound the face's tolerance. Pcurves go with each use
//! where they run on the edge's own parameter; seams get theirs, iso
//! lines on the same parameter.

use super::{Edge, Face, Loop, Shape, Shell, Solid, Use, Vertex};
use crate::geom::{
    Circle, Curve, Curve2, Direction, Frame, KnotVector, Line, Line2, NurbsCurve, NurbsSurface,
    Point2, Point3, Reparam, Rigid, Surface, Vector2,
};
use crate::mesh::uv::collapsed_rows;
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, ShellId, VertexId};
use crate::{Error, HealError, math};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Samples taken along each use to find where loops wind and cross.
const SAMPLES: usize = 32;

/// A use's pcurve on the body's surface: its curve and the map from the
/// edge's parameter to its own.
type Pc = Option<(Curve2, [f64; 2])>;

/// The shape being cut, with each use's pcurve on the body's surface and
/// how a face's export surface relates to it.
struct Cut {
    s: Shape,
    /// Per face, loop and use, parallel to the shape's loops.
    pc: Vec<Vec<Vec<Pc>>>,
    /// Per face: how the export surface's parameters follow from the
    /// body surface's (a pipe's angle goes through `Reparam::Angle`).
    reparam: Vec<[Reparam; 2]>,
    /// Per face: its name in the body, for refusals.
    names: Vec<Name>,
}

/// `body` as a shape with seams. Fails with [`HealError::BadDomain`],
/// naming a face, where a face can't be cut open (its loops wind both
/// ways, or cross the seam more than once wherever it goes), and like
/// [`crate::geom::Pipe::to_nurbs_within`] where a pipe face can't be
/// approximated.
pub fn seamed(body: &Body, pipe_tolerance: f64) -> Result<Shape, Error> {
    let mut cut = copy(body);
    for k in 0..cut.s.faces.len() {
        pipe_to_nurbs(&mut cut, k, pipe_tolerance)?;
    }
    for k in 0..cut.s.faces.len() {
        seam(&mut cut, k)?;
    }
    Ok(cut.s)
}

/// The body's entities as they are, with every pcurve that runs on its
/// edge's own parameter given as the file's.
fn copy(b: &Body) -> Cut {
    let mut s = Shape::default();
    let mut surfaces = BTreeMap::new();
    let mut curves = BTreeMap::new();
    let mut curves2 = BTreeMap::new();
    let mut vertices: BTreeMap<VertexId, usize> = BTreeMap::new();
    let mut edges: BTreeMap<EdgeId, usize> = BTreeMap::new();
    for v in b.vertices() {
        vertices.insert(v, s.vertices.len());
        s.vertices.push(Vertex {
            point: b.vertices[v].point,
            tolerance: 0.0,
        });
    }
    for e in b.edges() {
        let edge = &b.edges[e];
        let c = *curves.entry(edge.curve).or_insert_with(|| {
            s.curves.push(b.curves[edge.curve].clone());
            s.curves.len() - 1
        });
        edges.insert(e, s.edges.len());
        s.edges.push(Edge {
            curve: c,
            interval: edge.interval,
            start: vertices[&edge.start],
            end: vertices[&edge.end],
            tolerance: 0.0,
        });
    }
    let mut pc = Vec::new();
    let mut names = Vec::new();
    let mut faces: BTreeMap<FaceId, usize> = BTreeMap::new();
    for f in b.faces() {
        let face = &b.faces[f];
        let surface = *surfaces.entry(face.surface).or_insert_with(|| {
            s.surfaces.push(b.surfaces[face.surface].clone());
            s.surfaces.len() - 1
        });
        let mut loops = Vec::new();
        let mut pcs = Vec::new();
        for l in b.face_loops(f) {
            if let Some(v) = b.loops[l].vertex {
                loops.push(Loop::Vertex(vertices[&v]));
                pcs.push(Vec::new());
                continue;
            }
            let mut uses = Vec::new();
            let mut list = Vec::new();
            for fin in b.loop_fins(l) {
                let p = b.fins[fin].pcurve;
                let file = p.filter(|p| p.map == [0.0, 1.0]).map(|p| {
                    *curves2.entry(p.curve).or_insert_with(|| {
                        s.curves2.push(b.curves2[p.curve].clone());
                        s.curves2.len() - 1
                    })
                });
                uses.push(Use {
                    edge: edges[&b.fins[fin].edge],
                    sense: b.fins[fin].sense,
                    pcurve: file,
                });
                list.push(p.map(|p| (b.curves2[p.curve].clone(), p.map)));
            }
            loops.push(Loop::Edges(uses));
            pcs.push(list);
        }
        faces.insert(f, s.faces.len());
        s.faces.push(Face {
            surface,
            sense: face.sense,
            loops,
            tolerance: 0.0,
        });
        pc.push(pcs);
        names.push(face.name);
    }
    for r in b.regions().skip(1) {
        let mut shells: Vec<(f64, usize)> = Vec::new();
        for sh in b.region_shells(r).collect::<Vec<ShellId>>() {
            let size = b
                .shell_faces(sh)
                .map(|f| crate::intersect::face_box(b, f))
                .reduce(|x, y| x.union(&y))
                .map_or(0.0, |x| (x.max() - x.min()).length());
            shells.push((size, s.shells.len()));
            s.shells.push(Shell {
                faces: b.shell_faces(sh).map(|f| faces[&f]).collect(),
                closed: true,
            });
        }
        // The outer shell's box holds every void's.
        let outer = shells
            .iter()
            .copied()
            .reduce(|a, b| if b.0 > a.0 { b } else { a })
            .expect("a solid region has a shell")
            .1;
        s.solids.push(Solid {
            outer,
            voids: shells.iter().map(|x| x.1).filter(|&x| x != outer).collect(),
        });
    }
    let n = s.faces.len();
    Cut {
        s,
        pc,
        reparam: vec![[Reparam::Identity; 2]; n],
        names,
    }
}

fn bad(cut: &Cut, k: usize) -> Error {
    HealError::BadDomain { face: cut.names[k] }.into()
}

/// A pipe face's surface replaced by a NURBS within `tolerance` over the
/// stretch of spine the face covers: periodic where the face wraps a
/// closed spine. Its uses lose their pcurves, which run on the pipe's
/// angle rather than the NURBS's parameter.
fn pipe_to_nurbs(cut: &mut Cut, k: usize, tolerance: f64) -> Result<(), Error> {
    let Surface::Pipe(pipe) = &cut.s.surfaces[cut.s.faces[k].surface] else {
        return Ok(());
    };
    let pipe = pipe.clone();
    let period = pipe.spine().period();
    let [d0, d1] = pipe.spine().domain();
    let info = loops(cut, k, (period, None));
    let (lo, hi) = info
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, l| {
            (a.0.min(l.extent[0][0]), a.1.max(l.extent[0][1]))
        });
    let wraps = info.iter().any(|l| l.wind[0] != 0) || info.iter().all(|l| l.hole);
    let range = match period {
        Some(p) if wraps || hi - lo >= p => {
            let s0 = if lo.is_finite() { lo } else { d0 };
            [s0, s0 + p]
        }
        _ => {
            let m = 1e-6 * (hi - lo).max(LINEAR_RESOLUTION);
            let (a, b) = (lo - m, hi + m);
            match period {
                Some(_) => [a, b],
                None => [a.max(d0), b.min(d1)],
            }
        }
    };
    let (mut n, reparam, bound) = pipe.to_nurbs_within(range, tolerance)?;
    if let Some(p) = period
        && range[1] - range[0] == p
    {
        n = periodic_along_u(&n)?;
    }
    cut.s.surfaces.push(Surface::Nurbs(Arc::new(n)));
    let face = &mut cut.s.faces[k];
    face.surface = cut.s.surfaces.len() - 1;
    face.tolerance = bound;
    for l in &mut face.loops {
        if let Loop::Edges(uses) = l {
            for u in uses {
                u.pcurve = None;
            }
        }
    }
    cut.reparam[k] = reparam;
    Ok(())
}

/// A clamped NURBS whose first and last rows along `u` are one section
/// (a pipe round a closed spine, over one period) made periodic along
/// `u`: the last row dropped, the end multiplicities those of the
/// interior.
fn periodic_along_u(n: &NurbsSurface) -> Result<NurbsSurface, Error> {
    let [ku, kv] = n.knots();
    let mut mults = ku.mults().to_vec();
    let inner = mults[1..mults.len() - 1]
        .iter()
        .copied()
        .max()
        .unwrap_or(ku.degree());
    let last = mults.len() - 1;
    mults[0] = inner;
    mults[last] = inner;
    let periodic = KnotVector::periodic(ku.degree(), ku.knots(), &mults)?;
    let [nu, nv] = n.counts();
    let keep = periodic.control_count();
    // The clamped net starts at the first row; the periodic one's first
    // control point sits `degree - inner` rows later in the clamped
    // numbering (none for a Hermite net, whose rows end every span).
    let _ = nu;
    let points: Vec<Point3> = (0..keep)
        .flat_map(|i| (0..nv).map(move |j| (i, j)))
        .map(|(i, j)| n.control_point(i, j))
        .collect();
    let weights: Vec<f64> = (0..keep)
        .flat_map(|i| (0..nv).map(move |j| n.weights()[i * nv + j]))
        .collect();
    NurbsSurface::new([periodic, kv.clone()], points, Some(weights))
}

/// What a face's loop does in its parameter plane.
#[derive(Clone, Debug)]
struct Info {
    /// Whole turns round each periodic direction.
    wind: [i64; 2],
    /// The unwrapped extent each way.
    extent: [[f64; 2]; 2],
    /// It runs clockwise about what it encloses, seen from the face: the
    /// face is outside it.
    hole: bool,
    /// A lone vertex's loop.
    lone: Option<usize>,
}

/// A use's parameters on its face's export surface at `n + 1` samples,
/// running the way the loop does.
fn samples(cut: &Cut, k: usize, l: usize, i: usize) -> Vec<(f64, Point2)> {
    let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
        return Vec::new();
    };
    let u = uses[i];
    let e = &cut.s.edges[u.edge];
    let [t0, t1] = e.interval;
    let rp = cut.reparam[k];
    (0..=SAMPLES)
        .map(|m| {
            let f = m as f64 / SAMPLES as f64;
            let f = if u.sense == crate::topo::Sense::Forward {
                f
            } else {
                1.0 - f
            };
            let t = t0 + (t1 - t0) * f;
            (t, uv(cut, k, l, i, t, rp))
        })
        .collect()
}

/// A use's parameters at edge parameter `t`, from its pcurve or, without
/// one, by projection onto the face's export surface.
fn uv(cut: &Cut, k: usize, l: usize, i: usize, t: f64, rp: [Reparam; 2]) -> Point2 {
    match &cut.pc[k][l][i] {
        Some((c2, map)) => {
            let q = c2.point(map[0] + map[1] * t);
            Point2::new(rp[0].to_nurbs(q.x), rp[1].to_nurbs(q.y))
        }
        None => {
            let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
                unreachable!("a use is in a loop of edges")
            };
            let e = &cut.s.edges[uses[i].edge];
            let surface = &cut.s.surfaces[cut.s.faces[k].surface];
            let (u, v) = surface.project(cut.s.curves[e.curve].point(t)).param;
            Point2::new(u, v)
        }
    }
}

fn unwrap(x: f64, near: f64, period: Option<f64>) -> f64 {
    match period {
        Some(p) => x + ((near - x) / p).round() * p,
        None => x,
    }
}

/// Each loop's windings, extent and side, over periods `periods`.
fn loops(cut: &Cut, k: usize, periods: (Option<f64>, Option<f64>)) -> Vec<Info> {
    let period = [periods.0, periods.1];
    let forward = cut.s.faces[k].sense == crate::topo::Sense::Forward;
    let mut out = Vec::new();
    for (l, lp) in cut.s.faces[k].loops.iter().enumerate() {
        let n = match lp {
            Loop::Vertex(v) => {
                out.push(Info {
                    wind: [0, 0],
                    extent: [[f64::INFINITY, f64::NEG_INFINITY]; 2],
                    hole: false,
                    lone: Some(*v),
                });
                continue;
            }
            Loop::Edges(uses) => uses.len(),
        };
        let mut x: Vec<[f64; 2]> = Vec::new();
        for i in 0..n {
            for (_, p) in samples(cut, k, l, i) {
                let raw = [p.x, p.y];
                let next = match x.last() {
                    Some(q) => core::array::from_fn(|d| unwrap(raw[d], q[d], period[d])),
                    None => raw,
                };
                x.push(next);
            }
        }
        let (first, last) = (x[0], x[x.len() - 1]);
        let wind: [i64; 2] = core::array::from_fn(|d| match period[d] {
            Some(p) => ((last[d] - first[d]) / p).round() as i64,
            None => 0,
        });
        let extent: [[f64; 2]; 2] = core::array::from_fn(|d| {
            let lo = x.iter().map(|p| p[d]).fold(f64::INFINITY, f64::min);
            let hi = x.iter().map(|p| p[d]).fold(f64::NEG_INFINITY, f64::max);
            [lo, hi]
        });
        let m = x.len();
        let area: f64 = (0..m)
            .map(|i| x[i][0] * x[(i + 1) % m][1] - x[(i + 1) % m][0] * x[i][1])
            .sum();
        out.push(Info {
            wind,
            extent,
            hole: wind == [0, 0] && (area < 0.0) == forward,
            lone: None,
        });
    }
    out
}

/// Where a loop crosses the parameter line `j = c`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Crossing {
    /// At the start of use `i`.
    Vertex(usize),
    /// Inside use `i`, at its edge's parameter `t`.
    Inside(usize, f64),
}

/// The signed distance from `c` in direction `j`, wrapped into half a
/// period either side.
fn offset(x: f64, c: f64, period: f64) -> f64 {
    let d = (x - c).rem_euclid(period);
    if d > period / 2.0 { d - period } else { d }
}

/// Every crossing of loop `l` of face `k` with `j = c`, and the closest
/// any sample comes to it otherwise.
fn crossings(cut: &Cut, k: usize, l: usize, j: usize, c: f64, period: f64) -> (Vec<Crossing>, f64) {
    let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
        return (Vec::new(), f64::INFINITY);
    };
    let eps = 1e-10 * period;
    let rp = cut.reparam[k];
    let mut out = Vec::new();
    let mut nearest = f64::INFINITY;
    for i in 0..uses.len() {
        let s = samples(cut, k, l, i);
        let g: Vec<f64> = s
            .iter()
            .map(|(_, p)| offset([p.x, p.y][j], c, period))
            .collect();
        if g[0].abs() <= eps {
            out.push(Crossing::Vertex(i));
        }
        for m in 1..g.len() {
            nearest = nearest.min(g[m].abs());
            let (a, b) = (g[m - 1], g[m]);
            if a.abs() <= eps || b.abs() <= eps || (a < 0.0) == (b < 0.0) {
                continue;
            }
            if (a - b).abs() >= period / 2.0 {
                continue;
            }
            // Bisection on the edge's parameter.
            let (mut ta, mut tb) = (s[m - 1].0, s[m].0);
            let ga = a;
            for _ in 0..80 {
                let tm = (ta + tb) / 2.0;
                let p = uv(cut, k, l, i, tm, rp);
                let gm = offset([p.x, p.y][j], c, period);
                if (gm < 0.0) == (ga < 0.0) {
                    ta = tm;
                } else {
                    tb = tm;
                }
            }
            out.push(Crossing::Inside(i, (ta + tb) / 2.0));
        }
    }
    (out, nearest)
}

/// The seam cut of face `k`, if it wraps.
fn seam(cut: &mut Cut, k: usize) -> Result<(), Error> {
    let surface = cut.s.surfaces[cut.s.faces[k].surface].clone();
    let periods = surface.periods();
    if periods == (None, None) {
        return Ok(());
    }
    let period = [periods.0, periods.1];
    let info = loops(cut, k, periods);
    let winds: [bool; 2] = core::array::from_fn(|d| info.iter().any(|x| x.wind[d] != 0));
    if winds[0] && winds[1] {
        return Err(bad(cut, k));
    }
    let covers = info.iter().all(|x| x.hole || x.lone.is_some());
    let collapsed = collapsed_rows(&surface);
    if let Some(j) = (0..2).find(|&d| winds[d]) {
        return one_seam(
            cut,
            k,
            &surface,
            &info,
            j,
            period[j].expect("winds"),
            collapsed,
        );
    }
    if !covers {
        return Ok(());
    }
    // Every loop a hole: the face covers its surface.
    match (period[0], period[1]) {
        (Some(pu), Some(pv)) => two_seams(cut, k, &surface, &info, [pu, pv]),
        (Some(p), None) | (None, Some(p)) => {
            let j = if period[0].is_some() { 0 } else { 1 };
            if collapsed[1 - j] != [true, true] {
                return Err(bad(cut, k));
            }
            one_seam(cut, k, &surface, &info, j, p, collapsed)
        }
        (None, None) => Ok(()),
    }
}

/// The parameter lines a seam in direction `j` may take: each vertex's on
/// the winding loops (or, where none winds, on every loop), the surface's
/// own seam, and the middles of the gaps the other loops leave.
fn candidates(cut: &Cut, k: usize, info: &[Info], j: usize, period: f64) -> Vec<f64> {
    let surface = &cut.s.surfaces[cut.s.faces[k].surface];
    // The winding loops' vertices, in the order of their points' bits,
    // so the choice doesn't depend on where loops start or which comes
    // first: each one's parameter as the surface projects it.
    let mut points: Vec<Point3> = Vec::new();
    for (l, x) in info.iter().enumerate() {
        if x.wind[j] == 0 {
            continue;
        }
        let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
            continue;
        };
        for u in uses {
            points.push(cut.s.vertices[start_of(&cut.s, u)].point);
        }
    }
    points.sort_by_key(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]);
    points.dedup();
    let rp = cut.reparam[k];
    let mut out: Vec<f64> = points
        .iter()
        .map(|&p| {
            let (u, v) = surface.project(p).param;
            [rp[0].to_nurbs(u), rp[1].to_nurbs(v)][j]
        })
        .collect();
    out.push(surface.domain()[j][0]);
    // Gaps between the extents of loops that don't wind.
    let mut spans: Vec<[f64; 2]> = info
        .iter()
        .filter(|x| x.wind[j] == 0 && x.lone.is_none())
        .flat_map(|x| {
            let [lo, hi] = x.extent[j];
            let a = lo.rem_euclid(period);
            [-period, 0.0, period].map(|s| [a + s, a + s + (hi - lo)])
        })
        .collect();
    spans.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut merged: Vec<[f64; 2]> = Vec::new();
    for s in spans {
        match merged.last_mut() {
            Some(m) if s[0] <= m[1] => m[1] = m[1].max(s[1]),
            _ => merged.push(s),
        }
    }
    let mut gaps: Vec<(f64, f64)> = merged
        .windows(2)
        .map(|w| (w[1][0] - w[0][1], (w[0][1] + w[1][0]) / 2.0))
        .filter(|g| (0.0..period).contains(&g.1))
        .collect();
    gaps.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.extend(gaps.into_iter().map(|g| g.1));
    out
}

/// The first candidate line that crosses each winding loop once and
/// keeps clear of every other loop.
fn choose(cut: &Cut, k: usize, info: &[Info], j: usize, period: f64) -> Option<f64> {
    let clear = 1e-6 * period;
    candidates(cut, k, info, j, period).into_iter().find(|&c| {
        info.iter().enumerate().all(|(l, x)| {
            if x.lone.is_some() {
                return true;
            }
            let (found, nearest) = crossings(cut, k, l, j, c, period);
            if x.wind[j] != 0 {
                found.len() == 1
            } else {
                found.is_empty() && nearest > clear
            }
        })
    })
}

/// The start vertex of use `i` of a loop.
fn start_of(s: &Shape, u: &Use) -> usize {
    let e = &s.edges[u.edge];
    match u.sense {
        crate::topo::Sense::Forward => e.start,
        crate::topo::Sense::Reversed => e.end,
    }
}

/// Splits edge `e` at `t`, in every loop that uses it, and returns the
/// new vertex. The pieces keep the edge's curve and its uses' pcurves.
fn split(cut: &mut Cut, e: usize, t: f64) -> usize {
    let edge = cut.s.edges[e];
    let x = cut.s.vertices.len();
    let point = cut.s.curves[edge.curve].point(t);
    cut.s.vertices.push(Vertex {
        point,
        tolerance: 0.0,
    });
    let e2 = cut.s.edges.len();
    cut.s.edges.push(Edge {
        interval: [t, edge.interval[1]],
        start: x,
        ..edge
    });
    cut.s.edges[e] = Edge {
        interval: [edge.interval[0], t],
        end: x,
        ..edge
    };
    for (k, face) in cut.s.faces.iter_mut().enumerate() {
        for (l, lp) in face.loops.iter_mut().enumerate() {
            let Loop::Edges(uses) = lp else {
                continue;
            };
            let mut i = 0;
            while i < uses.len() {
                if uses[i].edge != e {
                    i += 1;
                    continue;
                }
                let u = uses[i];
                let second = Use { edge: e2, ..u };
                let p = cut.pc[k][l][i].clone();
                match u.sense {
                    crate::topo::Sense::Forward => {
                        uses.insert(i + 1, second);
                        cut.pc[k][l].insert(i + 1, p);
                    }
                    crate::topo::Sense::Reversed => {
                        uses.insert(i, second);
                        cut.pc[k][l].insert(i, p);
                    }
                }
                i += 2;
            }
        }
    }
    x
}

/// The loop's uses turned to start at the use that starts at vertex `v`.
fn start_at(cut: &mut Cut, k: usize, l: usize, v: usize) -> Vec<(Use, Pc)> {
    let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
        unreachable!("a loop of edges")
    };
    let n = uses.len();
    let i = (0..n)
        .find(|&i| start_of(&cut.s, &uses[i]) == v)
        .expect("the loop passes the vertex");
    (0..n)
        .map(|m| {
            let a = (i + m) % n;
            (uses[a], cut.pc[k][l][a].clone())
        })
        .collect()
}

/// The vertex where loop `l` crosses `j = c`, splitting an edge there if
/// the crossing is inside it.
fn crossing_vertex(cut: &mut Cut, k: usize, l: usize, j: usize, c: f64, period: f64) -> usize {
    let (found, _) = crossings(cut, k, l, j, c, period);
    let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
        unreachable!("a loop of edges")
    };
    match found[0] {
        Crossing::Vertex(i) => start_of(&cut.s, &uses[i]),
        Crossing::Inside(i, t) => {
            let u = uses[i];
            let e = cut.s.edges[u.edge];
            let p = cut.s.curves[e.curve].point(t);
            // A crossing within resolution of an end is at that end.
            for v in [e.start, e.end] {
                if cut.s.vertices[v].point.distance(p) <= LINEAR_RESOLUTION {
                    return v;
                }
            }
            split(cut, u.edge, t)
        }
    }
}

/// The parameter line `j = c` of `surface`, running the other way on the
/// surface's own parameter (a cone's ruling as a degree 1 spline over
/// `range`, since a line runs on its length).
fn iso(surface: &Surface, j: usize, c: f64, range: [f64; 2]) -> Option<Curve> {
    let f = surface.frame();
    match (surface, j) {
        (Surface::Cylinder(cy), 0) => {
            let f = f?;
            let o = f.origin() + f.radial(c) * cy.radius();
            Line::from_unit(o, f.z()).ok().map(Curve::Line)
        }
        (Surface::Cone(_), 0) => {
            let knots = KnotVector::clamped(1, &range, &[2, 2]).ok()?;
            let pts = vec![surface.point(c, range[0]), surface.point(c, range[1])];
            NurbsCurve::new(knots, pts, None)
                .ok()
                .map(|n| Curve::Nurbs(Arc::new(n)))
        }
        (Surface::Sphere(sp), 0) => {
            let f = f?;
            let x = f.radial(c);
            let frame = Frame::new(f.origin(), x.cross(f.z().get()), x).ok()?;
            Circle::new(frame, sp.radius()).ok().map(Curve::Circle)
        }
        (Surface::Torus(t), 0) => {
            let f = f?;
            let x = f.radial(c);
            let frame = Frame::new(f.origin() + x * t.major(), x.cross(f.z().get()), x).ok()?;
            Circle::new(frame, t.minor()).ok().map(Curve::Circle)
        }
        (Surface::Torus(t), 1) => {
            let f = f?;
            let (sn, cs) = math::sin_cos(c);
            let frame =
                Frame::from_axes(f.origin() + f.z().get() * (t.minor() * sn), f.z(), f.x()).ok()?;
            Circle::new(frame, t.major() + t.minor() * cs)
                .ok()
                .map(Curve::Circle)
        }
        (Surface::Extrusion(e), 0) => Line::from_unit(e.curve().point(c), e.dir())
            .ok()
            .map(Curve::Line),
        (Surface::Revolution(r), 0) => {
            if c == 0.0 {
                return Some(r.curve().clone());
            }
            let fr = r.frame();
            let o = fr.origin().to_vector();
            let turn = Rigid::translation(o)
                .then_after(&Rigid::rotation(fr.z(), c))
                .then_after(&Rigid::translation(-o));
            Some(r.curve().transformed(&turn))
        }
        (Surface::Nurbs(n), _) => {
            let dir = if j == 0 { Direction::U } else { Direction::V };
            n.iso(dir, c).ok().map(|n| Curve::Nurbs(Arc::new(n)))
        }
        _ => None,
    }
}

/// A seam edge's pcurve: the line `j = at`, on the other parameter.
fn iso2(j: usize, at: f64) -> Curve2 {
    let (o, d) = if j == 0 {
        (Point2::new(at, 0.0), Vector2::new(0.0, 1.0))
    } else {
        (Point2::new(0.0, at), Vector2::new(1.0, 0.0))
    };
    Curve2::Line2(Line2::new(o, d).expect("a unit direction"))
}

/// Cuts a face that wraps in direction `j` open along one seam, from the
/// loop below it (or a pole) to the loop above it (or a pole).
fn one_seam(
    cut: &mut Cut,
    k: usize,
    surface: &Surface,
    info: &[Info],
    j: usize,
    period: f64,
    collapsed: [[bool; 2]; 2],
) -> Result<(), Error> {
    let kk = 1 - j;
    let forward = cut.s.faces[k].sense == crate::topo::Sense::Forward;
    // The face is on the left of each loop, about its normal: above (in
    // v) a loop running +u, and above (in u) a loop running -v.
    let up: i64 = if j == 0 { 1 } else { -1 };
    let up = if forward { up } else { -up };
    let winding: Vec<usize> = (0..info.len()).filter(|&l| info[l].wind[j] != 0).collect();
    if winding.iter().any(|&l| info[l].wind[j].abs() != 1) || winding.len() > 2 {
        return Err(bad(cut, k));
    }
    let below = winding.iter().copied().find(|&l| info[l].wind[j] == up);
    let above = winding.iter().copied().find(|&l| info[l].wind[j] == -up);
    if winding.len() == 2 && (below.is_none() || above.is_none()) {
        return Err(bad(cut, k));
    }
    let domain = surface.domain()[kk];
    // A missing side must be a pole of the surface.
    if (below.is_none() && !collapsed[kk][0]) || (above.is_none() && !collapsed[kk][1]) {
        return Err(bad(cut, k));
    }
    let Some(c) = choose(cut, k, info, j, period) else {
        return Err(bad(cut, k));
    };
    // The ends: crossing vertices on the loops, or poles.
    let lone_at = |cut: &Cut, end: usize| -> Option<(usize, usize)> {
        info.iter().enumerate().find_map(|(l, x)| {
            let v = x.lone?;
            let (u, w) = surface.project(cut.s.vertices[v].point).param;
            let at = [u, w][kk];
            ((at - domain[end]).abs() <= 1e-9 * (domain[1] - domain[0]).abs().max(1.0))
                .then_some((l, v))
        })
    };
    let mut drop_loops: Vec<usize> = Vec::new();
    let mut end_vertex = |cut: &mut Cut, side: Option<usize>, end: usize| -> (usize, f64) {
        match side {
            Some(l) => {
                let v = crossing_vertex(cut, k, l, j, c, period);
                let p = cut.s.vertices[v].point;
                let (u, w) = surface.project(p).param;
                (v, [u, w][kk])
            }
            None => {
                let at = domain[end];
                if let Some((l, v)) = lone_at(cut, end) {
                    drop_loops.push(l);
                    (v, at)
                } else {
                    let point = if j == 0 {
                        surface.point(c, at)
                    } else {
                        surface.point(at, c)
                    };
                    cut.s.vertices.push(Vertex {
                        point,
                        tolerance: 0.0,
                    });
                    (cut.s.vertices.len() - 1, at)
                }
            }
        }
    };
    let (vb, kb) = end_vertex(cut, below, 0);
    let (vt, kt) = end_vertex(cut, above, 1);
    // The k parameters at the ends, the top unwrapped above the bottom.
    let mut kt = kt;
    if let Some(p) = [surface.periods().0, surface.periods().1][kk] {
        kt = unwrap(kt, kb + p / 2.0, Some(p));
    }
    let kb = if below.is_none() { domain[0] } else { kb };
    let kt = if above.is_none() { domain[1] } else { kt };
    if kt <= kb {
        return Err(bad(cut, k));
    }
    let Some(curve) = iso(surface, j, c, [kb, kt]) else {
        return Err(bad(cut, k));
    };
    let seam = add_seam(cut, curve, [kb, kt], vb, vt, j, [c + up as f64 * period, c]);
    // The new loop: below from its crossing, the seam up, above from its
    // crossing, the seam down.
    let mut ring: Vec<(Use, Pc)> = Vec::new();
    if let Some(l) = below {
        ring.extend(start_at(cut, k, l, vb));
    }
    ring.push(seam.0);
    if let Some(l) = above {
        ring.extend(start_at(cut, k, l, vt));
    }
    ring.push(seam.1);
    let mut gone: Vec<usize> = below.into_iter().chain(above).chain(drop_loops).collect();
    gone.sort_unstable();
    replace_loops(cut, k, &gone, ring);
    Ok(())
}

/// Adds a seam edge from `start` to `end` on `curve` over `interval`, and
/// returns its two uses: forward on the copy at `copies[0]`, reversed on
/// the copy at `copies[1]`.
fn add_seam(
    cut: &mut Cut,
    curve: Curve,
    interval: [f64; 2],
    start: usize,
    end: usize,
    j: usize,
    copies: [f64; 2],
) -> ((Use, Pc), (Use, Pc)) {
    cut.s.curves.push(curve);
    let e = cut.s.edges.len();
    cut.s.edges.push(Edge {
        curve: cut.s.curves.len() - 1,
        interval,
        start,
        end,
        tolerance: 0.0,
    });
    let mut one = |at: f64, sense| {
        let c2 = iso2(j, at);
        cut.s.curves2.push(c2.clone());
        (
            Use {
                edge: e,
                sense,
                pcurve: Some(cut.s.curves2.len() - 1),
            },
            Some((c2, [0.0, 1.0])),
        )
    };
    (
        one(copies[0], crate::topo::Sense::Forward),
        one(copies[1], crate::topo::Sense::Reversed),
    )
}

/// Removes loops `gone` (sorted) of face `k` and adds `ring` as its first
/// loop.
fn replace_loops(cut: &mut Cut, k: usize, gone: &[usize], ring: Vec<(Use, Pc)>) {
    for &l in gone.iter().rev() {
        cut.s.faces[k].loops.remove(l);
        cut.pc[k].remove(l);
    }
    let (uses, pcs): (Vec<Use>, Vec<Pc>) = ring.into_iter().unzip();
    cut.s.faces[k].loops.insert(0, Loop::Edges(uses));
    cut.pc[k].insert(0, pcs);
}

/// Cuts a face that covers a surface periodic both ways open along two
/// seams through one vertex.
fn two_seams(
    cut: &mut Cut,
    k: usize,
    surface: &Surface,
    info: &[Info],
    period: [f64; 2],
) -> Result<(), Error> {
    let (Some(cu), Some(cv)) = (
        choose(cut, k, info, 0, period[0]),
        choose(cut, k, info, 1, period[1]),
    ) else {
        return Err(bad(cut, k));
    };
    let forward = cut.s.faces[k].sense == crate::topo::Sense::Forward;
    let v = cut.s.vertices.len();
    cut.s.vertices.push(Vertex {
        point: surface.point(cu, cv),
        tolerance: 0.0,
    });
    let (Some(su_curve), Some(sv_curve)) = (
        iso(surface, 0, cu, [cv, cv + period[1]]),
        iso(surface, 1, cv, [cu, cu + period[0]]),
    ) else {
        return Err(bad(cut, k));
    };
    // su runs along v at u = cu: its copies are the rectangle's left and
    // right sides; sv runs along u at v = cv: its bottom and top.
    let su = add_seam(
        cut,
        su_curve,
        [cv, cv + period[1]],
        v,
        v,
        0,
        [cu + period[0], cu],
    );
    let sv = add_seam(
        cut,
        sv_curve,
        [cu, cu + period[0]],
        v,
        v,
        1,
        [cv, cv + period[1]],
    );
    let ring = if forward {
        // Bottom, right, top, left: counterclockwise.
        vec![sv.0, su.0, sv.1, su.1]
    } else {
        // The same the other way round, each use on its own side.
        vec![
            (
                Use {
                    sense: crate::topo::Sense::Forward,
                    ..su.1.0
                },
                su.1.1,
            ),
            (
                Use {
                    sense: crate::topo::Sense::Forward,
                    ..sv.1.0
                },
                sv.1.1,
            ),
            (
                Use {
                    sense: crate::topo::Sense::Reversed,
                    ..su.0.0
                },
                su.0.1,
            ),
            (
                Use {
                    sense: crate::topo::Sense::Reversed,
                    ..sv.0.0
                },
                sv.0.1,
            ),
        ]
    };
    replace_loops(cut, k, &[], ring);
    Ok(())
}
