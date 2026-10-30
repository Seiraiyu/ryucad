//! Face–face intersection: where two faces meet, trimmed to both.
//!
//! The faces' surfaces are intersected inside the overlap of the faces'
//! boxes ([`surface_surface`]). Each branch is cut wherever an edge of
//! either face meets the other face's surface ([`curve_surface`], each
//! edge once per pair), since only there can it cross a face's boundary;
//! each stretch between cuts is then inside, on the boundary of, or
//! outside each face, as its middle is ([`point_in_face`]). Stretches
//! outside either face are dropped, as are touching points outside one.
//!
//! Faces on one surface (by the coincidence rule) overlap where their
//! regions do: each face's boundary is cut where it crosses the other's,
//! in the first face's parameter plane ([`crossings`]), and the pieces
//! inside or along the other face are the rims of the overlap.
//!
//! Faces on any surfaces meet the same way ([`surface_surface`] and
//! [`curve_surface`] take every surface). A NURBS or swept face and an
//! analytic one whose implicit form has one certified sign on every
//! Bézier patch of the first's exact NURBS form inside the region are
//! apart without a trace.

use super::Failure;
use super::cs::{CurveHit, curve_surface};
use super::fit::fit_pcurve;
use super::implicit::Implicit;
use super::inface::{Place, point_in_face, point_in_face_near};
use super::ss::SurfaceHit;
use crate::geom::intersect2::{Meet2, crossings};
use crate::geom::{
    Box3, Circle, Circle2, Curve, Curve2, Ellipse2, Frame, Frame2, Line2, NurbsCurve2, Point2,
    Point3, Surface, Vector2,
};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId};
use std::sync::Arc;

/// A stretch of the curve where two faces meet.
#[derive(Clone, Debug, PartialEq)]
pub struct Stretch {
    pub curve: Curve,
    pub range: [f64; 2],
    /// The surfaces touch along it without crossing.
    pub tangent: bool,
    pub tolerance: f64,
    /// Its pcurves on the two faces' surfaces, as in
    /// [`super::Branch::pcurves`].
    pub pcurves: [(Curve2, [f64; 2]); 2],
    /// Where its middle lies against each face: inside, or on an edge or
    /// a vertex.
    pub places: [Place; 2],
}

/// A point where the faces touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub point: Point3,
    pub places: [Place; 2],
    /// How far from `point` it stands for where the faces meet: 0 for a
    /// touch, half a sliver stretch's length and its tolerance for a
    /// sliver taken as a point.
    pub reach: f64,
}

/// A piece of one face's boundary inside or along the other face, where
/// the faces lie on one surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rim {
    /// Whose boundary: 0 for the first face, 1 for the second.
    pub of: usize,
    pub edge: EdgeId,
    /// The stretch of the edge's parameter.
    pub range: [f64; 2],
    /// Where its middle lies against the other face.
    pub place: Place,
}

/// How two faces meet.
#[derive(Clone, Debug, PartialEq)]
pub enum FaceMeet {
    /// On one surface, `same` when the surfaces' natural normals agree.
    /// `whole` when neither face has a boundary, so they cover each other.
    Coincident {
        same: bool,
        rims: Vec<Rim>,
        whole: bool,
    },
    /// Elsewhere: the stretches and points they share.
    Meet {
        stretches: Vec<Stretch>,
        contacts: Vec<Contact>,
    },
}

impl FaceMeet {
    fn apart() -> Self {
        Self::Meet {
            stretches: Vec::new(),
            contacts: Vec::new(),
        }
    }
}

/// How face `a.1` of body `a.0` and face `b.1` of body `b.0` meet.
pub fn face_face(a: (&Body, FaceId), b: (&Body, FaceId)) -> Result<FaceMeet, Failure> {
    face_face_within(a, b, 0.0)
}

