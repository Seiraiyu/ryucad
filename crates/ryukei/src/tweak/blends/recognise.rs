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

// Taken out in Task 5, once every function here is used.
#![allow(dead_code, unused_imports)]

use super::{Corner, RecogniseOptions, Recognised, Recognition};
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
    let mut sides = [*a, *b];
    sides.sort_by_key(|&g| body.faces[g].name);
    Some(Candidate {
        face: f,
        kind: BlendKind::Fillet { radius: r },
        sides,
        share: 0.0,
    })
}
