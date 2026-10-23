//! Pcurves for every use of an edge: exact where [`pcurve::exact`] finds
//! one, else the file's.
//!
//! A file's pcurve runs on its own parameter, which an importer may have
//! changed from the edge's (a line's is its length, whatever speed the
//! file gave it). Its affine map from the edge's parameter is found
//! rather than assumed: the identity first (most writers keep the two
//! parameters equal), then the map through the pcurve's parameters at
//! the edge's ends, and for a closed edge on a closed pcurve the turn
//! either way from its start. A map counts only where the pcurve fits
//! the edge at every sample, as an exact one must.

use super::names::Names;
use super::{Face, Loop, Shape, Use};
use crate::geom::{Curve, Curve2, Point2, Surface};
use crate::pcurve;
use crate::tol::LINEAR_RESOLUTION;
use crate::{Error, HealError};
use rayon::prelude::*;

/// Each face's pcurves, by loop and use: a curve and the affine map from
/// the edge's parameter to its own. A lone vertex's loop has none.
pub(super) type Pcurves = Vec<Vec<Vec<(Curve2, [f64; 2])>>>;

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
                        .map(|u| {
                            one(s, f, u).ok_or_else(|| {
                                HealError::NoPcurve {
                                    edge: names.edges[u.edge],
                                    face: names.faces[k],
                                }
                                .into()
                            })
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

fn one(s: &Shape, f: &Face, u: &Use) -> Option<(Curve2, [f64; 2])> {
    let e = &s.edges[u.edge];
    let surface = &s.surfaces[f.surface];
    let curve = &s.curves[e.curve];
    let tol = tolerance(s, f, u);
    if let Some(p) = pcurve::exact(surface, curve, e.interval, tol) {
        return Some(p);
    }
    let c2 = &s.curves2[u.pcurve?];
    let map = file_map(surface, curve, e.interval, c2, tol)?;
    Some((c2.clone(), map))
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
