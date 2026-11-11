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
//! in the first face's parameter plane ([`crossings_in`]), and the pieces
//! inside or along the other face are the rims of the overlap.
//!
//! Faces on any surfaces meet the same way ([`surface_surface`] and
//! [`curve_surface`] take every surface). A NURBS or swept face and an
//! analytic one whose implicit form has one certified sign on every
//! Bézier patch of the first's exact NURBS form inside the region are
//! apart without a trace.

use super::Failure;
use super::cs::{CurveHit, curve_surface_in};
use super::fit::fit_pcurve;
use super::implicit::Implicit;
use super::inface::{Place, point_in_face, point_in_face_near};
use super::ss::SurfaceHit;
use crate::geom::intersect2::{Meet2, crossings_in};
use crate::geom::{
    Box3, Circle, Circle2, Curve, Curve2, Ellipse2, Frame, Frame2, Line2, NurbsCurve2, Point2,
    Point3, Surface, Vector2,
};
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId};
use crate::work::Meter;
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
    /// How far past its tolerance each end is known: where a tolerant
    /// edge cut it, the distance from the edge's hit on the other surface
    /// to the end, both in the edge's tube (design 299); else 0.
    pub ends: [f64; 2],
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
    pub(crate) fn apart() -> Self {
        Self::Meet {
            stretches: Vec::new(),
            contacts: Vec::new(),
        }
    }
}

/// How face `a.1` of body `a.0` and face `b.1` of body `b.0` meet.
pub fn face_face(a: (&Body, FaceId), b: (&Body, FaceId)) -> Result<FaceMeet, Failure> {
    // unmetered: the public function takes no work
    face_face_in(a, b, &Meter::free())
}

/// [`face_face`] charging `m`.
pub(crate) fn face_face_in(
    a: (&Body, FaceId),
    b: (&Body, FaceId),
    m: &Meter,
) -> Result<FaceMeet, Failure> {
    face_face_within(a, b, 0.0, m)
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
    m: &Meter,
) -> Result<FaceMeet, Failure> {
    m.charge(1)?;
    let slack = max_tolerance(a).max(max_tolerance(b));
    let (ba, bb) = (face_box(a.0, a.1), face_box(b.0, b.1));
    let Some(region) = overlap(&ba, &bb, slack + margin) else {
        return Ok(FaceMeet::apart());
    };
    face_face_over(a, b, &region, m)
}

/// The boxes `region` holds outside cubes of half side `half` round
/// `points`: for each cube, what is left of every box it meets, cut into
/// up to six slabs (its x sides, then y, then z, each taken off what is
/// left). Together they cover the region less the cubes' insides.
pub(crate) fn region_outside(region: &Box3, points: &[Point3], half: f64) -> Vec<Box3> {
    let mut boxes = vec![*region];
    for p in points {
        let lo_c = Point3::new(p.x - half, p.y - half, p.z - half);
        let hi_c = Point3::new(p.x + half, p.y + half, p.z + half);
        let cube = Box3::new(lo_c, hi_c);
        let mut next = Vec::new();
        for bx in boxes {
            if !bx.intersects(&cube) {
                next.push(bx);
                continue;
            }
            let (mut lo, mut hi) = (bx.min(), bx.max());
            if lo.x < lo_c.x {
                next.push(Box3::new(lo, Point3::new(lo_c.x, hi.y, hi.z)));
                lo.x = lo_c.x;
            }
            if hi.x > hi_c.x {
                next.push(Box3::new(Point3::new(hi_c.x, lo.y, lo.z), hi));
                hi.x = hi_c.x;
            }
            if lo.y < lo_c.y {
                next.push(Box3::new(lo, Point3::new(hi.x, lo_c.y, hi.z)));
                lo.y = lo_c.y;
            }
            if hi.y > hi_c.y {
                next.push(Box3::new(Point3::new(lo.x, hi_c.y, lo.z), hi));
                hi.y = hi_c.y;
            }
            if lo.z < lo_c.z {
                next.push(Box3::new(lo, Point3::new(hi.x, hi.y, lo_c.z)));
            }
            if hi.z > hi_c.z {
                next.push(Box3::new(Point3::new(lo.x, lo.y, hi_c.z), hi));
            }
        }
        boxes = next;
    }
    boxes
}

