//! Pcurves for every use of an edge: exact where [`pcurve::exact`] finds
//! one, else the file's, else one fitted by projection.
//!
//! A file's pcurve runs on its own parameter, which an importer may have
//! changed from the edge's (a line's is its length, whatever speed the
//! file gave it). Its affine map from the edge's parameter is found
//! rather than assumed: the identity first (most writers keep the two
//! parameters equal), then the map through the pcurve's parameters at
//! the edge's ends, and for a closed edge on a closed pcurve the turn
//! either way from its start. A map counts only where the pcurve fits
//! the edge at every sample, as an exact one must.
//!
//! A fitted pcurve (a cubic spline through the edge's points projected
//! onto the surface, [`crate::intersect::fit_pcurve`]) comes with a
//! certified bound on how far the surface takes it from the edge; the
//! edge's tolerance grows to cover it, and its search stops at 512
//! samples. A fit is accepted within ten times the tolerance, so an edge
//! farther than that from the surface at any of 17 points is
//! `GapTooLarge` at once, with the gap measured.

use super::names::Names;
use super::{Face, Loop, Shape, Use};
use crate::geom::{Curve, Curve2, Point2, Surface};
use crate::intersect::fit_pcurve_within;
use crate::pcurve;
use crate::tol::LINEAR_RESOLUTION;
use crate::{Error, HealError};
use rayon::prelude::*;
use std::sync::Arc;

/// The most samples a fitted pcurve may take: an edge needing more is no
/// edge a file means, and searching further only costs time.
const FIT_SAMPLES: usize = 512;

/// A use's pcurve: a curve, the affine map from the edge's parameter to
/// its own, and how far its image may stray from the edge (0 unless it
/// was fitted).
pub(super) type Found = (Curve2, [f64; 2], f64);

/// Each face's pcurves, by loop and use. A lone vertex's loop has none.
pub(super) type Pcurves = Vec<Vec<Vec<Found>>>;

pub(super) fn pcurves(s: &Shape, names: &Names) -> Result<Pcurves, Error> {
    s.faces
        .par_iter()
        .enumerate()
        .map(|(k, f)| {
            f.loops
                .iter()
                .map(|l| match l {
                    Loop::Vertex(_) => Ok(Vec::new()),
                    Loop::Edges(uses) => uses
                        .iter()
                        .map(|u| match one(s, f, u) {
                            Ok(Some(found)) => Ok(found),
                            Ok(None) => Err(HealError::NoPcurve {
                                edge: names.edges[u.edge],
                                face: names.faces[k],
                            }
                            .into()),
                            Err(gap) => Err(HealError::GapTooLarge {
                                entity: names.edges[u.edge],
                                gap,
                            }
                            .into()),
                        })
                        .collect(),
                })
                .collect()
        })
        .collect()
}

/// The tolerance a use's pcurve must fit within.
pub(super) fn tolerance(s: &Shape, f: &Face, u: &Use) -> f64 {
    s.edges[u.edge]
        .tolerance
        .max(f.tolerance)
        .max(LINEAR_RESOLUTION)
}

/// A use's pcurve, `None` if none is found, or the gap between the edge
/// and the face's surface where that is too wide to fit one across.
fn one(s: &Shape, f: &Face, u: &Use) -> Result<Option<Found>, f64> {
    let e = &s.edges[u.edge];
    let surface = &s.surfaces[f.surface];
    let curve = &s.curves[e.curve];
    let tol = tolerance(s, f, u);
    if let Some((c2, map)) = pcurve::exact(surface, curve, e.interval, tol) {
        return Ok(Some((c2, map, 0.0)));
    }
    if let Some(i) = u.pcurve {
        let c2 = &s.curves2[i];
        if let Some(map) = file_map(surface, curve, e.interval, c2, tol) {
            return Ok(Some((c2.clone(), map, 0.0)));
        }
    }
    // A fit is accepted within ten times the tolerance: an edge farther
    // from the surface than that needn't be fitted to find out.
    let [t0, t1] = e.interval;
    let gap = (0..17)
        .map(|i| {
            let t = t0 + (t1 - t0) * i as f64 / 16.0;
            surface.project(curve.point(t)).distance
        })
        .fold(0.0, f64::max);
    if gap > 10.0 * tol {
        return Err(gap);
    }
    // An edge may lie its tolerance off the surface. A fit's bound can't
    // come within less than that, and its spans halve only while it is
    // under half their target: an edge with a tolerance of its own, or
    // measurably off the surface, aims at twice the larger of the two,
    // and its tolerance grows to the bound.
    let target = if tol > LINEAR_RESOLUTION || gap > LINEAR_RESOLUTION / 4.0 {
        2.0 * tol.max(gap)
    } else {
        LINEAR_RESOLUTION
    };
    Ok(
        fit_pcurve_within(surface, curve, e.interval, target, FIT_SAMPLES)
            .ok()
            .map(|(c2, bound)| (Curve2::Nurbs2(Arc::new(c2)), [0.0, 1.0], bound)),
    )
}

/// The affine map that makes a file's pcurve fit its edge, if any.
fn file_map(
    surface: &Surface,
    curve: &Curve,
    [t0, t1]: [f64; 2],
    c2: &Curve2,
    tol: f64,
) -> Option<[f64; 2]> {
    let fits = |map: [f64; 2]| pcurve::fits(surface, curve, [t0, t1], tol, &(c2.clone(), map));
    if fits([0.0, 1.0]) {
        return Some([0.0, 1.0]);
    }
    let (s0, s1) = (at(surface, c2, curve, t0)?, at(surface, c2, curve, t1)?);
    let mut maps = Vec::new();
    if s1 != s0 {
        let b = (s1 - s0) / (t1 - t0);
        maps.push([s0 - b * t0, b]);
    }
    if let Some(p) = c2.period() {
        for b in [p / (t1 - t0), -p / (t1 - t0)] {
            maps.push([s0 - b * t0, b]);
        }
    }
    maps.into_iter().find(|&m| fits(m))
}

/// The pcurve's parameter where it meets the edge's point at `t`: the
/// point's parameters on the surface, moved by whole periods to where
/// the pcurve is nearest, projected onto the pcurve.
fn at(surface: &Surface, c2: &Curve2, curve: &Curve, t: f64) -> Option<f64> {
    let (u, v) = surface.project(curve.point(t)).param;
    let (pu, pv) = surface.periods();
    let shifts = |p: Option<f64>| match p {
        Some(p) => vec![0.0, -p, p],
        None => vec![0.0],
    };
    let mut best: Option<(f64, f64)> = None;
    for du in shifts(pu) {
        for dv in shifts(pv) {
            let q = c2.project(Point2::new(u + du, v + dv));
            if best.is_none_or(|(d, _)| q.distance < d) {
                best = Some((q.distance, q.param));
            }
        }
    }
    best.map(|(_, s)| s)
}
