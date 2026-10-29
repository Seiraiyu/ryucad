//! What each vertex becomes, by the classes of the edges meeting there.
//!
//! The corners round a vertex form a fan: each corner a face, between the
//! fin of its loop ending at the vertex and the next one leaving it, and
//! between each corner and the next an edge of both faces. By the edges'
//! classes (see [`super::classify::Class`]):
//!
//! - **Moved**: only kept and smooth edges. The vertex moves along the
//!   faces' common normal by their distance, and stays one vertex.
//! - **Opening**: opening and smooth edges. Each run of corners joined by
//!   smooth edges goes to one point, `V + d n` with that run's normal;
//!   each opening edge's fill ends at the arc of the sphere of radius
//!   `|d|` about the vertex between its two runs' points (its section in
//!   the plane square to the edge). With three runs or more those arcs
//!   bound a face on the sphere; with two, both fills end on the one arc
//!   (or, an edge closed at the vertex, its fill wraps round).
//! - **Closing**: closing, kept and smooth edges, of three faces (or two).
//!   Their curves meet at one point: one curve cut by the third face's
//!   offset (or, of two faces, the point of the curve nearest where the
//!   vertex goes). It stays one vertex.
//! - **Mixed**: three edges of three faces, opening and closing, none
//!   smooth. One opening edge's fill ends where it meets the third face's
//!   offset (an end curve on that face, between its rails' crossings with
//!   it); two opening edges' fills trim each other from their rails'
//!   common point on the face they share to where the closing edge's
//!   curve meets them.
//!
//! Anything else is refused, naming the vertex and its case.

use super::classify::{Class, Plan};
use super::edges::{EdgeGeo, NewCurve, moved_point, project_on, sphere};
use crate::blend::spine::solve3;
use crate::geom::{Box3, Circle, Curve, Curve2, Frame, Point3, Surface, Vector3};
use crate::intersect::{CurveHit, Failure, SurfaceHit, curve_surface, surface_surface};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{EdgeId, FaceId, FinId, Sense, VertexId};
use crate::{Error, OffsetCase, OffsetError};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// How far a point found on one new curve may lie from another that
/// should pass through it, over their tolerances: further is uncertain.
const MEET: f64 = 100.0 * LINEAR_RESOLUTION;

/// One of the result's faces, before it is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Part {
    /// A face's offset.
    Face(FaceId),
    /// The fill rolled about an opening edge.
    Fill(EdgeId),
    /// The sphere about a vertex.
    Sphere(VertexId),
}

/// An edge a vertex makes: a section, an end curve or a trim.
#[derive(Clone, Debug)]
pub(crate) struct VEdge {
    pub(crate) curve: Curve,
    /// Increasing, from point `from` to point `to`.
    pub(crate) range: [f64; 2],
    pub(crate) from: usize,
    pub(crate) to: usize,
    pub(crate) tolerance: f64,
    /// The two faces it lies between, with its pcurve on each where known.
    pub(crate) parts: [(Part, Option<(Curve2, [f64; 2])>); 2],
}

/// A use of a vertex's edge, leaving point `from`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VUse {
    pub(crate) edge: usize,
    pub(crate) from: usize,
}

/// How a fill's loop passes a vertex, from its first rail to its second.
#[derive(Clone, Debug)]
pub(crate) enum FillEnd {
    Edges(Vec<VUse>),
    /// It wraps round: an edge closed at the vertex, filled all round.
    Wraps,
}

/// What a vertex becomes.
#[derive(Clone, Debug, Default)]
pub(crate) struct VertexGeo {
    /// It stays one vertex (and keeps its name).
    pub(crate) kept: bool,
    pub(crate) points: Vec<Point3>,
    /// Each end of an edge that becomes one curve: the point and the
    /// curve's parameter there. Keyed by the edge and whether it is its
    /// end (not its start).
    pub(crate) one: BTreeMap<(EdgeId, bool), (usize, f64)>,
    /// Each end of an opening edge: each rail's point and parameter, in
    /// the order of the edge's fins.
    pub(crate) open: BTreeMap<(EdgeId, bool), [(usize, f64); 2]>,
    /// Edges a face's loop takes at the corner after the fin that ends
    /// here, from where that fin's curve ends.
    pub(crate) inserts: BTreeMap<FinId, Vec<VUse>>,
    /// How each fill ending here closes its loop.
    pub(crate) fill_ends: BTreeMap<(EdgeId, bool), FillEnd>,
    pub(crate) edges: Vec<VEdge>,
    /// The sphere about the vertex, its sense and its loop.
    pub(crate) sphere: Option<(Surface, Sense, Vec<VUse>)>,
}

