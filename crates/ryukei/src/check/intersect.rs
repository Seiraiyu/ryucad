//! The self-intersection check.
//!
//! Every pair of faces whose boxes overlap ([`BoxTree`] over
//! [`face_box`], grown by the faces' tolerances) is intersected
//! ([`face_face`]), in parallel. Faces may meet only along an edge they
//! both use or at a vertex they both have: a stretch where they meet must
//! lie on one shared edge, and a touching point on one shared vertex or
//! edge; by the touch rule, a stretch within 100 tolerances of a shared
//! vertex or of a shared edge along all its length is benign too (two
//! fits of one intersection curve may lie that far apart where the faces
//! meet at a shallow angle). Out of a vertex the faces share, a stretch
//! inside at most one of them (an edge of one lying on the other) is
//! benign for as long as surfaces meeting tangentially stay within that
//! reach, `sqrt(2 × size × reach)`: the tip of a tangent wedge. Anything
//! else is
//! [`Fault::FacesCross`]; faces on one surface
//! whose regions overlap in an area are [`Fault::FacesOverlap`]; a pair
//! that can't be intersected is [`Fault::CannotCheck`].
//!
//! Faces tangent along an edge they share (a fillet and its support, two
//! blends across a section) can't be traced there. They are certified
//! apart outside a strip round their shared edges as wide as that reach
//! allows a tangent wedge, `sqrt(2 × size × reach)` ([`face_beside`]), or
//! reported as [`Fault::CannotCheck`].
//!
//! [`check_overlaps`] is the part of the check that finds
//! [`Fault::FacesOverlap`] alone: only pairs on coincident surfaces are
//! intersected, which is cheap enough to run on every import.
//!
//! Faces on one surface whose boundaries lie on each other's all round
//! (two solids stacked face to face) leave no rim inside either; for
//! both checks a point inside one (the first of a grid over its pcurves'
//! parameter box that the face holds) inside the other is an overlap,
//! since on a closed surface two faces with one boundary may be each
//! other's complement.

use super::Fault;
use crate::geom::Surface;
use crate::intersect::broad::BoxTree;
use crate::intersect::coincide::coincident;
use crate::intersect::{
    BESIDE_CELLS, FaceMeet, Place, face_beside, face_box, face_face_in, face_face_within,
    face_off_ball, faces_apart, point_in_face,
};
use crate::topo::{Body, EdgeId, FaceId, VertexId};
use crate::work::{Meter, Work};

/// Faces that cross, overlap or can't be checked, by pair in face order.
/// For a body that passes [`super::check`].
pub fn check_intersections(body: &Body) -> Vec<Fault> {
    match check_intersections_with(body, &Work::unlimited()) {
        Ok(faults) => faults,
        Err(_) => unreachable!("no work, no cancel"),
    }
}

/// [`check_intersections`] under `work`: `Cancelled` or `BudgetExceeded`
/// instead of the faults when it stops.
pub fn check_intersections_with(body: &Body, work: &Work) -> Result<Vec<Fault>, crate::Error> {
    crate::work::run(work, |m| Ok(check_intersections_in(body, m)))
}

/// [`check_intersections`] with a meter, for the operations that check
/// what they build (sweeps along paths, lofts, offsets).
pub(crate) fn check_intersections_in(body: &Body, m: &Meter) -> Vec<Fault> {
    check_pairs_in(body, m, |_, _| true)
}

/// [`check_intersections_in`] over only the pairs of faces `wanted`
/// says may meet wrongly (sheet metal's unfold and refold leave out
/// pairs that moved rigidly together).
pub(crate) fn check_pairs_in(
    body: &Body,
    m: &Meter,
    wanted: impl Fn(FaceId, FaceId) -> bool,
) -> Vec<Fault> {
    let faces: Vec<FaceId> = body.faces().collect();
    let boxes: Vec<_> = faces
        .iter()
        .enumerate()
        .map(|(i, &f)| (face_box(body, f).grow(slack(body, f)), i))
        .collect();
    let pairs: Vec<(usize, usize)> = BoxTree::new(boxes)
        .self_pairs()
        .into_iter()
        .filter(|&(i, j)| wanted(faces[i], faces[j]))
        .collect();
    m.par_map(&pairs, |m, &(i, j)| pair(body, faces[i], faces[j], m))
        .into_iter()
        .flatten()
        .collect()
}

/// Faces on coincident surfaces that overlap in an area, by pair in face
/// order: the [`Fault::FacesOverlap`] faults [`check_intersections`]
/// finds, intersecting no other pair. For a body that passes
/// [`super::check_topology`].
pub fn check_overlaps(body: &Body) -> Vec<Fault> {
    // unmetered: the public function takes no work
    check_overlaps_in(body, &Meter::free())
}

/// [`check_overlaps`] charging `m`, for healing.
pub(crate) fn check_overlaps_in(body: &Body, m: &Meter) -> Vec<Fault> {
    let faces: Vec<FaceId> = body.faces().collect();
    let boxes: Vec<_> = faces
        .iter()
        .enumerate()
        .map(|(i, &f)| (face_box(body, f).grow(slack(body, f)), i))
        .collect();
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    let pairs: Vec<(usize, usize)> = BoxTree::new(boxes)
        .self_pairs()
        .into_iter()
        .filter(|&(i, j)| coincident(surface(faces[i]), surface(faces[j])).is_some())
        .collect();
    m.par_map(&pairs, |m, &(i, j)| {
        let (a, b) = (faces[i], faces[j]);
        match face_face_in((body, a), (body, b), m) {
            Ok(meet @ FaceMeet::Coincident { .. }) => overlap(body, a, b, &meet),
            _ => None,
        }
    })
    .into_iter()
    .flatten()
    .collect()
}

