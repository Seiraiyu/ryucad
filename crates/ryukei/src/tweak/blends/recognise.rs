//! Blend recognition (design 399): fillets and chamfers read back from
//! any body's faces, certified.
//!
//! - A **fillet** of radius `r` is a face on a canal surface of that
//!   radius (a cylinder, a torus's tube, a pipe, or a NURBS certified
//!   within the tolerance of a cylinder or torus) whose *rails*, the
//!   edges that don't run across its spine, roll on exactly two faces,
//!   its supports: the stretch of spine under each rail lies on its
//!   support's offset by `r`, certified by `intersect::curve_surface`
//!   (the design's rule that the spine lies at distance `r` from both
//!   supports along their normals). It turns less than a half turn
//!   between them.
//! - A **chamfer** is a plane or cone face (a flat spline as its plane)
//!   whose edges to two faces are each set back a constant distance from
//!   where those faces' surfaces meet (a line, or a circle about a common
//!   axis), in closed form; of the pairs of faces that read so, the one
//!   setting back the least share of its supports' width, which must be
//!   under [`CHAMFER_SHARE`] (a mitred end's neighbours read so too, set
//!   back farther). A chamfer standing on another chamfer is left out,
//!   the narrower kept.
//! - A **corner** is a sphere face every edge of which runs into a fillet
//!   of its radius whose spine passes through its centre.
//!
//! Faces of one surface with the same supports and size are one blend;
//! blends of one size running on into each other (fillets across an
//! edge that is a section of both, chamfers across a tangent edge) are a
//! chain. A face that is nearly a blend but not within the tolerance is
//! left out.

use super::{Corner, RecogniseOptions, Recognised, Recognition};
use crate::blend::convex::tangent_along;
use crate::blend::{BlendKind, Chamfer};
use crate::geom::{Circle, Curve, Line, Point3, Surface, Vector3};
use crate::name::Name;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use crate::topo::{Body, EdgeId, FaceId, Sense};
use crate::work::Meter;
use crate::{Error, TweakError};
use std::collections::{BTreeMap, BTreeSet};

/// A chamfer sets back from its edge at most this share of a support's
/// width across the edge: a hexagonal prism's sides (a half each) are no
/// chamfers of a triangle's.
pub(crate) const CHAMFER_SHARE: f64 = 0.4;

/// A fillet turns at most a half turn less this between its supports: a
/// slot's round end, a half turn between parallel walls, is no fillet.
const FULL_ROUND: f64 = 0.01;

/// Samples taken along an edge to judge it.
const SAMPLES: usize = 5;

/// The canal surface a fillet lies on: its spine, the curve its
/// centres run along (a line, a circle, or a pipe's spine).
#[derive(Clone, Debug)]
pub(crate) struct Canal {
    pub(crate) spine: Curve,
}

impl Canal {
    /// The parameter of the spine's point nearest `p`.
    fn param(&self, p: Point3) -> f64 {
        self.spine.project(p).param
    }

    /// The point of the spine nearest `p`.
    fn foot(&self, p: Point3) -> Point3 {
        self.spine.point(self.param(p))
    }

    /// The stretch of the spine under `edge`: its samples' feet's
    /// parameters, unwrapped across a period, a whole period for a closed
    /// edge round a periodic spine.
    pub(crate) fn under(&self, body: &Body, edge: EdgeId) -> [f64; 2] {
        let ts: Vec<f64> = samples(body, edge)
            .into_iter()
            .map(|p| self.param(p))
            .collect();
        let period = self.spine.period();
        let e = &body.edges[edge];
        if let Some(period) = period
            && e.start == e.end
        {
            return [ts[0], ts[0] + period];
        }
        let mut out = vec![ts[0]];
        for &t in &ts[1..] {
            let last = *out.last().expect("one");
            let mut t = t;
            if let Some(period) = period {
                while t - last > period / 2.0 {
                    t -= period;
                }
                while last - t > period / 2.0 {
                    t += period;
                }
            }
            out.push(t);
        }
        let lo = out.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = out.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        [lo, hi]
    }
}