/// A corner of the fan: a face between the fin ending at the vertex and
/// the next leaving it.
#[derive(Clone, Copy, Debug)]
struct Corner {
    face: FaceId,
    fin_in: FinId,
    fin_out: FinId,
}

/// A step of the fan, from a corner to the next, across an edge.
#[derive(Clone, Copy, Debug)]
struct Step {
    edge: EdgeId,
    /// The step is at the edge's end, not its start.
    at_end: bool,
    /// The side (in the order of the edge's fins) of the corner before.
    side: usize,
}

/// Every vertex's new geometry, in parallel.
pub(crate) fn vertices(
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
) -> Result<BTreeMap<VertexId, VertexGeo>, Error> {
    let mut vs: Vec<VertexId> = plan
        .body
        .vertices()
        .filter(|&v| plan.body.vertices[v].edge.is_some())
        .collect();
    vs.sort_by_key(|&v| plan.body.vertices[v].name);
    vs.par_iter()
        .map(|&v| vertex(plan, geo, v).map(|g| (v, g)))
        .collect()
}

/// The corners round `v` and the steps between them, step `k` from
/// corner `k` to corner `k + 1`.
fn fan(plan: &Plan, v: VertexId) -> (Vec<Corner>, Vec<Step>) {
    let body = plan.body;
    let e0 = body.vertices[v].edge.expect("a vertex with edges");
    let f0 = body.edges[e0].fin;
    let start = if body.end_of(f0) == v {
        f0
    } else {
        body.fins[f0].prev
    };
    let mut corners = Vec::new();
    let mut steps = Vec::new();
    let mut f = start;
    loop {
        let g = body.fins[f].next;
        let face = plan.face_of(f);
        corners.push(Corner {
            face,
            fin_in: f,
            fin_out: g,
        });
        let e = body.fins[g].edge;
        let fins = plan.fins(e);
        steps.push(Step {
            edge: e,
            at_end: body.fins[g].sense == Sense::Reversed,
            side: usize::from(fins[0] != g),
        });
        f = body.fins[g].around;
        if f == start || corners.len() > body.fins.slot_count() {
            break;
        }
    }
    (corners, steps)
}

/// A fin's face's outward normal where it leaves the vertex.
fn normal_out(plan: &Plan, fin: FinId) -> Vector3 {
    let e = &plan.body.edges[plan.body.fins[fin].edge];
    let t = if plan.body.fins[fin].sense == Sense::Forward {
        e.interval[0]
    } else {
        e.interval[1]
    };
    plan.normal(fin, t).normalize().map_or(Vector3::ZERO, |n| n.get())
}

/// The parameter of an edge's end.
fn end_param(plan: &Plan, e: EdgeId, at_end: bool) -> f64 {
    plan.body.edges[e].interval[usize::from(at_end)]
}

fn vertex(
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    v: VertexId,
) -> Result<VertexGeo, Error> {
    let at = plan.body.vertices[v].name;
    let (corners, steps) = fan(plan, v);
    let class = |s: &Step| plan.class[&s.edge].0;
    let has = |c: Class| steps.iter().any(|s| class(s) == c);
    if steps.iter().all(|s| matches!(class(s), Class::Kept | Class::Smooth)) {
        return Ok(moved(plan, v, &corners, &steps));
    }
    if !has(Class::Closing) && !has(Class::Kept) {
        return opening(plan, v, &corners, &steps);
    }
    if !has(Class::Opening) {
        return closing(plan, geo, v, &corners, &steps);
    }
    let three = corners.len() == 3 && {
        let mut f: Vec<FaceId> = corners.iter().map(|c| c.face).collect();
        f.sort();
        f.dedup();
        f.len() == 3
    };
    if !three || has(Class::Smooth) || has(Class::Kept) {
        return Err(OffsetError::Unsupported {
            at,
            case: OffsetCase::MixedVertex,
        }
        .into());
    }
    let openings: Vec<usize> = (0..3).filter(|&k| class(&steps[k]) == Class::Opening).collect();
    if openings.len() == 1 {
        mixed_end(plan, geo, v, &corners, &steps, openings[0])
    } else {
        let c = (0..3)
            .find(|&k| class(&steps[k]) == Class::Closing)
            .expect("one closing step");
        mixed_trim(plan, geo, v, &corners, &steps, c)
    }
}