/// Whether two faces on one surface overlap in an area: a rim inside the
/// other face, or neither has a boundary, or a point inside one lies
/// inside the other.
fn overlap(body: &Body, a: FaceId, b: FaceId, meet: &FaceMeet) -> Option<Fault> {
    fault(a, b, meet).or_else(|| {
        (inside_other(body, a, b) || inside_other(body, b, a))
            .then_some(Fault::FacesOverlap { a, b })
    })
}

/// Points of a grid in each direction over a face's parameter box when
/// looking for a point inside it.
const PROBES: usize = 16;

/// Whether a point inside face `a` (the first of a grid over its
/// pcurves' parameter box that it holds) lies inside face `b`, on a
/// coincident surface.
fn inside_other(body: &Body, a: FaceId, b: FaceId) -> bool {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for l in body.face_loops(a) {
        for fin in body.loop_fins(l) {
            let e = &body.edges[body.fins[fin].edge];
            for k in 0..=16 {
                let t = e.interval[0] + (e.interval[1] - e.interval[0]) * k as f64 / 16.0;
                if let Some(q) = body.fin_uv(fin, t) {
                    lo = [lo[0].min(q.x), lo[1].min(q.y)];
                    hi = [hi[0].max(q.x), hi[1].max(q.y)];
                }
            }
        }
    }
    if !(lo[0] < hi[0] && lo[1] < hi[1]) {
        return false;
    }
    let (sa, sb) = (
        &body.surfaces[body.faces[a].surface],
        &body.surfaces[body.faces[b].surface],
    );
    for i in 0..PROBES {
        for j in 0..PROBES {
            let uv = crate::geom::Point2::new(
                lo[0] + (hi[0] - lo[0]) * (i as f64 + 0.5) / PROBES as f64,
                lo[1] + (hi[1] - lo[1]) * (j as f64 + 0.5) / PROBES as f64,
            );
            if point_in_face(body, a, uv) != Ok(Place::Inside) {
                continue;
            }
            let (u, v) = sb.project(sa.point(uv.x, uv.y)).param;
            return point_in_face(body, b, crate::geom::Point2::new(u, v)) == Ok(Place::Inside);
        }
    }
    false
}

/// The fault two faces of a body make, if any.
fn pair(body: &Body, a: FaceId, b: FaceId, m: &Meter) -> Option<Fault> {
    // Faces tangent along an edge they share can't be traced there: they
    // are certified apart beside it, and only traced if that fails.
    let shared = shared_edges(body, a, b);
    // Faces sharing no edge are first certified apart over their own
    // parameter boxes under a small budget: most pairs whose boxes overlap
    // are apart, and a trace that can't settle them fails only after its
    // whole cap (ctc_05: 99.6 s for one pair that `faces_apart` then
    // certified in a unit, #302). A pair certified apart has no stretch
    // inside either face, so the verdict is the trace's.
    if shared.is_empty() && faces_apart((body, a), (body, b), EARLY_APART, m) {
        return None;
    }
    // Faces sharing vertices but no edge are first tried apart outside the
    // balls round those vertices under a small part limit, before any
    // trace: a trace near such a vertex can run for minutes and fail
    // (ctc_02's AP242 edition: a NURBS against a cone, 450 s, then this
    // in a second). Apart outside the balls, no crossing lies outside
    // them, and a trace's touches inside them are pruned: the verdict is
    // the trace's.
    let touching = shared.is_empty() && !shared_vertices(body, a, b).is_empty();
    if touching && apart_at_vertices(body, a, b, EARLY_VERTICES, m) {
        return None;
    }
    // One face running along the other's surface on its own edges is
    // certified apart cheaply before any trace (#302's CP_Radial: 24 s of
    // failed traces, then this in a few thousand units). Under the early
    // part limit, so pairs it can't settle there cost little: the full
    // search runs after the traces (below).
    if touching && apart_by_own_edges(body, a, b, EARLY_VERTICES, m) {
        return None;
    }
    // Faces tangent along an edge they share are tried apart beside it
    // under the ladder's smaller limits, then traced under
    // [`TANGENT_TRACE_FIRST`] units, then apart under its largest limit and
    // across a section plane: ctc_05's fillet torus and the cylinders it
    // runs on failed every search apart, 140,288 units and 10 s a pair on
    // a Xeon E5-2650, and then the trace settled each in under a hundred
    // (#302). A pair settled either way is settled as before.
    let tangent = tangent(body, a, b, &shared);
    let mut traced = None;
    if tangent {
        if apart_beside_by(body, a, b, &shared, &LADDER[..2], false, m) {
            return None;
        }
        match m.capped_by(TANGENT_TRACE_FIRST, || {
            face_face_in((body, a), (body, b), m)
        }) {
            Ok(meet) => traced = Some(meet),
            Err(_) => {
                if apart_beside_by(body, a, b, &shared, &LADDER[2..], true, m) {
                    return None;
                }
            }
        }
    }
    // Faces touching at a vertex they share touch at a corner of both
    // boxes, where the region's sides cut the trace: again with the
    // region reaching an eighth of the smaller face's size past it.
    //
    // Faces sharing vertices but no edge are first tried apart outside the
    // balls round those vertices, which is cheap, before the wider trace,
    // which can run for minutes and fail (ctc_02's AP242 edition: a NURBS
    // and a cone, 284 s, then `apart_at_vertices` in 4 s). Apart outside
    // the balls, no crossing lies outside them, and the trace's touches
    // inside them are pruned: the verdict is the trace's.
    //
    // The wider trace is tried first under [`WITHIN_FIRST`] units: most
    // such pairs (a fillet torus and the cylinder it rounds, touching at
    // the corners they share) settle there in a few thousand, where the
    // full search apart fails after its whole budget (NIST FTC 10: ten
    // pairs, 65,536 units and 2.5 s each on a Xeon E5-2650, then the trace
    // in 19,000 units, #303). Either succeeding settles the pair, as
    // before; only the order changed, and the trace that ran minutes runs
    // only after the search apart has failed.
    let mut vertices_tried = false;
    let meet = traced
        .map_or_else(|| face_face_in((body, a), (body, b), m), Ok)
        .or_else(|e| {
            if shared_vertices(body, a, b).is_empty() {
                return Err(e);
            }
            let size = |f: FaceId| {
                let x = face_box(body, f);
                (x.max() - x.min()).length()
            };
            let within = || face_face_within((body, a), (body, b), size(a).min(size(b)) / 8.0, m);
            if shared.is_empty() {
                vertices_tried = true;
                if let Ok(meet) = m.capped_by(WITHIN_FIRST, within) {
                    return Ok(meet);
                }
                if apart_at_vertices(body, a, b, BESIDE_CELLS, m) {
                    return Ok(FaceMeet::apart());
                }
            }
            // The grown trace's last try is capped too: a pipe and a torus
            // touching where two offset fills meet (#416) traced 1.5M units
            // more, after its capped try and the search apart had failed,
            // and a KiCad disc capacitor's cylinder tangent to a revolution
            // 834,000 (13 s on a Xeon E5-2650), both to end `CannotCheck`
            // all the same. A pair it can't settle under the cap can't be
            // checked, the conservative verdict.
            m.capped_by(WITHIN_LAST, within)
        });
    match meet {
        Ok(meet @ FaceMeet::Coincident { .. }) => overlap(body, a, b, &meet),
        Ok(meet) => fault(a, b, &prune(body, a, b, meet)),
        // Faces crossing at an edge they share where it can't be traced (a
        // pipe cut off at its spine's end), or touching at vertices they
        // share, apart beside them all the same.
        Err(_) if !tangent && !shared.is_empty() && apart_beside(body, a, b, &shared, m) => None,
        Err(_)
            if shared.is_empty()
                && !vertices_tried
                && apart_at_vertices(body, a, b, BESIDE_CELLS, m) =>
        {
            None
        }
        // Faces sharing vertices whose every trace failed, one running
        // along the other's surface on edges of its own (#302).
        Err(_) if shared.is_empty() && apart_by_own_edges(body, a, b, BESIDE_CELLS, m) => None,
        // Else traced outside cubes round the vertices they share, within
        // the touch reach, where a near-tangency stops the trace (#264).
        Err(_) if shared.is_empty() => match outside_vertices(body, a, b, m) {
            Some(meet) => fault(a, b, &prune(body, a, b, meet)),
            None => Some(Fault::CannotCheck { a, b }),
        },
        Err(_) => Some(Fault::CannotCheck { a, b }),
    }
}