/// [`face_face`] with both faces' boxes grown by `margin` more before
/// they are overlapped into the region the surfaces are intersected in.
/// Faces touching at a vertex they share touch at a corner of both boxes,
/// so the region's sides (and the patches cut to them) pass through the
/// touch, where no crossing can be certified; a region reaching well
/// past it leaves the touch inside a cell, where it is settled as a
/// point. The stretches are the same: each is placed in both faces.
pub(crate) fn face_face_within(
    a: (&Body, FaceId),
    b: (&Body, FaceId),
    margin: f64,
) -> Result<FaceMeet, Failure> {
    let faces = [a, b];
    let sa = surface_of(a)?;
    let sb = surface_of(b)?;
    let slack = max_tolerance(a).max(max_tolerance(b));
    let (ba, bb) = (face_box(a.0, a.1), face_box(b.0, b.1));
    let Some(region) = overlap(&ba, &bb, slack + margin) else {
        return Ok(FaceMeet::apart());
    };
    // A NURBS or swept face certified apart from an analytic surface by
    // the signs of its patches is apart at once, without a trace.
    let pipe = |s: &Surface| matches!(s, Surface::Pipe(_));
    match (Implicit::of(sa), Implicit::of(sb)) {
        (Some(i), None) if !pipe(sb) => {
            if let Ok(apart) = certified_apart(&i, b, &region) {
                return Ok(apart);
            }
        }
        (None, Some(i)) if !pipe(sa) => {
            if let Ok(apart) = certified_apart(&i, a, &region) {
                return Ok(apart);
            }
        }
        _ => {}
    }
    let hit = match super::ss::meet(sa, sb, &region, false) {
        // A trace can fail where the surfaces meet only outside the faces
        // (sides of one loft or sweep along their shared edge, which these
        // pieces of them no longer reach): faces certified apart over
        // their own parameter boxes are apart. Else, unless they meet along
        // an edge or at a vertex they share (which the self-intersection
        // check settles by `beside`), the surfaces carried past their
        // domain edges are traced: a curve along one lies inside then.
        Err(Failure::Uncertain) => {
            if super::beside::faces_apart(a, b, FACES_APART) {
                return Ok(FaceMeet::apart());
            }
            if sharing(a, b) {
                return Err(Failure::Uncertain);
            }
            super::ss::meet(sa, sb, &region, true)?
        }
        other => other?,
    };
    let (branches, points) = match hit {
        SurfaceHit::Coincident { same } => return coincident(faces, [sa, sb], same),
        SurfaceHit::Meet { branches, points } => (branches, points),
    };
    // Surfaces that don't meet in the region: nothing for edges to cut.
    if branches.is_empty() && points.is_empty() {
        return Ok(FaceMeet::apart());
    }
    // Where each face's edges meet the other's surface. An edge both faces
    // use lies on both surfaces (the body is sound): only its ends cut.
    let mut cuts: Vec<Point3> = Vec::new();
    let shared: Vec<EdgeId> = if core::ptr::eq(a.0, b.0) {
        let theirs = edges(b);
        edges(a)
            .into_iter()
            .filter(|e| theirs.contains(e))
            .collect()
    } else {
        Vec::new()
    };
    for (face, other) in [(a, sb), (b, sa)] {
        for e in edges(face) {
            let edge = &face.0.edges[e];
            let curve = &face.0.curves[edge.curve];
            if shared.contains(&e) {
                cuts.extend([edge.start, edge.end].map(|v| face.0.vertices[v].point));
                continue;
            }
            for hit in curve_surface(curve, edge.interval, other, edge.tolerance.get())? {
                match hit {
                    CurveHit::Point { point, .. } => cuts.push(point),
                    CurveHit::On { t } => cuts.extend(t.map(|t| curve.point(t))),
                }
            }
        }
    }
    let mut stretches = Vec::new();
    let mut contacts = Vec::new();
    for br in branches {
        let reach = br.tolerance + slack;
        let [r0, r1] = br.range;
        let period = br
            .curve
            .period()
            .filter(|&p| (r1 - r0 - p).abs() <= 1e-12 * p);
        let mut at: Vec<f64> = cuts
            .iter()
            .filter(|&&c| br.curve.distance(c) <= reach)
            .map(|&c| {
                let t = br.curve.project(c).param;
                // A branch of a periodic curve may run past the period's
                // end (one cut at a sphere's poles): its cuts there project
                // a period back.
                match br.curve.period() {
                    Some(p) => r0 + (t - r0).rem_euclid(p),
                    None => t,
                }
            })
            .filter(|&t| r0 <= t && t <= r1)
            .collect();
        at.sort_by(f64::total_cmp);
        at.dedup_by(|x, y| br.curve.point(*x).distance(br.curve.point(*y)) <= reach);
        let pieces: Vec<[f64; 2]> = match period {
            Some(p) if !at.is_empty() => (0..at.len())
                .map(|i| {
                    [
                        at[i],
                        if i + 1 < at.len() {
                            at[i + 1]
                        } else {
                            at[0] + p
                        },
                    ]
                })
                .collect(),
            _ => {
                let mut ends = vec![r0];
                ends.extend(at.iter().copied().filter(|&t| t > r0 && t < r1));
                ends.push(r1);
                ends.windows(2).map(|w| [w[0], w[1]]).collect()
            }
        };
        for [s0, s1] in pieces {
            let mid = (s0 + s1) / 2.0;
            // A piece a few tolerances long, at a cut or where the region
            // clips the branch, is no stretch.
            let short = 4.0 * reach;
            if br.curve.point(s0).distance(br.curve.point(s1)) <= short
                && br.curve.point(mid).distance(br.curve.point(s0)) <= short
            {
                continue;
            }
            // A stretch that wraps past the end of a closed branch's period
            // leaves a fitted pcurve's domain: fit it again over the stretch,
            // before its middle is placed through it.
            let mut pcurves = br.pcurves.clone();
            let mut tolerance = br.tolerance;
            for (k, s) in [sa, sb].into_iter().enumerate() {
                let (c2, map) = &pcurves[k];
                let [d0, d1] = c2.domain();
                let [p0, p1] = [s0, s1].map(|t| map[0] + map[1] * t);
                let slack = 1e-12 * (d1 - d0).abs().max(1.0);
                if p0.min(p1) < d0 - slack || p0.max(p1) > d1 + slack {
                    let (n, bound) = fit_pcurve(s, &br.curve, [s0, s1])?;
                    pcurves[k] = (Curve2::Nurbs2(Arc::new(n)), [0.0, 1.0]);
                    tolerance = tolerance.max(bound);
                }
            }
            // The stretch is within its tolerance of the true curve, as is
            // any edge fitted to it: such an edge is within both of it.
            let mut places = [Place::Outside; 2];
            for k in 0..2 {
                let (c2, map) = &pcurves[k];
                let uv = c2.point(map[0] + map[1] * mid);
                places[k] =
                    point_in_face_near(faces[k].0, faces[k].1, uv, tolerance + super::SLACK)?;
            }
            if places.contains(&Place::Outside) {
                continue;
            }
            // A stretch a few tolerances long at a corner of both faces
            // (its middle on an edge of each) is a sliver: within the
            // tolerances it lies along both faces' edges at once, so it
            // can't say which carriers run together. It is a point, as a
            // traced sliver under 10 tolerances is, reaching both its ends
            // so the corner is one vertex (#276).
            let length = br.curve.point(s0).distance(br.curve.point(s1));
            if places.iter().all(|p| matches!(p, Place::OnEdge(_)))
                && length <= 10.0 * (tolerance + super::SLACK)
            {
                contacts.push(Contact {
                    point: br.curve.point(mid),
                    places,
                    reach: length / 2.0 + tolerance,
                });
                continue;
            }
            stretches.push(Stretch {
                curve: br.curve.clone(),
                range: [s0, s1],
                tangent: br.tangent,
                tolerance,
                pcurves,
                places,
            });
        }
    }
    for p in points {
        let mut places = [Place::Outside; 2];
        for (k, s) in [sa, sb].into_iter().enumerate() {
            let (u, v) = s.project(p).param;
            places[k] = point_in_face(faces[k].0, faces[k].1, Point2::new(u, v))?;
        }
        if !places.contains(&Place::Outside) {
            contacts.push(Contact {
                point: p,
                places,
                reach: 0.0,
            });
        }
    }
    Ok(FaceMeet::Meet {
        stretches,
        contacts,
    })
}

