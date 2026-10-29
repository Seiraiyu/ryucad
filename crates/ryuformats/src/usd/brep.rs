//! A body as an omniSolid `BrepArray` prim: its exact B-rep, as NVIDIA's
//! `omniSolid` schema (`source/schema/omniSolid/resources/schema.usda` at
//! usd-brep [`SCHEMA_COMMIT`]) describes it. The schema is a proposal
//! that leaves rules unstated; where it does, this follows what usd-brep's
//! own writers and its SMLib reader do at that commit, as noted.
//!
//! The body is first cut open along seams by [`ryukei::heal::seamed`],
//! as STEP's writer cuts it, so every face is a region of its surface's
//! parameter plane bounded like a disk with holes; pipes and sweeps come
//! out as NURBS within the pipe tolerance. Then:
//!
//! - **Regions and shells.** One brep for the body: the infinite void
//!   region first, holding each solid's outer shell seen from outside;
//!   then each solid region, its outer shell seen from inside and each
//!   cavity's shell seen from the material, followed by each cavity's void
//!   region. Each face has two faceuses, `same` on the side its surface's
//!   natural normal points to: the void's side for a face of sense
//!   `Forward` (whose normal points out of the material), the material's
//!   for `Reversed`.
//! - **Faces** are numbered by their names (the mesh's face indices are
//!   the same, [`face_order`]); each has its loops, the outer one first,
//!   running counterclockwise in its parameter plane (about the surface's
//!   natural normal, not the face's: usd-brep stores loops "in use
//!   direction" and its OCCT reader reverses a reversed face's), holes
//!   clockwise, a lone vertex as a loop with no edgeuses.
//! - **Edgeuses** follow their loops; one is `same` where its loop runs
//!   along its edge's curve. Every edge has two, which name each other as
//!   the next round the edge, and an edgeuse enters its face from the
//!   bottom exactly when it is `same` (the right-hand rule round the
//!   edge with loops counterclockwise about the normal; usd-brep's OCCT
//!   reader writes the same).
//! - **Geometry.** Planes, cylinders, cones, spheres and tori as their
//!   analytic APIs (ryukei's parameters are the schema's, which are
//!   STEP's), NURBS as `BrepSurfaceNurbAPI` (clamped, weights always),
//!   extrusions and revolutions as their exact NURBS. Lines, circles and
//!   ellipses as their APIs, NURBS edges clamped. Every edgeuse has a
//!   `BrepCurveUvNurbAPI` curve on its edge's parameter (an affine function
//!   of it, as SMLib's reader requires), running the way the edgeuse
//!   does: the file's pcurve or the body's where they hold, else an exact
//!   one, else one fitted and certified ([`ryukei::pcurve::fitted`]). A
//!   circle or an ellipse in the parameter plane is its rational form,
//!   whose parameter is the edge's only at its ends. Each face's curves
//!   are moved by whole periods into one band of each periodic direction:
//!   between its seams where it has them, else cut where its curves leave
//!   a gap, the gap that leaves its outer loop counterclockwise.
//! - **Units.** Lengths are in the stage's unit: points, radii, a line's
//!   parameter, and the linear parameters of planes, cylinders and cones
//!   (their pcurves and ranges with them); angles stay radians and NURBS
//!   parameters as they are.
//! - **Tolerance and extent.** `intersectTol3d` is the largest tolerance
//!   of the body's edges and vertices, of a face's approximation and of a
//!   fitted pcurve, at least [`LINEAR_RESOLUTION`]. `brep:extent` is the
//!   faces' boxes and every control point, grown by it; the prim's
//!   `extent` holds it in `f32`, rounded outward.
//! - **Names.** `ryucad:faceName`, `ryucad:edgeName` and
//!   `ryucad:vertexName` give each face, edge and vertex the body's name
//!   (32 hex digits); a seam, or a vertex a seam added, has `""`. A piece
//!   of an edge a seam split keeps the edge's name.

use super::layer::{BadName, Prim, Specifier, Value};
use crate::scene::Unit;
use ryukei::geom::{
    Box3, Curve, Curve2, Direction, KnotVector, NurbsCurve2, NurbsSurface, Point2, Point3, Reparam,
    Surface,
};
use ryukei::heal::{Loop, Origins, Shape, Use, seamed_with_origins};
use ryukei::name::Name;
use ryukei::tol::LINEAR_RESOLUTION;
use ryukei::topo::{Body, FaceId, Sense};
use ryukei::{Error, HealError};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The usd-brep commit whose `omniSolid` schema this writes, the one the
/// devcontainer builds SMLib from.
pub const SCHEMA_COMMIT: &str = "c9c5979f04cea8c38213004d4f04df0dcfb40d05";

/// `loop:vertexIndex` for a loop of edgeuses, as usd-brep writes it.
const NO_OBJECT: u32 = 9_999_999;

/// Why a body can't be written as a BrepArray: a face the schema can't
/// hold, by name, and why. Its loops can't be cut open along seams
/// (`seams`); an edge of it has no pcurve that could be found or
/// certified (`pcurve`); its curves fit in no band of a period
/// (`domain`); or its pipe or sweep surface won't approximate within the
/// tolerance (`approximation`: the least-named face of the body, as STEP's
/// writer names it, since the kernel names none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrepError {
    pub face: Name,
    pub reason: &'static str,
}

impl std::fmt::Display for BrepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "face {} can't be written as a BrepArray ({})",
            self.face, self.reason
        )
    }
}

impl std::error::Error for BrepError {}

/// The faces of `body` in the order a stage numbers them: by name.
pub fn face_order(body: &Body) -> Vec<FaceId> {
    let mut faces: Vec<FaceId> = body.faces().collect();
    faces.sort_by_key(|&f| body.face(f).expect("a live face").name);
    faces
}

/// One use of an edge, lowered: the edge, whether the loop runs along
/// it, and its curve in the face's parameters, on the edge's parameter:
/// running with the edge until [`face`] turns it to run as the loop does.
#[derive(Clone, Debug)]
struct Lowered {
    edge: usize,
    same: bool,
    uv: NurbsCurve2,
    /// A seam's use: placed where the seam cut, never moved.
    seam: bool,
}

#[derive(Clone, Debug)]
enum LoopOut {
    Edges(Vec<Lowered>),
    Vertex(usize),
}

/// A face, lowered: its geometry, loops (outer first), parameter range
/// and the largest tolerance it adds.
#[derive(Clone, Debug)]
struct FaceOut {
    geometry: Geometry,
    sense: Sense,
    loops: Vec<LoopOut>,
    range: [[f64; 2]; 2],
    tolerance: f64,
}

#[derive(Clone, Debug)]
enum Geometry {
    Analytic(Surface),
    Nurbs(NurbsSurface),
}

