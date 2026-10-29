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
//! loop, else the surface's own seam, else one between those, else one
//! on a fixed grid over the period), running the other way from the loop below
//! the face to the one above it, or to a pole (a sphere's, a cone's
//! apex) where there is none; a loop crossed inside an edge has the edge
//! split there. A torus face with no loops that wind gets two seams, one
//! each way, through one vertex.
//!
//! A pipe face becomes a NURBS within `pipe_tolerance`
//! ([`crate::geom::Pipe::to_nurbs_within`], periodic along a closed spine
//! it wraps), its bound the face's tolerance; so does a sweep face
//! ([`crate::geom::Sweep::to_nurbs_within`], periodic along a closed path
//! it wraps). Pcurves go with each use
//! where they run on the edge's own parameter; seams get theirs, iso
//! lines on the same parameter.

use super::{Edge, Face, Loop, Shape, Shell, Solid, Use, Vertex};
use crate::geom::{
    Circle, Curve, Curve2, Direction, Frame, KnotVector, Line, Line2, NurbsCurve, NurbsSurface,
    Point2, Point3, Rigid, Surface, Vector2,
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

/// The shape being cut, with each face's name in the body, for
/// refusals, and where each entity comes from.
struct Cut {
    s: Shape,
    names: Vec<Name>,
    origins: Origins,
}

/// Where each entity of a [`seamed_with_origins`] shape comes from in the
/// body, index for index: the face it is (faces are never split or
/// added), the edge it is or is a piece of (`None` for a seam), and the
/// vertex it is (`None` for one a seam added: where it crosses a loop
/// inside an edge, or a pole it runs to).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origins {
    pub faces: Vec<FaceId>,
    pub edges: Vec<Option<EdgeId>>,
    pub vertices: Vec<Option<VertexId>>,
}

/// `body` as a shape with seams. Fails with [`HealError::BadDomain`],
/// naming a face, where a face can't be cut open (its loops wind both
/// ways, or cross the seam more than once wherever it goes), and like
/// [`crate::geom::Pipe::to_nurbs_within`] where a pipe face can't be
/// approximated.
pub fn seamed(body: &Body, pipe_tolerance: f64) -> Result<Shape, Error> {
    seamed_with_origins(body, pipe_tolerance).map(|(s, _)| s)
}

/// [`seamed`], with where each entity of the shape comes from in the
/// body: what a writer needs to carry the body's names and pcurves over.
pub fn seamed_with_origins(body: &Body, pipe_tolerance: f64) -> Result<(Shape, Origins), Error> {
    let mut cut = copy(body);
    // Faces are cut in the order of the names their geometry gives them:
    // a seam may split an edge another face then ends its own seam on,
    // so the order must not depend on the body's arrays.
    let names = super::names::names(crate::name::OpKey(0), &cut.s).faces;
    let mut order: Vec<usize> = (0..cut.s.faces.len()).collect();
    order.sort_by_key(|&k| names[k]);
    for &k in &order {
        pipe_to_nurbs(&mut cut, k, pipe_tolerance)?;
        sweep_to_nurbs(&mut cut, k, pipe_tolerance)?;
    }
    for &k in &order {
        seam(&mut cut, k)?;
    }
    debug_assert_eq!(cut.origins.edges.len(), cut.s.edges.len());
    debug_assert_eq!(cut.origins.vertices.len(), cut.s.vertices.len());
    let (shape, [vord, eord, ford]) = canonical(cut.s);
    let o = cut.origins;
    let origins = Origins {
        faces: ford.iter().map(|&k| o.faces[k]).collect(),
        edges: eord.iter().map(|&k| o.edges[k]).collect(),
        vertices: vord.iter().map(|&k| o.vertices[k]).collect(),
    };
    Ok((shape, origins))
}