/// Only kept and smooth edges: one point, the vertex moved along its
/// faces' common normal.
fn moved(plan: &Plan, v: VertexId, corners: &[Corner], steps: &[Step]) -> VertexGeo {
    let p = plan.body.vertices[v].point;
    let c = corners[0];
    let d = plan.d(c.face);
    let point = if d == 0.0 {
        p
    } else {
        p + normal_out(plan, c.fin_out) * d
    };
    let mut out = VertexGeo {
        kept: true,
        points: vec![point],
        ..VertexGeo::default()
    };
    for s in steps {
        out.one
            .insert((s.edge, s.at_end), (0, end_param(plan, s.edge, s.at_end)));
    }
    out
}

/// Opening and smooth edges: the runs' points, and the fills' ends on the
/// sphere about the vertex.
fn opening(
    plan: &Plan,
    v: VertexId,
    corners: &[Corner],
    steps: &[Step],
) -> Result<VertexGeo, Error> {
    let at = plan.body.vertices[v].name;
    let p = plan.body.vertices[v].point;
    let d = plan.d(corners[0].face);
    // Runs: corners joined by smooth steps, starting after an opening one.
    let n = corners.len();
    let first = (0..n)
        .find(|&k| plan.class[&steps[k].edge].0 == Class::Opening)
        .expect("an opening step");
    let mut run_of = vec![0usize; n];
    let mut points: Vec<Point3> = Vec::new();
    for i in 1..=n {
        let k = (first + i) % n;
        let prev = (k + n - 1) % n;
        if i == 1 || plan.class[&steps[prev].edge].0 == Class::Opening {
            points.push(p + normal_out(plan, corners[k].fin_out) * d);
        }
        run_of[k] = points.len() - 1;
    }
    let mut out = VertexGeo {
        points,
        ..VertexGeo::default()
    };
    let opening_steps: Vec<usize> = (0..n)
        .filter(|&k| plan.class[&steps[k].edge].0 == Class::Opening)
        .collect();
    for (k, s) in steps.iter().enumerate() {
        let next = (k + 1) % n;
        let t = end_param(plan, s.edge, s.at_end);
        match plan.class[&s.edge].0 {
            Class::Opening => {
                // Side `s.side` is the corner before, the other the one after.
                let mut ends = [(0, t); 2];
                ends[s.side] = (run_of[k], t);
                ends[1 - s.side] = (run_of[next], t);
                out.open.insert((s.edge, s.at_end), ends);
            }
            _ => {
                out.one.insert((s.edge, s.at_end), (run_of[k], t));
            }
        }
    }
    if opening_steps.len() == 2 {
        let (a, b) = (steps[opening_steps[0]], steps[opening_steps[1]]);
        if a.edge == b.edge {
            // An edge closed at the vertex: its fill wraps round.
            out.fill_ends.insert((a.edge, a.at_end), FillEnd::Wraps);
            out.fill_ends.insert((b.edge, b.at_end), FillEnd::Wraps);
            return Ok(out);
        }
        // Both fills end on one arc.
        let ends = out.open[&(a.edge, a.at_end)];
        let arc = arc(
            plan,
            a,
            ends,
            (&out.points, p, d),
            [Part::Fill(a.edge), Part::Fill(b.edge)],
        )
        .ok_or_else(|| super::uncertain(at, "vertices.rs#7"))?;
        out.edges.push(arc);
        for s in [a, b] {
            let ends = out.open[&(s.edge, s.at_end)];
            out.fill_ends.insert(
                (s.edge, s.at_end),
                FillEnd::Edges(vec![VUse {
                    edge: 0,
                    from: ends[0].0,
                }]),
            );
        }
        return Ok(out);
    }
    // Three runs or more: a face on the sphere, bounded by the arcs.
    let mut seen: Vec<(usize, usize)> = Vec::new();
    for &k in &opening_steps {
        let s = steps[k];
        let ends = out.open[&(s.edge, s.at_end)];
        let pair = (ends[0].0.min(ends[1].0), ends[0].0.max(ends[1].0));
        if ends[0].0 == ends[1].0 || seen.contains(&pair) {
            return Err(OffsetError::Unsupported {
                at,
                case: OffsetCase::SharedSection,
            }
            .into());
        }
        seen.push(pair);
        let arc = arc(
            plan,
            s,
            ends,
            (&out.points, p, d),
            [Part::Fill(s.edge), Part::Sphere(v)],
        )
        .ok_or_else(|| super::uncertain(at, "vertices.rs#8"))?;
        let e = out.edges.len();
        out.edges.push(arc);
        out.fill_ends.insert(
            (s.edge, s.at_end),
            FillEnd::Edges(vec![VUse {
                edge: e,
                from: ends[0].0,
            }]),
        );
    }
    let surface = sphere(p, d.abs(), &out.points).ok_or_else(|| super::uncertain(at, "vertices.rs#9"))?;
    // The sphere runs each arc the other way from its fill: a fill runs
    // from its first side to its second at its edge's start, and back at
    // its end, so the sphere from the second to the first at a start.
    let mut lp = Vec::new();
    for (i, &k) in opening_steps.iter().enumerate() {
        let s = steps[k];
        let ends = out.open[&(s.edge, s.at_end)];
        lp.push(VUse {
            edge: i,
            from: ends[usize::from(!s.at_end)].0,
        });
    }
    // Chain them head to tail.
    let mut chained = vec![lp[0]];
    let mut rest: Vec<VUse> = lp[1..].to_vec();
    while !rest.is_empty() {
        let last = chained[chained.len() - 1];
        let e = &out.edges[last.edge];
        let to = if last.from == e.from { e.to } else { e.from };
        let Some(k) = rest.iter().position(|u| u.from == to) else {
            return Err(super::uncertain(at, "vertices.rs#10").into());
        };
        chained.push(rest.remove(k));
    }
    let sense = if d > 0.0 {
        Sense::Forward
    } else {
        Sense::Reversed
    };
    out.sphere = Some((surface, sense, chained));
    Ok(out)
}

