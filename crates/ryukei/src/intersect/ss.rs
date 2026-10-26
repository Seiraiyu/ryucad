//! Surface–surface intersection: the entry point and its results.
//!
//! A coincident pair is reported as such; a pair with a closed form
//! ([`super::ss_exact`]) gets exact curves; any other analytic pair is
//! traced ([`super::ss_trace`]) and fitted ([`super::fit`]), and any other
//! surface (a pipe, a NURBS, a swept surface) and an analytic one are
//! traced in the first's parameter plane ([`super::sheet_trace`]). Every
//! branch
//! comes with its pcurve on each surface: an exact one where
//! [`crate::pcurve::exact`] finds it, otherwise one fitted by the curve's
//! parameter with a certified bound, which the branch's tolerance covers.
//! A branch is cut where it passes a sphere's pole or a cone's apex,
//! since no single pcurve can run through one.

use super::Failure;
use super::coincide::coincident;
use super::fit::{fit_branch, fit_pcurve};
use super::implicit::Implicit;
use super::sheet_trace;
use super::ss_exact;
use super::ss_trace::{Traced, rank, trace};
use crate::geom::{Box3, Curve, Curve2, Point3, Surface};
use crate::tol::LINEAR_RESOLUTION;
use std::sync::Arc;

/// One branch of the curve where two surfaces meet.
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub curve: Curve,
    /// The stretch of the curve's parameter the branch covers: a whole
    /// period for a closed curve.
    pub range: [f64; 2],
    /// The surfaces touch along the branch without crossing.
    pub tangent: bool,
    /// The branch is within this of both surfaces, and so are its
    /// pcurves' images of it (at least
    /// [`crate::tol::LINEAR_RESOLUTION`]).
    pub tolerance: f64,
    /// The branch on each surface, in the order the surfaces were given:
    /// a pcurve and the affine map from the curve's parameter to the
    /// pcurve's, as in [`crate::topo::Pcurve`].
    pub pcurves: [(Curve2, [f64; 2]); 2],
}

/// How two surfaces meet.
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceHit {
    /// The same surface by the coincidence rule; `same` when their natural
    /// normals agree.
    Coincident { same: bool },
    /// Where they meet in the region: curves, and points where they touch
    /// or where a curve degenerates (a cone's apex, a pole).
    Meet {
        branches: Vec<Branch>,
        points: Vec<Point3>,
    },
}

/// A branch before its pcurves.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Raw {
    pub(crate) curve: Curve,
    pub(crate) range: [f64; 2],
    pub(crate) tangent: bool,
    pub(crate) tolerance: f64,
}

/// Where surfaces `a` and `b` meet inside `region`. Curves are clipped to
/// the region where they are unbounded, and closed curves that meet it
/// are returned whole.
///
/// A pipe, a NURBS or a swept surface and an analytic one are traced in
/// the first's parameter plane, as [`pipe_surface`] traces a pipe.
///
/// Fails with `Unsupported` unless at least one surface is analytic, and
/// with `Uncertain` where a branch's topology, a singular point or a fit
/// could not be certified.
pub fn surface_surface(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    match (Implicit::of(a).is_some(), Implicit::of(b).is_some()) {
        (true, true) => {}
        (false, false) => return Err(Failure::Unsupported),
        _ => return sheet_surface(a, b, region),
    }
    if let Some(same) = coincident(a, b) {
        return Ok(SurfaceHit::Coincident { same });
    }
    if let Some((raws, mut points)) = ss_exact::exact(a, b, region) {
        let mut branches = Vec::new();
        for raw in raws {
            for piece in split_at_poles(raw, [a, b]) {
                // A sliver the region clips off a curve is a point.
                if let Some(p) = sliver(&piece) {
                    points.push(p);
                    continue;
                }
                branches.push(with_pcurves(piece, [a, b])?);
            }
        }
        return Ok(SurfaceHit::Meet { branches, points });
    }
    let topo = trace(a, b, region)?;
    // The tracer takes the surface of lower rank as its first.
    let swapped = rank(a) > rank(b);
    let order = if swapped { [b, a] } else { [a, b] };
    let mut chains = topo.branches;
    for s in [a, b] {
        for pole in poles(s) {
            if a.distance(pole) <= LINEAR_RESOLUTION && b.distance(pole) <= LINEAR_RESOLUTION {
                chains = cut_at(chains, pole);
            }
        }
    }
    let mut points = topo.points;
    let branches = fitted(&chains, order, swapped, &mut points)?;
    Ok(SurfaceHit::Meet { branches, points })
}