/// The shape in the order of the names its geometry gives it (on a fixed
/// key): vertices, edges, faces, shells and solids sorted by name, each
/// face's loops by their least edge's, each loop starting at its
/// least-named edge, geometry numbered by first use. A body and the same
/// body read back, numbered differently, give the same shape. With it,
/// the old index of each new vertex, edge and face.
fn canonical(s: Shape) -> (Shape, [Vec<usize>; 3]) {
    let names = super::names::names(crate::name::OpKey(0), &s);
    let sorted = |list: &[Name]| {
        let mut o: Vec<usize> = (0..list.len()).collect();
        o.sort_by_key(|&i| list[i]);
        let mut pos = vec![0; list.len()];
        for (k, &i) in o.iter().enumerate() {
            pos[i] = k;
        }
        (o, pos)
    };
    let (vord, vpos) = sorted(&names.vertices);
    let (eord, epos) = sorted(&names.edges);
    let (ford, fpos) = sorted(&names.faces);
    let mut out = Shape::default();
    let mut curves: BTreeMap<usize, usize> = BTreeMap::new();
    let mut surfaces: BTreeMap<usize, usize> = BTreeMap::new();
    out.vertices = vord.iter().map(|&v| s.vertices[v]).collect();
    for &e in &eord {
        let edge = s.edges[e];
        let c = *curves.entry(edge.curve).or_insert_with(|| {
            out.curves.push(s.curves[edge.curve].clone());
            out.curves.len() - 1
        });
        out.edges.push(Edge {
            curve: c,
            start: vpos[edge.start],
            end: vpos[edge.end],
            ..edge
        });
    }
    for &k in &ford {
        let f = &s.faces[k];
        let surface = *surfaces.entry(f.surface).or_insert_with(|| {
            out.surfaces.push(s.surfaces[f.surface].clone());
            out.surfaces.len() - 1
        });
        let mut loops: Vec<(Name, Loop)> = f
            .loops
            .iter()
            .map(|l| match l {
                Loop::Vertex(v) => (names.vertices[*v], Loop::Vertex(vpos[*v])),
                Loop::Edges(uses) => {
                    let first = (0..uses.len())
                        .min_by_key(|&i| (names.edges[uses[i].edge], uses[i].sense as u8))
                        .expect("a loop has edges");
                    let ring: Vec<Use> = (0..uses.len())
                        .map(|m| {
                            let u = uses[(first + m) % uses.len()];
                            Use {
                                edge: epos[u.edge],
                                pcurve: u.pcurve.map(|p| {
                                    out.curves2.push(s.curves2[p].clone());
                                    out.curves2.len() - 1
                                }),
                                ..u
                            }
                        })
                        .collect();
                    (names.edges[uses[first].edge], Loop::Edges(ring))
                }
            })
            .collect();
        loops.sort_by_key(|x| x.0);
        out.faces.push(Face {
            surface,
            loops: loops.into_iter().map(|x| x.1).collect(),
            ..f.clone()
        });
    }
    let mut shells: Vec<(Vec<usize>, usize)> = s
        .shells
        .iter()
        .enumerate()
        .map(|(k, sh)| {
            let mut faces: Vec<usize> = sh.faces.iter().map(|&f| fpos[f]).collect();
            faces.sort_unstable();
            (faces, k)
        })
        .collect();
    shells.sort();
    let mut shell_pos = vec![0; shells.len()];
    for (i, (faces, k)) in shells.into_iter().enumerate() {
        shell_pos[k] = i;
        out.shells.push(Shell {
            faces,
            closed: s.shells[k].closed,
        });
    }
    let mut solids: Vec<Solid> = s
        .solids
        .iter()
        .map(|so| {
            let mut voids: Vec<usize> = so.voids.iter().map(|&v| shell_pos[v]).collect();
            voids.sort_unstable();
            Solid {
                outer: shell_pos[so.outer],
                voids,
            }
        })
        .collect();
    solids.sort_by_key(|so| so.outer);
    out.solids = solids;
    (out, [vord, eord, ford])
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
    let mut origins = Origins::default();
    for v in b.vertices() {
        origins.vertices.push(Some(v));
        vertices.insert(v, s.vertices.len());
        s.vertices.push(Vertex {
            point: b.vertices[v].point,
            tolerance: 0.0,
            seam: false,
        });
    }
    for e in b.edges() {
        let edge = &b.edges[e];
        let c = *curves.entry(edge.curve).or_insert_with(|| {
            s.curves.push(b.curves[edge.curve].clone());
            s.curves.len() - 1
        });
        edges.insert(e, s.edges.len());
        origins.edges.push(Some(e));
        s.edges.push(Edge {
            curve: c,
            interval: edge.interval,
            start: vertices[&edge.start],
            end: vertices[&edge.end],
            tolerance: 0.0,
        });
    }
    let mut names = Vec::new();
    let mut faces: BTreeMap<FaceId, usize> = BTreeMap::new();
    for f in b.faces() {
        let face = &b.faces[f];
        let surface = *surfaces.entry(face.surface).or_insert_with(|| {
            s.surfaces.push(b.surfaces[face.surface].clone());
            s.surfaces.len() - 1
        });
        let mut loops = Vec::new();
        for l in b.face_loops(f) {
            if let Some(v) = b.loops[l].vertex {
                loops.push(Loop::Vertex(vertices[&v]));
                continue;
            }
            let mut uses = Vec::new();
            for fin in b.loop_fins(l) {
                let file = b.fins[fin].pcurve.and_then(|p| {
                    if p.map == [0.0, 1.0] {
                        Some(*curves2.entry(p.curve).or_insert_with(|| {
                            s.curves2.push(b.curves2[p.curve].clone());
                            s.curves2.len() - 1
                        }))
                    } else {
                        let c = on_edge_parameter(&b.curves2[p.curve], p.map)?;
                        s.curves2.push(c);
                        Some(s.curves2.len() - 1)
                    }
                });
                uses.push(Use {
                    edge: edges[&b.fins[fin].edge],
                    sense: b.fins[fin].sense,
                    pcurve: file,
                });
            }
            loops.push(Loop::Edges(uses));
        }
        faces.insert(f, s.faces.len());
        origins.faces.push(f);
        s.faces.push(Face {
            surface,
            sense: face.sense,
            loops,
            tolerance: 0.0,
        });
        names.push(face.name);
    }
    for r in b.regions().skip(1) {
        let mut shells: Vec<(f64, usize)> = Vec::new();
        for sh in b.region_shells(r).collect::<Vec<ShellId>>() {
            let size = b
                .shell_faces(sh)
                .map(|f| crate::intersect::coarse_box(b, f))
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
    Cut { s, names, origins }
}

/// A spline pcurve on its edge's own parameter: its knots moved through
/// the inverse of the affine map `s = a + b t`, and where the map runs
/// the pcurve backwards (an edge of a boolean used against its curve), a
/// clamped spline's knots and control points read the other way. Other
/// kinds, and a periodic spline run backwards, go without: healing finds
/// those again.
fn on_edge_parameter(c: &Curve2, [a, b]: [f64; 2]) -> Option<Curve2> {
    let Curve2::Nurbs2(n) = c else {
        return None;
    };
    if b == 0.0 {
        return None;
    }
    let k = n.knots();
    let mut knots: Vec<f64> = k.knots().iter().map(|&x| (x - a) / b).collect();
    let mut mults = k.mults().to_vec();
    let mut points: Vec<_> = n.control_points().collect();
    let mut weights = n.weights().to_vec();
    if b < 0.0 {
        if k.is_periodic() {
            return None;
        }
        knots.reverse();
        mults.reverse();
        points.reverse();
        weights.reverse();
    }
    let kv = if k.is_periodic() {
        KnotVector::periodic(k.degree(), &knots, &mults).ok()?
    } else {
        KnotVector::clamped(k.degree(), &knots, &mults).ok()?
    };
    crate::geom::NurbsCurve2::new(kv, points, Some(weights))
        .ok()
        .map(|n| Curve2::Nurbs2(Arc::new(n)))
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
    let (mut n, _, bound) = pipe.to_nurbs_within(range, tolerance)?;
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
    Ok(())
}

/// A sweep face's surface replaced by a NURBS within `tolerance` over the
/// stretch of path the face covers, and all of its profile: periodic
/// along the path where the face wraps a closed one, and round a closed
/// profile as the profile's own NURBS is. Its uses lose their pcurves, as
/// a pipe face's do.
fn sweep_to_nurbs(cut: &mut Cut, k: usize, tolerance: f64) -> Result<(), Error> {
    let Surface::Sweep(sweep) = &cut.s.surfaces[cut.s.faces[k].surface] else {
        return Ok(());
    };
    let sweep = sweep.clone();
    let periods = sweep.periods();
    let [d0, d1] = sweep.path_range();
    let info = loops(cut, k, periods);
    let (lo, hi) = info
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, l| {
            (a.0.min(l.extent[1][0]), a.1.max(l.extent[1][1]))
        });
    let wraps = info.iter().any(|l| l.wind[1] != 0) || info.iter().all(|l| l.hole);
    let range = match periods.1 {
        Some(p) if wraps || hi - lo >= p => {
            let s0 = if lo.is_finite() { lo } else { d0 };
            [s0, s0 + p]
        }
        _ => {
            let m = 1e-6 * (hi - lo).max(LINEAR_RESOLUTION);
            [(lo - m).max(d0), (hi + m).min(d1)]
        }
    };
    let (mut n, _, bound) = sweep.to_nurbs_within(range, tolerance)?;
    if let Some(p) = periods.1
        && range[1] - range[0] == p
    {
        n = periodic_along_v(&n)?;
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
    Ok(())
}

/// As [`periodic_along_u`], along `v`: a sweep's NURBS round a closed path
/// over one period.
fn periodic_along_v(n: &NurbsSurface) -> Result<NurbsSurface, Error> {
    let [ku, kv] = n.knots();
    let mut mults = kv.mults().to_vec();
    let inner = mults[1..mults.len() - 1]
        .iter()
        .copied()
        .max()
        .unwrap_or(kv.degree());
    let last = mults.len() - 1;
    mults[0] = inner;
    mults[last] = inner;
    let periodic = KnotVector::periodic(kv.degree(), kv.knots(), &mults)?;
    let [nu, nv] = n.counts();
    let keep = periodic.control_count();
    let points: Vec<Point3> = (0..nu)
        .flat_map(|i| (0..keep).map(move |j| (i, j)))
        .map(|(i, j)| n.control_point(i, j))
        .collect();
    let weights: Vec<f64> = (0..nu)
        .flat_map(|i| (0..keep).map(move |j| n.weights()[i * nv + j]))
        .collect();
    NurbsSurface::new([ku.clone(), periodic], points, Some(weights))
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
    (0..=SAMPLES)
        .map(|m| {
            let f = m as f64 / SAMPLES as f64;
            let f = if u.sense == crate::topo::Sense::Forward {
                f
            } else {
                1.0 - f
            };
            let t = t0 + (t1 - t0) * f;
            (t, uv(cut, k, l, i, t))
        })
        .collect()
}

/// A use's parameters at edge parameter `t`: its curve's point there,
/// projected onto the face's surface. Geometry alone decides, never a
/// pcurve, so a body and the same body read back are cut alike.
fn uv(cut: &Cut, k: usize, l: usize, i: usize, t: f64) -> Point2 {
    let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
        unreachable!("a use is in a loop of edges")
    };
    let e = &cut.s.edges[uses[i].edge];
    let surface = &cut.s.surfaces[cut.s.faces[k].surface];
    let (u, v) = surface.project(cut.s.curves[e.curve].point(t)).param;
    Point2::new(u, v)
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
    let surface = &cut.s.surfaces[cut.s.faces[k].surface];
    let (domain, collapsed) = (surface.domain(), collapsed_rows(surface));
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
        let mut raw: Vec<[f64; 2]> = Vec::new();
        for i in 0..n {
            raw.extend(samples(cut, k, l, i).into_iter().map(|(_, p)| [p.x, p.y]));
        }
        // On a pole, a sample's parameter in direction `d` is whatever
        // projection makes of a point where it is undefined.
        let on_pole = |q: &[f64; 2], d: usize| {
            let o = 1 - d;
            (0..2).any(|e| {
                let at = domain[o][e];
                collapsed[o][e] && (q[o] - at).abs() <= 1e-9 * at.abs().max(1.0)
            })
        };
        // Across a pole the free parameter's jump is free, so a loop
        // through a pole winds round neither direction it could (a lune
        // between two meridians, or a sphere's hemisphere bounded by one
        // circle through both poles): as the mesher reads it.
        let through_pole: [bool; 2] = core::array::from_fn(|d| raw.iter().any(|q| on_pole(q, d)));
        let x = match (0..2).find(|&d| through_pole[d] && period[d].is_some()) {
            Some(d) => across_poles(&raw, d, period, |q| on_pole(q, d)),
            None => None,
        }
        .unwrap_or_else(|| {
            let mut x: Vec<[f64; 2]> = Vec::with_capacity(raw.len());
            for &r in &raw {
                let next = match x.last() {
                    Some(q) => core::array::from_fn(|d| unwrap(r[d], q[d], period[d])),
                    None => r,
                };
                x.push(next);
            }
            x
        });
        let (first, last) = (x[0], x[x.len() - 1]);
        let wind: [i64; 2] = core::array::from_fn(|d| match period[d] {
            Some(_) if through_pole[d] => 0,
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

/// A loop's samples unwrapped where it passes through a pole in its
/// free direction `d`: each run of samples on the pole becomes two, at
/// the free parameters of the samples either side of it, and the step
/// between them, along the pole, is whatever closes the loop (the
/// shortest way, unless that would leave it wound round). So a cone's
/// lune between two rulings through its apex reads as the lune, not as
/// a hole round the whole cone. `None` where every sample is on a pole.
fn across_poles(
    raw: &[[f64; 2]],
    d: usize,
    period: [Option<f64>; 2],
    on_pole: impl Fn(&[f64; 2]) -> bool,
) -> Option<Vec<[f64; 2]>> {
    let p = period[d]?;
    let n = raw.len();
    let s0 = (0..n).find(|&i| !on_pole(&raw[i]))?;
    // The points in order from `s0`, each step's free jump marked.
    let mut pts: Vec<([f64; 2], bool)> = Vec::new();
    let mut m = 0;
    while m < n {
        let i = (s0 + m) % n;
        if !on_pole(&raw[i]) {
            pts.push((raw[i], false));
            m += 1;
            continue;
        }
        let run = (m..n).take_while(|&r| on_pole(&raw[(s0 + r) % n])).count();
        let before = raw[(s0 + m + n - 1) % n];
        let after = raw[(s0 + m + run) % n];
        let mut a = raw[i];
        a[d] = before[d];
        let mut b = raw[i];
        b[d] = after[d];
        pts.push((a, false));
        pts.push((b, true));
        m += run;
    }
    let mut x: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
    let mut free = None;
    for (k, &(r, jump)) in pts.iter().enumerate() {
        let next = match x.last() {
            None => r,
            Some(q) if jump => {
                free.get_or_insert(k);
                let mut next = core::array::from_fn(|e| unwrap(r[e], q[e], period[e]));
                next[d] = q[d] + offset(r[d], q[d], p);
                next
            }
            Some(q) => core::array::from_fn(|e| unwrap(r[e], q[e], period[e])),
        };
        x.push(next);
    }
    // Close the loop: the samples end where they began.
    let turns = ((x[x.len() - 1][d] - x[0][d]) / p).round();
    if let Some(f) = free
        && turns != 0.0
    {
        for q in &mut x[f..] {
            q[d] -= turns * p;
        }
    }
    Some(x)
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
/// any sample comes to it otherwise. The loop's samples are walked in
/// order: a crossing is a change of side, through any samples on the
/// line (a touch, which comes back to its side, is none); a jump of half
/// a period is the loop passing the far side, not a crossing. On the
/// line at a use's first sample is at its vertex; else the place is
/// refined between the samples either side.
fn crossings(cut: &Cut, k: usize, l: usize, j: usize, c: f64, period: f64) -> (Vec<Crossing>, f64) {
    let Loop::Edges(uses) = &cut.s.faces[k].loops[l] else {
        return (Vec::new(), f64::INFINITY);
    };
    let eps = 1e-10 * period;
    // (use, sample, edge parameter, signed offset), each vertex once.
    let mut walk: Vec<(usize, usize, f64, f64)> = Vec::new();
    for i in 0..uses.len() {
        let s = samples(cut, k, l, i);
        for (m, (t, p)) in s.iter().enumerate().take(s.len() - 1) {
            let g = offset([p.x, p.y][j], c, period);
            walk.push((i, m, *t, if g.abs() <= eps { 0.0 } else { g }));
        }
    }
    let nearest = walk
        .iter()
        .map(|x| x.3.abs())
        .filter(|&g| g > 0.0)
        .fold(f64::INFINITY, f64::min);
    let n = walk.len();
    // Start just after a sample off the line, so every run of samples on
    // it is seen whole.
    let Some(first) = (0..n).find(|&m| walk[m].3 != 0.0) else {
        return (Vec::new(), nearest);
    };
    let mut out = Vec::new();
    let mut last = walk[first];
    let mut zeros: Vec<usize> = Vec::new();
    for step in 1..=n {
        let m = (first + step) % n;
        let x = walk[m];
        if x.3 == 0.0 {
            zeros.push(m);
            continue;
        }
        let jump = (x.3 - last.3).abs() >= period / 2.0;
        if (x.3 < 0.0) != (last.3 < 0.0) && !jump {
            out.push(match zeros.first() {
                Some(&z) if walk[z].1 == 0 => Crossing::Vertex(walk[z].0),
                Some(&z) => Crossing::Inside(walk[z].0, walk[z].2),
                None => {
                    // Between `last` and `x`: within one use unless `x`
                    // is the next use's vertex, so inside `last`'s use.
                    let i = last.0;
                    let tb = if x.0 == i {
                        x.2
                    } else {
                        let e = &cut.s.edges[uses[i].edge];
                        match uses[i].sense {
                            crate::topo::Sense::Forward => e.interval[1],
                            crate::topo::Sense::Reversed => e.interval[0],
                        }
                    };
                    Crossing::Inside(i, refine(cut, k, uses[i].edge, j, c, period, last.2, tb))
                }
            });
        }
        zeros.clear();
        last = x;
    }
    (out, nearest)
}

/// Bisection on an edge's parameter between `ta` and `tb`, either side of
/// `j = c`, measuring the curve's point as the surface projects it, down
/// to two neighbouring floats: the upper is the answer. It depends only
/// on the curve and the surface, never on a pcurve or where the search
/// began, so writing a body again splits it at the same bits.
#[allow(clippy::too_many_arguments)]
fn refine(cut: &Cut, k: usize, e: usize, j: usize, c: f64, period: f64, ta: f64, tb: f64) -> f64 {
    let curve = &cut.s.curves[cut.s.edges[e].curve];
    let surface = &cut.s.surfaces[cut.s.faces[k].surface];
    let below = |t: f64| {
        let (pu, pv) = surface.project(curve.point(t)).param;
        offset([pu, pv][j], c, period) < 0.0
    };
    let (mut lo, mut hi) = (ta.min(tb), ta.max(tb));
    let side = below(lo);
    for _ in 0..2200 {
        let mid = lo + (hi - lo) / 2.0;
        if mid <= lo || mid >= hi {
            break;
        }
        if below(mid) == side {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
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

/// The parameter lines a seam in direction `j` may take, in order: each
/// vertex's on the winding loops, as the surface projects it, the
/// surface's own seam, the middles between those, and a grid over the
/// period. None depends on a pcurve, so a body written again takes the
/// same seam.
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
    let mut out: Vec<f64> = points
        .iter()
        .map(|&p| {
            let (u, v) = surface.project(p).param;
            [u, v][j]
        })
        .collect();
    out.push(surface.domain()[j][0]);
    // Between neighbouring vertices: where a loop runs back on itself,
    // or along a parameter line, lines through its vertices cross it
    // more than once.
    let mut at: Vec<f64> = out.iter().map(|&x| x.rem_euclid(period)).collect();
    at.sort_by(f64::total_cmp);
    at.dedup();
    if let (Some(&first), Some(&last)) = (at.first(), at.last()) {
        let mut mids: Vec<f64> = at.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect();
        mids.push(((last + first + period) / 2.0).rem_euclid(period));
        out.extend(mids);
    }
    // Then a grid over the period, coarse to fine: fixed values, so a
    // face whose loops are all holes takes the same seam whatever
    // pcurves its edges have.
    let d0 = surface.domain()[j][0];
    for level in 1..=8u32 {
        let n = 1u32 << level;
        for i in (1..n).step_by(2) {
            out.push(d0 + period * f64::from(i) / f64::from(n));
        }
    }
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
        seam: true,
    });
    cut.origins.vertices.push(None);
    // A closed edge on a periodic curve ends one period after it starts,
    // as healing will join it again: its rounding mustn't name its pieces.
    let end = match cut.s.curves[edge.curve].period() {
        Some(p) if edge.start == edge.end => edge.interval[0] + p,
        _ => edge.interval[1],
    };
    let e2 = cut.s.edges.len();
    cut.origins.edges.push(cut.origins.edges[e]);
    cut.s.edges.push(Edge {
        interval: [t, end],
        start: x,
        ..edge
    });
    cut.s.edges[e] = Edge {
        interval: [edge.interval[0], t],
        end: x,
        ..edge
    };
    for face in &mut cut.s.faces {
        for lp in &mut face.loops {
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
                match u.sense {
                    crate::topo::Sense::Forward => uses.insert(i + 1, second),
                    crate::topo::Sense::Reversed => uses.insert(i, second),
                }
                i += 2;
            }
        }
    }
    x
}

/// The loop's uses turned to start at the use that starts at vertex `v`.
fn start_at(cut: &Cut, k: usize, l: usize, v: usize) -> Vec<Use> {
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
            uses[a]
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
        (Surface::Revolution(r), 1) => {
            // The circle the curve's point at `c` turns round the axis,
            // on the revolution's own angle.
            let fr = r.frame();
            let d = r.curve().point(c) - fr.origin();
            let (rho, h) = (d.dot(fr.x().get()), d.dot(fr.z().get()));
            let frame = Frame::from_axes(fr.origin() + fr.z().get() * h, fr.z(), fr.x()).ok()?;
            Circle::new(frame, rho).ok().map(Curve::Circle)
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
    // The point of each pole, and the lone vertex there, if any.
    let pole = |end: usize| {
        if j == 0 {
            surface.point(c, domain[end])
        } else {
            surface.point(domain[end], c)
        }
    };
    let lone_at = |cut: &Cut, end: usize| -> Option<(usize, usize)> {
        info.iter().enumerate().find_map(|(l, x)| {
            let v = x.lone?;
            (cut.s.vertices[v].point.distance(pole(end)) <= 10.0 * LINEAR_RESOLUTION)
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
                    cut.s.vertices.push(Vertex {
                        point: pole(end),
                        tolerance: 0.0,
                        seam: true,
                    });
                    cut.origins.vertices.push(None);
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
    let mut ring: Vec<Use> = Vec::new();
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
) -> (Use, Use) {
    cut.s.curves.push(curve);
    let e = cut.s.edges.len();
    cut.origins.edges.push(None);
    cut.s.edges.push(Edge {
        curve: cut.s.curves.len() - 1,
        interval,
        start,
        end,
        tolerance: 0.0,
    });
    let mut one = |at: f64, sense| {
        cut.s.curves2.push(iso2(j, at));
        Use {
            edge: e,
            sense,
            pcurve: Some(cut.s.curves2.len() - 1),
        }
    };
    (
        one(copies[0], crate::topo::Sense::Forward),
        one(copies[1], crate::topo::Sense::Reversed),
    )
}

/// Removes loops `gone` (sorted) of face `k` and adds `ring` as its first
/// loop.
fn replace_loops(cut: &mut Cut, k: usize, gone: &[usize], ring: Vec<Use>) {
    for &l in gone.iter().rev() {
        cut.s.faces[k].loops.remove(l);
    }
    cut.s.faces[k].loops.insert(0, Loop::Edges(ring));
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
        seam: true,
    });
    cut.origins.vertices.push(None);
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
        let flip = |u: Use| Use {
            sense: u.sense.flipped(),
            ..u
        };
        vec![flip(su.1), flip(sv.1), flip(su.0), flip(sv.0)]
    };
    replace_loops(cut, k, &[], ring);
    Ok(())
}