/// The arc of an opening edge's section at the vertex, from its first
/// side's point to its second's: on the circle of radius `|d|` about the
/// vertex in the plane square to the edge there, the shorter way.
fn arc(
    plan: &Plan,
    s: Step,
    ends: [(usize, f64); 2],
    (points, p, d): (&[Point3], Point3, f64),
    parts: [Part; 2],
) -> Option<VEdge> {
    let e = &plan.body.edges[s.edge];
    let t = plan.body.curves[e.curve].eval(e.interval[usize::from(s.at_end)]).d1;
    let (a, b) = (points[ends[0].0], points[ends[1].0]);
    let (x0, y0) = (a - p, b - p);
    let mut frame = Frame::new(p, t, x0).ok()?;
    let mut angle = crate::math::atan2(y0.dot(frame.y().get()), y0.dot(frame.x().get()));
    if angle < 0.0 {
        frame = Frame::new(p, -t, x0).ok()?;
        angle = -angle;
    }
    if !(angle > 0.0) {
        return None;
    }
    let circle = Circle::new(frame, d.abs()).ok()?;
    let curve = Curve::Circle(circle);
    let gap = curve.point(0.0).distance(a).max(curve.point(angle).distance(b));
    Some(VEdge {
        curve,
        range: [0.0, angle],
        from: ends[0].0,
        to: ends[1].0,
        tolerance: LINEAR_RESOLUTION.max(2.0 * gap),
        parts: parts.map(|x| (x, None)),
    })
}

/// The new curve an edge becomes, unless it opens.
fn one(geo: &BTreeMap<EdgeId, EdgeGeo>, e: EdgeId) -> &NewCurve {
    match &geo[&e] {
        EdgeGeo::One(c) => c,
        EdgeGeo::Open { .. } => unreachable!("an edge that becomes one curve"),
    }
}