/// [`face_face`] over `region` alone: what the faces' surfaces share
/// there, placed in both faces. [`face_face_within`] is it over the
/// faces' boxes' overlap.
pub(crate) fn face_face_over(
    a: (&Body, FaceId),
    b: (&Body, FaceId),
    region: &Box3,
    m: &Meter,
) -> Result<FaceMeet, Failure> {
    let region = *region;
    let faces = [a, b];
    let sa = surface_of(a)?;
    let sb = surface_of(b)?;
    let slack = max_tolerance(a).max(max_tolerance(b));
    // Planes that are one over the region though not everywhere (a part
    // placed by a pose written in single precision, standing on a board:
    // issue #436) meet as coincident faces there, not in a line the
    // faces cross at an angle no trace can certify.
    if let Some(same) = super::coincide::planes_within(sa, sb, &region) {
        return coincident(faces, [sa, sb], same, m);
    }
    // A NURBS or swept face certified apart from an analytic surface by
    // the signs of its patches is apart at once, without a trace.
    let pipe = |s: &Surface| matches!(s, Surface::Pipe(_));
    match (Implicit::of(sa), Implicit::of(sb)) {
        (Some(i), None) if !pipe(sb) => {
            if let Ok(apart) = certified_apart(&i, b, &region, m) {
                return Ok(apart);
            }
        }
        (None, Some(i)) if !pipe(sa) => {
            if let Ok(apart) = certified_apart(&i, a, &region, m) {
                return Ok(apart);
            }
        }
        _ => {}
    }
    // Faces sharing an edge that lies along a bounded domain edge of one
    // of two surfaces with no implicit form meet along that domain edge,
    // which lies on the side of every cell of the four-parameter trace
    // beside it, where no crossing can be certified: the trace exhausted
    // its face search there, at every split fraction and in every grown
    // region, and ended `Uncertain` (#302: KiCad's SMA connector, 25 spline
    // pairs at 1.5M units each). On the surfaces carried past their domain
    // edges the shared edge lies inside cells, and is traced at once with
    // whatever else they share; a crossing elsewhere is still found.
    let carried = along_domain_edge(a, b, sa, sb);
    let first = if carried {
        super::ss::carried(sa, sb, &region, m)
    } else {
        super::ss::meet(sa, sb, &region, false, m)
    };
    let hit = match first {
        // A trace can fail where the surfaces meet only outside the faces
        // (sides of one loft or sweep along their shared edge, which these
        // pieces of them no longer reach): faces certified apart over
        // their own parameter boxes are apart. Else, unless they meet along
        // an edge or at a vertex they share (which the self-intersection
        // check settles by `beside`), the surfaces carried past their
        // domain edges are traced: a curve along one lies inside then.
        Err(Failure::Uncertain) => {
            if super::beside::faces_apart(a, b, FACES_APART, m) {
                return Ok(FaceMeet::apart());
            }
            if let Some(meet) = along_rims(a, b, sa, sb, m)? {
                return Ok(meet);
            }
            if sharing(a, b) && !ends_together(sa, sb) {
                return Err(Failure::Uncertain);
            }
            match super::ss::meet(sa, sb, &region, true, m) {
                // The trace's cells can fall just wrong for one region's
                // bits (a tolerant face's box, grown by its rims' tubes,
                // put a sphere's tangency with a cylinder there): traced
                // once more in a region grown by a hair.
                Err(Failure::Uncertain) => {
                    let hair = slack + crate::tol::TOUCH_TOLERANCE;
                    super::ss::meet(sa, sb, &region.grow(hair), false, m)?
                }
                other => other?,
            }
        }
        other => other?,
    };
    let (branches, points) = match hit {
        SurfaceHit::Coincident { same } => return coincident(faces, [sa, sb], same, m),
        SurfaceHit::Meet { branches, points } => (branches, points),
    };
    // Surfaces that don't meet in the region: nothing for edges to cut.
    if branches.is_empty() && points.is_empty() {
        return Ok(FaceMeet::apart());
    }
    // Where each face's edges meet the other's surface. An edge both faces
    // use lies on both surfaces (the body is sound): only its ends cut.
    // Each cut with the tube it is known within past the branch's reach:
    // a tolerant edge's hit on the other surface is where the edge
    // crosses it, which may lie that far from where the branch passes
    // through the edge's tube (design 299). Exact edges' cuts take none.
    let mut cuts: Vec<(Point3, f64)> = Vec::new();
    let shared: Vec<EdgeId> = if core::ptr::eq(a.0, b.0) {
        let theirs = edges(b);
        edges(a)
            .into_iter()
            .filter(|e| theirs.contains(e))
            .collect()
    } else {
        Vec::new()
    };
    // A cut only splits a branch it lies within reach of: an edge whose
    // box meets no branch's box cuts nothing (a block's long edges against
    // each pin it is drilled by).
    let near: Vec<Box3> = branches
        .iter()
        .map(|br| br.curve.bounds(br.range).grow(br.tolerance + slack))
        .collect();
    for (face, other) in [(a, sb), (b, sa)] {
        for e in edges(face) {
            let edge = &face.0.edges[e];
            let curve = &face.0.curves[edge.curve];
            let reach = curve.bounds(edge.interval).grow(edge.tolerance.get());
            if !near.iter().any(|x| x.intersects(&reach)) {
                continue;
            }
            if shared.contains(&e) {
                cuts.extend([edge.start, edge.end].map(|v| (face.0.vertices[v].point, 0.0)));
                continue;
            }
            let tube = Some(edge.tolerance.get())
                .filter(|&t| t > crate::tol::TOUCH_TOLERANCE)
                .unwrap_or(0.0);
            for hit in curve_surface_in(curve, edge.interval, other, edge.tolerance.get(), m)? {
                match hit {
                    CurveHit::Point { point, .. } => cuts.push((point, tube)),
                    CurveHit::On { t } => cuts.extend(t.map(|t| (curve.point(t), tube))),
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
            .filter(|&&(c, tube)| br.curve.distance(c) <= reach + tube)
            .map(|&(c, _)| {
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
        // How far each tolerant cut lies from where it cuts the branch.
        let off: Vec<(Point3, f64)> = cuts
            .iter()
            .filter(|&&(c, tube)| tube > 0.0 && br.curve.distance(c) <= reach + tube)
            .map(|&(c, _)| {
                let foot = br.curve.point(br.curve.project(c).param);
                (foot, foot.distance(c))
            })
            .collect();
        let end_reach = |t: f64| {
            let p = br.curve.point(t);
            off.iter()
                .filter(|x| x.0.distance(p) <= reach)
                .map(|x| x.1)
                .fold(0.0, f64::max)
        };
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
            // A piece placed in both faces.
            m.charge(1)?;
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
                    let (n, bound) = fit_pcurve(s, &br.curve, [s0, s1], m)?;
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
            // A stretch running beside an edge of either face, from one of
            // its vertices to the other, within the touch rule's reach all
            // along, is a touch along that edge, not a cut: a plane tangent
            // to a round where the round meets the face the plane lies on,
            // tilted by a file's single-precision pose, cuts the round
            // 2e-11 m deep in an arc 4e-8 m beside the edge (#436), which
            // splitting can't order against the edge. The edge's other
            // face, on the plane's surface, already meets it as coincident
            // faces there.
            if (0..2).any(|k| {
                !matches!(places[k], Place::OnEdge(_) | Place::OnVertex(_))
                    && beside_edge(faces[k], &br.curve, [s0, s1], tolerance)
            }) {
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
                ends: [end_reach(s0), end_reach(s1)],
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

/// How far apart two faces' normals may be, at the middle of edges of
/// theirs lying along each other, for [`along_rims`] to take them as
/// tangent there: a hundredth of a radian, the self-intersection check's
/// rule for faces tangent along an edge they share.
const NEARLY_TANGENT: f64 = 1e-2;

/// Faces of different bodies, tangent along edges of each that lie on one
/// curve (a file's two solids glued across a plane, a torus tube running
/// on into a straight cylinder across the rim both carry: issue #584),
/// whose surfaces' trace stayed uncertain. As surfaces they may cross
/// elsewhere too (the torus and the cylinder along a second curve through
/// two points of the rim, on the cylinder's side), but where one face is
/// certified to keep off the other's surface outside a strip round those
/// edges ([`face_beside`](super::beside::face_beside)), as wide as
/// surfaces meeting tangentially stay within the touch rule's reach of
/// each other, `sqrt(2 × size × reach)`, the faces meet only along the
/// edges: the self-intersection check's rule for faces tangent along an
/// edge they share. Each pair of edges is then a tangent stretch along
/// the first's curve, placed on both edges. `None` where no such edges
/// are found or neither face is certified, so the caller goes on as it
/// would have.
fn along_rims(
    a: (&Body, FaceId),
    b: (&Body, FaceId),
    sa: &Surface,
    sb: &Surface,
    m: &Meter,
) -> Result<Option<FaceMeet>, Failure> {
    if core::ptr::eq(a.0, b.0) {
        return Ok(None);
    }
    let mut rims: Vec<(EdgeId, EdgeId, f64)> = Vec::new();
    for ea in edges(a) {
        for eb in edges(b) {
            if let Some(gap) = edges_together((a.0, ea), (b.0, eb)) {
                let x = &a.0.edges[ea];
                let p = a.0.curves[x.curve].point(0.5 * (x.interval[0] + x.interval[1]));
                let normal = |s: &Surface| {
                    let (u, v) = s.project(p).param;
                    s.normal(u, v)
                };
                if let (Some(na), Some(nb)) = (normal(sa), normal(sb))
                    && na.get().cross(nb.get()).length() <= NEARLY_TANGENT
                {
                    rims.push((ea, eb, gap));
                }
            }
        }
    }
    if rims.is_empty() {
        return Ok(None);
    }
    let slack = max_tolerance(a).max(max_tolerance(b));
    let reach = 100.0 * slack;
    let size = {
        let bx = face_box(a.0, a.1).union(&face_box(b.0, b.1));
        (bx.max() - bx.min()).length()
    };
    let strip = (2.0 * size * reach).sqrt();
    let rails = |k: usize| -> Vec<EdgeId> {
        let mut r: Vec<EdgeId> = rims
            .iter()
            .map(|x| if k == 0 { x.0 } else { x.1 })
            .collect();
        r.sort();
        r.dedup();
        r
    };
    let (ra, rb) = (rails(0), rails(1));
    let keeps_off = |f: (&Body, FaceId), other: &Surface, rails: &[EdgeId], cells: usize| {
        super::beside::face_beside(f.0, f.1, other, rails, &[], strip, cells, m).is_ok()
    };
    // Each face's search under a growing part limit, the other's between:
    // the face that keeps off the other's surface usually does in a few
    // parts, while the other runs on and exhausts any limit.
    let apart = [1 << 9, 1 << 12, super::BESIDE_CELLS]
        .into_iter()
        .any(|cells| keeps_off(a, sb, &ra, cells) || keeps_off(b, sa, &rb, cells))
        || either_side(a, b, sa, sb, &rims, strip, m);
    if !apart {
        return Ok(None);
    }
    let fin_pcurve = |(body, face): (&Body, FaceId), e: EdgeId| {
        body.face_loops(face)
            .flat_map(|l| body.loop_fins(l).collect::<Vec<_>>())
            .find(|&fin| body.fins[fin].edge == e)
            .and_then(|fin| body.fins[fin].pcurve)
            .map(|pc| (body.curves2[pc.curve].clone(), pc.map))
    };
    let mut stretches = Vec::new();
    for (ea, eb, gap) in rims {
        let x = &a.0.edges[ea];
        let y = &b.0.edges[eb];
        let curve = a.0.curves[x.curve].clone();
        let range = x.interval;
        let mut tolerance = x.tolerance.get().max(y.tolerance.get()).max(gap);
        let pa = fin_pcurve(a, ea).ok_or(Failure::Uncertain)?;
        // The other face's pcurve is its own edge's where the two edges
        // are one curve over one interval, else fitted along the first's.
        let pb = match fin_pcurve(b, eb) {
            Some(pc) if b.0.curves[y.curve] == curve && y.interval == range => pc,
            _ => {
                let (n, bound) = fit_pcurve(sb, &curve, range, m)?;
                tolerance = tolerance.max(bound);
                (Curve2::Nurbs2(Arc::new(n)), [0.0, 1.0])
            }
        };
        let pcurves = [pa, pb];
        let mid = 0.5 * (range[0] + range[1]);
        let mut places = [Place::Outside; 2];
        for (k, f) in [a, b].into_iter().enumerate() {
            let (c2, map) = &pcurves[k];
            let uv = c2.point(map[0] + map[1] * mid);
            places[k] = point_in_face_near(f.0, f.1, uv, tolerance + super::SLACK)?;
        }
        if !places
            .iter()
            .all(|p| matches!(p, Place::OnEdge(_) | Place::OnVertex(_)))
        {
            return Ok(None);
        }
        stretches.push(Stretch {
            curve,
            range,
            tangent: true,
            tolerance,
            pcurves,
            places,
            ends: [0.0; 2],
        });
    }
    Ok(Some(FaceMeet::Meet {
        stretches,
        contacts: Vec::new(),
    }))
}

/// Whether two faces lie either side of the plane of the planar curve
/// their edges [`along_rims`] found lie along, outside the strip round
/// them: the self-intersection check's rule for blends across a section.
/// Each face is certified by [`face_beside`](super::beside::face_beside)
/// against the plane ([`super::section_plane`]), and neither may lie on
/// both sides of it. As surfaces they may still cross elsewhere (a torus
/// tube and the cylinder running on from it cross along a curve through
/// two points of their rim, on both sides of its plane, each surface
/// beyond its own face), so neither face need keep off the other's
/// surface, but faces on opposite sides of a plane meet only on it.
fn either_side(
    a: (&Body, FaceId),
    b: (&Body, FaceId),
    sa: &Surface,
    sb: &Surface,
    rims: &[(EdgeId, EdgeId, f64)],
    strip: f64,
    m: &Meter,
) -> bool {
    let x = &a.0.edges[rims[0].0];
    let curve = &a.0.curves[x.curve];
    // Every rim on one curve, the plane's.
    if rims
        .iter()
        .any(|r| &a.0.curves[a.0.edges[r.0].curve] != curve)
    {
        return false;
    }
    let tolerance = rims.iter().map(|r| r.2).fold(x.tolerance.get(), f64::max);
    let Some(plane) = super::section_plane(curve, x.interval, tolerance, sa, sb) else {
        return false;
    };
    let rails = |k: usize| -> Vec<EdgeId> {
        let mut r: Vec<EdgeId> = rims
            .iter()
            .map(|x| if k == 0 { x.0 } else { x.1 })
            .collect();
        r.sort();
        r.dedup();
        r
    };
    let on_plane = |f: (&Body, FaceId), rails: &[EdgeId]| {
        super::beside::face_beside(f.0, f.1, &plane, rails, &[], strip, super::BESIDE_CELLS, m).ok()
    };
    match (on_plane(a, &rails(0)), on_plane(b, &rails(1))) {
        (Some(pa), Some(pb)) => {
            let one = |s: [bool; 2]| s != [true, true];
            one(pa)
                && one(pb)
                && (pa[0] && pb[1] || pa[1] && pb[0] || pa == [false; 2] || pb == [false; 2])
        }
        _ => false,
    }
}

/// How far apart an edge of one body and an edge of another lie, where
/// they lie along each other end to end: both closed, or each one's ends
/// at the other's (either way round) within the touch rule's reach for
/// their tolerances, and every point of each certified within a reach of
/// the other ([`within_reach`]), the least of a ladder from their
/// tolerances' sum up to that touch reach. `None` otherwise.
fn edges_together((ba, ea): (&Body, EdgeId), (bb, eb): (&Body, EdgeId)) -> Option<f64> {
    let (x, y) = (&ba.edges[ea], &bb.edges[eb]);
    let (ca, cb) = (&ba.curves[x.curve], &bb.curves[y.curve]);
    let (ta, tb) = (x.tolerance.get(), y.tolerance.get());
    let most = crate::tol::touch_reach(ta.max(tb));
    if !ca
        .bounds(x.interval)
        .grow(most)
        .intersects(&cb.bounds(y.interval))
    {
        return None;
    }
    let at = |b: &Body, v: crate::topo::VertexId| b.vertices[v].point;
    match (x.start == x.end, y.start == y.end) {
        (true, true) => {}
        (false, false) => {
            let (a0, a1) = (at(ba, x.start), at(ba, x.end));
            let (b0, b1) = (at(bb, y.start), at(bb, y.end));
            let near = |p: Point3, q: Point3| p.distance(q) <= most;
            if !(near(a0, b0) && near(a1, b1) || near(a0, b1) && near(a1, b0)) {
                return None;
            }
        }
        _ => return None,
    }
    let mut r = (ta + tb).max(LINEAR_RESOLUTION);
    loop {
        let r_now = r.min(most);
        if within_reach((ca, x.interval), (cb, y.interval), r_now)
            && within_reach((cb, y.interval), (ca, x.interval), r_now)
        {
            return Some(r_now);
        }
        if r_now >= most {
            return None;
        }
        r *= 4.0;
    }
}

/// Whether `curve` over `range` runs beside an edge of the face: its ends
/// each within the touch rule's reach of one of the edge's vertices (the
/// two, for an open edge), and every point of each certified within that
/// reach of the other ([`within_reach`]). The reach is the touch rule's for
/// the larger of the edge's tolerance and the curve's `tolerance`: an
/// explicit tolerance rule, as the touch rule is.
fn beside_edge(
    (body, face): (&Body, FaceId),
    curve: &Curve,
    range: [f64; 2],
    tolerance: f64,
) -> bool {
    let (p0, p1) = (curve.point(range[0]), curve.point(range[1]));
    edges((body, face)).into_iter().any(|e| {
        let edge = &body.edges[e];
        let reach = crate::tol::touch_reach(edge.tolerance.get().max(tolerance));
        let at = |v: crate::topo::VertexId| body.vertices[v].point;
        let (a, b) = (at(edge.start), at(edge.end));
        let ends = (p0.distance(a) <= reach && p1.distance(b) <= reach)
            || (p0.distance(b) <= reach && p1.distance(a) <= reach);
        let ec = &body.curves[edge.curve];
        ends && within_reach((curve, range), (ec, edge.interval), reach)
            && within_reach((ec, edge.interval), (curve, range), reach)
    })
}

/// Whether every point of `c` over its range is certified within `reach`
/// of `other` over its own. A stretch `[s, t]` of `c` with middle `m`
/// passes when the distance from `c(m)` to `other` (to a point of it, so
/// an upper bound on the distance) plus `|c'| (t - s) / 2`, `|c'|` bounded
/// by the interval enclosure of the derivative over the stretch (the sum
/// of its components' magnitudes), is within `reach`: every point of the
/// stretch is then that close. A stretch that doesn't pass is halved, down
/// to `1 / 2^12` of the range; one that still doesn't is no proof.
fn within_reach(
    (c, [c0, c1]): (&Curve, [f64; 2]),
    (other, range): (&Curve, [f64; 2]),
    reach: f64,
) -> bool {
    const DEPTH: u32 = 12;
    let mut stack: Vec<([f64; 2], u32)> = (0..16)
        .map(|i| {
            let f = |k: f64| c0 + (c1 - c0) * k / 16.0;
            ([f(i as f64), f(i as f64 + 1.0)], 0)
        })
        .collect();
    while let Some(([s, t], depth)) = stack.pop() {
        let m = 0.5 * (s + t);
        let d = super::inface::edge_distance(other, range, c.point(m));
        let speed: f64 = c.jet_box([s.min(t), s.max(t)])[1]
            .iter()
            .map(|x| x.mag())
            .sum();
        // The bound's own rounding: a part in 10⁹ of what it adds up, and
        // the evaluator's slack.
        let bound = (d + speed * 0.5 * (t - s).abs()) * (1.0 + 1e-9) + super::SLACK;
        if bound <= reach {
            continue;
        }
        if depth >= DEPTH || !bound.is_finite() {
            return false;
        }
        stack.push(([s, m], depth + 1));
        stack.push(([m, t], depth + 1));
    }
    true
}

fn surface_of((body, face): (&Body, FaceId)) -> Result<&Surface, Failure> {
    let f = body.face(face).ok_or(Failure::Uncertain)?;
    body.surface(f.surface).ok_or(Failure::Uncertain)
}

/// The largest tolerance of a face's edges and vertices.
pub(crate) fn max_tolerance((body, face): (&Body, FaceId)) -> f64 {
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

/// Whether both surfaces are sweeps, or offsets of sweeps, whose boxes
/// along their paths end together (neighbouring sides of a sweep along
/// an open path, shrunk or grown, #223): faces sharing an edge there are
/// traced on their surfaces carried past those ends, since neither the
/// trace nor `beside` (the check's) settles them where both end at once.
fn ends_together(a: &Surface, b: &Surface) -> bool {
    let along = |s: &Surface| match s {
        Surface::Sweep(w) => Some(w.domain()[1]),
        Surface::Offset(o) => match o.base() {
            Surface::Sweep(_) => Some(o.domain()[1]),
            _ => None,
        },
        _ => None,
    };
    let near = |x: f64, y: f64| (x - y).abs() <= 1e-9 * (1.0 + y.abs());
    match (along(a), along(b)) {
        (Some(x), Some(y)) => x.iter().any(|&p| y.iter().any(|&q| near(p, q))),
        _ => false,
    }
}

/// Whether faces `a` and `b` of one body, on surfaces `sa` and `sb` with
/// no implicit form and not coincident, share an edge whose pcurve on one
/// of them lies exactly on a bounded (not periodic) edge of its surface's
/// domain: its held parameter equal to the domain's end, a line along the
/// other parameter or a spline whose control points all hold it.
fn along_domain_edge(a: (&Body, FaceId), b: (&Body, FaceId), sa: &Surface, sb: &Surface) -> bool {
    if !core::ptr::eq(a.0, b.0)
        || Implicit::of(sa).is_some()
        || Implicit::of(sb).is_some()
        || super::coincide::coincident(sa, sb).is_some()
        || ends_together(sa, sb)
    {
        return false;
    }
    let body = a.0;
    let theirs = edges(b);
    edges(a)
        .into_iter()
        .filter(|e| theirs.contains(e))
        .any(|e| {
            body.edge_fins(e).any(|fin| {
                let f = body.loops[body.fins[fin].loop_].face;
                (f == a.1 || f == b.1) && fin_on_domain_edge(body, fin)
            })
        })
}

/// Whether a fin's pcurve lies exactly on a bounded edge of its face's
/// surface's domain.
fn fin_on_domain_edge(body: &Body, fin: crate::topo::FinId) -> bool {
    let Some(pc) = body.fins[fin].pcurve else {
        return false;
    };
    let s = &body.surfaces[body.faces[body.loops[body.fins[fin].loop_].face].surface];
    let domain = s.domain();
    let (pu, pv) = s.periods();
    let curve = &body.curves2[pc.curve];
    (0..2).any(|d| {
        let periodic = if d == 0 { pu.is_some() } else { pv.is_some() };
        let at = |p: Point2| if d == 0 { p.x } else { p.y };
        !periodic
            && domain[d].iter().any(|&end| match curve {
                Curve2::Line2(l) => {
                    at(l.origin()) == end && (if d == 0 { l.dir().x } else { l.dir().y }) == 0.0
                }
                Curve2::Nurbs2(n) => n.control_points().all(|p| at(p) == end),
                _ => false,
            })
    })
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
/// (on a clamped spine); a NURBS's, revolution's, sweep's, offset's or bent surface's inside its
/// surface's enclosure over the face's parameter box ([`face_rect`]),
/// grown by its edges' tolerances; any other surface's face inside the
/// surface's own box.
pub fn face_box(body: &Body, face: FaceId) -> Box3 {
    let surface = &body.surfaces[body.faces[face].surface];
    if matches!(
        surface,
        Surface::Nurbs(_)
            | Surface::Revolution(_)
            | Surface::Sweep(_)
            | Surface::Offset(_)
            | Surface::Bent(_)
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
    m: &Meter,
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
        m.charge(1)?;
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
    m: &Meter,
) -> Result<FaceMeet, Failure> {
    // The faces' own pcurves first, which they hold exactly (ctc_05: two
    // faces on one cylinder sharing two spline edges fitted them for 23
    // to 35 s); where the rims those give can't be certified, the rims
    // from exact pcurves and fits, as before (stc_07's AP242 edition: a
    // file's spline pcurves on a cylinder `crossings` couldn't settle).
    coincident_by(faces, surfaces, same, true, m)
        .or_else(|_| coincident_by(faces, surfaces, same, false, m))
}

/// [`coincident`], taking each fin's own pcurve first when `own_first`,
/// else an exact one, then a fit, then its own.
fn coincident_by(
    faces: [(&Body, FaceId); 2],
    surfaces: [&Surface; 2],
    same: bool,
    own_first: bool,
    m: &Meter,
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
            let own = || own_pcurve(body, face, e, turned(surfaces[k], plane));
            // Coincident surfaces lie within resolution of each other, so
            // an edge of a face on the other one may lie that much further
            // off the common surface than its own tolerance: two blocks'
            // faces a resolution apart, one turned (#558), whose straight
            // edge was fitted as a spline of thousands of spans that
            // `crossings` couldn't settle against the other's line.
            let reach = if surfaces[k] == plane {
                edge.tolerance.get()
            } else {
                edge.tolerance.get() + LINEAR_RESOLUTION
            };
            let exact = || crate::pcurve::exact(plane, curve, edge.interval, reach);
            let fitted = || match fit_pcurve(plane, curve, edge.interval, m) {
                Ok((n, _)) => Ok((Curve2::Nurbs2(Arc::new(n)), [0.0, 1.0])),
                Err(err) => Err(err),
            };
            let (c2, map) = if own_first {
                match own().or_else(exact) {
                    Some(p) => p,
                    None => fitted()?,
                }
            } else {
                match exact() {
                    Some(p) => p,
                    // An edge of a face on this very surface, too far off it
                    // to fit (a tolerant edge read from a file), has its own
                    // pcurve here.
                    None => match fitted() {
                        Ok(p) => p,
                        Err(err) => own().ok_or(err)?,
                    },
                }
            };
            chains[k].push((e, c2, map, edge.interval));
        }
    }
    let (pu, pv) = plane.periods();
    let one_body = std::ptr::eq(faces[0].0, faces[1].0);
    let mut rims = Vec::new();
    for k in 0..2 {
        let other = 1 - k;
        for (e, c2, map, interval) in &chains[k] {
            let mut cuts: Vec<f64> = Vec::new();
            for (d, d2, dmap, dint) in &chains[other] {
                // An edge both faces use meets itself all along: its only
                // cuts are its own ends, which bound its pieces anyway. Its
                // two fins' pcurves, fitted apart (healing's along a
                // meridian), can't be certified to overlap (#264).
                if one_body && d == e {
                    continue;
                }
                let range_a = interval.map(|t| map[0] + map[1] * t);
                let range_b = dint.map(|t| dmap[0] + dmap[1] * t);
                let tol = LINEAR_RESOLUTION;
                for shift in shifts(c2, range_a, d2, range_b, [pu, pv]) {
                    let moved = shifted(d2, shift).ok_or(Failure::Uncertain)?;
                    let found = crossings_in((c2, range_a), (&moved, range_b), tol, m)
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
                let mid = curve.point((w[0] + w[1]) / 2.0);
                // A piece whose ends meet is empty unless it goes round:
                // a closed edge no cut divides is a rim all round.
                let start = curve.point(w[0]);
                if start.distance(curve.point(w[1])) <= LINEAR_RESOLUTION
                    && start.distance(mid) <= LINEAR_RESOLUTION
                {
                    continue;
                }
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

/// The pcurve a fin of `edge` in `face` has, carried onto the surface
/// wanted by `turn` ([`turned`]).
fn own_pcurve(
    body: &Body,
    face: FaceId,
    edge: EdgeId,
    turn: Option<Turn>,
) -> Option<(Curve2, [f64; 2])> {
    let turn = turn?;
    body.edge_fins(edge).find_map(|fin| {
        let f = &body.fins[fin];
        let pc = f.pcurve?;
        if body.loops[f.loop_].face != face {
            return None;
        }
        Some((turn.carry(&body.curves2[pc.curve])?, pc.map))
    })
}

/// How a point's parameters on one copy of a surface move to the same
/// point's on another ([`turned`]): `(u, v)` to `(u, v) + shift`, or,
/// for a cylinder whose copy's axis runs the other way, to
/// `shift - (u, v)` (a half turn of the parameter plane: `u` runs round
/// the other way and `v` along the other way, so the natural normal, the
/// radius, is the same).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Turn {
    shift: Vector2,
    reversed: bool,
}

impl Turn {
    const NONE: Self = Self {
        shift: Vector2::ZERO,
        reversed: false,
    };

    /// Where the point at `p` goes.
    #[cfg(test)]
    fn apply(&self, p: Point2) -> Point2 {
        if self.reversed {
            Point2::new(self.shift.x - p.x, self.shift.y - p.y)
        } else {
            p + self.shift
        }
    }

    /// A plane curve carried by the map, on its own parameter.
    fn carry(&self, c: &Curve2) -> Option<Curve2> {
        if self.reversed {
            use crate::geom::ParamFlip;
            Some(
                c.mirrored(ParamFlip::U(self.shift.x))
                    .mirrored(ParamFlip::V(self.shift.y)),
            )
        } else {
            shifted(c, self.shift)
        }
    }
}

/// How a point's parameters on `own` move to the same point's on `onto`,
/// for the same surface or a copy of a surface of revolution turned about
/// its own axis (two copies of one torus placed differently in a file):
/// by the angle from `onto`'s frame's `x` to `own`'s along `u`. A copy of
/// a cylinder may also sit anywhere along its axis, and its axis may lie
/// within [`crate::tol::ANGULAR_RESOLUTION`] of the other's (a file's frames, one
/// `(1, 1.8e-32, 0)` and the other `(1, 0, 0)`: a gear's shaft in its
/// bore, #240), its `v` then moving by the origins' offset along the
/// axis; or of its reverse (two solids of a file glued across a plane,
/// each holding a copy of one cylinder framed the other way: Voron's Z
/// Belt Drive, #584), `(u, v)` then going to `(α - u, d - v)`, `α` that
/// angle and `d` the offset along `onto`'s axis. `None` for any other
/// pair.
fn turned(own: &Surface, onto: &Surface) -> Option<Turn> {
    if own == onto {
        return Some(Turn::NONE);
    }
    let same_kind = matches!(
        (own, onto),
        (Surface::Cylinder(_), Surface::Cylinder(_))
            | (Surface::Cone(_), Surface::Cone(_))
            | (Surface::Sphere(_), Surface::Sphere(_))
            | (Surface::Torus(_), Surface::Torus(_))
    );
    let (a, b) = (own.frame()?, onto.frame()?);
    let axis = b.z().get();
    if !same_kind {
        return None;
    }
    let cylinder_on_axis = matches!(own, Surface::Cylinder(_))
        && super::coincide::parallel(a.z(), b.z())
        && super::coincide::off_axis(b, a.origin()) <= LINEAR_RESOLUTION;
    let (along, reversed) =
        if a.z().get() == axis && a.origin().distance(b.origin()) <= LINEAR_RESOLUTION {
            (0.0, false)
        } else if cylinder_on_axis {
            ((a.origin() - b.origin()).dot(axis), a.z().dot(b.z()) < 0.0)
        } else {
            return None;
        };
    let (ax, bx) = (a.x().get(), b.x().get());
    let angle = crate::math::atan2(bx.cross(ax).dot(axis), bx.dot(ax));
    Some(Turn {
        shift: Vector2::new(angle, along),
        reversed,
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

    /// A cylinder's rim: its own fin's pcurve and the exact one give the
    /// same parameters at the same edge parameter, so `coincident` may take
    /// either.
    #[test]
    fn own_and_exact_pcurves_agree_on_a_cylinders_rim() {
        let (b, _) = primitive::cylinder(OpKey(1), 1.0, 2.0, &Rigid::IDENTITY).unwrap();
        let side = faces_of(&b)
            .into_iter()
            .find(|&f| matches!(b.surfaces[b.faces[f].surface], Surface::Cylinder(_)))
            .unwrap();
        let surface = &b.surfaces[b.faces[side].surface];
        let mut seen = 0;
        for e in edges((&b, side)) {
            let edge = &b.edges[e];
            let curve = &b.curves[edge.curve];
            let (oc, om) = own_pcurve(&b, side, e, Some(Turn::NONE)).expect("own");
            let (xc, xm) =
                crate::pcurve::exact(surface, curve, edge.interval, edge.tolerance.get())
                    .expect("exact");
            for k in 0..3 {
                let t = edge.interval[0] + (edge.interval[1] - edge.interval[0]) * k as f64 / 2.0;
                let p = oc.point(om[0] + om[1] * t);
                let q = xc.point(xm[0] + xm[1] * t);
                let at = |p: Point2| surface.point(p.x, p.y);
                assert!(at(p).distance(curve.point(t)) <= 1e-9, "{t}");
                assert!(at(p).distance(at(q)) <= 1e-9, "{t}");
            }
            seen += 1;
        }
        assert!(seen >= 2);
    }

    #[test]
    fn a_turned_copy_maps_parameters_by_the_turn() {
        use crate::geom::{Frame, Point3, Torus};
        let frame = |x: Vector3| {
            Frame::new(Point3::new(0.1, 0.2, 0.3), Vector3::new(0.0, 0.0, 1.0), x).unwrap()
        };
        let own = Surface::Torus(Torus::new(frame(Vector3::new(1.0, 0.0, 0.0)), 2.0, 0.5).unwrap());
        let onto =
            Surface::Torus(Torus::new(frame(Vector3::new(-0.6, 0.8, 0.0)), 2.0, 0.5).unwrap());
        let d = turned(&own, &onto).unwrap();
        assert!(!d.reversed);
        for (u, v) in [(0.0, 0.0), (1.0, 2.0), (4.0, -1.0)] {
            let p = own.point(u, v);
            let q = d.apply(Point2::new(u, v));
            let q = onto.point(q.x, q.y);
            assert!(p.distance(q) <= 1e-12, "({u}, {v})");
        }
        assert_eq!(turned(&own, &own), Some(Turn::NONE));
        // Upside down, or another kind, it isn't a turn.
        let flipped = Surface::Torus(
            Torus::new(
                Frame::new(
                    Point3::new(0.1, 0.2, 0.3),
                    Vector3::new(0.0, 0.0, -1.0),
                    Vector3::new(1.0, 0.0, 0.0),
                )
                .unwrap(),
                2.0,
                0.5,
            )
            .unwrap(),
        );
        assert_eq!(turned(&own, &flipped), None);
    }

    /// #240: a gear's shaft and its bore, two solids of one file, on
    /// copies of one cylinder whose frames sit 2.4 mm apart along the axis
    /// and whose axes differ by 1.8e-32 in y. The copy's parameters map by
    /// the turn and the offset along the axis; a cone's don't (its radius
    /// changes along it), nor a cylinder's off the axis.
    /// #584: Voron's Z Belt Drive A holds a 29 mm cylinder in each of its
    /// two solids, framed opposite ways along one axis (the file's own
    /// frames, its z within 5e-13 of +x and of -x, its origins 1.2 nm
    /// apart). A point's parameters on one go to the other's by a half
    /// turn of the parameter plane, and its pcurves are carried so.
    #[test]
    fn a_cylinders_copy_framed_the_other_way_maps_parameters_by_a_half_turn() {
        use crate::geom::{Cylinder, Frame, Line2, Point3};
        let frame = |o: (f64, f64, f64), z: (f64, f64, f64), x: (f64, f64, f64)| {
            Frame::new(
                Point3::new(o.0, o.1, o.2),
                Vector3::new(z.0, z.1, z.2),
                Vector3::new(x.0, x.1, x.2),
            )
            .unwrap()
        };
        let cyl = |f: Frame| Surface::Cylinder(Cylinder::new(f, 0.029).unwrap());
        let own = cyl(frame(
            (0.0435585471878337, 0.00084712953816565, 0.144052266354309),
            (-1.0, 7.73833776444511e-15, -5.40445820287305e-13),
            (-7.73833776444472e-15, -1.0, -6.76774011079913e-16),
        ));
        let onto = cyl(frame(
            (0.043558545946657604, 0.00084712953816565, 0.144052266354309),
            (1.0, -7.73785684805121e-15, 5.27255052488395e-13),
            (-7.73785684805081e-15, -1.0, -6.76774011734469e-16),
        ));
        let d = turned(&own, &onto).unwrap();
        assert!(d.reversed);
        for (u, v) in [(0.0, 0.0), (1.0, -0.01), (4.0, -0.018), (6.0, -0.005)] {
            let p = own.point(u, v);
            let q = d.apply(Point2::new(u, v));
            let q = onto.point(q.x, q.y);
            assert!(p.distance(q) <= 1e-12, "({u}, {v}): {}", p.distance(q));
        }
        // A pcurve carried so traces the same points.
        let line =
            Curve2::Line2(Line2::new(Point2::new(0.5, -0.002), Vector2::new(0.6, -0.8)).unwrap());
        let moved = d.carry(&line).unwrap();
        for t in [0.0, 0.004, 0.01] {
            let (a, b) = (line.point(t), moved.point(t));
            assert!(own.point(a.x, a.y).distance(onto.point(b.x, b.y)) <= 1e-12);
        }
        // Back the other way it is a half turn too.
        assert!(turned(&onto, &own).unwrap().reversed);
    }

    #[test]
    fn a_cylinders_copy_along_its_axis_maps_parameters_by_the_offset() {
        use crate::geom::{Cone, Cylinder, Frame, Point3};
        let frame = |o: (f64, f64, f64), z: (f64, f64, f64), x: (f64, f64, f64)| {
            Frame::new(
                Point3::new(o.0, o.1, o.2),
                Vector3::new(z.0, z.1, z.2),
                Vector3::new(x.0, x.1, x.2),
            )
            .unwrap()
        };
        let cyl = |f: Frame| Surface::Cylinder(Cylinder::new(f, 0.0025).unwrap());
        let own = cyl(frame(
            (0.00230133859452508, 0.00358508586504737, 0.0),
            (1.0, 1.810374147724e-32, 0.0),
            (0.0, 0.0, -1.0),
        ));
        let onto = cyl(frame(
            (-9.86614054749133e-5, 0.00358508586504737, 0.0),
            (1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
        ));
        let d = turned(&own, &onto).unwrap();
        assert!(!d.reversed);
        for (u, v) in [(0.0, 0.0), (1.0, 2e-3), (4.0, -1e-3)] {
            let p = own.point(u, v);
            let q = d.apply(Point2::new(u, v));
            let q = onto.point(q.x, q.y);
            assert!(p.distance(q) <= 1e-15, "({u}, {v}): {}", p.distance(q));
        }
        let off = cyl(frame((0.0, 0.0036, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)));
        assert_eq!(turned(&own, &off), None);
        let cone = |o| {
            Surface::Cone(
                Cone::new(frame(o, (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)), 0.0025, 0.3).unwrap(),
            )
        };
        assert_eq!(
            turned(&cone((0.0, 0.0, 0.0)), &cone((0.001, 0.0, 0.0))),
            None
        );
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
    fn face_face_in_halts_and_the_public_one_does_not() {
        use crate::work::Work;
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
        let (fa, fb) = ((&rod, side), (&plate, top));
        let w = Work::unlimited();
        let m = Meter::new(&w);
        let metered = face_face_in(fa, fb, &m).unwrap();
        assert!(m.spent() > 0);
        assert_eq!(metered, face_face(fa, fb).unwrap());
        let w = Work::unlimited().with_budget(0);
        let m = Meter::new(&w);
        assert!(face_face_in(fa, fb, &m).is_err());
        assert!(m.halted());
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

    /// A region less cubes round points is covered by the boxes
    /// `region_outside` gives, which don't meet the cubes' insides and
    /// add up to the region's volume less the cubes' parts inside it.
    #[test]
    fn a_region_less_cubes_is_its_slabs() {
        let region = Box3::new(Point3::new(0.0, 0.0, 0.0), Point3::new(4.0, 3.0, 2.0));
        let points = [Point3::new(1.0, 1.0, 1.0), Point3::new(4.0, 3.0, 2.0)];
        let boxes = region_outside(&region, &points, 0.5);
        let volume = |b: &Box3| {
            let d = b.max() - b.min();
            d.x * d.y * d.z
        };
        let total: f64 = boxes.iter().map(volume).sum();
        // 24 less a whole cube (1) and the corner cube's eighth (0.125).
        assert!((total - (24.0 - 1.0 - 0.125)).abs() < 1e-12, "{total}");
        for b in &boxes {
            for p in &points {
                let inside = Box3::new(
                    Point3::new(p.x - 0.499, p.y - 0.499, p.z - 0.499),
                    Point3::new(p.x + 0.499, p.y + 0.499, p.z + 0.499),
                );
                assert!(!b.intersects(&inside), "{b:?}");
            }
        }
    }
}