/// The edges of `f` whose curve lies within `reach` of `other` at a
/// quarter, a half and three quarters along: where the face may run along
/// the other's surface. Which edges are taken only chooses the strips;
/// [`apart_by_own_edges`] certifies them.
fn edges_on(body: &Body, f: FaceId, other: &Surface, reach: f64) -> Vec<EdgeId> {
    let mut out: Vec<EdgeId> = body
        .face_loops(f)
        .flat_map(|l| {
            body.loop_fins(l)
                .map(|fin| body.fins[fin].edge)
                .collect::<Vec<_>>()
        })
        .filter(|&e| {
            let x = &body.edges[e];
            let c = &body.curves[x.curve];
            (1..4).all(|k| {
                let t = x.interval[0] + (x.interval[1] - x.interval[0]) * f64::from(k) / 4.0;
                other.distance(c.point(t)) <= reach
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether face `f` keeps more than `strip` from every point of `edges`'
/// curves outside balls of radius `ball` round `points`: each edge's
/// range halved until its piece's box ([`crate::geom::Curve::piece_bounds`])
/// lies in a ball, or the ball round the box grown by `strip` and the
/// edge's tolerance is certified clear of the face ([`face_off_ball`]), 20
/// halvings at most. Certified, since each box holds its piece of the 3D
/// curve and the faces' pcurves lie within the edge's tolerance of it.
/// Each piece is charged a unit; a halt, a cap, or a face with a fin that
/// has no pcurve (which [`face_off_ball`] can't search) answers `false`.
fn face_off_edges(
    body: &Body,
    f: FaceId,
    edges: &[EdgeId],
    points: &[crate::geom::Point3],
    strip: f64,
    ball: f64,
    m: &Meter,
) -> bool {
    let unbounded = body
        .face_loops(f)
        .any(|l| body.loop_fins(l).any(|fin| body.fins[fin].pcurve.is_none()));
    if unbounded {
        return false;
    }
    let middle = |bx: &crate::geom::Box3| bx.min() + (bx.max() - bx.min()) * 0.5;
    let half = |bx: &crate::geom::Box3| (bx.max() - bx.min()).length() / 2.0;
    for &e in edges {
        let x = &body.edges[e];
        let curve = &body.curves[x.curve];
        let grow = strip + x.tolerance.get();
        let mut stack = vec![(x.interval, 0u32)];
        while let Some((iv, depth)) = stack.pop() {
            if m.charge(1).is_err() {
                return false;
            }
            let bx = curve.piece_bounds(iv);
            let (c, r) = (middle(&bx), half(&bx));
            if points.iter().any(|p| c.distance(*p) + r <= ball) {
                continue;
            }
            match face_off_ball(body, f, c, r + grow, m) {
                Ok(true) => continue,
                Err(_) if m.halted() => return false,
                _ => {}
            }
            if depth >= 20 {
                return false;
            }
            let mid = (iv[0] + iv[1]) / 2.0;
            stack.push(([iv[0], mid], depth + 1));
            stack.push(([mid, iv[1]], depth + 1));
        }
    }
    true
}

/// Whether faces sharing vertices but no edge, one of which runs along
/// the other's surface on edges of its own, are certified to meet only
/// near the vertices: a can's rim tangent to its wall along the rim's
/// edge with a third face, the wall's own edge running on from the vertex
/// the other way (#302's CP_Radial, design 444). That face keeps off the
/// other's surface outside strips round those edges and balls round the
/// vertices ([`face_beside`], under `cells` parts), and the other face
/// keeps off the strips outside balls of two strips ([`face_off_edges`]).
/// The faces' common points then lie within three strips of a vertex,
/// where tangent wedges touch.
fn apart_by_own_edges(body: &Body, a: FaceId, b: FaceId, cells: usize, m: &Meter) -> bool {
    let vertices = shared_vertices(body, a, b);
    if vertices.is_empty() {
        return false;
    }
    let reach = TOUCH * slack(body, a).max(slack(body, b));
    let size = {
        let bx = face_box(body, a).union(&face_box(body, b));
        (bx.max() - bx.min()).length()
    };
    let strip = (2.0 * size * reach).sqrt();
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    let points: Vec<_> = vertices.iter().map(|&v| body.vertices[v].point).collect();
    [(a, b), (b, a)].into_iter().any(|(x, y)| {
        let ex = edges_on(body, x, surface(y), reach);
        !ex.is_empty()
            && face_beside(body, x, surface(y), &ex, &vertices, strip, cells, m).is_ok()
            && face_off_edges(body, y, &ex, &points, strip, 2.0 * strip, m)
    })
}

/// How faces sharing vertices but no edge meet outside cubes round those
/// vertices, half the touch reach wide, so a cube's corner is within
/// `0.87` of the reach and whatever the faces share inside is a touch by
/// [`TOUCH`] (the rule [`prune`] applies): the region less the cubes,
/// traced box by box, each under [`WITHIN_LAST`] units, so no cell lies
/// on a near-tangency at a vertex (#264: planes tangent to fillet tori at
/// saddles of their tubes, at the vertex each pair shares). `None` where
/// any box's trace fails or meets coincident faces.
fn outside_vertices(body: &Body, a: FaceId, b: FaceId, m: &Meter) -> Option<FaceMeet> {
    let shared = shared_vertices(body, a, b);
    if shared.is_empty() {
        return None;
    }
    let tol = slack(body, a).max(slack(body, b));
    let half = 0.5 * TOUCH * tol;
    let (x, y) = (face_box(body, a).grow(tol), face_box(body, b).grow(tol));
    if !x.intersects(&y) {
        return Some(FaceMeet::apart());
    }
    let region = crate::geom::Box3::new(x.min().max(y.min()), x.max().min(y.max()));
    let points: Vec<_> = shared.iter().map(|&v| body.vertices[v].point).collect();
    let (mut stretches, mut contacts) = (Vec::new(), Vec::new());
    for bx in crate::intersect::region_outside(&region, &points, half) {
        let traced = m.capped_by(WITHIN_LAST, || {
            crate::intersect::face_face_over((body, a), (body, b), &bx, m)
        });
        let Ok(FaceMeet::Meet {
            stretches: s,
            contacts: c,
        }) = traced
        else {
            return None;
        };
        stretches.extend(s);
        contacts.extend(c);
    }
    Some(FaceMeet::Meet {
        stretches,
        contacts,
    })
}

/// The edges two faces both use, each once.
fn shared_edges(body: &Body, a: FaceId, b: FaceId) -> Vec<EdgeId> {
    let edges = |f: FaceId| -> Vec<EdgeId> {
        body.face_loops(f)
            .flat_map(|l| {
                body.loop_fins(l)
                    .map(|fin| body.fins[fin].edge)
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let eb = edges(b);
    let mut out: Vec<EdgeId> = edges(a).into_iter().filter(|e| eb.contains(e)).collect();
    out.sort();
    out.dedup();
    out
}

/// How far apart two faces' normals may be at an edge they share for them
/// to be certified apart beside it before being traced: a hundredth of a
/// radian. Surfaces meeting at a crease that slight (a sweep twisted
/// across a joint where its path's curvature jumps) are as hard to trace
/// as tangent ones, and as easy to keep apart.
const NEARLY_TANGENT: f64 = 1e-2;

/// Whether two faces meet tangentially, or nearly, along an edge they
/// share: their surfaces' normals are parallel, within
/// [`NEARLY_TANGENT`], at the middle of one of them. This only chooses how
/// the pair is checked; both ways are certified.
fn tangent(body: &Body, a: FaceId, b: FaceId, shared: &[EdgeId]) -> bool {
    shared.iter().any(|&e| {
        let x = &body.edges[e];
        let p = body.curves[x.curve].point((x.interval[0] + x.interval[1]) / 2.0);
        let normal = |f: FaceId| {
            let s = &body.surfaces[body.faces[f].surface];
            let (u, v) = s.project(p).param;
            s.normal(u, v)
        };
        match (normal(a), normal(b)) {
            (Some(na), Some(nb)) => na.get().cross(nb.get()).length() <= NEARLY_TANGENT,
            _ => false,
        }
    })
}

/// Whether two faces tangent along the edges they share are certified to
/// meet nowhere else, outside a strip round those edges as wide as
/// surfaces meeting tangentially stay within the touch rule's reach of
/// each other, `sqrt(2 × size × reach)`: one face keeps off the other's
/// surface there ([`face_beside`]), as a fillet does off the face it
/// rolls on, or the two lie on either side of the plane of a planar edge
/// they share, as blends do across a section.
fn apart_beside(body: &Body, a: FaceId, b: FaceId, shared: &[EdgeId], m: &Meter) -> bool {
    apart_beside_by(body, a, b, shared, &LADDER, true, m)
}

/// [`apart_beside`] under the part limits `rungs` only, and the section
/// plane only when `plane` says.
fn apart_beside_by(
    body: &Body,
    a: FaceId,
    b: FaceId,
    shared: &[EdgeId],
    rungs: &[usize],
    plane: bool,
    m: &Meter,
) -> bool {
    let reach = TOUCH * slack(body, a).max(slack(body, b));
    let size = {
        let bx = face_box(body, a).union(&face_box(body, b));
        (bx.max() - bx.min()).length()
    };
    let strip = (2.0 * size * reach).sqrt();
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    let side = |x: FaceId, of: &Surface, cells: usize| {
        face_beside(body, x, of, shared, &[], strip, cells, m).ok()
    };
    // The more curved face first: a fillet keeps off the surface of the
    // face it rolls on, while that face may well run on past the fillet's
    // surface.
    let (x, y) = if rank(surface(a)) >= rank(surface(b)) {
        (a, b)
    } else {
        (b, a)
    };
    // Faces on one surface can't keep off each other's surface: every
    // part of one lies on the other, so each search would only exhaust its
    // budget (issue #160: seconds a pair for a straight sweep's sides
    // across its joints). Only a section plane can part them; failing
    // that, they are intersected as coincident faces.
    let same = coincident(surface(a), surface(b)).is_some();
    // Each face's search under a growing part limit, the other's between:
    // the face that keeps off the other's surface usually does in a few
    // parts, while the other runs on and exhausts any limit (#301). A
    // search that ends within a limit gives what it gives under a larger
    // one, so the verdict is the full searches'.
    if !same {
        for &cells in rungs {
            if side(x, surface(y), cells).is_some() || side(y, surface(x), cells).is_some() {
                return true;
            }
        }
    }
    if !plane {
        return false;
    }
    let Some(plane) = section_plane(body, a, b, shared) else {
        return false;
    };
    // Every edge of either face on the section's curve lies on its plane,
    // not only the shared ones: a lead's end circle cut where its bend's
    // circle crosses the axis gives its lune two pieces and its lens the
    // third (#316). Each gets the shared edges' strip, by the same rule.
    let curves: Vec<&crate::geom::Curve> = shared
        .iter()
        .map(|&e| &body.curves[body.edges[e].curve])
        .collect();
    let mut rails: Vec<EdgeId> = [a, b]
        .into_iter()
        .flat_map(|f| body.face_loops(f).flat_map(|l| body.loop_fins(l)))
        .map(|fin| body.fins[fin].edge)
        .filter(|&e| curves.contains(&&body.curves[body.edges[e].curve]))
        .collect();
    rails.sort();
    rails.dedup();
    let on_plane =
        |x: FaceId| face_beside(body, x, &plane, &rails, &[], strip, BESIDE_CELLS, m).ok();
    match (on_plane(a), on_plane(b)) {
        (Some(sa), Some(sb)) => {
            let one = |s: [bool; 2]| s != [true, true];
            one(sa)
                && one(sb)
                && (sa[0] && sb[1] || sa[1] && sb[0] || sa == [false; 2] || sb == [false; 2])
        }
        _ => false,
    }
}

/// The vertices two faces both have, each once.
fn shared_vertices(body: &Body, a: FaceId, b: FaceId) -> Vec<VertexId> {
    let vertices = |f: FaceId| -> Vec<VertexId> {
        body.face_loops(f)
            .flat_map(|l| {
                body.loop_fins(l)
                    .filter_map(|fin| body.fin_start(fin))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let vb = vertices(b);
    let mut shared: Vec<VertexId> = vertices(a).into_iter().filter(|v| vb.contains(v)).collect();
    shared.sort();
    shared.dedup();
    shared
}

/// Whether two faces sharing vertices but no edge, which may touch at
/// those vertices as tangent wedges do, are certified apart outside the
/// balls round them as wide as the reach allows a tangent wedge
/// ([`face_beside`]), one face keeping off the other's surface there.
fn apart_at_vertices(body: &Body, a: FaceId, b: FaceId, cells: usize, m: &Meter) -> bool {
    let shared = shared_vertices(body, a, b);
    if shared.is_empty() {
        return false;
    }
    let reach = TOUCH * slack(body, a).max(slack(body, b));
    let size = {
        let bx = face_box(body, a).union(&face_box(body, b));
        (bx.max() - bx.min()).length()
    };
    let strip = (2.0 * size * reach).sqrt();
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    face_beside(body, a, surface(b), &[], &shared, strip, cells, m).is_ok()
        || face_beside(body, b, surface(a), &[], &shared, strip, cells, m).is_ok()
}

/// How curved a surface is, by kind: a plane least, a pipe most.
fn rank(s: &Surface) -> u8 {
    match s {
        Surface::Plane(_) => 0,
        Surface::Cylinder(_) | Surface::Cone(_) => 1,
        Surface::Sphere(_) => 2,
        Surface::Torus(_) => 3,
        _ => 4,
    }
}

/// The plane of a lone shared edge that is planar: a circle's or an
/// ellipse's, or the plane through three of a NURBS's control points
/// (spread as far apart as they go) when every control point lies within
/// the edge's tolerance of it (so the curve does, inside their hull).
/// For a straight edge, the plane through it square to the faces' common
/// tangent plane at its middle (neighbouring legs of a sweep tangent
/// along a section, #260): faces tangent along the line lie either side
/// of it where each leaves the line its own way. The plane only
/// separates the faces, both certified there by `face_beside`, so a
/// wrong plane only fails; the strip round the edge covers how far the
/// edge lies from it.
fn section_plane(body: &Body, a: FaceId, b: FaceId, shared: &[EdgeId]) -> Option<Surface> {
    // Several shared edges on one curve are pieces of one section (a
    // circle the faces meet along cut at vertices both have: a lead's end
    // and the lune of its bend, split where the bend's circle crosses its
    // axis, issue #316): its plane, as for one edge.
    let (e, rest) = shared.split_first()?;
    let curve = &body.curves[body.edges[*e].curve];
    if rest
        .iter()
        .any(|x| &body.curves[body.edges[*x].curve] != curve)
    {
        return lines_plane(body, shared);
    }
    let x = &body.edges[*e];
    let surface = |f: FaceId| &body.surfaces[body.faces[f].surface];
    crate::intersect::section_plane(curve, x.interval, x.tolerance.get(), surface(a), surface(b))
}

/// The plane of several shared straight edges on different lines (a
/// cone's two halves split along two generators in one plane: NIST CTC
/// 02's AP242 edition, #264): through the first and the end of the others
/// farthest from its line, when every edge's ends lie within its
/// tolerance of it (so the edge does). `None` otherwise. It only
/// separates faces, each certified on its side by `face_beside`.
fn lines_plane(body: &Body, shared: &[EdgeId]) -> Option<Surface> {
    use crate::geom::{Curve, Frame, Plane, Point3};
    if shared
        .iter()
        .any(|&e| !matches!(body.curves[body.edges[e].curve], Curve::Line(_)))
    {
        return None;
    }
    let ends = |e: EdgeId| {
        let x = &body.edges[e];
        let c = &body.curves[x.curve];
        [c.point(x.interval[0]), c.point(x.interval[1])]
    };
    let [p0, p1] = ends(shared[0]);
    let d = p1 - p0;
    let off_line = |p: &Point3| (*p - p0).cross(d).length();
    let far = shared[1..]
        .iter()
        .flat_map(|&e| ends(e))
        .max_by(|p, q| off_line(p).total_cmp(&off_line(q)))?;
    let frame = Frame::new(p0, d.cross(far - p0), d).ok()?;
    let off = |p: Point3| (p - p0).dot(frame.z().get()).abs();
    shared
        .iter()
        .all(|&e| {
            ends(e)
                .iter()
                .all(|&p| off(p) <= body.edges[e].tolerance.get())
        })
        .then_some(Surface::Plane(Plane::new(frame)))
}

/// The largest tolerance on a face's boundary.
fn slack(body: &Body, f: FaceId) -> f64 {
    let mut t = crate::tol::LINEAR_RESOLUTION;
    for l in body.face_loops(f) {
        for fin in body.loop_fins(l) {
            let e = &body.edges[body.fins[fin].edge];
            t = t
                .max(e.tolerance.get())
                .max(body.vertices[e.start].tolerance.get())
                .max(body.vertices[e.end].tolerance.get());
        }
    }
    t
}

/// A stretch no longer than this many tolerances, all within as many of
/// a vertex both faces have, touches at the vertex: the touch rule of
/// [`crate::intersect::curve_surface`].
const TOUCH: f64 = 100.0;

/// The part limits [`apart_beside`] tries each face's search under, the
/// last [`BESIDE_CELLS`].
const LADDER: [usize; 3] = [1 << 9, 1 << 12, BESIDE_CELLS];

/// The most pairs of parts `faces_apart` tests before a pair sharing no
/// edge is traced.
const EARLY_APART: usize = 1 << 12;

/// The most parts each face's search examines in [`apart_at_vertices`]
/// before a pair sharing vertices but no edge is traced.
const EARLY_VERTICES: usize = 1 << 9;

/// The units a trace of faces tangent along an edge they share is first
/// given, between the beside ladder's smaller limits and its largest.
const TANGENT_TRACE_FIRST: u64 = 1 << 14;

/// The units the wider trace of faces sharing vertices but no edge is
/// first given, before the search apart outside the vertices' balls: that
/// search's own budget, two faces of [`BESIDE_CELLS`] parts.
const WITHIN_FIRST: u64 = 1 << 16;

/// The units the wider trace of faces sharing vertices is given last, once
/// the trace in the faces' own region (and, for faces sharing no edge,
/// the capped wider trace and the search apart) has failed. No pinned or
/// committed body's check reaches it.
const WITHIN_LAST: u64 = 1 << 18;

/// Drops the stretches and contacts that, by the touch rule, are only the
/// faces touching at a vertex they share.
fn prune(body: &Body, a: FaceId, b: FaceId, meet: FaceMeet) -> FaceMeet {
    let FaceMeet::Meet {
        stretches,
        contacts,
    } = meet
    else {
        return meet;
    };
    let vertices = |f: FaceId| -> Vec<crate::topo::VertexId> {
        let mut out = Vec::new();
        for l in body.face_loops(f) {
            if let Some(v) = body.loop_(l).and_then(|l| l.vertex) {
                out.push(v);
            }
            for fin in body.loop_fins(l) {
                out.extend(body.fin_start(fin));
            }
        }
        out
    };
    let (va, vb) = (vertices(a), vertices(b));
    let shared: Vec<crate::geom::Point3> = va
        .iter()
        .filter(|v| vb.contains(v))
        .map(|&v| body.vertices[v].point)
        .collect();
    let at_shared = |pts: &[crate::geom::Point3], reach: f64| {
        shared
            .iter()
            .any(|q| pts.iter().all(|p| p.distance(*q) <= reach))
    };
    // Edges both faces use: a stretch fitted to the same curve as one of
    // them may lie farther from it than both tolerances where the faces
    // meet at a shallow angle, but within the touch rule's reach.
    let edges = |f: FaceId| -> Vec<crate::topo::EdgeId> {
        body.face_loops(f)
            .flat_map(|l| {
                body.loop_fins(l)
                    .map(|fin| body.fins[fin].edge)
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let (ea, eb) = (edges(a), edges(b));
    let shared_edges: Vec<crate::topo::EdgeId> =
        ea.iter().copied().filter(|e| eb.contains(e)).collect();
    let along_shared = |pts: &[crate::geom::Point3], reach: f64| {
        shared_edges.iter().any(|&e| {
            let x = &body.edges[e];
            let c = &body.curves[x.curve];
            pts.iter().all(|&p| {
                let foot = c.project(p);
                let t = foot.param;
                let inside = match c.period() {
                    Some(per) => {
                        let t = x.interval[0] + (t - x.interval[0]).rem_euclid(per);
                        t <= x.interval[1]
                    }
                    None => t >= x.interval[0] && t <= x.interval[1],
                };
                (inside && foot.distance <= reach)
                    || c.point(x.interval[0]).distance(p) <= reach
                    || c.point(x.interval[1]).distance(p) <= reach
            })
        })
    };
    let tol = slack(body, a).max(slack(body, b));
    let size = {
        let bx = face_box(body, a).union(&face_box(body, b));
        (bx.max() - bx.min()).length()
    };
    let stretches = stretches
        .into_iter()
        .filter(|s| {
            let reach = TOUCH * s.tolerance.max(tol);
            let pts: Vec<_> = (0..=8)
                .map(|k| {
                    s.curve
                        .point(s.range[0] + (s.range[1] - s.range[0]) * f64::from(k) / 8.0)
                })
                .collect();
            // Out of a tangent wedge's tip at a vertex the faces share: an
            // edge of one lying on the other (or both edges running
            // together) within the reach, for no longer than surfaces
            // meeting tangentially stay that close.
            let length: f64 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
            let from_shared = !s.places.iter().all(|p| *p == Place::Inside)
                && length <= (2.0 * size * reach).sqrt()
                && shared
                    .iter()
                    .any(|q| pts[0].distance(*q) <= reach || pts[8].distance(*q) <= reach);
            !at_shared(&pts, reach) && !along_shared(&pts, reach) && !from_shared
        })
        .collect();
    let contacts = contacts
        .into_iter()
        .filter(|c| !at_shared(&[c.point], TOUCH * tol))
        .collect();
    FaceMeet::Meet {
        stretches,
        contacts,
    }
}

/// The fault two faces' meeting makes, if any.
fn fault(a: FaceId, b: FaceId, meet: &FaceMeet) -> Option<Fault> {
    match meet {
        FaceMeet::Coincident { rims, whole, .. } => (*whole
            || rims.iter().any(|r| r.place == Place::Inside))
        .then_some(Fault::FacesOverlap { a, b }),
        FaceMeet::Meet {
            stretches,
            contacts,
        } => {
            let shared_edge =
                |p: [Place; 2]| matches!(p, [Place::OnEdge(x), Place::OnEdge(y)] if x == y);
            let shared = |p: [Place; 2]| {
                shared_edge(p) || matches!(p, [Place::OnVertex(x), Place::OnVertex(y)] if x == y)
            };
            let bad = stretches.iter().any(|s| !shared_edge(s.places))
                || contacts.iter().any(|c| !shared(c.places));
            bad.then_some(Fault::FacesCross { a, b })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point3, Rigid, Vector3};
    use crate::name::OpKey;
    use crate::primitive;
    use crate::topo::fixture::polyhedron;

    /// The caps on the traces tried before and after the costly searches
    /// apart (#302, #303, #416) stay below a trace's own scope, so each is
    /// a bound and the last grown trace never outspends the first.
    #[test]
    fn early_and_last_traces_are_capped_below_a_trace() {
        let trace = crate::work::Scope::Trace.limit();
        const { assert!(TANGENT_TRACE_FIRST < WITHIN_FIRST) };
        const { assert!(WITHIN_FIRST < WITHIN_LAST) };
        assert!(WITHIN_LAST < trace / 64);
    }

    #[test]
    fn primitives_do_not_cross_themselves() {
        let t = Rigid::rotation(Vector3::new(1.0, 2.0, 0.5).normalize().unwrap(), 0.7);
        let k = OpKey(1);
        for b in [
            primitive::block(k, 1.0, 2.0, 3.0, &t).unwrap().0,
            primitive::cylinder(k, 1.0, 2.0, &t).unwrap().0,
            primitive::cone(k, 1.0, 2.0, &t).unwrap().0,
            primitive::frustum(k, 1.0, 0.3, 2.0, &t).unwrap().0,
            primitive::sphere(k, 1.0, &t).unwrap().0,
            primitive::torus(k, 2.0, 0.5, &t).unwrap().0,
        ] {
            assert_eq!(check_intersections(&b), []);
        }
    }

    /// A triangular bipyramid on the base (0,0,0), (2,0,0), (0,2,0), with
    /// apexes `d` and `e`.
    fn bipyramid(d: Point3, e: Point3) -> Body {
        let pts = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(0.0, 2.0, 0.0),
            d,
            e,
        ];
        polyhedron(
            &pts,
            &[
                &[3, 0, 1],
                &[3, 1, 2],
                &[3, 2, 0],
                &[4, 1, 0],
                &[4, 2, 1],
                &[4, 0, 2],
            ],
        )
    }

    #[test]
    fn a_folded_bipyramid_crosses_itself() {
        let sound = bipyramid(Point3::new(0.5, 0.5, 1.0), Point3::new(0.5, 0.5, -1.0));
        assert_eq!(check_intersections(&sound), []);
        // Both apexes above the base, one leaning across the other's
        // faces.
        let folded = bipyramid(Point3::new(0.5, 0.5, 1.0), Point3::new(1.5, 1.5, 2.0));
        let faults = check_intersections(&folded);
        assert!(!faults.is_empty());
        assert!(
            faults.iter().all(|f| matches!(f, Fault::FacesCross { .. })),
            "{faults:?}"
        );
    }

    #[test]
    fn a_flat_cones_side_meets_its_base_at_an_edge() {
        // Radius 82, height a millimetre: the side meets the base at
        // about 1e-5 rad, which is no tangency to be checked beside it.
        let t = Rigid::rotation(Vector3::new(0.2, -0.2, 1.0).normalize().unwrap(), 2.9);
        let b = primitive::cone(OpKey(1), 81.8, 0.001, &t).unwrap().0;
        assert_eq!(check_intersections(&b), []);
    }

    #[test]
    fn the_rules() {
        use crate::intersect::{Contact, Rim};
        use crate::topo::{EdgeId, Id, VertexId};
        let (a, b) = (FaceId::from_index(0), FaceId::from_index(1));
        let (e, f) = (EdgeId::from_index(0), EdgeId::from_index(1));
        let (v, w) = (VertexId::from_index(0), VertexId::from_index(1));
        let overlap = |place| FaceMeet::Coincident {
            same: true,
            rims: vec![Rim {
                of: 0,
                edge: e,
                range: [0.0, 1.0],
                place,
            }],
            whole: false,
        };
        assert_eq!(
            fault(a, b, &overlap(Place::Inside)),
            Some(Fault::FacesOverlap { a, b })
        );
        assert_eq!(fault(a, b, &overlap(Place::OnEdge(f))), None);
        let touch = |places| FaceMeet::Meet {
            stretches: vec![],
            contacts: vec![Contact {
                point: Point3::ORIGIN,
                places,
                reach: 0.0,
            }],
        };
        assert_eq!(
            fault(a, b, &touch([Place::OnVertex(v), Place::OnVertex(v)])),
            None
        );
        assert_eq!(
            fault(a, b, &touch([Place::OnVertex(v), Place::OnVertex(w)])),
            Some(Fault::FacesCross { a, b })
        );
        assert_eq!(
            fault(a, b, &touch([Place::OnEdge(e), Place::Inside])),
            Some(Fault::FacesCross { a, b })
        );
    }

    /// A unit cube by [`polyhedron`], its faces bottom, top, then the
    /// sides, and its vertices in the order given.
    fn unit_cube() -> Body {
        let p = |x: f64, y: f64, z: f64| Point3::new(x, y, z);
        polyhedron(
            &[
                p(0.0, 0.0, 0.0),
                p(1.0, 0.0, 0.0),
                p(1.0, 1.0, 0.0),
                p(0.0, 1.0, 0.0),
                p(0.0, 0.0, 1.0),
                p(1.0, 0.0, 1.0),
                p(1.0, 1.0, 1.0),
                p(0.0, 1.0, 1.0),
            ],
            &[
                &[0, 3, 2, 1],
                &[4, 5, 6, 7],
                &[0, 1, 5, 4],
                &[1, 2, 6, 5],
                &[2, 3, 7, 6],
                &[3, 0, 4, 7],
            ],
        )
    }

    /// Faces that share no vertex are never apart by their own edges: the
    /// certificate needs the vertices its balls are round, and leaves
    /// every other pair to the trace's verdict.
    #[test]
    fn faces_sharing_no_vertex_are_not_apart_by_their_own_edges() {
        let body = unit_cube();
        let faces: Vec<FaceId> = body.faces().collect();
        // unmetered: a test of a pair that settles at once
        let m = Meter::free();
        assert!(!apart_by_own_edges(
            &body,
            faces[0],
            faces[1],
            BESIDE_CELLS,
            &m
        ));
    }

    /// The bottom's edges kept off the top face: certified under no limit,
    /// and stopped at once by a cancel or a spent budget, with no piece
    /// searched past the halt.
    #[test]
    fn a_halt_stops_the_search_along_edges() {
        use crate::work::Work;
        use std::sync::atomic::AtomicBool;
        let body = unit_cube();
        let on = |f: FaceId, z: f64| {
            body.face_loops(f).all(|l| {
                body.loop_fins(l).all(|fin| {
                    let e = &body.edges[body.fins[fin].edge];
                    (body.vertices[e.start].point.z - z).abs() < 1e-12
                })
            })
        };
        let top = body.faces().find(|&f| on(f, 1.0)).unwrap();
        let bottom = body.faces().find(|&f| on(f, 0.0)).unwrap();
        let edges: Vec<EdgeId> = body
            .face_loops(bottom)
            .flat_map(|l| {
                body.loop_fins(l)
                    .map(|fin| body.fins[fin].edge)
                    .collect::<Vec<_>>()
            })
            .collect();
        let w = Work::unlimited();
        let m = Meter::new(&w);
        assert!(face_off_edges(&body, top, &edges, &[], 0.1, 0.2, &m));
        let full = m.spent();
        assert!(full >= edges.len() as u64, "{full}");
        // Cancelled before it starts.
        let flag = AtomicBool::new(true);
        let w = Work::unlimited().with_cancel(&flag);
        let m = Meter::new(&w);
        assert!(!face_off_edges(&body, top, &edges, &[], 0.1, 0.2, &m));
        assert!(m.halted());
        assert_eq!(m.spent(), 1);
        // A budget of one piece.
        let w = Work::unlimited().with_budget(1);
        let m = Meter::new(&w);
        assert!(!face_off_edges(&body, top, &edges, &[], 0.1, 0.2, &m));
        assert!(m.halted());
        assert!(m.spent() < full, "{} of {full}", m.spent());
    }

    /// Several shared straight edges in one plane give its section plane
    /// (a cone's halves split along two generators, NIST CTC 02's AP242
    /// edition, #264); skew ones give none.
    #[test]
    fn coplanar_shared_lines_give_their_plane() {
        let body = unit_cube();
        // The edge between the cube's corners `p` and `q`.
        let edge = |p: [f64; 3], q: [f64; 3]| {
            let at = |v: VertexId, c: [f64; 3]| {
                body.vertices[v]
                    .point
                    .distance(Point3::new(c[0], c[1], c[2]))
                    < 1e-12
            };
            body.edges()
                .find(|&e| {
                    let x = &body.edges[e];
                    (at(x.start, p) && at(x.end, q)) || (at(x.start, q) && at(x.end, p))
                })
                .unwrap()
        };
        // The bottom's edges y = 0 and y = 1 lie in z = 0.
        let bottom = [
            edge([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            edge([1.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
        ];
        let plane = lines_plane(&body, &bottom).expect("a plane");
        let Surface::Plane(_) = &plane else {
            panic!("{plane:?}")
        };
        let f = plane.frame().unwrap();
        assert!(f.z().get().z.abs() > 1.0 - 1e-12);
        // The bottom's y = 0 edge and the top's x = 1 edge are skew.
        let skew = [
            edge([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
            edge([1.0, 0.0, 1.0], [1.0, 1.0, 1.0]),
        ];
        assert!(lines_plane(&body, &skew).is_none());
    }
}