/// The parameter of the point of `c` nearest `x`, if it is within `MEET`
/// and their tolerances.
fn on(c: &NewCurve, x: Point3, slack: f64) -> Option<f64> {
    let (t, q) = project_on(&c.curve, c.range, x);
    (q.distance(x) <= MEET + c.tolerance + slack).then_some(t)
}

/// The hit of `c` with `surface` nearest `estimate`.
fn hit(c: &NewCurve, surface: &Surface, estimate: Point3, at: crate::name::Name) -> Result<Option<(f64, Point3)>, Error> {
    let hits = curve_surface(&c.curve, c.range, surface, c.tolerance.max(LINEAR_RESOLUTION))
        .map_err(|_| super::uncertain(at, "vertices.rs#11"))?;
    Ok(hits
        .into_iter()
        .filter_map(|h| match h {
            CurveHit::Point { t, point, .. } => Some((t, point)),
            CurveHit::On { .. } => None,
        })
        .min_by(|a, b| a.1.distance(estimate).total_cmp(&b.1.distance(estimate))))
}

/// The point the three corners' faces' tangent planes at the vertex meet
/// at once each is moved by its distance.
fn moved3(plan: &Plan, p: Point3, corners: &[Corner]) -> Option<Point3> {
    let mut rows = [Vector3::ZERO; 3];
    let mut rhs = [0.0; 3];
    for (k, c) in corners.iter().take(3).enumerate() {
        let n = normal_out(plan, c.fin_out);
        rows[k] = n;
        rhs[k] = n.dot(p.to_vector()) + plan.d(c.face);
    }
    solve3(rows, rhs)
}

/// Closing, kept and smooth edges: one point, where their curves meet.
fn closing(
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    v: VertexId,
    corners: &[Corner],
    steps: &[Step],
) -> Result<VertexGeo, Error> {
    let at = plan.body.vertices[v].name;
    let p = plan.body.vertices[v].point;
    let mut faces: Vec<FaceId> = corners.iter().map(|c| c.face).collect();
    faces.sort();
    faces.dedup();
    let (x, t0, j) = match (corners.len(), faces.len()) {
        (3, 3) => {
            // A curve the offsets move with, if any, cut by the third face.
            let j = (0..3)
                .find(|&k| plan.class[&steps[k].edge].0 != Class::Closing)
                .unwrap_or(0);
            let third = corners[(j + 2) % 3].face;
            let estimate = moved3(plan, p, corners).unwrap_or(p);
            let c = one(geo, steps[j].edge);
            let (t, x) = hit(c, &plan.surface[&third], estimate, at)?.ok_or(
                OffsetError::EdgeVanishes {
                    edge: plan.body.edges[steps[j].edge].name,
                },
            )?;
            (x, t, j)
        }
        (2, 2) => {
            let ns = [normal_out(plan, corners[0].fin_out), normal_out(plan, corners[1].fin_out)];
            let ds = [plan.d(corners[0].face), plan.d(corners[1].face)];
            let estimate = moved_point(p, ns, ds);
            let c = one(geo, steps[0].edge);
            let (t, x) = project_on(&c.curve, c.range, estimate);
            (x, t, 0)
        }
        _ => {
            return Err(OffsetError::Unsupported {
                at,
                case: OffsetCase::ClosingVertex,
            }
            .into());
        }
    };
    let mut out = VertexGeo {
        kept: true,
        points: vec![x],
        ..VertexGeo::default()
    };
    for (k, s) in steps.iter().enumerate() {
        let t = if k == j {
            t0
        } else {
            let c = one(geo, s.edge);
            on(c, x, one(geo, steps[j].edge).tolerance).ok_or_else(|| super::uncertain(at, "vertices.rs#12"))?
        };
        out.one.insert((s.edge, s.at_end), (0, t));
    }
    Ok(out)
}

/// The rails and fill of an opening edge.
fn open(geo: &BTreeMap<EdgeId, EdgeGeo>, e: EdgeId) -> (&[NewCurve; 2], &Surface) {
    match &geo[&e] {
        EdgeGeo::Open { rails, fill, .. } => (rails, fill),
        EdgeGeo::One(_) => unreachable!("an opening edge"),
    }
}

