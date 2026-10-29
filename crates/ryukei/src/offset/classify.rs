//! What an offset starts from: each face's distance and offset surface,
//! and how each edge's faces move: apart (opening), together (closing),
//! along together (smooth), or not at all (kept).

use super::OffsetSpec;
use crate::blend::convex::{Bend, BendFail, bend};
use crate::geom::{Surface, Vector3};
use crate::intersect::face_rect;
use crate::name::Name;
use crate::tol::LINEAR_RESOLUTION;
use crate::topo::{Body, EdgeId, FaceId, FinId, Sense};
use crate::{Error, OffsetCase, OffsetError};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// How far apart, at most, the offsets of an edge's faces may lie along
/// it for the edge to count as smooth: `|d|` times the angle between the
/// faces' normals, at 17 samples. Within it, both offsets meet one curve
/// within the tolerance an intersection's fit is allowed
/// (`10 × LINEAR_RESOLUTION`), so the curve is their common edge.
pub(crate) const SMOOTH_GAP: f64 = 10.0 * LINEAR_RESOLUTION;

/// Samples along an edge at which smoothness is judged, ends included.
const SAMPLES: usize = 17;

/// How far past its own box a spline face's offset reaches, at least, as
/// a share of the box's width each way: closing edges are found a little
/// way past where the face ends.
const MARGIN: f64 = 0.1;

/// How an edge's faces move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// Both faces stay (distance zero): the edge stays as it is.
    Kept,
    /// The faces are tangent all along it: their offsets meet along the
    /// edge moved by the distance.
    Smooth,
    /// The offsets move apart: a ball rolled about the edge fills the gap.
    Opening,
    /// The offsets cross: the edge becomes their intersection.
    Closing,
}

/// The faces, their distances and offsets, and the edges' classes.
pub(crate) struct Plan<'a> {
    pub(crate) body: &'a Body,
    /// Each face's distance, positive outwards.
    pub(crate) distance: BTreeMap<FaceId, f64>,
    /// Each face's offset surface, keeping the surface's parameters.
    pub(crate) surface: BTreeMap<FaceId, Surface>,
    /// Each edge's class, and for a smooth edge how far its faces'
    /// offsets lie apart.
    pub(crate) class: BTreeMap<EdgeId, (Class, f64)>,
    /// The live edges in name order.
    pub(crate) edges: Vec<EdgeId>,
}

impl<'a> Plan<'a> {
    /// The plan for `spec`: fails with `BadDistance`, `NoSuchFace` (a name
    /// that isn't a face's, or one named twice), `FaceCollapses`, and for
    /// edges `Unsupported` (`PartlySmooth`) or `Uncertain`.
    pub(crate) fn new(body: &'a Body, spec: &OffsetSpec) -> Result<Self, Error> {
        let mut faces: Vec<FaceId> = body.faces().collect();
        if faces.is_empty() {
            return Err(OffsetError::BadDistance.into());
        }
        faces.sort_by_key(|&f| body.faces[f].name);
        let by_name: BTreeMap<Name, FaceId> =
            faces.iter().map(|&f| (body.faces[f].name, f)).collect();
        let mut distance: BTreeMap<FaceId, f64> = faces.iter().map(|&f| (f, spec.distance)).collect();
        let mut named: Vec<Name> = Vec::new();
        for &(face, d) in &spec.faces {
            let f = *by_name
                .get(&face)
                .ok_or(OffsetError::NoSuchFace { face })?;
            if named.contains(&face) {
                return Err(OffsetError::NoSuchFace { face }.into());
            }
            named.push(face);
            distance.insert(f, d);
        }
        let usable = |d: f64| d.is_finite() && (d == 0.0 || d.abs() >= 10.0 * LINEAR_RESOLUTION);
        if !distance.values().all(|&d| usable(d)) || distance.values().all(|&d| d == 0.0) {
            return Err(OffsetError::BadDistance.into());
        }
        let surface: BTreeMap<FaceId, Surface> = faces
            .par_iter()
            .map(|&f| offset_surface(body, f, distance[&f]).map(|s| (f, s)))
            .collect::<Result<_, Error>>()?;
        let mut edges: Vec<EdgeId> = body.edges().collect();
        edges.sort_by_key(|&e| body.edges[e].name);
        let class: BTreeMap<EdgeId, (Class, f64)> = edges
            .par_iter()
            .map(|&e| classify(body, &distance, e).map(|c| (e, c)))
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            body,
            distance,
            surface,
            class,
            edges,
        })
    }

    /// The edge's two fins: `[0]` runs along it, `[1]` against it.
    pub(crate) fn fins(&self, e: EdgeId) -> [FinId; 2] {
        fins(self.body, e)
    }

    /// The face a fin lies on.
    pub(crate) fn face_of(&self, fin: FinId) -> FaceId {
        self.body.loops[self.body.fins[fin].loop_].face
    }

    /// The face's outward normal at the edge's parameter `t`, through the
    /// fin's pcurve; zero where its surface has none.
    pub(crate) fn normal(&self, fin: FinId, t: f64) -> Vector3 {
        normal(self.body, fin, t)
    }

    /// The face's distance along its outward normal.
    pub(crate) fn d(&self, f: FaceId) -> f64 {
        self.distance[&f]
    }
}