/// Traced branches fitted with their pcurves on the surfaces in `order`,
/// given back in the caller's order (`swapped` when that is the
/// reverse). A branch whose points all lie within ten resolutions of its
/// first is a point by rule, added to `points`.
fn fitted(
    chains: &[Traced],
    order: [&Surface; 2],
    swapped: bool,
    points: &mut Vec<Point3>,
) -> Result<Vec<Branch>, Failure> {
    let mut branches = Vec::with_capacity(chains.len());
    for traced in chains {
        let first = traced.points[0];
        if traced
            .points
            .iter()
            .all(|p| p.distance(first) <= 10.0 * LINEAR_RESOLUTION)
        {
            points.push(first);
            continue;
        }
        let fitted = fit_branch(order, traced)?;
        let [p0, p1] = fitted.pcurves;
        let identity = [0.0, 1.0];
        let (p0, p1) = (
            (Curve2::Nurbs2(Arc::new(p0)), identity),
            (Curve2::Nurbs2(Arc::new(p1)), identity),
        );
        let range = fitted.curve.domain();
        branches.push(Branch {
            curve: Curve::Nurbs(Arc::new(fitted.curve)),
            range,
            tangent: false,
            tolerance: fitted.tolerance,
            pcurves: if swapped { [p1, p0] } else { [p0, p1] },
        });
    }
    Ok(branches)
}

/// Where a pipe meets an analytic surface inside `region`: one of `a`
/// and `b` must be a pipe and the other analytic (else `Unsupported`).
/// The branches are traced in the pipe's parameter plane (see
/// [`super::sheet_trace`]), cut where they pass a pole or an apex of the
/// analytic surface, and fitted with a pcurve on each, in the order the
/// surfaces were given. A tangency along a curve (a fillet against its
/// own support) is `Uncertain`.
pub fn pipe_surface(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    match (a, b) {
        (Surface::Pipe(_), o) | (o, Surface::Pipe(_)) if Implicit::of(o).is_some() => {
            sheet_surface(a, b, region)
        }
        _ => Err(Failure::Unsupported),
    }
}

/// Where a surface with no implicit form (a pipe, a NURBS, a swept
/// surface) meets an analytic one inside `region`, one of each in either
/// order (else `Unsupported`): traced in the first's parameter plane,
/// cut where branches pass a pole or an apex of either surface, and
/// fitted with a pcurve on each, in the order the surfaces were given.
fn sheet_surface(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    let (sheet, other, swapped) = match (Implicit::of(a).is_some(), Implicit::of(b).is_some()) {
        (false, true) => (a, b, false),
        (true, false) => (b, a, true),
        _ => return Err(Failure::Unsupported),
    };
    let topo = sheet_trace::trace(sheet, other, region)?;
    let mut chains = topo.branches;
    for pole in poles(other).into_iter().chain(poles(sheet)) {
        if sheet.distance(pole) <= LINEAR_RESOLUTION && other.distance(pole) <= LINEAR_RESOLUTION {
            chains = cut_at(chains, pole);
        }
    }
    let mut points = topo.points;
    let branches = fitted(&chains, [sheet, other], swapped, &mut points)?;
    Ok(SurfaceHit::Meet { branches, points })
}