/// A face read as a fillet or a chamfer: its kind and its supports, the
/// two faces either side, by name order.
#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub(crate) face: FaceId,
    pub(crate) kind: BlendKind,
    pub(crate) sides: [FaceId; 2],
    /// A chamfer's setback over its support's width, the larger side's.
    share: f64,
}

/// The face across `edge` from `f`, if the edge has two fins.
pub(crate) fn across(body: &Body, edge: EdgeId, f: FaceId) -> Option<FaceId> {
    let faces: Vec<FaceId> = body
        .edge_fins(edge)
        .map(|fin| body.loops[body.fins[fin].loop_].face)
        .collect();
    match faces.as_slice() {
        [a, b] if *a == f && *b != f => Some(*b),
        [a, b] if *b == f && *a != f => Some(*a),
        _ => None,
    }
}

/// Every edge of face `f`, each once.
pub(crate) fn edges_of(body: &Body, f: FaceId) -> Vec<EdgeId> {
    let mut out = BTreeSet::new();
    for l in body.face_loops(f) {
        for fin in body.loop_fins(l) {
            out.insert(body.fins[fin].edge);
        }
    }
    out.into_iter().collect()
}

/// Points along `edge`, its ends included.
fn samples(body: &Body, edge: EdgeId) -> Vec<Point3> {
    let e = &body.edges[edge];
    let c = &body.curves[e.curve];
    let [t0, t1] = e.interval;
    (0..SAMPLES)
        .map(|i| c.point(t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64))
        .collect()
}

/// The face's outward normal at `p`, through its surface's projection.
fn outward(body: &Body, f: FaceId, p: Point3) -> Option<Vector3> {
    let face = &body.faces[f];
    let s = &body.surfaces[face.surface];
    let (u, v) = s.project(p).param;
    let n = s.normal(u, v)?.get();
    Some(if face.sense == Sense::Forward { n } else { -n })
}

/// The canal surface face `f` lies on and its radius: a cylinder, a
/// torus's tube, a pipe, or a NURBS fitted to one of the first two.
fn canal(body: &Body, f: FaceId, tol: f64) -> Option<(Canal, f64)> {
    let surface = &body.surfaces[body.faces[f].surface];
    match surface {
        Surface::Cylinder(c) => {
            let fr = surface.frame()?;
            let spine = Curve::Line(Line::from_unit(fr.origin(), fr.z()).ok()?);
            Some((Canal { spine }, c.radius()))
        }
        Surface::Torus(t) => {
            let spine = Curve::Circle(Circle::new(*surface.frame()?, t.major()).ok()?);
            Some((Canal { spine }, t.minor()))
        }
        Surface::Pipe(p) => Some((
            Canal {
                spine: p.spine().clone(),
            },
            p.radius(),
        )),
        Surface::Nurbs(n) => super::fit::fit(body, f, n, tol),
        _ => None,
    }
}

/// Whether `edge` runs across the spine: its samples' feet on it lie
/// within the tolerances of each other.
fn section(body: &Body, canal: &Canal, edge: EdgeId, tol: f64) -> bool {
    let feet: Vec<Point3> = samples(body, edge)
        .into_iter()
        .map(|p| canal.foot(p))
        .collect();
    let reach = 2.0 * (tol + body.edges[edge].tolerance.get());
    feet.iter().all(|p| p.distance(feet[0]) <= reach)
}

/// The slack the rails' tangency takes at `edge` of a face of radius `r`.
fn slack(body: &Body, edge: EdgeId, r: f64, tol: f64) -> f64 {
    ANGULAR_RESOLUTION + 2.0 * (tol + body.edges[edge].tolerance.get()) / r
}