impl Geometry {
    fn surface(&self) -> Surface {
        match self {
            Geometry::Analytic(s) => s.clone(),
            Geometry::Nurbs(n) => Surface::Nurbs(Arc::new(n.clone())),
        }
    }

    /// What each parameter is multiplied by in the stage's unit `k`.
    fn scale(&self, k: f64) -> [f64; 2] {
        match self {
            Geometry::Analytic(Surface::Plane(_)) => [k, k],
            Geometry::Analytic(Surface::Cylinder(_) | Surface::Cone(_)) => [1.0, k],
            _ => [1.0, 1.0],
        }
    }
}

/// What lowering a body needs at hand.
struct Cx<'a> {
    body: &'a Body,
    shape: &'a Shape,
    origins: &'a Origins,
}

/// `body` as a `BrepArray` prim named `name`, in `unit`, pipes and
/// sweeps within `pipe_tolerance`.
pub fn brep_array(
    body: &Body,
    name: &str,
    unit: Unit,
    pipe_tolerance: f64,
) -> Result<Prim, BrepError> {
    let (shape, origins) = seamed_with_origins(body, pipe_tolerance).map_err(|e| match e {
        Error::InvalidHeal(HealError::BadDomain { face }) => BrepError {
            face,
            reason: "seams",
        },
        _ => BrepError {
            face: face_order(body)
                .first()
                .and_then(|&f| body.face(f))
                .map_or(Name::from_u128(0), |f| f.name),
            reason: "approximation",
        },
    })?;
    let cx = Cx {
        body,
        shape: &shape,
        origins: &origins,
    };
    let mut order: Vec<usize> = (0..shape.faces.len()).collect();
    order.sort_by_key(|&i| body.face(origins.faces[i]).expect("a live face").name);
    let mut faces = Vec::with_capacity(order.len());
    for &i in &order {
        faces.push(face(&cx, i)?);
    }
    let mut position = vec![0; shape.faces.len()];
    for (n, &i) in order.iter().enumerate() {
        position[i] = n;
    }
    Ok(pack(&cx, name, unit, &faces, &position)
        .unwrap_or_else(|e| unreachable!("the BrepArray's names are the schema's: {e}")))
}

/// The face's name in the body.
fn face_name(cx: &Cx, k: usize) -> Name {
    cx.body.face(cx.origins.faces[k]).expect("a live face").name
}