/// Traced branches with the one that passes `pole` (a point of both
/// surfaces) cut there: the pole goes in at the nearest stretch of the
/// branch's points, and becomes an end of both pieces (both ends of a
/// closed branch, opened there).
fn cut_at(chains: Vec<Traced>, pole: Point3) -> Vec<Traced> {
    let seg_distance = |a: Point3, b: Point3| {
        let d = b - a;
        let len2 = d.length_squared();
        let t = if len2 > 0.0 {
            ((pole - a).dot(d) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (a + d * t).distance(pole)
    };
    // The nearest stretch over all branches, if the pole is within its
    // length of it.
    let mut best: Option<(usize, usize, f64)> = None;
    for (k, c) in chains.iter().enumerate() {
        let n = c.points.len();
        let last = if c.closed { n } else { n - 1 };
        for i in 0..last {
            let (p, q) = (c.points[i], c.points[(i + 1) % n]);
            let d = seg_distance(p, q);
            if d <= p.distance(q).max(LINEAR_RESOLUTION) && best.is_none_or(|b| d < b.2) {
                best = Some((k, i, d));
            }
        }
    }
    let Some((k, i, _)) = best else {
        return chains;
    };
    let mut out = Vec::with_capacity(chains.len() + 1);
    for (j, c) in chains.into_iter().enumerate() {
        if j != k {
            out.push(c);
            continue;
        }
        let n = c.points.len();
        if c.closed {
            let mut pts = vec![pole];
            pts.extend((1..=n).map(|m| c.points[(i + m) % n]));
            pts.push(pole);
            out.push(Traced {
                points: pts,
                closed: false,
            });
        } else {
            let mut first: Vec<Point3> = c.points[..=i].to_vec();
            first.push(pole);
            let mut second = vec![pole];
            second.extend_from_slice(&c.points[i + 1..]);
            for part in [first, second] {
                if part.len() >= 2 {
                    out.push(Traced {
                        points: part,
                        closed: false,
                    });
                }
            }
        }
    }
    out
}

/// The points where a surface's parameterization collapses: a sphere's
/// poles, a cone's apex, a revolution's poles (its curve's ends on the
/// axis), a NURBS's collapsed boundary rows.
fn poles(s: &Surface) -> Vec<Point3> {
    match s {
        Surface::Sphere(sp) => {
            let f = s.frame().expect("an analytic surface");
            let z = f.z().get() * sp.radius();
            vec![f.origin() + z, f.origin() - z]
        }
        Surface::Cone(c) => {
            let f = s.frame().expect("an analytic surface");
            vec![f.origin() + f.z().get() * c.apex_v()]
        }
        Surface::Revolution(r) => {
            let [d0, d1] = r.curve().domain();
            [d0, d1]
                .into_iter()
                .zip(r.poles())
                .filter(|&(_, pole)| pole)
                .map(|(t, _)| r.curve().point(t))
                .collect()
        }
        Surface::Nurbs(n) => n.collapsed_rows(),
        _ => Vec::new(),
    }
}

/// A branch cut where it passes a pole or apex of either surface.
fn split_at_poles(raw: Raw, surfaces: [&Surface; 2]) -> Vec<Raw> {
    let [r0, r1] = raw.range;
    let period = raw
        .curve
        .period()
        .filter(|&p| (r1 - r0 - p).abs() <= 1e-12 * p);
    let mut cuts: Vec<f64> = Vec::new();
    for s in surfaces {
        for pole in poles(s) {
            let foot = raw.curve.project(pole);
            let at_end = |t: f64| raw.curve.point(t).distance(pole) <= raw.tolerance;
            if foot.distance > raw.tolerance || (period.is_none() && (at_end(r0) || at_end(r1))) {
                continue;
            }
            let t = match period {
                Some(p) => r0 + (foot.param - r0).rem_euclid(p),
                None => foot.param,
            };
            let inside = period.is_some() || (t > r0 && t < r1);
            if inside && !cuts.iter().any(|&c| (c - t).abs() <= 1e-12) {
                cuts.push(t);
            }
        }
    }
    if cuts.is_empty() {
        return vec![raw];
    }
    cuts.sort_by(f64::total_cmp);
    let stretches: Vec<[f64; 2]> = match period {
        Some(p) => (0..cuts.len())
            .map(|i| {
                let next = if i + 1 < cuts.len() {
                    cuts[i + 1]
                } else {
                    cuts[0] + p
                };
                [cuts[i], next]
            })
            .collect(),
        None => {
            let mut ends = vec![r0];
            ends.extend(&cuts);
            ends.push(r1);
            ends.windows(2).map(|w| [w[0], w[1]]).collect()
        }
    };
    stretches
        .into_iter()
        .map(|range| Raw {
            range,
            ..raw.clone()
        })
        .collect()
}

/// The middle of a branch that stays within ten tolerances of it, which
/// is a point by rule.
fn sliver(raw: &Raw) -> Option<Point3> {
    let [r0, r1] = raw.range;
    let mid = raw.curve.point((r0 + r1) / 2.0);
    (0..=8)
        .all(|k| {
            raw.curve
                .point(r0 + (r1 - r0) * f64::from(k) / 8.0)
                .distance(mid)
                <= 10.0 * raw.tolerance
        })
        .then_some(mid)
}

/// A branch with its pcurve on each surface: exact where it can be,
/// otherwise fitted, the branch's tolerance growing to cover the fit.
fn with_pcurves(raw: Raw, surfaces: [&Surface; 2]) -> Result<Branch, Failure> {
    let mut tolerance = raw.tolerance;
    let mut found = Vec::with_capacity(2);
    for s in surfaces {
        match crate::pcurve::exact(s, &raw.curve, raw.range, raw.tolerance) {
            Some(p) => found.push(p),
            None => {
                let (fitted, bound) = fit_pcurve(s, &raw.curve, raw.range)?;
                tolerance = tolerance.max(bound);
                found.push((Curve2::Nurbs2(Arc::new(fitted)), [0.0, 1.0]));
            }
        }
    }
    let [p0, p1]: [(Curve2, [f64; 2]); 2] = found.try_into().expect("two pcurves");
    Ok(Branch {
        curve: raw.curve,
        range: raw.range,
        tangent: raw.tangent,
        tolerance,
        pcurves: [p0, p1],
    })
}
