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
//! An exact or file pcurve that fits within the tolerance but misses the
//! edge's ends by more than resolution (an iso-line under a spline edge
//! whose parameter isn't affine in the line's, as writers approximate
//! arcs) is mapped again through its parameters nearest the edge's ends,
//! where that still fits: the pcurves of a face's neighbouring edges then
//! meet where their vertex projects, as the mesher needs, rather than
//! passing each other across a seam there.
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
use crate::intersect::{fit_pcurve_within, pcurve_certified};
use crate::pcurve;
use crate::tol::LINEAR_RESOLUTION;
use crate::{Error, HealError};
use rayon::prelude::*;
use std::sync::Arc;

/// The most samples a fitted pcurve may take, but for a spline edge of
/// more than a quarter as many pieces: an edge needing more is no edge a
/// file means, and searching further only costs time.
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
        let map = pinned(surface, curve, e.interval, tol, &c2, map);
        if let Some(bound) = bounded(surface, curve, e.interval, &c2, map, tol) {
            return Ok(Some((c2, map, bound)));
        }
    }
    if let Some(i) = u.pcurve {
        let c2 = &s.curves2[i];
        if let Some(map) = file_map(surface, curve, e.interval, c2, tol) {
            let map = pinned(surface, curve, e.interval, tol, c2, map);
            if let Some(bound) = bounded(surface, curve, e.interval, c2, map, tol) {
                return Ok(Some((c2.clone(), map, bound)));
            }
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
    // A spline edge of many pieces (a blend's rail fitted on a spline
    // face) needs about as many samples: four a piece, if that is more.
    let pieces = match curve {
        Curve::Nurbs(n) => n.pieces().len(),
        _ => 0,
    };
    let samples = FIT_SAMPLES.max(4 * pieces);
    let r = fit_pcurve_within(surface, curve, e.interval, target, samples);
    Ok(r.ok()
        .map(|(c2, bound)| (Curve2::Nurbs2(Arc::new(c2)), [0.0, 1.0], bound)))
}

/// Samples per knot span when screening a pcurve (see [`bounded`]).
const SCREEN: usize = 8;

/// The farthest `surface` takes `c2` from `curve` at samples dense in
/// both: [`SCREEN`] per span of 64 equal ones, of the curve's knot spans
/// and of the pcurve's.
fn screened(
    surface: &Surface,
    curve: &Curve,
    [t0, t1]: [f64; 2],
    c2: &Curve2,
    map: [f64; 2],
) -> f64 {
    let mut stops: Vec<f64> = (0..=64).map(|i| t0 + (t1 - t0) * i as f64 / 64.0).collect();
    if let Curve::Nurbs(n) = curve {
        stops.extend(n.knots().knots().iter().copied());
    }
    if let Curve2::Nurbs2(n) = c2
        && map[1] != 0.0
    {
        stops.extend(n.knots().knots().iter().map(|&s| (s - map[0]) / map[1]));
    }
    stops.retain(|&t| (t0..=t1).contains(&t));
    stops.sort_by(f64::total_cmp);
    stops.dedup();
    let mut worst = 0.0f64;
    for w in stops.windows(2) {
        for k in 0..SCREEN {
            let t = w[0] + (w[1] - w[0]) * k as f64 / SCREEN as f64;
            let q = c2.point(map[0] + map[1] * t);
            worst = worst.max(surface.point(q.x, q.y).distance(curve.point(t)));
        }
    }
    worst
}

/// How far a pcurve found at 17 samples may stray from its edge. Where
/// the edge is a spline or the surface not analytic, a file's pcurve (an
/// approximation) or an iso-line (under an edge whose parameter isn't
/// affine in the surface's) can meet the edge at those samples and stray
/// between them, so it is screened at many more: within `tol` there it
/// counts as before, 0. Past it, a line or a polynomial spline on an
/// analytic, spline or pipe surface is certified: its bound where that
/// is within ten times `tol` (the edge's tolerance grows to it, as for a
/// fit), else `None`, and it is fitted instead, as is a pcurve that
/// can't be certified. Analytic pairs give 0: their exact pcurves are
/// exact.
fn bounded(
    surface: &Surface,
    curve: &Curve,
    interval: [f64; 2],
    c2: &Curve2,
    map: [f64; 2],
    tol: f64,
) -> Option<f64> {
    let analytic_curve = matches!(curve, Curve::Line(_) | Curve::Circle(_) | Curve::Ellipse(_));
    let analytic_surface = matches!(
        surface,
        Surface::Plane(_)
            | Surface::Cylinder(_)
            | Surface::Cone(_)
            | Surface::Sphere(_)
            | Surface::Torus(_)
    );
    if analytic_curve && analytic_surface {
        return Some(0.0);
    }
    if screened(surface, curve, interval, c2, map) <= tol {
        return Some(0.0);
    }
    match pcurve_certified(surface, curve, interval, c2, map, tol)? {
        b if b <= tol => Some(0.0),
        b if b <= 10.0 * tol => Some(b),
        _ => None,
    }
}

/// `map`, or where the pcurve's image misses either end of the edge by
/// more than resolution, the map through the pcurve's parameters nearest
/// the ends, if that fits too.
fn pinned(
    surface: &Surface,
    curve: &Curve,
    [t0, t1]: [f64; 2],
    tol: f64,
    c2: &Curve2,
    map: [f64; 2],
) -> [f64; 2] {
    let misses = |m: [f64; 2], t: f64| {
        let q = c2.point(m[0] + m[1] * t);
        surface.point(q.x, q.y).distance(curve.point(t)) > LINEAR_RESOLUTION
    };
    if !misses(map, t0) && !misses(map, t1) {
        return map;
    }
    // The pcurve's parameter nearest the edge's point at `t`, its
    // surface parameters and its own moved by whole periods to be nearest
    // where the map puts them.
    let (pu, pv) = surface.periods();
    let near = |x: f64, w: f64, p: Option<f64>| match p {
        Some(p) => x + ((w - x) / p).round() * p,
        None => x,
    };
    let end = |t: f64| {
        let want = c2.point(map[0] + map[1] * t);
        let (u, v) = surface.project(curve.point(t)).param;
        let q = Point2::new(near(u, want.x, pu), near(v, want.y, pv));
        near(c2.project(q).param, map[0] + map[1] * t, c2.period())
    };
    let (s0, s1) = (end(t0), end(t1));
    if s1 == s0 {
        return map;
    }
    let b = (s1 - s0) / (t1 - t0);
    let new = [s0 - b * t0, b];
    if pcurve::fits(surface, curve, [t0, t1], tol, &(c2.clone(), new)) {
        new
    } else {
        map
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Cylinder, Frame, KnotVector, NurbsCurve, Point3};
    use crate::topo::Sense;

    /// A spline line up a unit cylinder's side, `x = 1, y = 0`, from
    /// `z = 0` to `1` over `[0, 16]`: sixteen quadratic pieces meeting at
    /// the integers, each piece's middle control point `bulge` further up.
    /// With a bulge the line runs at an uneven speed, but meets the even
    /// one at the integers.
    fn shape(bulge: f64) -> Shape {
        let at = |z: f64| Point3::new(1.0, 0.0, z / 16.0);
        let mut points = vec![at(0.0)];
        for k in 0..16 {
            let k = f64::from(k);
            points.push(Point3::new(1.0, 0.0, (k + 0.5) / 16.0 + bulge));
            points.push(at(k + 1.0));
        }
        let knots: Vec<f64> = (0..=16).map(f64::from).collect();
        let mut mults = vec![2; 17];
        mults[0] = 3;
        mults[16] = 3;
        let kv = KnotVector::clamped(2, &knots, &mults).unwrap();
        let line = NurbsCurve::new(kv, points, None).unwrap();
        Shape {
            surfaces: vec![Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap())],
            curves: vec![Curve::Nurbs(Arc::new(line))],
            curves2: vec![],
            vertices: vec![
                super::super::Vertex {
                    point: at(0.0),
                    tolerance: 0.0,
                    seam: false,
                },
                super::super::Vertex {
                    point: at(16.0),
                    tolerance: 0.0,
                    seam: false,
                },
            ],
            edges: vec![super::super::Edge {
                curve: 0,
                interval: [0.0, 16.0],
                start: 0,
                end: 1,
                tolerance: 0.0,
            }],
            faces: vec![Face {
                surface: 0,
                sense: Sense::Forward,
                loops: vec![Loop::Edges(vec![use_()])],
                tolerance: 0.0,
            }],
            shells: vec![],
            solids: vec![],
        }
    }

    fn use_() -> Use {
        Use {
            edge: 0,
            sense: Sense::Forward,
            pcurve: None,
        }
    }

    #[test]
    fn an_iso_line_that_strays_between_its_samples_is_fitted_instead() {
        // At an even speed the side's ruling is the edge's pcurve.
        let s = shape(0.0);
        let (c2, _, bound) = one(&s, &s.faces[0], &use_()).unwrap().unwrap();
        assert!(matches!(c2, Curve2::Line2(_)));
        assert_eq!(bound, 0.0);
        // A micrometre off it between the integers, where the 17 samples
        // all fall: screened, certified past ten tolerances, and fitted.
        let s = shape(1e-6);
        let (c2, map, bound) = one(&s, &s.faces[0], &use_()).unwrap().unwrap();
        assert!(matches!(c2, Curve2::Nurbs2(_)));
        let tol = bound.max(LINEAR_RESOLUTION);
        let (surface, curve) = (&s.surfaces[0], &s.curves[0]);
        for k in 0..=1600 {
            let t = 16.0 * f64::from(k) / 1600.0;
            let q = c2.point(map[0] + map[1] * t);
            assert!(
                surface.point(q.x, q.y).distance(curve.point(t)) <= tol,
                "at {t}"
            );
        }
    }
}