/// A face lowered: its geometry, its uses' curves placed in one band of
/// each period, its loops in the schema's orientation, outer first.
fn face(cx: &Cx, k: usize) -> Result<FaceOut, BrepError> {
    let f = &cx.shape.faces[k];
    let surface = &cx.shape.surfaces[f.surface];
    let unrepresentable = |reason| BrepError {
        face: face_name(cx, k),
        reason,
    };
    let mut tolerance = f.tolerance;
    // Each loop's uses in the face's own parameters first.
    let mut loops: Vec<LoopOut> = Vec::new();
    for l in &f.loops {
        loops.push(match l {
            Loop::Vertex(v) => LoopOut::Vertex(*v),
            Loop::Edges(uses) => {
                let mut out = Vec::with_capacity(uses.len());
                for u in uses {
                    let (uv, bound) =
                        pcurve(cx, k, surface, u).ok_or_else(|| unrepresentable("pcurve"))?;
                    tolerance = tolerance.max(bound);
                    out.push(Lowered {
                        edge: u.edge,
                        same: u.sense == Sense::Forward,
                        uv,
                        seam: cx.origins.edges[u.edge].is_none(),
                    });
                }
                LoopOut::Edges(out)
            }
        });
    }
    // An extrusion or a revolution becomes its exact NURBS over the range
    // its curves cover, and its curves are found again on that.
    let mut s = surface.clone();
    if let Surface::Extrusion(_) | Surface::Revolution(_) = surface {
        place(&mut loops, surface, tolerance.max(LINEAR_RESOLUTION))
            .ok_or_else(|| unrepresentable("domain"))?;
        let (n, reparam) = surface
            .to_nurbs(range(&loops, surface))
            .map_err(|_| unrepresentable("pcurve"))?;
        s = Surface::Nurbs(Arc::new(n));
        for l in &mut loops {
            if let LoopOut::Edges(uses) = l {
                for x in uses.iter_mut() {
                    let (uv, bound) =
                        converted(cx, &s, x, reparam).ok_or_else(|| unrepresentable("pcurve"))?;
                    tolerance = tolerance.max(bound);
                    x.uv = uv;
                }
            }
        }
    }
    // The schema's orientation: counterclockwise about the surface's
    // normal, so a reversed face's loops run the other way; and each
    // curve, until now running with its edge, runs as its loop does.
    for l in &mut loops {
        if let LoopOut::Edges(uses) = l {
            if f.sense == Sense::Reversed {
                uses.reverse();
                for x in uses.iter_mut() {
                    x.same = !x.same;
                }
            }
            for x in uses.iter_mut().filter(|x| !x.same) {
                x.uv = x.uv.reversed();
            }
        }
    }
    place(&mut loops, &s, tolerance.max(LINEAR_RESOLUTION))
        .ok_or_else(|| unrepresentable("domain"))?;
    // The outer loop first: the one enclosing the most.
    let outer = loops
        .iter()
        .enumerate()
        .filter_map(|(i, l)| match l {
            LoopOut::Edges(u) => Some((i, area(u))),
            LoopOut::Vertex(_) => None,
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0)
        .ok_or_else(|| unrepresentable("domain"))?;
    let first = loops.remove(outer);
    loops.insert(0, first);
    loops.sort_by_key(|l| matches!(l, LoopOut::Vertex(_)));
    // A NURBS clamped at the start of the band its curves lie in, along
    // each periodic direction, and its curves moved with it.
    let geometry = match &s {
        Surface::Nurbs(n) => {
            let mut n = (**n).clone();
            for (j, dir) in [Direction::U, Direction::V].into_iter().enumerate() {
                let Some(p) = periods(&Surface::Nurbs(Arc::new(n.clone())))[j] else {
                    continue;
                };
                let r = range(&loops, &Surface::Nurbs(Arc::new(n.clone())))[j];
                n = n
                    .part(dir, [r[0], r[0] + p])
                    .map_err(|_| unrepresentable("domain"))?;
                let shift = n.domain()[j][0] - r[0];
                if shift != 0.0 {
                    let d = if j == 0 { [shift, 0.0] } else { [0.0, shift] };
                    for l in &mut loops {
                        if let LoopOut::Edges(uses) = l {
                            for x in uses.iter_mut() {
                                x.uv = moved(&x.uv, d);
                            }
                        }
                    }
                }
            }
            Geometry::Nurbs(n.to_clamped())
        }
        s => Geometry::Analytic(s.clone()),
    };
    let range = range(&loops, &geometry.surface());
    Ok(FaceOut {
        geometry,
        sense: f.sense,
        loops,
        range,
        tolerance,
    })
}

/// A surface's periods, as an array.
fn periods(s: &Surface) -> [Option<f64>; 2] {
    let (u, v) = s.periods();
    [u, v]
}

/// The body's fin for a use of the shape, where the shape's face and edge
/// are the body's own and its surface is the body face's (not a pipe or
/// sweep the shape turned into a NURBS): its pcurve and map.
fn body_pcurve(cx: &Cx, k: usize, u: &Use) -> Option<(Curve2, [f64; 2])> {
    let edge = cx.origins.edges[u.edge]?;
    let face = cx.origins.faces[k];
    let b = cx.body;
    let shape_surface = &cx.shape.surfaces[cx.shape.faces[k].surface];
    let body_surface = b.surface(b.face(face)?.surface)?;
    if std::mem::discriminant(shape_surface) != std::mem::discriminant(body_surface) {
        return None;
    }
    let fin = b.edge_fins(edge).find(|&fin| {
        b.fin(fin)
            .is_some_and(|x| x.sense == u.sense && b.loop_(x.loop_).is_some_and(|l| l.face == face))
    })?;
    let p = b.fin(fin)?.pcurve?;
    Some((b.curve2(p.curve)?.clone(), p.map))
}

/// The tolerance of the body's edge a use runs along (resolution for a
/// seam).
fn edge_tolerance(cx: &Cx, edge: usize) -> f64 {
    cx.origins.edges[edge]
        .map(|e| cx.body.edge(e).expect("a live edge").tolerance.get())
        .unwrap_or(LINEAR_RESOLUTION)
        .max(LINEAR_RESOLUTION)
}

/// A use's curve in its face's parameters, on its edge's parameter and
/// running with its edge, and the bound it adds (0 if exact): the
/// shape's pcurve, else the body's, else an exact one, else a fitted one.
fn pcurve(cx: &Cx, k: usize, surface: &Surface, u: &Use) -> Option<(NurbsCurve2, f64)> {
    let e = &cx.shape.edges[u.edge];
    let curve = &cx.shape.curves[e.curve];
    let tol = edge_tolerance(cx, u.edge);
    let found = u
        .pcurve
        .map(|p| (cx.shape.curves2[p].clone(), [0.0, 1.0]))
        .or_else(|| body_pcurve(cx, k, u))
        .or_else(|| ryukei::pcurve::exact(surface, curve, e.interval, tol));
    if let Some((c, map)) = found
        && let Some(n) = on_edge(&c, map, e.interval)
    {
        return Some((n, 0.0));
    }
    let (n, bound) = ryukei::pcurve::fitted(surface, curve, e.interval, tol)?;
    Some((n, bound))
}

/// A use's curve on an extrusion or a revolution's NURBS form `n`, whose
/// parameters relate to the swept surface's by `reparam`: a seam (a line
/// along one parameter) with its constant carried over, where the other
/// parameter is unchanged; else an exact pcurve on `n`, else a fitted one.
fn converted(
    cx: &Cx,
    n: &Surface,
    x: &Lowered,
    reparam: [Reparam; 2],
) -> Option<(NurbsCurve2, f64)> {
    let e = &cx.shape.edges[x.edge];
    let curve = &cx.shape.curves[e.curve];
    let tol = edge_tolerance(cx, x.edge);
    // `x.uv` runs with its edge here: loops aren't turned yet.
    if x.seam
        && let [a, b] = [x.uv.control_points().next()?, x.uv.control_points().last()?]
        && x.uv.degree() == 1
        && x.uv.control_points().len() == 2
    {
        for j in 0..2 {
            let other = 1 - j;
            let c = |q: Point2| if j == 0 { q.x } else { q.y };
            if c(a) == c(b) && reparam[other] == Reparam::Identity {
                let mut p = [a, b];
                for q in &mut p {
                    let mut c = [q.x, q.y];
                    c[j] = reparam[j].to_nurbs(c[j]);
                    *q = Point2::new(c[0], c[1]);
                }
                return Some((line(p[0], p[1], e.interval)?, 0.0));
            }
        }
    }
    if let Some((c, map)) = ryukei::pcurve::exact(n, curve, e.interval, tol)
        && let Some(n) = on_edge(&c, map, e.interval)
    {
        return Some((n, 0.0));
    }
    ryukei::pcurve::fitted(n, curve, e.interval, tol)
}

/// A degree 1 curve from `a` to `b` over `[t0, t1]`.
fn line(a: Point2, b: Point2, [t0, t1]: [f64; 2]) -> Option<NurbsCurve2> {
    let knots = KnotVector::clamped(1, &[t0, t1], &[2, 2]).ok()?;
    NurbsCurve2::new(knots, vec![a, b], None).ok()
}

/// `c` at `s = map[0] + map[1] t` over `t` in `[t0, t1]`, as a clamped
/// NURBS on `t`: exact for lines and splines; a circle or an ellipse is
/// its rational form, whose knots are moved affinely onto `[t0, t1]`.
fn on_edge(c: &Curve2, [a, b]: [f64; 2], [t0, t1]: [f64; 2]) -> Option<NurbsCurve2> {
    let (s0, s1) = (a + b * t0, a + b * t1);
    if let Curve2::Line2(_) = c {
        return line(c.point(s0), c.point(s1), [t0, t1]);
    }
    let (lo, hi) = if s0 <= s1 { (s0, s1) } else { (s1, s0) };
    let n = match c {
        Curve2::Nurbs2(n) => n.part([lo, hi]).ok()?,
        _ => {
            let (n, r) = c.to_nurbs([lo, hi]).ok()?;
            n.part([r.to_nurbs(lo), r.to_nurbs(hi)]).ok()?
        }
    };
    let n = if s0 > s1 { n.reversed() } else { n };
    remapped(&n, [t0, t1])
}

/// `n` (clamped) with its knots moved affinely onto `[t0, t1]`.
fn remapped(n: &NurbsCurve2, [t0, t1]: [f64; 2]) -> Option<NurbsCurve2> {
    let k = n.knots();
    let [d0, d1] = k.domain();
    let last = k.knots().len() - 1;
    let knots: Vec<f64> = k
        .knots()
        .iter()
        .enumerate()
        .map(|(i, &x)| match i {
            0 => t0,
            i if i == last => t1,
            _ => t0 + (x - d0) * (t1 - t0) / (d1 - d0),
        })
        .collect();
    let kv = KnotVector::clamped(k.degree(), &knots, k.mults()).ok()?;
    NurbsCurve2::new(kv, n.control_points().collect(), Some(n.weights().to_vec())).ok()
}

/// `n` moved by `d` in its plane.
fn moved(n: &NurbsCurve2, d: [f64; 2]) -> NurbsCurve2 {
    let points = n
        .control_points()
        .map(|p| Point2::new(p.x + d[0], p.y + d[1]))
        .collect();
    NurbsCurve2::new(n.knots().clone(), points, Some(n.weights().to_vec()))
        .expect("moving a spline keeps it valid")
}

/// Points along `n`: `count + 1` at equal steps of its parameter.
fn samples(n: &NurbsCurve2, count: usize) -> impl Iterator<Item = Point2> + '_ {
    let [a, b] = n.domain();
    (0..=count).map(move |i| n.point(a + (b - a) * i as f64 / count as f64))
}