/// Whether the spine under `edge` lies on face `g`'s surface's offset by
/// `r` one way or the other, certified all along: the canal surface is
/// then tangent to `g`'s surface along the edge, the design's rule that a
/// fillet's spine lies at distance `r` from its supports along their
/// normals. Each offset is met by `intersect::curve_surface`, which must
/// give one stretch on it covering the whole stretch of spine.
fn rolls_on(
    body: &Body,
    canal: &Canal,
    r: f64,
    edge: EdgeId,
    g: FaceId,
    tol: f64,
    m: &Meter,
) -> bool {
    let range = canal.under(body, edge);
    if range[1] - range[0] <= 0.0 {
        return false;
    }
    let reach = tol + body.edges[edge].tolerance.get();
    let support = &body.surfaces[body.faces[g].surface];
    [r, -r].into_iter().any(|d| {
        let Some(Ok(off)) = support.offset(d) else {
            return false;
        };
        matches!(
            crate::intersect::curve_surface_in(&canal.spine, range, &off, reach, m).as_deref(),
            Ok([crate::intersect::CurveHit::On { t: [a, b] }]) if *a <= range[0] && *b >= range[1]
        )
    })
}

/// Face `f` read as a fillet.
pub(crate) fn fillet(body: &Body, f: FaceId, tol: f64, m: &Meter) -> Option<Candidate> {
    let (canal, r) = canal(body, f, tol)?;
    let mut rails: BTreeMap<FaceId, Vec<EdgeId>> = BTreeMap::new();
    for e in edges_of(body, f) {
        let Some(g) = across(body, e, f) else {
            continue;
        };
        if section(body, &canal, e, tol) {
            continue;
        }
        if rolls_on(body, &canal, r, e, g, tol, m) {
            rails.entry(g).or_default().push(e);
        }
    }
    let sides: Vec<FaceId> = rails.keys().copied().collect();
    let [a, b] = sides.as_slice() else {
        return None;
    };
    // The turn between the supports, at the middles of a rail each side.
    let mid = |e: EdgeId| samples(body, e)[SAMPLES / 2];
    let n0 = outward(body, f, mid(rails[a][0]))?;
    let n1 = outward(body, f, mid(rails[b][0]))?;
    let turn = crate::math::acos(n0.dot(n1).clamp(-1.0, 1.0));
    if turn > core::f64::consts::PI - FULL_ROUND {
        return None;
    }
    // The supports' own normals there: walls facing apart are a full
    // round's, however far past their tangency its rails run. A rib's
    // round top whose wall lies 4.7 µm inside its tangent (NIST CTC 03)
    // turns 0.08 rad past a half turn, which `acos` of the face's own
    // normals folds back below it.
    let s0 = outward(body, *a, mid(rails[a][0]))?;
    let s1 = outward(body, *b, mid(rails[b][0]))?;
    if crate::math::acos(s0.dot(s1).clamp(-1.0, 1.0)) > core::f64::consts::PI - FULL_ROUND {
        return None;
    }
    let mut sides = [*a, *b];
    sides.sort_by_key(|&g| body.faces[g].name);
    Some(Candidate {
        face: f,
        kind: BlendKind::Fillet { radius: r },
        sides,
        share: 0.0,
    })
}

/// A surface as chamfers are read on it: a flat NURBS as its plane
/// (`blend::spine::twin`, every control point within half the
/// resolution of it), anything else as it is.
fn flat(s: &Surface) -> std::borrow::Cow<'_, Surface> {
    match crate::blend::spine::twin(s) {
        Some(p) => std::borrow::Cow::Owned(p),
        None => std::borrow::Cow::Borrowed(s),
    }
}

/// Where two support surfaces meet, in closed form.
enum Meet {
    /// A line: a point on it and its unit direction.
    Line(Point3, Vector3),
    /// A circle: its centre, its axis and its radius.
    Circle(Point3, Vector3, f64),
}