fn surface_of((body, face): (&Body, FaceId)) -> Result<&Surface, Failure> {
    let f = body.face(face).ok_or(Failure::Uncertain)?;
    body.surface(f.surface).ok_or(Failure::Uncertain)
}

/// The largest tolerance of a face's edges and vertices.
fn max_tolerance((body, face): (&Body, FaceId)) -> f64 {
    let mut t = LINEAR_RESOLUTION;
    for e in edges((body, face)) {
        let edge = &body.edges[e];
        t = t
            .max(edge.tolerance.get())
            .max(body.vertices[edge.start].tolerance.get())
            .max(body.vertices[edge.end].tolerance.get());
    }
    t
}

/// Whether two faces of one body share an edge or a vertex.
fn sharing(a: (&Body, FaceId), b: (&Body, FaceId)) -> bool {
    if !core::ptr::eq(a.0, b.0) {
        return false;
    }
    let (ea, eb) = (edges(a), edges(b));
    let ends = |es: &[EdgeId]| -> Vec<crate::topo::VertexId> {
        es.iter()
            .flat_map(|&e| [a.0.edges[e].start, a.0.edges[e].end])
            .collect()
    };
    let va = ends(&ea);
    ea.iter().any(|e| eb.contains(e)) || ends(&eb).iter().any(|v| va.contains(v))
}