/// A loop's signed area in the parameter plane (positive
/// counterclockwise), from its curves' samples.
fn area(uses: &[Lowered]) -> f64 {
    let mut sum = 0.0;
    let points: Vec<Point2> = uses.iter().flat_map(|x| samples(&x.uv, 16)).collect();
    for i in 0..points.len() {
        let (p, q) = (points[i], points[(i + 1) % points.len()]);
        sum += p.x * q.y - q.x * p.y;
    }
    sum / 2.0
}

/// Moves each use's curve by whole periods into one band of each periodic
/// direction of `surface`. The band starts at the face's seam across that
/// direction where it has one, else at a cut in a gap its curves leave
/// (the widest first) that leaves its loops enclosing a positive area;
/// `None` where no cut does. In each loop, a use follows on from the one
/// before it (from the seam, where the loop has one), but for one after a
/// pole, which goes where its middle falls in the band.
fn place(loops: &mut [LoopOut], surface: &Surface, tolerance: f64) -> Option<()> {
    let periods = periods(surface);
    for (j, p) in periods.into_iter().enumerate() {
        let Some(p) = p else { continue };
        let at = |q: Point2| if j == 0 { q.x } else { q.y };
        let along = |q: Point2, d: f64| {
            if j == 0 {
                Point2::new(q.x + d, q.y)
            } else {
                Point2::new(q.x, q.y + d)
            }
        };
        // A seam across this direction: a line at one value of it.
        let across = |x: &Lowered| {
            x.seam && {
                let pts: Vec<Point2> = x.uv.control_points().collect();
                pts.iter().all(|&q| at(q) == at(pts[0]))
            }
        };
        // Whether the surface collapses along this direction at `q`.
        let pole = |q: Point2| {
            let o = surface.point(q.x, q.y);
            [p / 3.0, 2.0 * p / 3.0].into_iter().all(|d| {
                let r = along(q, d);
                surface.point(r.x, r.y).distance(o) <= tolerance
            })
        };
        let all: Vec<&Lowered> = loops
            .iter()
            .flat_map(|l| match l {
                LoopOut::Edges(u) => u.as_slice(),
                LoopOut::Vertex(_) => &[],
            })
            .collect();
        let seam = all
            .iter()
            .filter(|x| across(x))
            .map(|x| at(x.uv.control_points().next().expect("a point")))
            .reduce(f64::min);
        let starts: Vec<f64> = match seam {
            Some(c) => vec![c],
            None => {
                let mut s: Vec<f64> = all
                    .iter()
                    .flat_map(|x| samples(&x.uv, 8).map(at).collect::<Vec<_>>())
                    .map(|v| v.rem_euclid(p))
                    .collect();
                if s.is_empty() {
                    continue;
                }
                s.sort_by(f64::total_cmp);
                s.dedup();
                let mut gaps: Vec<(f64, f64)> = (0..s.len())
                    .map(|i| {
                        let next = if i + 1 < s.len() { s[i + 1] } else { s[0] + p };
                        (next - s[i], (s[i] + next) / 2.0)
                    })
                    .filter(|g| g.0 > 1e-9 * p)
                    .collect();
                gaps.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
                gaps.into_iter().map(|g| g.1).collect()
            }
        };
        let shift = |x: &mut Lowered, by: f64| {
            if by != 0.0 {
                let d = if j == 0 { [by, 0.0] } else { [0.0, by] };
                x.uv = moved(&x.uv, d);
            }
        };
        let ends = |x: &Lowered| {
            let [a, b] = x.uv.domain();
            (x.uv.point(a), x.uv.point(b))
        };
        let mut placed = false;
        for start in starts {
            let mut trial: Vec<LoopOut> = loops.to_vec();
            for l in &mut trial {
                let LoopOut::Edges(uses) = l else { continue };
                let n = uses.len();
                let first = (0..n).find(|&i| across(&uses[i])).unwrap_or(0);
                let mut prev: Option<Point2> = None;
                for m in 0..n {
                    let x = &mut uses[(first + m) % n];
                    if across(x) {
                        prev = Some(ends(x).1);
                        continue;
                    }
                    let by = match prev.filter(|&q| !pole(q)) {
                        // On from the use before.
                        Some(q) => ((at(q) - at(ends(x).0)) / p).round() * p,
                        // Where its middle falls in the band.
                        None => {
                            let [d0, d1] = x.uv.domain();
                            let mid = at(x.uv.point((d0 + d1) / 2.0));
                            let mut by = ((start - mid) / p).ceil() * p;
                            if mid + by >= start + p {
                                by -= p;
                            }
                            by
                        }
                    };
                    shift(x, by);
                    prev = Some(ends(x).1);
                }
            }
            let total: f64 = trial
                .iter()
                .map(|l| match l {
                    LoopOut::Edges(u) => area(u),
                    LoopOut::Vertex(_) => 0.0,
                })
                .sum();
            if seam.is_some() || total > 0.0 {
                loops.clone_from_slice(&trial);
                placed = true;
                break;
            }
        }
        if !placed {
            return None;
        }
    }
    Some(())
}

/// The box of a face's curves in its parameters, held to its surface's
/// domain where that is bounded.
fn range(loops: &[LoopOut], surface: &Surface) -> [[f64; 2]; 2] {
    let mut r = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for l in loops {
        if let LoopOut::Edges(uses) = l {
            for x in uses {
                let b = x.uv.bounds(x.uv.domain());
                for (j, (lo, hi)) in [(b.min().x, b.max().x), (b.min().y, b.max().y)]
                    .into_iter()
                    .enumerate()
                {
                    r[j][0] = r[j][0].min(lo);
                    r[j][1] = r[j][1].max(hi);
                }
            }
        }
    }
    let d = surface.domain();
    for (j, p) in periods(surface).into_iter().enumerate() {
        match p {
            // No longer than a period: the curves' bounds may pass it by a
            // rounding error.
            Some(p) => r[j][1] = r[j][1].min(r[j][0] + p),
            None => {
                r[j][0] = r[j][0].max(d[j][0]);
                r[j][1] = r[j][1].min(d[j][1]);
            }
        }
    }
    r
}