/// Where surfaces `a` and `b` meet: two planes in a line, a plane square
/// to a cylinder's or cone's axis in a circle.
fn meet(a: &Surface, b: &Surface) -> Option<Meet> {
    let axial = |p: &crate::geom::Plane, s: &Surface| -> Option<Meet> {
        let n = p.frame().z().get();
        let (fr, radius_at): (&crate::geom::Frame, Box<dyn Fn(f64) -> f64>) = match s {
            Surface::Cylinder(c) => {
                let r = c.radius();
                (s.frame()?, Box::new(move |_| r))
            }
            Surface::Cone(c) => {
                let (r, t) = (c.radius(), crate::math::tan(c.half_angle()));
                (s.frame()?, Box::new(move |v| r + v * t))
            }
            _ => return None,
        };
        let z = fr.z().get();
        if n.cross(z).length() > ANGULAR_RESOLUTION {
            return None;
        }
        let o = fr.origin();
        let h = (p.frame().origin() - o).dot(z);
        Some(Meet::Circle(o + z * h, z, radius_at(h)))
    };
    match (a, b) {
        (Surface::Plane(p), Surface::Plane(q)) => {
            let (na, nb) = (p.frame().z().get(), q.frame().z().get());
            let u = na.cross(nb);
            let len2 = u.length_squared();
            if len2 < 1e-12 {
                return None;
            }
            let (da, db) = (
                p.frame().origin().to_vector().dot(na),
                q.frame().origin().to_vector().dot(nb),
            );
            let x = (nb.cross(u) * da + u.cross(na) * db) / len2;
            Some(Meet::Line(Point3::ORIGIN + x, u / crate::math::sqrt(len2)))
        }
        (Surface::Plane(p), s) | (s, Surface::Plane(p)) => axial(p, s),
        _ => None,
    }
}

/// How far `edge` of a chamfer is set back from where its supports meet,
/// the same all along within `tol`: a line parallel to a line, a circle
/// about a circle's axis.
fn setback(body: &Body, edge: EdgeId, m: &Meet, tol: f64) -> Option<f64> {
    let e = &body.edges[edge];
    let reach = tol + e.tolerance.get();
    let pts = samples(body, edge);
    let d: Vec<f64> = match (m, &body.curves[e.curve]) {
        (Meet::Line(o, dir), crate::geom::Curve::Line(l)) => {
            if l.dir().get().cross(*dir).length() > ANGULAR_RESOLUTION + reach {
                return None;
            }
            pts.iter()
                .map(|&p| {
                    let w = p - *o;
                    (w - *dir * w.dot(*dir)).length()
                })
                .collect()
        }
        (Meet::Line(o, dir), crate::geom::Curve::Nurbs(n)) => {
            // A spline edge is a straight setback when every control point
            // lies within the reach of the line through its first, parallel
            // to the meeting: the curve lies in the hull, so within the
            // reach of that line too.
            let w = n.weights();
            if w.iter().any(|&x| x <= 0.0) {
                return None;
            }
            let first = n.control_points().next()?;
            let dist = |p: Point3| {
                let q = p - first;
                (q - *dir * q.dot(*dir)).length()
            };
            if n.control_points().any(|p| dist(p) > reach) {
                return None;
            }
            let q = first - *o;
            let d = (q - *dir * q.dot(*dir)).length();
            vec![d, d]
        }
        (Meet::Circle(c, z, big), crate::geom::Curve::Circle(_)) => pts
            .iter()
            .map(|&p| {
                let w = p - *c;
                let h = w.dot(*z);
                let rho = (w - *z * h).length();
                crate::math::sqrt((rho - big) * (rho - big) + h * h)
            })
            .collect(),
        _ => return None,
    };
    let (lo, hi) = d
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &x| {
            (a.min(x), b.max(x))
        });
    (hi - lo <= reach && lo > 10.0 * LINEAR_RESOLUTION).then_some((lo + hi) / 2.0)
}