/// A face's edges, each once, in loop order.
fn edges((body, face): (&Body, FaceId)) -> Vec<EdgeId> {
    let mut out: Vec<EdgeId> = Vec::new();
    for l in body.face_loops(face) {
        for fin in body.loop_fins(l) {
            let e = body.fins[fin].edge;
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

/// The overlap of two boxes grown by `slack`, if they meet.
fn overlap(a: &Box3, b: &Box3, slack: f64) -> Option<Box3> {
    let (a, b) = (a.grow(slack), b.grow(slack));
    a.intersects(&b)
        .then(|| Box3::new(a.min().max(b.min()), a.max().min(b.max())))
}

/// A box holding a face: its edges' boxes, and what its surface bulges
/// beyond them. A plane's face lies in its boundary's hull; a cylinder's
/// or a cone's between the circles at the extremes of its loops' axial
/// parameter; an extrusion's between its curve moved to those extremes; a
/// pipe's within its radius of its spine over the stretch its loops span
/// (on a clamped spine); a NURBS's, revolution's, sweep's or offset's inside its
/// surface's enclosure over the face's parameter box ([`face_rect`]),
/// grown by its edges' tolerances; any other surface's face inside the
/// surface's own box.
pub fn face_box(body: &Body, face: FaceId) -> Box3 {
    let surface = &body.surfaces[body.faces[face].surface];
    if matches!(
        surface,
        Surface::Nurbs(_) | Surface::Revolution(_) | Surface::Sweep(_) | Surface::Offset(_)
    ) && let Some(b) = spline_box(body, face)
    {
        return b;
    }
    coarse_box(body, face)
}

/// [`face_box`] as it was before spline and swept faces had boxes of
/// their own: their surfaces' boxes. Healing sizes the meshes it judges
/// shells by from it, so what it makes doesn't change with the boxes.
pub(crate) fn coarse_box(body: &Body, face: FaceId) -> Box3 {
    let surface = &body.surfaces[body.faces[face].surface];
    let mut out: Option<Box3> = None;
    let add = |b: Box3, out: &mut Option<Box3>| {
        *out = Some(match out {
            Some(o) => o.union(&b),
            None => b,
        });
    };
    let mut v_range = [f64::INFINITY, f64::NEG_INFINITY];
    let mut u_range = [f64::INFINITY, f64::NEG_INFINITY];
    for l in body.face_loops(face) {
        if let Some(v) = body.loops[l].vertex {
            let p = body.vertices[v].point;
            add(Box3::new(p, p), &mut out);
            let (pu, pv) = surface.project(p).param;
            v_range = [v_range[0].min(pv), v_range[1].max(pv)];
            u_range = [u_range[0].min(pu), u_range[1].max(pu)];
        }
        for fin in body.loop_fins(l) {
            let fi = &body.fins[fin];
            let e = &body.edges[fi.edge];
            add(body.curves[e.curve].bounds(e.interval), &mut out);
            if let Some(pc) = fi.pcurve {
                let [s0, s1] = e.interval.map(|t| pc.param(t));
                let b = body.curves2[pc.curve].bounds([s0.min(s1), s0.max(s1)]);
                v_range = [v_range[0].min(b.min().y), v_range[1].max(b.max().y)];
                u_range = [u_range[0].min(b.min().x), u_range[1].max(b.max().x)];
            }
        }
    }
    let swept = |out: &mut Option<Box3>| -> Option<()> {
        let [v0, v1] = v_range;
        if v0 > v1 || v0.is_nan() || v1.is_nan() {
            return None;
        }
        match surface {
            Surface::Plane(_) => {}
            Surface::Cylinder(_) | Surface::Cone(_) => {
                let f = surface.frame()?;
                for v in [v0, v1] {
                    let (q, r) = (surface.point(0.0, v), f.origin() + f.z().get() * v);
                    let radius = q.distance(r);
                    match Frame::new(r, f.z().get(), f.x().get()) {
                        Ok(frame) if radius >= LINEAR_RESOLUTION => {
                            let ring = Circle::new(frame, radius).ok()?;
                            let b = Curve::Circle(ring).bounds([0.0, 1.0]);
                            *out = Some(out.map_or(b, |o| o.union(&b)));
                        }
                        _ => {
                            *out = Some(out.map_or(Box3::new(r, r), |o| o.union(&Box3::new(r, r))))
                        }
                    }
                }
            }
            // About a closed spine a face between two loops may be the
            // stretch that wraps round, outside their range.
            Surface::Pipe(p) => {
                let [u0, u1] = u_range;
                if u0 > u1 || p.spine().period().is_some() {
                    return None;
                }
                // A pipe on a rail (a spline face's fillet) by its spine's
                // enclosure over just that stretch: its spine is carried
                // on past the face's ends, and its pieces' hulls reach the
                // whole way.
                let b = if p.rail().is_some() {
                    let e = p.spine().jet_box([u0, u1])[0];
                    Box3::new(
                        Point3::new(e[0].lo(), e[1].lo(), e[2].lo()),
                        Point3::new(e[0].hi(), e[1].hi(), e[2].hi()),
                    )
                } else {
                    p.spine().bounds([u0, u1])
                }
                .grow(p.radius() * (1.0 + 1e-12));
                *out = Some(out.map_or(b, |o| o.union(&b)));
            }
            Surface::Extrusion(e) => {
                let c = e.curve().bounds(e.curve().domain());
                let d = e.dir().get();
                for v in [v0, v1] {
                    let b = Box3::new(c.min() + d * v, c.max() + d * v);
                    *out = Some(out.map_or(b, |o| o.union(&b)));
                }
            }
            _ => {
                let b = surface.bounds();
                *out = Some(out.map_or(b, |o| o.union(&b)));
            }
        }
        Some(())
    };
    if swept(&mut out).is_none() || matches!(surface, Surface::Sphere(_) | Surface::Torus(_)) {
        let b = surface.bounds();
        out = Some(out.map_or(b, |o| o.union(&b)));
    }
    out.unwrap_or_else(|| surface.bounds())
}

/// The most pairs of parts [`super::beside::faces_apart`] tests when a
/// trace fails.
const FACES_APART: usize = 1 << 16;

/// [`face_box`] for a face on a NURBS, revolution or sweep: the surface's
/// enclosure over the face's parameter box, grown by the largest of its
/// edges' tolerances (its boundary may stray that far) and a hair of its
/// size (the pieces were cut in floating point). `None` where the face
/// has no parameter box or the surface no enclosure over it.
fn spline_box(body: &Body, face: FaceId) -> Option<Box3> {
    let surface = &body.surfaces[body.faces[face].surface];
    let rect = face_rect(body, face)?;
    let b = super::cells::enclosure(surface, rect)?;
    let reach = edges((body, face))
        .iter()
        .map(|&e| body.edges[e].tolerance.get())
        .fold(0.0, f64::max);
    let size = (b.max() - b.min()).length();
    Some(b.grow(reach + 1e-12 * size + 1e-15))
}

/// The box of parameters a face lies over: its pcurves' (each over its
/// range, its pieces cut to it), its lone vertices' feet, the whole of a
/// periodic direction, and on to a collapsed end of the other direction
/// where one is periodic (a face round a pole reaches it without an
/// edge), inside the domain. `None` for a fin with no pcurve, a face with
/// no loops, or a direction with no bound.
pub(crate) fn face_rect(body: &Body, face: FaceId) -> Option<[[f64; 2]; 2]> {
    let surface = &body.surfaces[body.faces[face].surface];
    let mut tight = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    let mut take = |x: [f64; 2], y: [f64; 2]| {
        tight[0] = [tight[0][0].min(x[0]), tight[0][1].max(x[1])];
        tight[1] = [tight[1][0].min(y[0]), tight[1][1].max(y[1])];
    };
    for l in body.face_loops(face) {
        if let Some(v) = body.loops[l].vertex {
            let (pu, pv) = surface.project(body.vertices[v].point).param;
            take([pu, pu], [pv, pv]);
        }
        for fin in body.loop_fins(l) {
            let fi = &body.fins[fin];
            let pc = fi.pcurve?;
            let [s0, s1] = body.edges[fi.edge].interval.map(|t| pc.param(t));
            let range = [s0.min(s1), s0.max(s1)];
            let c2 = &body.curves2[pc.curve];
            let b = match c2 {
                Curve2::Nurbs2(n) => n.clipped_bounds(range),
                _ => c2.bounds(range),
            };
            take([b.min().x, b.max().x], [b.min().y, b.max().y]);
        }
    }
    if tight[0][0] > tight[0][1] || tight[1][0] > tight[1][1] {
        return None;
    }
    let domain = surface.domain();
    let (pu, pv) = surface.periods();
    let periods = [pu, pv];
    let rows = crate::mesh::uv::collapsed_rows(surface);
    let mut rect = [[0.0; 2]; 2];
    for k in 0..2 {
        rect[k] = if periods[k].is_some() {
            domain[k]
        } else {
            let mut r = tight[k];
            if periods[1 - k].is_some() {
                if rows[k][0] {
                    r[0] = domain[k][0];
                }
                if rows[k][1] {
                    r[1] = domain[k][1];
                }
            }
            [r[0].max(domain[k][0]), r[1].min(domain[k][1])]
        };
        if !(rect[k][0].is_finite() && rect[k][1].is_finite()) || rect[k][0] > rect[k][1] {
            return None;
        }
    }
    Some(rect)
}

/// A face on a surface with no implicit form, certified apart from an
/// analytic surface: the implicit form has one certified sign on every
/// Bézier patch of the face's surface inside the region, halved a few
/// times.
fn certified_apart(
    analytic: &Implicit,
    (body, face): (&Body, FaceId),
    region: &Box3,
) -> Result<FaceMeet, Failure> {
    let surface = &body.surfaces[body.faces[face].surface];
    let fb = face_box(body, face);
    let mut v_range = [f64::INFINITY, f64::NEG_INFINITY];
    for l in body.face_loops(face) {
        for fin in body.loop_fins(l) {
            if let Some(pc) = body.fins[fin].pcurve {
                let e = &body.edges[body.fins[fin].edge];
                let [s0, s1] = e.interval.map(|t| pc.param(t));
                let b = body.curves2[pc.curve].bounds([s0.min(s1), s0.max(s1)]);
                v_range = [v_range[0].min(b.min().y), v_range[1].max(b.max().y)];
            }
        }
    }
    let [[u0, u1], _] = surface.domain();
    let ranges = [
        [u0.max(-1e6), u1.min(1e6)],
        if v_range[0] <= v_range[1] {
            v_range
        } else {
            surface.domain()[1]
        },
    ];
    let (nurbs, _) = surface.to_nurbs(ranges).map_err(|_| Failure::Unsupported)?;
    for patch in nurbs.patches() {
        let pb = patch.bounds();
        if !pb.intersects(region) || !pb.intersects(&fb) {
            continue;
        }
        let (f, _) = analytic.on_patch(patch);
        if !signed_after_halving(&f, 4) {
            return Err(Failure::Unsupported);
        }
    }
    Ok(FaceMeet::apart())
}

fn signed_after_halving(f: &crate::geom::bernstein2::Bernstein2, depth: u32) -> bool {
    let r = f.range();
    if r.lo() > 0.0 || r.hi() < 0.0 {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let (l, h) = f.split_s(0.5);
    [l, h].iter().all(|g| {
        let (x, y) = g.split_t(0.5);
        signed_after_halving(&x, depth - 1) && signed_after_halving(&y, depth - 1)
    })
}

/// An edge in a plane of parameters: its pcurve, the map from the edge's
/// parameter to the pcurve's, and the edge's interval.
type Edge2 = (EdgeId, Curve2, [f64; 2], [f64; 2]);

/// Faces on one surface: each face's boundary cut where it crosses the
/// other's, in the first face's parameter plane, and the pieces inside or
/// along the other face kept as rims.
fn coincident(
    faces: [(&Body, FaceId); 2],
    surfaces: [&Surface; 2],
    same: bool,
) -> Result<FaceMeet, Failure> {
    let loopless =
        |(b, f): (&Body, FaceId)| b.face_loops(f).all(|l| b.loops[l].first_fin.is_none());
    let whole = loopless(faces[0]) && loopless(faces[1]);
    // Every fin of both faces in the parameter plane of a surface holding
    // both (the first face's, unless the second's or a plane both lie on
    // reaches further).
    let common = super::coincide::common(surfaces[0], surfaces[1]);
    let plane = &common;
    let mut chains: [Vec<Edge2>; 2] = [Vec::new(), Vec::new()];
    for (k, &(body, face)) in faces.iter().enumerate() {
        for e in edges((body, face)) {
            let edge = &body.edges[e];
            let curve = &body.curves[edge.curve];
            let (c2, map) =
                match crate::pcurve::exact(plane, curve, edge.interval, edge.tolerance.get()) {
                    Some(p) => p,
                    None => match fit_pcurve(plane, curve, edge.interval) {
                        Ok((n, _)) => (Curve2::Nurbs2(Arc::new(n)), [0.0, 1.0]),
                        // An edge of a face on this very surface, too far
                        // off it to fit (a tolerant edge read from a file),
                        // has its own pcurve here.
                        Err(err) => own_pcurve(body, face, e, surfaces[k] == plane).ok_or(err)?,
                    },
                };
            chains[k].push((e, c2, map, edge.interval));
        }
    }
    let (pu, pv) = plane.periods();
    let mut rims = Vec::new();
    for k in 0..2 {
        let other = 1 - k;
        for (e, c2, map, interval) in &chains[k] {
            let mut cuts: Vec<f64> = Vec::new();
            for (_, d2, dmap, dint) in &chains[other] {
                let range_a = interval.map(|t| map[0] + map[1] * t);
                let range_b = dint.map(|t| dmap[0] + dmap[1] * t);
                let tol = LINEAR_RESOLUTION;
                for shift in shifts(c2, range_a, d2, range_b, [pu, pv]) {
                    let moved = shifted(d2, shift).ok_or(Failure::Uncertain)?;
                    let found = crossings((c2, range_a), (&moved, range_b), tol)
                        .ok_or(Failure::Uncertain)?;
                    for m in found {
                        let back = |s: f64| (s - map[0]) / map[1];
                        match m {
                            Meet2::Point { ta, .. } => cuts.push(back(ta)),
                            Meet2::Overlap { ta, .. } => cuts.extend(ta.map(back)),
                        }
                    }
                }
            }
            let [t0, t1] = *interval;
            let mut ends = vec![t0];
            let mut inner: Vec<f64> = cuts.into_iter().filter(|&t| t > t0 && t < t1).collect();
            inner.sort_by(f64::total_cmp);
            ends.extend(inner);
            ends.push(t1);
            let (body, _) = faces[k];
            let curve = &body.curves[body.edges[*e].curve];
            for w in ends.windows(2) {
                if curve.point(w[0]).distance(curve.point(w[1])) <= LINEAR_RESOLUTION {
                    continue;
                }
                let mid = curve.point((w[0] + w[1]) / 2.0);
                let (u, v) = surfaces[other].project(mid).param;
                let place = point_in_face(faces[other].0, faces[other].1, Point2::new(u, v))?;
                if place != Place::Outside {
                    rims.push(Rim {
                        of: k,
                        edge: *e,
                        range: [w[0], w[1]],
                        place,
                    });
                }
            }
        }
    }
    Ok(FaceMeet::Coincident { same, rims, whole })
}

/// The pcurve a fin of `edge` in `face` has, when the face's surface is
/// the one wanted (`same`).
fn own_pcurve(body: &Body, face: FaceId, edge: EdgeId, same: bool) -> Option<(Curve2, [f64; 2])> {
    if !same {
        return None;
    }
    body.edge_fins(edge).find_map(|fin| {
        let f = &body.fins[fin];
        let pc = f.pcurve?;
        (body.loops[f.loop_].face == face).then(|| (body.curves2[pc.curve].clone(), pc.map))
    })
}

/// The shifts by whole periods that bring curve `b` near curve `a` in the
/// parameter plane.
pub(crate) fn shifts(
    a: &Curve2,
    ra: [f64; 2],
    b: &Curve2,
    rb: [f64; 2],
    periods: [Option<f64>; 2],
) -> Vec<Vector2> {
    let ba = a.bounds([ra[0].min(ra[1]), ra[0].max(ra[1])]);
    let bb = b.bounds([rb[0].min(rb[1]), rb[0].max(rb[1])]);
    let along = |k: usize, lo_a: f64, hi_a: f64, lo_b: f64, hi_b: f64| -> Vec<f64> {
        match periods[k] {
            None => vec![0.0],
            Some(p) => {
                let first = ((lo_a - hi_b) / p).floor() as i64;
                let last = ((hi_a - lo_b) / p).ceil() as i64;
                (first..=last).map(|i| i as f64 * p).collect()
            }
        }
    };
    let us = along(0, ba.min().x, ba.max().x, bb.min().x, bb.max().x);
    let vs = along(1, ba.min().y, ba.max().y, bb.min().y, bb.max().y);
    us.iter()
        .flat_map(|&u| vs.iter().map(move |&v| Vector2::new(u, v)))
        .collect()
}

/// A plane curve moved by `d`.
pub(crate) fn shifted(c: &Curve2, d: Vector2) -> Option<Curve2> {
    if d == Vector2::ZERO {
        return Some(c.clone());
    }
    let frame = |f: &Frame2| Frame2::new(f.origin() + d, f.x(), f.is_ccw()).ok();
    Some(match c {
        Curve2::Line2(l) => Curve2::Line2(Line2::new(l.origin() + d, l.dir()).ok()?),
        Curve2::Circle2(x) => Curve2::Circle2(Circle2::new(frame(x.frame())?, x.radius()).ok()?),
        Curve2::Ellipse2(x) => {
            Curve2::Ellipse2(Ellipse2::new(frame(x.frame())?, x.major(), x.minor()).ok()?)
        }
        Curve2::Nurbs2(n) => {
            let pts = n.control_points().map(|p| p + d).collect();
            Curve2::Nurbs2(Arc::new(
                NurbsCurve2::new(n.knots().clone(), pts, Some(n.weights().to_vec())).ok()?,
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;

    fn faces_of(b: &Body) -> Vec<FaceId> {
        b.faces().collect()
    }

    #[test]
    fn a_blocks_adjacent_faces_meet_along_their_edge() {
        let (b, _) = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY).unwrap();
        let fs = faces_of(&b);
        for &f in &fs {
            for &g in &fs {
                if f >= g {
                    continue;
                }
                let shared: Vec<EdgeId> = edges((&b, f))
                    .into_iter()
                    .filter(|e| edges((&b, g)).contains(e))
                    .collect();
                match face_face((&b, f), (&b, g)).unwrap() {
                    FaceMeet::Meet {
                        stretches,
                        contacts,
                    } => {
                        assert!(contacts.is_empty());
                        if shared.is_empty() {
                            assert!(stretches.is_empty(), "{f:?} {g:?}: {stretches:?}");
                        } else {
                            assert_eq!(stretches.len(), 1);
                            assert_eq!(stretches[0].places, [Place::OnEdge(shared[0]); 2]);
                        }
                    }
                    other => panic!("{other:?}"),
                }
            }
        }
    }

    #[test]
    fn crossing_blocks() {
        let (a, _) = primitive::block(OpKey(1), 2.0, 2.0, 2.0, &Rigid::IDENTITY).unwrap();
        let place = Rigid::translation(Vector3::new(1.0, 0.5, 0.5));
        let (b, _) = primitive::block(OpKey(2), 2.0, 1.0, 1.0, &place).unwrap();
        // Count face pairs that cross through each other's interiors.
        let mut crossing = 0;
        for f in faces_of(&a) {
            for g in faces_of(&b) {
                if let FaceMeet::Meet { stretches, .. } = face_face((&a, f), (&b, g)).unwrap() {
                    crossing += stretches
                        .iter()
                        .filter(|s| s.places == [Place::Inside; 2])
                        .count();
                }
            }
        }
        // b pokes out of a's +x face: four of b's side faces cross it.
        assert_eq!(crossing, 4);
    }

    #[test]
    fn a_cylinder_through_a_plate() {
        let (plate, _) = primitive::block(OpKey(1), 4.0, 4.0, 1.0, &Rigid::IDENTITY).unwrap();
        let (rod, _) = primitive::cylinder(
            OpKey(2),
            0.5,
            3.0,
            &Rigid::translation(Vector3::new(2.0, 2.0, -1.0)),
        )
        .unwrap();
        let top = faces_of(&plate)
            .into_iter()
            .find(|&f| {
                let s = &plate.surfaces[plate.faces[f].surface];
                matches!(s, Surface::Plane(_)) && s.distance(Point3::new(2.0, 2.0, 1.0)) < 1e-12
            })
            .unwrap();
        let side = faces_of(&rod)
            .into_iter()
            .find(|&f| matches!(rod.surfaces[rod.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let FaceMeet::Meet { stretches, .. } = face_face((&plate, top), (&rod, side)).unwrap()
        else {
            panic!()
        };
        assert_eq!(stretches.len(), 1);
        assert!(matches!(stretches[0].curve, Curve::Circle(_)));
        assert_eq!(stretches[0].places, [Place::Inside; 2]);
    }

    #[test]
    fn coplanar_faces() {
        let (a, _) = primitive::block(OpKey(1), 2.0, 2.0, 1.0, &Rigid::IDENTITY).unwrap();
        // The same height, shifted: their tops overlap.
        let (b, _) = primitive::block(
            OpKey(2),
            2.0,
            2.0,
            1.0,
            &Rigid::translation(Vector3::new(1.0, 1.0, 0.0)),
        )
        .unwrap();
        // The face on the plane z = 1.
        let top = |body: &Body| {
            faces_of(body)
                .into_iter()
                .find(|&f| {
                    let s = &body.surfaces[body.faces[f].surface];
                    matches!(s, Surface::Plane(_))
                        && s.frame().unwrap().z().get().z.abs() > 0.5
                        && s.distance(Point3::new(0.0, 0.0, 1.0)) < 1e-12
                })
                .unwrap()
        };
        let (ta, tb) = (top(&a), top(&b));
        let FaceMeet::Coincident { rims, whole, .. } = face_face((&a, ta), (&b, tb)).unwrap()
        else {
            panic!()
        };
        assert!(!whole);
        assert!(rims.iter().any(|r| r.place == Place::Inside));
        // Apart side by side on one plane: no rim inside the other.
        let (c, _) = primitive::block(
            OpKey(3),
            2.0,
            2.0,
            1.0,
            &Rigid::translation(Vector3::new(5.0, 0.0, 0.0)),
        )
        .unwrap();
        let tc = top(&c);
        assert!(
            matches!(face_face((&a, ta), (&c, tc)).unwrap(), FaceMeet::Meet { ref stretches, .. } if stretches.is_empty())
        );
    }

    #[test]
    fn faces_of_valid_bodies_meet_only_on_their_edges() {
        let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        let bodies = [
            primitive::cylinder(OpKey(1), 1.0, 2.0, &t).unwrap().0,
            primitive::cone(OpKey(1), 1.0, 2.0, &t).unwrap().0,
            primitive::frustum(OpKey(1), 1.0, 0.5, 2.0, &t).unwrap().0,
            primitive::sphere(OpKey(1), 1.0, &t).unwrap().0,
            primitive::torus(OpKey(1), 2.0, 0.5, &t).unwrap().0,
        ];
        for b in &bodies {
            let fs = faces_of(b);
            for (i, &f) in fs.iter().enumerate() {
                for &g in &fs[i + 1..] {
                    let FaceMeet::Meet {
                        stretches,
                        contacts,
                    } = face_face((b, f), (b, g)).unwrap()
                    else {
                        panic!("coincident")
                    };
                    for s in &stretches {
                        assert!(
                            s.places
                                .iter()
                                .all(|p| matches!(p, Place::OnEdge(_) | Place::OnVertex(_))),
                            "{s:?}"
                        );
                    }
                    assert!(contacts.iter().all(|c| !c.places.contains(&Place::Inside)));
                }
            }
        }
    }

    #[test]
    fn stretches_wrapping_round_a_closed_branch_have_pcurves_all_along() {
        // Equal cylinders crossing at right angles meet in two ellipses
        // that cross each other where the cylinders touch; the faces' own
        // edges are far off, so each ellipse is one stretch, and one of
        // them is cut where the other face's box clips it. Every stretch's
        // pcurves must fit it over its whole range.
        let (a, _) = primitive::cylinder(
            OpKey(1),
            1.0,
            4.0,
            &Rigid::translation(Vector3::new(0.0, 0.0, -2.0)),
        )
        .unwrap();
        let across = Rigid::rotation(
            Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(),
            core::f64::consts::FRAC_PI_2,
        )
        .then_after(&Rigid::translation(Vector3::new(0.0, 0.0, -4.0)));
        let (b, _) = primitive::cylinder(OpKey(2), 1.0, 4.0, &across).unwrap();
        let side = |body: &Body| {
            faces_of(body)
                .into_iter()
                .find(|&f| matches!(body.surfaces[body.faces[f].surface], Surface::Cylinder(_)))
                .unwrap()
        };
        let (fa, fb) = (side(&a), side(&b));
        let FaceMeet::Meet { stretches, .. } = face_face((&a, fa), (&b, fb)).unwrap() else {
            panic!()
        };
        assert!(!stretches.is_empty());
        for st in &stretches {
            for (k, (body, f)) in [(&a, fa), (&b, fb)].into_iter().enumerate() {
                let s = &body.surfaces[body.faces[f].surface];
                let (c2, map) = &st.pcurves[k];
                for i in 0..=16 {
                    let t = st.range[0] + (st.range[1] - st.range[0]) * f64::from(i) / 16.0;
                    let q = c2.point(map[0] + map[1] * t);
                    let d = s.point(q.x, q.y).distance(st.curve.point(t));
                    assert!(d <= st.tolerance * 1.01, "{d} at {t}");
                }
            }
        }
    }

    #[test]
    fn a_torus_cut_by_a_slab() {
        let (torus, _) = primitive::torus(OpKey(1), 3.0, 1.0, &Rigid::IDENTITY).unwrap();
        let tilt = Rigid::rotation(Vector3::new(1.0, 0.0, 0.0).normalize().unwrap(), 0.2);
        // A slab 0.4 thick through the torus's centre, tilted a little: each
        // big face cuts the tube in two loops round the axis.
        let centred = Rigid::translation(Vector3::new(-5.0, -5.0, -0.2));
        let (slab, _) =
            primitive::block(OpKey(2), 10.0, 10.0, 0.4, &tilt.then_after(&centred)).unwrap();
        let tf = faces_of(&torus)[0];
        let mut found = 0;
        for g in faces_of(&slab) {
            if let FaceMeet::Meet { stretches, .. } = face_face((&torus, tf), (&slab, g)).unwrap() {
                found += stretches
                    .iter()
                    .filter(|s| s.places == [Place::Inside; 2])
                    .count();
            }
        }
        assert_eq!(found, 4);
    }

    #[test]
    fn spheres_on_one_surface_cover_each_other() {
        let (a, _) = primitive::sphere(OpKey(1), 1.0, &Rigid::IDENTITY).unwrap();
        let (b, _) = primitive::sphere(
            OpKey(2),
            1.0,
            &Rigid::rotation(Vector3::new(0.0, 1.0, 0.0).normalize().unwrap(), 1.0),
        )
        .unwrap();
        let m = face_face((&a, faces_of(&a)[0]), (&b, faces_of(&b)[0])).unwrap();
        assert_eq!(
            m,
            FaceMeet::Coincident {
                same: true,
                rims: vec![],
                whole: true
            }
        );
    }

    #[test]
    fn swept_faces_must_be_certified_apart() {
        use crate::geom::{KnotVector, Point2};
        use crate::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile, extrude};
        // A blob: a periodic cubic round the origin, extruded 1 up.
        let n = 8;
        let pts: Vec<Point2> = (0..n)
            .map(|i| {
                let (s, c) =
                    crate::math::sin_cos(core::f64::consts::TAU * f64::from(i) / f64::from(n));
                let k = [1.0, 0.8, 1.1, 0.9][i as usize % 4];
                Point2::new(k * c, k * s)
            })
            .collect();
        let knots: Vec<f64> = (0..=n).map(f64::from).collect();
        let kv = KnotVector::periodic(3, &knots, &vec![1; n as usize + 1]).unwrap();
        let blob = Curve2::Nurbs2(Arc::new(NurbsCurve2::new(kv, pts, None).unwrap()));
        let profile = ValidProfile::new(Profile {
            frame: Frame::WORLD,
            loops: vec![Loop {
                key: LoopKey(1),
                segments: vec![Segment {
                    key: SegmentKey(1),
                    curve: blob,
                    t0: 0.0,
                    t1: f64::from(n),
                }],
            }],
        })
        .unwrap();
        let (body, _) = extrude(OpKey(1), &profile, Vector3::new(0.0, 0.0, 1.0), 0.0, 1.0).unwrap();
        let side = faces_of(&body)
            .into_iter()
            .find(|&f| matches!(body.surfaces[body.faces[f].surface], Surface::Extrusion(_)))
            .unwrap();
        // A block beside it, its box overlapping the blob's: apart.
        let (near, _) = primitive::block(
            OpKey(2),
            1.0,
            1.0,
            1.0,
            &Rigid::translation(Vector3::new(0.95, 0.95, 0.0)),
        )
        .unwrap();
        for g in faces_of(&near) {
            assert_eq!(
                face_face((&body, side), (&near, g)),
                Ok(FaceMeet::apart()),
                "{g:?}"
            );
        }
        // One cutting through it: its faces across the blob meet the side
        // in stretches on both.
        let (through, _) = primitive::block(
            OpKey(3),
            1.0,
            1.0,
            0.5,
            &Rigid::translation(Vector3::new(0.5, -0.5, 0.25)),
        )
        .unwrap();
        let mut met = 0;
        for g in faces_of(&through) {
            let FaceMeet::Meet { stretches, .. } = face_face((&body, side), (&through, g)).unwrap()
            else {
                panic!("coincident")
            };
            for st in &stretches {
                let p = st.curve.point((st.range[0] + st.range[1]) / 2.0);
                let s = &body.surfaces[body.faces[side].surface];
                assert!(s.distance(p) <= st.tolerance);
            }
            met += stretches.len();
        }
        assert!(met >= 2, "{met}");
    }
}