/// Whether a face's only loop is four lines along its parameters around
/// its range (to rounding: a billionth of the range's size).
fn rectangular(f: &FaceOut) -> bool {
    let [LoopOut::Edges(uses)] = f.loops.as_slice() else {
        return false;
    };
    let r = f.range;
    let slack = |j: usize| 1e-9 * (r[j][1] - r[j][0] + r[j][0].abs() + r[j][1].abs());
    let on = |x: f64, j: usize| (x - r[j][0]).abs() <= slack(j) || (x - r[j][1]).abs() <= slack(j);
    uses.len() == 4
        && uses.iter().all(|x| {
            let p: Vec<Point2> = x.uv.control_points().collect();
            x.uv.degree() == 1
                && p.len() == 2
                && ((p[0].x == p[1].x && on(p[0].x, 0)) || (p[0].y == p[1].y && on(p[0].y, 1)))
        })
}

/// The packed arrays of one brep, filled face by face.
#[derive(Default)]
struct Arrays {
    region_shells: Vec<u32>,
    region_types: Vec<String>,
    shell_faceuses: Vec<u32>,
    faceuse_faces: Vec<u32>,
    faceuse_types: Vec<String>,
    face_loops: Vec<u32>,
    face_surfaces: Vec<String>,
    face_trims: Vec<String>,
    face_ranges: Vec<[f64; 2]>,
    loop_edgeuses: Vec<u32>,
    loop_vertices: Vec<u32>,
    edgeuse_edges: Vec<u32>,
    edgeuse_types: Vec<String>,
    edgeuse_next: Vec<u32>,
    edgeuse_entries: Vec<String>,
    uv: NurbsArrays2,
    edge_types: Vec<String>,
    edge_ranges: Vec<f64>,
    edge_vertices: Vec<[i32; 2]>,
    edge_nurbs: NurbsArrays3,
    /// Lines (origin and direction), circles and ellipses.
    lines: Analytic,
    circles: Analytic,
    ellipses: Analytic,
    surface_nurbs: SurfaceArrays,
    planes: Analytic,
    cylinders: Analytic,
    cones: Analytic,
    spheres: Analytic,
    tori: Analytic,
    points: Vec<[f64; 3]>,
}

/// Analytic geometry: each one's origin, axis (a line's direction) and
/// reference direction, and its numbers (radii, an angle).
#[derive(Default)]
struct Analytic {
    origins: Vec<[f64; 3]>,
    axes: Vec<[f64; 3]>,
    refs: Vec<[f64; 3]>,
    a: Vec<f64>,
    b: Vec<f64>,
}

#[derive(Default)]
struct NurbsArrays2 {
    points: Vec<[f64; 2]>,
    counts: Vec<u32>,
    orders: Vec<u32>,
    knots: Vec<f64>,
    weights: Vec<f64>,
}

#[derive(Default)]
struct NurbsArrays3 {
    points: Vec<[f64; 3]>,
    counts: Vec<u32>,
    orders: Vec<u32>,
    knots: Vec<f64>,
    weights: Vec<f64>,
}

#[derive(Default)]
struct SurfaceArrays {
    points: Vec<[f64; 3]>,
    counts: [Vec<u32>; 2],
    orders: [Vec<u32>; 2],
    knots: [Vec<f64>; 2],
    weights: Vec<f64>,
}

fn v3(p: Point3, k: f64) -> [f64; 3] {
    [p.x * k, p.y * k, p.z * k]
}