/// How wide face `g` is across where its chamfer's supports meet: its
/// vertices' greatest distance from the meeting.
fn width(body: &Body, g: FaceId, m: &Meet) -> f64 {
    let mut w: f64 = 0.0;
    for e in edges_of(body, g) {
        let edge = &body.edges[e];
        for v in [edge.start, edge.end] {
            let p = body.vertices[v].point;
            let d = match m {
                Meet::Line(o, dir) => {
                    let q = p - *o;
                    (q - *dir * q.dot(*dir)).length()
                }
                Meet::Circle(c, z, big) => {
                    let q = p - *c;
                    let h = q.dot(*z);
                    let rho = (q - *z * h).length();
                    crate::math::sqrt((rho - big) * (rho - big) + h * h)
                }
            };
            w = w.max(d);
        }
    }
    w
}

/// Face `f` read as a chamfer.
pub(crate) fn chamfer(body: &Body, f: FaceId, tol: f64) -> Option<Candidate> {
    if !matches!(
        flat(&body.surfaces[body.faces[f].surface]).as_ref(),
        Surface::Plane(_) | Surface::Cone(_)
    ) {
        return None;
    }
    let mut by: BTreeMap<FaceId, Vec<EdgeId>> = BTreeMap::new();
    for e in edges_of(body, f) {
        if let Some(g) = across(body, e, f) {
            by.entry(g).or_default().push(e);
        }
    }
    let faces: Vec<FaceId> = by.keys().copied().collect();
    let mut found: Option<Candidate> = None;
    let mut ambiguous = false;
    for (i, &a) in faces.iter().enumerate() {
        for &b in &faces[i + 1..] {
            let (sa, sb) = (
                flat(&body.surfaces[body.faces[a].surface]),
                flat(&body.surfaces[body.faces[b].surface]),
            );
            let Some(m) = meet(&sa, &sb) else { continue };
            let side = |g: FaceId| -> Option<f64> {
                let ds: Option<Vec<f64>> =
                    by[&g].iter().map(|&e| setback(body, e, &m, tol)).collect();
                let ds = ds?;
                let d = ds[0];
                ds.iter().all(|x| (x - d).abs() <= tol).then_some(d)
            };
            let (Some(da), Some(db)) = (side(a), side(b)) else {
                continue;
            };
            // A support every edge of which the chamfer bounds (a disc
            // capping a frustum's rim) has nothing beyond its rail to
            // measure a width by, and no mitre to mistake: its share is 0.
            let share_of = |g: FaceId, d: f64| {
                if edges_of(body, g).len() == by[&g].len() {
                    0.0
                } else {
                    d / width(body, g, &m)
                }
            };
            let share = share_of(a, da).max(share_of(b, db));
            if share > CHAMFER_SHARE {
                continue;
            }
            // The pair setting back the least share of its supports is
            // the chamfer's (a mitred end's neighbours read as supports
            // too, set back farther); two pairs within the tolerance of
            // each other: neither.
            match &found {
                Some(c) if c.share + tol / width(body, a, &m).max(LINEAR_RESOLUTION) < share => {
                    continue;
                }
                Some(c) if (c.share - share).abs() * width(body, a, &m) <= tol => {
                    ambiguous = true;
                    continue;
                }
                _ => ambiguous = false,
            }
            let mut sides = [a, b];
            let mut ds = [da, db];
            if body.faces[b].name < body.faces[a].name {
                sides.swap(0, 1);
                ds.swap(0, 1);
            }
            let kind = if (ds[0] - ds[1]).abs() <= tol {
                BlendKind::Chamfer(Chamfer::Equal {
                    distance: (ds[0] + ds[1]) / 2.0,
                })
            } else {
                BlendKind::Chamfer(Chamfer::Distances {
                    first: ds[0],
                    second: ds[1],
                    first_face: body.faces[sides[0]].name,
                })
            };
            found = Some(Candidate {
                face: f,
                kind,
                sides,
                share,
            });
        }
    }
    if ambiguous { None } else { found }
}