/// The branch of where `a` and `b` meet near `p` passing within reach of
/// both points, cut between them: the stretch between them on an open
/// branch, the shorter arc on a closed one. The edge
/// runs from `xs[0]` (point `ids[0]`) to `xs[1]`.
fn between(
    (a, b): (&Surface, &Surface),
    p: Point3,
    reach: f64,
    xs: [Point3; 2],
    ids: [usize; 2],
    parts: [Part; 2],
    at: crate::name::Name,
) -> Result<VEdge, Error> {
    let region = Box3::from_points([p, xs[0], xs[1]])
        .expect("points")
        .grow(3.0 * reach);
    let hit = surface_surface(a, b, &region).map_err(|f| match f {
        Failure::Uncertain => Error::from(super::uncertain(at, "vertices.rs#13")),
        Failure::Unsupported => OffsetError::Unsupported {
            at,
            case: OffsetCase::MixedVertex,
        }
        .into(),
    })?;
    let SurfaceHit::Meet { branches, .. } = hit else {
        return Err(super::uncertain(at, "vertices.rs#14").into());
    };
    let mut best: Option<(f64, f64, f64, crate::intersect::Branch)> = None;
    for br in branches {
        let (ta, qa) = project_on(&br.curve, br.range, xs[0]);
        let (tb, qb) = project_on(&br.curve, br.range, xs[1]);
        let worst = qa.distance(xs[0]).max(qb.distance(xs[1]));
        if worst <= MEET + br.tolerance && best.as_ref().is_none_or(|b| worst < b.0) {
            best = Some((worst, ta, tb, br));
        }
    }
    let (gap, ta, tb, br) = best.ok_or_else(|| super::uncertain(at, "vertices.rs#15"))?;
    let period = (br.curve.period() == Some(br.range[1] - br.range[0])).then(|| br.range[1] - br.range[0]);
    // The stretch from `ta` to `tb`, as an increasing range and whether it
    // runs backwards.
    let (range, backwards) = match period {
        None => {
            if ta < tb {
                ([ta, tb], false)
            } else {
                ([tb, ta], true)
            }
        }
        Some(per) => {
            // The shorter arc: an end or a trim at one vertex never takes
            // more than half its curve.
            let fwd = if tb > ta { tb } else { tb + per };
            let back = if ta > tb { ta } else { ta + per };
            if length(&br.curve, [ta, fwd]) <= length(&br.curve, [tb, back]) {
                ([ta, fwd], false)
            } else {
                ([tb, back], true)
            }
        }
    };
    if !(range[0] < range[1]) {
        return Err(super::uncertain(at, "vertices.rs#16").into());
    }
    let [pa, pb] = br.pcurves;
    let (from, to) = if backwards {
        (ids[1], ids[0])
    } else {
        (ids[0], ids[1])
    };
    Ok(VEdge {
        curve: br.curve,
        range,
        from,
        to,
        tolerance: br.tolerance.max(gap),
        parts: [(parts[0], Some(pa)), (parts[1], Some(pb))],
    })
}