fn d3(v: ryukei::geom::Vector3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// The packed arrays as the prim's attributes.
fn pack(
    cx: &Cx,
    name: &str,
    unit: Unit,
    faces: &[FaceOut],
    position: &[usize],
) -> Result<Prim, BadName> {
    let shape = cx.shape;
    let k = unit.per_meter();
    let mut a = Arrays::default();
    let mut tolerance = LINEAR_RESOLUTION;
    for e in cx.origins.edges.iter().flatten() {
        tolerance = tolerance.max(cx.body.edge(*e).expect("a live edge").tolerance.get());
    }
    for v in cx.origins.vertices.iter().flatten() {
        tolerance = tolerance.max(cx.body.vertex(*v).expect("a live vertex").tolerance.get());
    }
    for f in faces {
        tolerance = tolerance.max(f.tolerance);
    }
    // Regions, shells and faceuses: each face's side away from the
    // material is `same` when its sense is forward.
    let faceuses = |a: &mut Arrays, shell: &[usize], material: bool| {
        let mut list: Vec<usize> = shell.iter().map(|&f| position[f]).collect();
        list.sort_unstable();
        a.shell_faceuses.push(list.len() as u32);
        for f in list {
            a.faceuse_faces.push(f as u32);
            let away = faces[f].sense == Sense::Forward;
            let same = away != material;
            a.faceuse_types
                .push(if same { "same" } else { "opposite" }.into());
        }
    };
    a.region_types.push("voidRegion".into());
    a.region_shells.push(shape.solids.len() as u32);
    for so in &shape.solids {
        faceuses(&mut a, &shape.shells[so.outer].faces, false);
    }
    for so in &shape.solids {
        a.region_types.push("solidRegion".into());
        a.region_shells.push(1 + so.voids.len() as u32);
        faceuses(&mut a, &shape.shells[so.outer].faces, true);
        for &v in &so.voids {
            faceuses(&mut a, &shape.shells[v].faces, true);
        }
        for &v in &so.voids {
            a.region_types.push("voidRegion".into());
            a.region_shells.push(1);
            faceuses(&mut a, &shape.shells[v].faces, false);
        }
    }
    // Edges and vertices, as the shape numbers them.
    for e in &shape.edges {
        a.edge_vertices.push([e.start as i32, e.end as i32]);
        let range = curve(&mut a, &shape.curves[e.curve], e.interval, k);
        a.edge_ranges.extend(range);
    }
    // Faces, loops and edgeuses; each edge's two uses name each other.
    let mut uses_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for f in faces {
        a.face_loops.push(f.loops.len() as u32);
        let s = f.geometry.scale(k);
        a.face_ranges
            .push([f.range[0][0] * s[0], f.range[1][0] * s[1]]);
        a.face_ranges
            .push([f.range[0][1] * s[0], f.range[1][1] * s[1]]);
        a.face_trims.push(
            if rectangular(f) {
                "rectangular"
            } else {
                "general"
            }
            .into(),
        );
        surface(&mut a, &f.geometry, k);
        for l in &f.loops {
            match l {
                LoopOut::Vertex(v) => {
                    a.loop_edgeuses.push(0);
                    a.loop_vertices.push(*v as u32);
                }
                LoopOut::Edges(uses) => {
                    a.loop_edgeuses.push(uses.len() as u32);
                    a.loop_vertices.push(NO_OBJECT);
                    // Each curve on its edge's range as written (a line's
                    // is in the stage's unit), in the stage's unit.
                    let curves: Vec<NurbsCurve2> = uses
                        .iter()
                        .map(|x| {
                            let r = [a.edge_ranges[2 * x.edge], a.edge_ranges[2 * x.edge + 1]];
                            let n = remapped(&x.uv.to_clamped(), r).expect("an increasing range");
                            scaled(&n, s)
                        })
                        .collect();
                    // A loop whose curves don't meet end to end jumps
                    // along a pole row, which a loop of UV curves can't:
                    // its edgeuses have the schema's (0, 0) record, no UV
                    // curve, and the reader trims from the edges.
                    let meets = (0..curves.len()).all(|i| {
                        let next = &curves[(i + 1) % curves.len()];
                        let (a, b) = (end(&curves[i]), start(next));
                        a.distance(b) <= UV_CLOSURE
                    });
                    for (x, uv) in uses.iter().zip(&curves) {
                        uses_of
                            .entry(x.edge)
                            .or_default()
                            .push(a.edgeuse_edges.len());
                        a.edgeuse_edges.push(x.edge as u32);
                        a.edgeuse_types
                            .push(if x.same { "same" } else { "opposite" }.into());
                        a.edgeuse_entries
                            .push(if x.same { "bottomEntry" } else { "topEntry" }.into());
                        a.edgeuse_next.push(0);
                        if meets {
                            nurbs2(&mut a.uv, uv);
                        } else {
                            a.uv.counts.push(0);
                            a.uv.orders.push(0);
                        }
                    }
                }
            }
        }
    }
    for list in uses_of.values() {
        for (i, &u) in list.iter().enumerate() {
            a.edgeuse_next[u] = list[(i + 1) % list.len()] as u32;
        }
    }
    for v in &shape.vertices {
        a.points.push(v3(v.point, k));
    }
    // The extent: every face's box, every control point and vertex.
    let mut boxed = Box3::from_points(shape.vertices.iter().map(|v| v.point))
        .unwrap_or(Box3::from_point(Point3::ORIGIN));
    for &f in &cx.origins.faces {
        boxed = boxed.union(&ryukei::intersect::face_box(cx.body, f));
    }
    let mut lo = v3(boxed.min(), k);
    let mut hi = v3(boxed.max(), k);
    for p in a
        .edge_nurbs
        .points
        .iter()
        .chain(&a.surface_nurbs.points)
        .chain(&a.points)
    {
        for j in 0..3 {
            lo[j] = lo[j].min(p[j]);
            hi[j] = hi[j].max(p[j]);
        }
    }
    let t = tolerance * k;
    for j in 0..3 {
        lo[j] -= t;
        hi[j] += t;
    }
    let mut p = Prim::new(Specifier::Def, Some("BrepArray"), name)?;
    let mut apis: Vec<String> = Vec::new();
    if !a.points.is_empty() {
        apis.push("BrepPointAPI:vertexPoint".into());
    }
    let kinds = [
        (
            "BrepCurve3dNurbAPI:edge3dNurb",
            !a.edge_nurbs.counts.is_empty(),
        ),
        ("BrepCurve3dLineAPI:edge3dLine", !a.lines.origins.is_empty()),
        (
            "BrepCurve3dCircleAPI:edge3dCircle",
            !a.circles.origins.is_empty(),
        ),
        (
            "BrepCurve3dEllipseAPI:edge3dEllipse",
            !a.ellipses.origins.is_empty(),
        ),
        ("BrepCurveUvNurbAPI", !a.uv.counts.is_empty()),
        ("BrepSurfaceNurbAPI", !a.surface_nurbs.counts[0].is_empty()),
        ("BrepSurfacePlaneAPI", !a.planes.origins.is_empty()),
        ("BrepSurfaceCylinderAPI", !a.cylinders.origins.is_empty()),
        ("BrepSurfaceConeAPI", !a.cones.origins.is_empty()),
        ("BrepSurfaceSphereAPI", !a.spheres.origins.is_empty()),
        ("BrepSurfaceTorusAPI", !a.tori.origins.is_empty()),
    ];
    apis.extend(kinds.iter().filter(|x| x.1).map(|x| x.0.to_owned()));
    p.meta.api_schemas = apis;
    let f32_down = |x: f64| {
        let y = x as f32;
        if (y as f64) > x { y.next_down() } else { y }
    };
    let f32_up = |x: f64| {
        let y = x as f32;
        if (y as f64) < x { y.next_up() } else { y }
    };
    p.uniform(
        "extent",
        Value::Float3Array(vec![lo.map(f32_down), hi.map(f32_up)]),
    )?;
    let toks = |v: Vec<String>| Value::TokenArray(v);
    p.uniform("brep:intersectTol3d", Value::DoubleArray(vec![t]))?;
    p.uniform("brep:extent", Value::Double3Array(vec![lo, hi]))?;
    p.uniform(
        "brep:regionCount",
        Value::UIntArray(vec![a.region_types.len() as u32]),
    )?;
    p.uniform("region:shellCount", Value::UIntArray(a.region_shells))?;
    p.uniform("region:type", toks(a.region_types))?;
    let shells = a.shell_faceuses.len();
    p.uniform("shell:faceuseCount", Value::UIntArray(a.shell_faceuses))?;
    p.uniform("shell:wireEdgeCount", Value::UIntArray(vec![0; shells]))?;
    p.uniform("shell:pointType", toks(vec!["none".into(); shells]))?;
    p.uniform("faceuse:faceIndex", Value::UIntArray(a.faceuse_faces))?;
    p.uniform("faceuse:orientationType", toks(a.faceuse_types))?;
    p.uniform("face:loopCount", Value::UIntArray(a.face_loops))?;
    p.uniform("face:surfaceType", toks(a.face_surfaces))?;
    p.uniform("face:trimType", toks(a.face_trims))?;
    p.uniform("face:range", Value::Double2Array(a.face_ranges))?;
    p.uniform("loop:edgeuseCount", Value::UIntArray(a.loop_edgeuses))?;
    p.uniform("loop:vertexIndex", Value::UIntArray(a.loop_vertices))?;
    p.uniform("edgeuse:edgeIndex", Value::UIntArray(a.edgeuse_edges))?;
    p.uniform("edgeuse:orientationType", toks(a.edgeuse_types))?;
    p.uniform(
        "edgeuse:nextRadialEUIndex",
        Value::UIntArray(a.edgeuse_next),
    )?;
    p.uniform("edgeuse:thisRadialEntryType", toks(a.edgeuse_entries))?;
    p.uniform("edge:curveType", toks(a.edge_types))?;
    p.uniform("edge:range", Value::DoubleArray(a.edge_ranges))?;
    p.uniform("edge:vertexIndices", Value::Int2Array(a.edge_vertices))?;
    p.uniform("wireEdge:curveType", toks(Vec::new()))?;
    p.uniform("wireEdge:range", Value::DoubleArray(Vec::new()))?;
    p.uniform("wireEdge:vertexIndices", Value::Int2Array(Vec::new()))?;
    let vertices = a.points.len();
    p.uniform(
        "vertex:pointType",
        toks(vec!["BrepPointAPI".into(); vertices]),
    )?;
    if vertices > 0 {
        p.uniform(
            "brep:vertexPoint:point:position",
            Value::Point3dArray(a.points),
        )?;
    }
    let n = &a.edge_nurbs;
    if !n.counts.is_empty() {
        let at = "brep:edge3dNurb:curve3d:nurb";
        p.uniform(
            &format!("{at}:controlVertices"),
            Value::Point3dArray(n.points.clone()),
        )?;
        p.uniform(
            &format!("{at}:vertexCount"),
            Value::UIntArray(n.counts.clone()),
        )?;
        p.uniform(&format!("{at}:order"), Value::UIntArray(n.orders.clone()))?;
        p.uniform(&format!("{at}:knots"), Value::DoubleArray(n.knots.clone()))?;
        p.uniform(
            &format!("{at}:weights"),
            Value::DoubleArray(n.weights.clone()),
        )?;
    }
    if !a.lines.origins.is_empty() {
        let at = "brep:edge3dLine:curve3d:line";
        p.uniform(
            &format!("{at}:origin"),
            Value::Point3dArray(a.lines.origins),
        )?;
        p.uniform(
            &format!("{at}:direction"),
            Value::Vector3dArray(a.lines.axes),
        )?;
    }
    for (inst, kind, x, radii) in [
        ("edge3dCircle", "circle", a.circles, ["radius", ""]),
        (
            "edge3dEllipse",
            "ellipse",
            a.ellipses,
            ["xRadius", "yRadius"],
        ),
    ] {
        if x.origins.is_empty() {
            continue;
        }
        let at = format!("brep:{inst}:curve3d:{kind}");
        p.uniform(&format!("{at}:center"), Value::Point3dArray(x.origins))?;
        p.uniform(&format!("{at}:axis"), Value::Vector3dArray(x.axes))?;
        p.uniform(&format!("{at}:refDirection"), Value::Vector3dArray(x.refs))?;
        p.uniform(&format!("{at}:{}", radii[0]), Value::DoubleArray(x.a))?;
        if !radii[1].is_empty() {
            p.uniform(&format!("{at}:{}", radii[1]), Value::DoubleArray(x.b))?;
        }
    }
    if !a.uv.counts.is_empty() {
        let at = "brep:curveUv:nurb";
        p.uniform(
            &format!("{at}:controlVertices"),
            Value::Double2Array(a.uv.points),
        )?;
        p.uniform(&format!("{at}:vertexCount"), Value::UIntArray(a.uv.counts))?;
        p.uniform(&format!("{at}:order"), Value::UIntArray(a.uv.orders))?;
        p.uniform(&format!("{at}:knots"), Value::DoubleArray(a.uv.knots))?;
        p.uniform(&format!("{at}:weights"), Value::DoubleArray(a.uv.weights))?;
    }
    let s = a.surface_nurbs;
    if !s.counts[0].is_empty() {
        let at = "brep:surface:nurb";
        let [cu, cv] = s.counts;
        let [ou, ov] = s.orders;
        let [ku, kv] = s.knots;
        p.uniform(
            &format!("{at}:controlVertices"),
            Value::Point3dArray(s.points),
        )?;
        p.uniform(&format!("{at}:uVertexCount"), Value::UIntArray(cu))?;
        p.uniform(&format!("{at}:vVertexCount"), Value::UIntArray(cv))?;
        p.uniform(&format!("{at}:uOrder"), Value::UIntArray(ou))?;
        p.uniform(&format!("{at}:vOrder"), Value::UIntArray(ov))?;
        p.uniform(&format!("{at}:uKnots"), Value::DoubleArray(ku))?;
        p.uniform(&format!("{at}:vKnots"), Value::DoubleArray(kv))?;
        p.uniform(&format!("{at}:weights"), Value::DoubleArray(s.weights))?;
    }
    if !a.planes.origins.is_empty() {
        let at = "brep:surface:plane";
        p.uniform(
            &format!("{at}:origin"),
            Value::Point3dArray(a.planes.origins),
        )?;
        p.uniform(&format!("{at}:axis"), Value::Vector3dArray(a.planes.axes))?;
        p.uniform(
            &format!("{at}:refDirection"),
            Value::Vector3dArray(a.planes.refs),
        )?;
    }
    let analytic =
        |p: &mut Prim, kind: &str, x: Analytic, names: [&str; 3]| -> Result<(), BadName> {
            if x.origins.is_empty() {
                return Ok(());
            }
            let at = format!("brep:surface:{kind}");
            p.uniform(
                &format!("{at}:{}", names[0]),
                Value::Point3dArray(x.origins),
            )?;
            p.uniform(&format!("{at}:axis"), Value::Vector3dArray(x.axes))?;
            p.uniform(&format!("{at}:refDirection"), Value::Vector3dArray(x.refs))?;
            p.uniform(&format!("{at}:{}", names[1]), Value::DoubleArray(x.a))?;
            if !names[2].is_empty() {
                p.uniform(&format!("{at}:{}", names[2]), Value::DoubleArray(x.b))?;
            }
            Ok(())
        };
    analytic(&mut p, "cylinder", a.cylinders, ["origin", "radius", ""])?;
    analytic(&mut p, "cone", a.cones, ["origin", "radius", "semiAngle"])?;
    analytic(&mut p, "sphere", a.spheres, ["center", "radius", ""])?;
    analytic(
        &mut p,
        "torus",
        a.tori,
        ["origin", "majorRadius", "minorRadius"],
    )?;
    // Names.
    let hex = |n: Option<Name>| n.map_or(String::new(), |n| n.to_string());
    let mut face_names = vec![String::new(); faces.len()];
    for (i, &f) in cx.origins.faces.iter().enumerate() {
        face_names[position[i]] = hex(Some(cx.body.face(f).expect("a live face").name));
    }
    let edge_names = cx
        .origins
        .edges
        .iter()
        .map(|e| hex(e.and_then(|e| cx.body.edge(e)).map(|x| x.name)))
        .collect();
    let vertex_names = cx
        .origins
        .vertices
        .iter()
        .map(|v| hex(v.and_then(|v| cx.body.vertex(v)).map(|x| x.name)))
        .collect();
    p.uniform("ryucad:faceName", Value::StringArray(face_names))?
        .custom();
    p.uniform("ryucad:edgeName", Value::StringArray(edge_names))?
        .custom();
    p.uniform("ryucad:vertexName", Value::StringArray(vertex_names))?
        .custom();
    Ok(p)
}

/// A surface's API token and packed values.
fn surface(a: &mut Arrays, g: &Geometry, k: f64) {
    let frame_of = |s: &Surface| *s.frame().expect("an analytic surface");
    let token = match g {
        Geometry::Nurbs(n) => {
            let [nu, nv] = n.counts();
            let [du, dv] = n.degrees();
            let [ku, kv] = n.knots();
            for i in 0..nu {
                for j in 0..nv {
                    a.surface_nurbs.points.push(v3(n.control_point(i, j), k));
                }
            }
            a.surface_nurbs.weights.extend_from_slice(n.weights());
            a.surface_nurbs.counts[0].push(nu as u32);
            a.surface_nurbs.counts[1].push(nv as u32);
            a.surface_nurbs.orders[0].push(du as u32 + 1);
            a.surface_nurbs.orders[1].push(dv as u32 + 1);
            a.surface_nurbs.knots[0].extend_from_slice(ku.flat());
            a.surface_nurbs.knots[1].extend_from_slice(kv.flat());
            "BrepSurfaceNurbAPI"
        }
        Geometry::Analytic(s) => {
            let f = frame_of(s);
            let (o, z, x) = (v3(f.origin(), k), d3(f.z().get()), d3(f.x().get()));
            match s {
                Surface::Plane(_) => {
                    push(&mut a.planes, o, z, x, 0.0, 0.0);
                    "BrepSurfacePlaneAPI"
                }
                Surface::Cylinder(c) => {
                    push(&mut a.cylinders, o, z, x, c.radius() * k, 0.0);
                    "BrepSurfaceCylinderAPI"
                }
                Surface::Cone(c) => {
                    push(&mut a.cones, o, z, x, c.radius() * k, c.half_angle());
                    "BrepSurfaceConeAPI"
                }
                Surface::Sphere(c) => {
                    push(&mut a.spheres, o, z, x, c.radius() * k, 0.0);
                    "BrepSurfaceSphereAPI"
                }
                Surface::Torus(t) => {
                    push(&mut a.tori, o, z, x, t.major() * k, t.minor() * k);
                    "BrepSurfaceTorusAPI"
                }
                _ => unreachable!("swept surfaces are NURBS by now"),
            }
        }
    };
    a.face_surfaces.push(token.into());
}

fn push(x: &mut Analytic, o: [f64; 3], z: [f64; 3], r: [f64; 3], a: f64, b: f64) {
    x.origins.push(o);
    x.axes.push(z);
    x.refs.push(r);
    x.a.push(a);
    x.b.push(b);
}

/// How far apart two UV curves of a loop may end and start, in the
/// stage's units: a tenth of the validator's (BA.763's 1e-6).
const UV_CLOSURE: f64 = 1e-7;

/// A clamped UV curve, packed.
fn nurbs2(a: &mut NurbsArrays2, n: &NurbsCurve2) {
    a.counts.push(n.control_points().len() as u32);
    a.orders.push(n.degree() as u32 + 1);
    a.knots.extend_from_slice(n.knots().flat());
    a.weights.extend_from_slice(n.weights());
    a.points.extend(n.control_points().map(|p| [p.x, p.y]));
}

/// `n` with its parameters scaled by `s`.
fn scaled(n: &NurbsCurve2, s: [f64; 2]) -> NurbsCurve2 {
    if s == [1.0, 1.0] {
        return n.clone();
    }
    let points = n
        .control_points()
        .map(|p| Point2::new(p.x * s[0], p.y * s[1]))
        .collect();
    NurbsCurve2::new(n.knots().clone(), points, Some(n.weights().to_vec()))
        .expect("scaling a spline keeps it valid")
}

fn start(n: &NurbsCurve2) -> Point2 {
    n.point(n.domain()[0])
}

fn end(n: &NurbsCurve2) -> Point2 {
    n.point(n.domain()[1])
}

/// An angle range moved by whole turns so it ends in `(0, 2π]`, as the
/// validator wants a circle's or an ellipse's (BA.630).
fn primary([t0, t1]: [f64; 2]) -> [f64; 2] {
    let turn = core::f64::consts::TAU;
    let k = (t1 / turn).ceil() - 1.0;
    if k == 0.0 {
        return [t0, t1];
    }
    [t0 - k * turn, t1 - k * turn]
}

/// An edge's curve, packed: its token and its range in the stage's unit.
fn curve(a: &mut Arrays, c: &Curve, [t0, t1]: [f64; 2], k: f64) -> [f64; 2] {
    let (token, range) = match c {
        Curve::Line(l) => {
            a.lines.origins.push(v3(l.origin(), k));
            a.lines.axes.push(d3(l.dir().get()));
            ("BrepCurve3dLineAPI", [t0 * k, t1 * k])
        }
        Curve::Circle(c) => {
            let f = c.frame();
            let (o, z, x) = (v3(f.origin(), k), d3(f.z().get()), d3(f.x().get()));
            push(&mut a.circles, o, z, x, c.radius() * k, 0.0);
            ("BrepCurve3dCircleAPI", primary([t0, t1]))
        }
        Curve::Ellipse(e) => {
            let f = e.frame();
            let (o, z, x) = (v3(f.origin(), k), d3(f.z().get()), d3(f.x().get()));
            push(&mut a.ellipses, o, z, x, e.major() * k, e.minor() * k);
            ("BrepCurve3dEllipseAPI", primary([t0, t1]))
        }
        Curve::Nurbs(n) => {
            // A periodic curve (or an edge past a clamped one's knots) is
            // written as the part the edge runs over.
            let n = if n.knots().is_periodic() {
                n.part([t0, t1]).unwrap_or_else(|_| n.to_clamped())
            } else {
                (**n).clone()
            };
            let range = if n.knots().is_periodic() {
                [t0, t1]
            } else {
                let [d0, d1] = n.domain();
                if t0 >= d0 && t1 <= d1 {
                    [t0, t1]
                } else {
                    [d0, d1]
                }
            };
            let m = &mut a.edge_nurbs;
            m.counts.push(n.control_points().len() as u32);
            m.orders.push(n.degree() as u32 + 1);
            m.knots.extend_from_slice(n.knots().flat());
            m.weights.extend_from_slice(n.weights());
            m.points.extend(n.control_points().map(|p| v3(p, k)));
            ("BrepCurve3dNurbAPI", range)
        }
        _ => unreachable!("every curve kind is written"),
    };
    a.edge_types.push(token.into());
    range
}