/// A blend's size: a fillet's radius, a chamfer's larger setback.
pub(crate) fn size(kind: &BlendKind) -> f64 {
    match *kind {
        BlendKind::Fillet { radius } => radius,
        BlendKind::Chamfer(Chamfer::Equal { distance }) => distance,
        BlendKind::Chamfer(Chamfer::Distances { first, second, .. }) => first.max(second),
        BlendKind::Chamfer(Chamfer::DistanceAngle { distance, .. }) => distance,
    }
}

/// Whether two kinds are one within `tol`.
fn alike(a: &BlendKind, b: &BlendKind, tol: f64) -> bool {
    match (a, b) {
        (BlendKind::Fillet { radius: x }, BlendKind::Fillet { radius: y }) => (x - y).abs() <= tol,
        (
            BlendKind::Chamfer(Chamfer::Equal { distance: x }),
            BlendKind::Chamfer(Chamfer::Equal { distance: y }),
        ) => (x - y).abs() <= tol,
        (
            BlendKind::Chamfer(Chamfer::Distances {
                first: a1,
                second: a2,
                first_face: fa,
            }),
            BlendKind::Chamfer(Chamfer::Distances {
                first: b1,
                second: b2,
                first_face: fb,
            }),
        ) => fa == fb && (a1 - b1).abs() <= tol && (a2 - b2).abs() <= tol,
        _ => false,
    }
}

/// Whether blend faces `a` and `b` of one kind run on into each other
/// across `edge`: fillets where the edge is a section of both canals
/// (their spines meet there), chamfers where their faces are tangent all
/// along it.
fn joins(body: &Body, edge: EdgeId, a: FaceId, b: FaceId, kind: &BlendKind, tol: f64) -> bool {
    match kind {
        BlendKind::Fillet { .. } => [a, b]
            .into_iter()
            .all(|f| canal(body, f, tol).is_some_and(|(c, _)| section(body, &c, edge, tol))),
        _ => {
            let r = size(kind).max(10.0 * LINEAR_RESOLUTION);
            tangent_along(body, edge, slack(body, edge, r, tol)) == Some(true)
        }
    }
}