/// One opening edge (step `k`) and two closing: the fill ends on the
/// third face's offset.
fn mixed_end(
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    v: VertexId,
    corners: &[Corner],
    steps: &[Step],
    k: usize,
) -> Result<VertexGeo, Error> {
    let at = plan.body.vertices[v].name;
    let p = plan.body.vertices[v].point;
    let s = steps[k];
    let third = corners[(k + 2) % 3];
    let o3 = &plan.surface[&third.face];
    let (rails, fill) = open(geo, s.edge);
    let estimate = moved3(plan, p, corners).unwrap_or(p);
    // Where each rail crosses the third face's offset.
    let mut xs = [Point3::ORIGIN; 2];
    let mut ts = [0.0; 2];
    for side in 0..2 {
        let (t, x) = hit(&rails[side], o3, estimate, at)?.ok_or(OffsetError::EdgeVanishes {
            edge: plan.body.edges[s.edge].name,
        })?;
        xs[side] = x;
        ts[side] = t;
    }
    let mut out = VertexGeo {
        points: xs.to_vec(),
        ..VertexGeo::default()
    };
    out.open.insert((s.edge, s.at_end), [(0, ts[0]), (1, ts[1])]);
    // The closing edges: step k + 1 runs from the corner after the
    // opening edge to the third face, step k + 2 from the third face back.
    let before = 1 - s.side; // the side of corner k + 1
    for (step, side) in [((k + 1) % 3, before), ((k + 2) % 3, s.side)] {
        let st = steps[step];
        let c = one(geo, st.edge);
        let t = on(c, xs[side], rails[side].tolerance).ok_or_else(|| super::uncertain(at, "vertices.rs#17"))?;
        out.one.insert((st.edge, st.at_end), (side, t));
    }
    let reach = plan.d(corners[k].face).abs();
    let end = between(
        (fill, o3),
        p,
        reach,
        xs,
        [0, 1],
        [Part::Fill(s.edge), Part::Face(third.face)],
        at,
    )?;
    out.edges.push(end);
    out.fill_ends.insert(
        (s.edge, s.at_end),
        FillEnd::Edges(vec![VUse { edge: 0, from: 0 }]),
    );
    // On the third face, from the incoming closing edge's end (the side of
    // the corner after the opening edge) to the outgoing one's start.
    out.inserts
        .insert(third.fin_in, vec![VUse { edge: 0, from: before }]);
    Ok(out)
}

/// Two opening edges and one closing (step `c`): the fills trim each
/// other.
fn mixed_trim(
    plan: &Plan,
    geo: &BTreeMap<EdgeId, EdgeGeo>,
    v: VertexId,
    corners: &[Corner],
    steps: &[Step],
    c: usize,
) -> Result<VertexGeo, Error> {
    let at = plan.body.vertices[v].name;
    let p = plan.body.vertices[v].point;
    let (s1, s2) = (steps[(c + 1) % 3], steps[(c + 2) % 3]);
    // The face both fills roll on: after step s1, before step s2.
    let shared = corners[(c + 2) % 3];
    let d = plan.d(shared.face);
    let pb = p + normal_out(plan, shared.fin_out) * d;
    let (r1, f1) = open(geo, s1.edge);
    let (r2, f2) = open(geo, s2.edge);
    let closing = one(geo, steps[c].edge);
    let estimate = moved3(plan, p, corners).unwrap_or(p);
    let (tq, q) = hit(closing, f1, estimate, at)?.ok_or(OffsetError::EdgeVanishes {
        edge: plan.body.edges[steps[c].edge].name,
    })?;
    let mut out = VertexGeo {
        points: vec![pb, q],
        ..VertexGeo::default()
    };
    out.one.insert((steps[c].edge, steps[c].at_end), (1, tq));
    // Step s1 runs from the closing step's far face to the shared face:
    // its side before is away from the shared face; s2's after is.
    for (s, rails, shared_side) in [(s1, r1, 1 - s1.side), (s2, r2, s2.side)] {
        let mut ends = [(0, 0.0); 2];
        for side in 0..2 {
            let (idx, x) = if side == shared_side { (0, pb) } else { (1, q) };
            let t = on(&rails[side], x, closing.tolerance).ok_or_else(|| super::uncertain(at, "vertices.rs#18"))?;
            ends[side] = (idx, t);
        }
        out.open.insert((s.edge, s.at_end), ends);
    }
    let trim = between(
        (f1, f2),
        p,
        d.abs(),
        [pb, q],
        [0, 1],
        [Part::Fill(s1.edge), Part::Fill(s2.edge)],
        at,
    )?;
    out.edges.push(trim);
    for s in [s1, s2] {
        let ends = out.open[&(s.edge, s.at_end)];
        out.fill_ends.insert(
            (s.edge, s.at_end),
            FillEnd::Edges(vec![VUse {
                edge: 0,
                from: ends[0].0,
            }]),
        );
    }
    Ok(out)
}

/// A curve's length over `range`, by 64 chords.
fn length(c: &Curve, [a, b]: [f64; 2]) -> f64 {
    (0..64)
        .map(|i| {
            let t = |k: usize| a + (b - a) * k as f64 / 64.0;
            c.point(t(i)).distance(c.point(t(i + 1)))
        })
        .sum()
}