/// The edge's two fins: `[0]` runs along it, `[1]` against it.
pub(crate) fn fins(body: &Body, e: EdgeId) -> [FinId; 2] {
    let fins: Vec<FinId> = body.edge_fins(e).collect();
    if body.fins[fins[0]].sense == Sense::Forward {
        [fins[0], fins[1]]
    } else {
        [fins[1], fins[0]]
    }
}

/// A fin's face's outward normal at its edge's parameter `t`.
pub(crate) fn normal(body: &Body, fin: FinId, t: f64) -> Vector3 {
    let face = &body.faces[body.loops[body.fins[fin].loop_].face];
    let s = &body.surfaces[face.surface];
    let sign = if face.sense == Sense::Forward {
        1.0
    } else {
        -1.0
    };
    body.fin_uv(fin, t)
        .and_then(|uv| s.normal(uv.x, uv.y))
        .map_or(Vector3::ZERO, |n| n.get() * sign)
}

/// The face's surface moved by its distance along its outward normal:
/// the surface itself at distance zero, an analytic surface or a pipe of
/// its own kind, any other an [`crate::geom::Offset`] over the face's box
/// grown by [`MARGIN`] each way (or the box alone, where that folds).
fn offset_surface(body: &Body, f: FaceId, d: f64) -> Result<Surface, Error> {
    let face = &body.faces[f];
    let s = &body.surfaces[face.surface];
    if d == 0.0 {
        return Ok(s.clone());
    }
    let d = if face.sense == Sense::Forward { d } else { -d };
    let collapses = || -> Error {
        OffsetError::FaceCollapses { face: face.name }.into()
    };
    if s.frame().is_some() || matches!(s, Surface::Pipe(_)) {
        return s
            .offset(d)
            .ok_or_else(collapses)?
            .map_err(|_| collapses());
    }
    let whole = s.domain();
    let (pu, pv) = s.periods();
    let periods = [pu, pv];
    let rect = face_rect(body, f).unwrap_or(whole);
    // Past the box by a share of it, and at least three times the
    // distance in model space: closing edges are searched for within two
    // distances of their edges, so no two faces' offsets end together
    // there, where no crossing could be certified.
    let mid = s.eval(
        (rect[0][0] + rect[0][1]) / 2.0,
        (rect[1][0] + rect[1][1]) / 2.0,
    );
    let speed = [mid.du.length(), mid.dv.length()];
    let grown: [[f64; 2]; 2] = core::array::from_fn(|k| {
        let [a, b] = rect[k];
        let reach = if speed[k] > 0.0 {
            3.0 * d.abs() / speed[k]
        } else {
            0.0
        };
        let m = ((b - a) * MARGIN).max(reach);
        match periods[k] {
            Some(p) if b - a + 2.0 * m >= p => whole[k],
            Some(_) => [a - m, b + m],
            None => [(a - m).max(whole[k][0]), (b + m).min(whole[k][1])],
        }
    });
    match s.offset_within(d, grown) {
        Some(Ok(o)) => Ok(o),
        _ => s
            .offset_within(d, rect)
            .ok_or_else(collapses)?
            .map_err(|_| collapses()),
    }
}

/// How the edge's faces move, and for a smooth edge how far apart their
/// offsets lie along it.
fn classify(
    body: &Body,
    distance: &BTreeMap<FaceId, f64>,
    e: EdgeId,
) -> Result<(Class, f64), Error> {
    let at = body.edges[e].name;
    let [fa, fb] = fins(body, e);
    let face = |fin: FinId| body.loops[body.fins[fin].loop_].face;
    let (da, db) = (distance[&face(fa)], distance[&face(fb)]);
    if da != db {
        return Ok((Class::Closing, 0.0));
    }
    if da == 0.0 {
        return Ok((Class::Kept, 0.0));
    }
    let [t0, t1] = body.edges[e].interval;
    let mut gap = 0.0f64;
    let mut smooth = true;
    for i in 0..SAMPLES {
        let t = t0 + (t1 - t0) * i as f64 / (SAMPLES - 1) as f64;
        let (a, b) = (normal(body, fa, t), normal(body, fb, t));
        let angle = crate::math::atan2(a.cross(b).length(), a.dot(b));
        gap = gap.max(angle * da.abs());
        if !(angle * da.abs() <= SMOOTH_GAP) {
            smooth = false;
        }
    }
    if smooth {
        return Ok((Class::Smooth, gap));
    }
    let convex = match bend(body, e) {
        Ok(Bend::Convex) => true,
        Ok(Bend::Concave) => false,
        Ok(Bend::Smooth) | Err(BendFail::Changes) => {
            return Err(OffsetError::Unsupported {
                at,
                case: OffsetCase::PartlySmooth,
            }
            .into());
        }
        Err(BendFail::Uncertain) => return Err(super::uncertain(at, "classify.rs#1").into()),
    };
    Ok((
        if convex == (da > 0.0) {
            Class::Opening
        } else {
            Class::Closing
        },
        0.0,
    ))
}