/// `body`'s blends and corners.
pub(crate) fn recognise(
    body: &Body,
    opts: &RecogniseOptions,
    m: &Meter,
) -> Result<Recognition, Error> {
    let tol = opts.tolerance;
    if !(LINEAR_RESOLUTION..=crate::tol::TOLERANCE_CAP).contains(&tol) {
        return Err(TweakError::BadTolerance.into());
    }
    let faces: Vec<FaceId> = body.faces().collect();
    let halt = |f: FaceId| -> Error {
        TweakError::Uncertain {
            at: body.faces[f].name,
        }
        .into()
    };
    let read: Vec<Result<Option<Candidate>, Error>> = m.par_map(&faces, |m, &f| {
        m.charge(1).map_err(|_| halt(f))?;
        let found = fillet(body, f, tol, m).or_else(|| chamfer(body, f, tol));
        if m.halted() {
            return Err(halt(f));
        }
        Ok(found)
    });
    let mut found: Vec<Candidate> = Vec::new();
    for r in read {
        if let Some(c) = r? {
            found.push(c);
        }
    }
    // Chamfers standing on chamfers: the narrower first, by name.
    found.sort_by(|a, b| {
        let chamfer = |c: &Candidate| matches!(c.kind, BlendKind::Chamfer(_));
        (chamfer(a), a.share, body.faces[a.face].name)
            .partial_cmp(&(chamfer(b), b.share, body.faces[b.face].name))
            .expect("finite")
    });
    let mut kept: Vec<Candidate> = Vec::new();
    for c in found {
        if matches!(c.kind, BlendKind::Chamfer(_)) {
            let clash = kept
                .iter()
                .any(|k| k.sides.contains(&c.face) || c.sides.contains(&k.face));
            if clash {
                continue;
            }
        }
        kept.push(c);
    }
    kept.sort_by_key(|c| body.faces[c.face].name);
    // One blend: faces on one surface with the same supports and size.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, c) in kept.iter().enumerate() {
        let s = &body.surfaces[body.faces[c.face].surface];
        let home = groups.iter_mut().find(|g| {
            let k = &kept[g[0]];
            k.sides == c.sides
                && alike(&k.kind, &c.kind, tol)
                && crate::intersect::coincident(&body.surfaces[body.faces[k.face].surface], s)
                    == Some(true)
        });
        match home {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    let group_of: BTreeMap<FaceId, usize> = groups
        .iter()
        .enumerate()
        .flat_map(|(g, members)| {
            let kept = &kept;
            members.iter().map(move |&i| (kept[i].face, g))
        })
        .collect();
    // Chains: blends of one size joined across tangent edges.
    let mut chain: Vec<usize> = (0..groups.len()).collect();
    fn root(chain: &mut [usize], mut i: usize) -> usize {
        while chain[i] != i {
            chain[i] = chain[chain[i]];
            i = chain[i];
        }
        i
    }
    for (g, members) in groups.iter().enumerate() {
        for &i in members {
            let c = &kept[i];
            for e in edges_of(body, c.face) {
                let Some(other) = across(body, e, c.face) else {
                    continue;
                };
                let Some(&h) = group_of.get(&other) else {
                    continue;
                };
                if h == g || !alike(&kept[groups[h][0]].kind, &c.kind, tol) {
                    continue;
                }
                if joins(body, e, c.face, other, &c.kind, tol) {
                    let (a, b) = (root(&mut chain, g), root(&mut chain, h));
                    chain[a.max(b)] = a.min(b);
                }
            }
        }
    }
    let mut ids: BTreeMap<usize, usize> = BTreeMap::new();
    let mut blends = Vec::new();
    for (g, members) in groups.iter().enumerate() {
        let r = root(&mut chain, g);
        let next = ids.len();
        let id = *ids.entry(r).or_insert(next);
        let first = &kept[members[0]];
        let mut fs: Vec<Name> = members
            .iter()
            .map(|&i| body.faces[kept[i].face].name)
            .collect();
        fs.sort();
        blends.push(Recognised {
            faces: fs,
            kind: first.kind,
            supports: first.sides.map(|s| body.faces[s].name),
            chain: id,
        });
    }
    // Corners: spheres every edge of which is a section of a fillet of
    // their radius whose spine passes through their centre.
    let mut corners = Vec::new();
    for f in body.faces() {
        let surface = &body.surfaces[body.faces[f].surface];
        let Surface::Sphere(sp) = surface else {
            continue;
        };
        let (r, centre) = (sp.radius(), surface.frame().expect("a sphere's").origin());
        let mut joined = BTreeSet::new();
        let mut all = true;
        for e in edges_of(body, f) {
            let fits = across(body, e, f)
                .and_then(|g| group_of.get(&g).map(|&h| (g, h)))
                .filter(|&(g, h)| {
                    let BlendKind::Fillet { radius } = kept[groups[h][0]].kind else {
                        return false;
                    };
                    (radius - r).abs() <= tol
                        && canal(body, g, tol).is_some_and(|(c, _)| {
                            c.foot(centre).distance(centre) <= tol + body.edges[e].tolerance.get()
                        })
                });
            match fits {
                Some((_, h)) => {
                    joined.insert(h);
                }
                None => all = false,
            }
        }
        if all && joined.len() >= 2 {
            corners.push(Corner {
                face: body.faces[f].name,
                blends: joined.into_iter().collect(),
            });
        }
    }
    corners.sort_by_key(|c| c.face);
    Ok(Recognition { blends, corners })
}
