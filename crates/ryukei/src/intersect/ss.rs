//! Surface–surface intersection: the entry point and its results.
//!
//! A coincident pair is reported as such; a pair with a closed form
//! ([`super::ss_exact`]) gets exact curves; any other analytic pair is
//! traced ([`super::ss_trace`]) and fitted ([`super::fit`]), and any other
//! surface (a pipe, a NURBS, a swept surface) and an analytic one are
//! traced in the first's parameter plane ([`super::sheet_trace`]), and
//! two surfaces with neither form in their four parameters together
//! ([`super::general`]). Every branch
//! comes with its pcurve on each surface: an exact one where
//! [`crate::pcurve::exact`] finds it, otherwise one fitted by the curve's
//! parameter with a certified bound, which the branch's tolerance covers.
//! A branch is cut where it passes a sphere's pole or a cone's apex,
//! since no single pcurve can run through one.

use super::Failure;
use super::coincide::coincident;
use super::fit::{fit_along, fit_branch, fit_pcurve, fit_pcurve_within};
use super::general;
use super::implicit::Implicit;
use super::sheet_trace;
use super::ss_exact;
use super::ss_trace::{TangentTrace, Traced, rank, trace};
use crate::geom::{Box3, Curve, Curve2, KnotVector, NurbsCurve2, Point2, Point3, Surface, Vector2};
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
/// the first's parameter plane, as [`pipe_surface`] traces a pipe; two
/// surfaces with no implicit form in their four parameters together.
///
/// Where the trace is uncertain the surfaces are carried past their
/// bounded domain edges ([`extended`]) and what is found there is clipped
/// back to their domains, so no branch leaves either surface.
///
/// Fails with `Uncertain` where a branch's topology, a singular point or
/// a fit could not be certified.
pub fn surface_surface(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    meet(a, b, region, false).or_else(|e| match e {
        Failure::Uncertain => carried(a, b, region).map(|hit| within_domains(hit, [a, b])),
        e => Err(e),
    })
}

/// [`surface_surface`], trying the surfaces carried past their bounded
/// domain edges only with `extend`: [`super::face_face`] first certifies
/// faces apart, which is cheap, and never extends faces sharing an edge,
/// which the self-intersection check settles by [`super::beside`].
pub(crate) fn meet(
    a: &Surface,
    b: &Surface,
    region: &Box3,
    extend: bool,
) -> Result<SurfaceHit, Failure> {
    if let Some(same) = coincident(a, b) {
        return Ok(SurfaceHit::Coincident { same });
    }
    match (Implicit::of(a).is_some(), Implicit::of(b).is_some()) {
        (true, true) => {}
        _ => {
            return traced_any(a, b, region).or_else(|e| match e {
                // A curve along a bounded surface's domain edge (loft sides
                // meeting at their lateral, a side standing on a plane)
                // lies on the side of every cell beside it, where no
                // crossing can be certified: on the surfaces carried past
                // their edges, by different fractions so their carried
                // edges don't end together, it lies inside.
                Failure::Uncertain if extend => carried(a, b, region),
                e => Err(e),
            });
        }
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

/// Where two surfaces meet, one at least with no implicit form, on the
/// surfaces carried past their bounded domain edges, by different
/// fractions so their carried edges don't end together. `Uncertain` when
/// both have implicit forms or neither has such an edge. What is found
/// may run past the surfaces' domains ([`within_domains`] clips it).
fn carried(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    if Implicit::of(a).is_some() && Implicit::of(b).is_some() {
        return Err(Failure::Uncertain);
    }
    match (extended(a, 16.0), extended(b, 32.0 / 3.0)) {
        (None, None) => Err(Failure::Uncertain),
        (ea, eb) => traced_any(ea.as_ref().unwrap_or(a), eb.as_ref().unwrap_or(b), region),
    }
}

/// What [`carried`] found, cut back to the surfaces themselves: each
/// branch to the stretches whose pcurves lie in both surfaces' domains (a
/// carried surface keeps its parameters over its own domain), a stretch
/// shorter than ten tolerances a point; points within resolution of both.
fn within_domains(hit: SurfaceHit, surfaces: [&Surface; 2]) -> SurfaceHit {
    let SurfaceHit::Meet {
        branches,
        mut points,
    } = hit
    else {
        return hit;
    };
    points.retain(|&p| {
        surfaces
            .iter()
            .all(|s| s.distance(p) <= 2.0 * LINEAR_RESOLUTION)
    });
    let mut out = Vec::with_capacity(branches.len());
    for br in branches {
        let runs = inside_runs(&br, surfaces);
        if runs.len() == 1 && runs[0] == br.range {
            out.push(br);
            continue;
        }
        for range in runs {
            let ends = range.map(|t| br.curve.point(t));
            let b = br.curve.bounds(range);
            if b.min().distance(b.max()) < 10.0 * br.tolerance {
                points.push(ends[0]);
                continue;
            }
            out.push(Branch {
                range,
                ..br.clone()
            });
        }
    }
    SurfaceHit::Meet {
        branches: out,
        points,
    }
}

/// How finely [`inside_runs`] cuts a branch's range: halvings at most,
/// and parts at most.
const CLIP_DEPTH: u32 = 48;
const CLIP_PARTS: usize = 4096;

/// The stretches of a branch's range over which its pcurve on each
/// surface lies in that surface's domain (periodic directions aside, each
/// domain widened by a billionth), in order: parts certified in or out by
/// the pcurves' bounds, the rest halved, down to [`CLIP_DEPTH`] halvings or
/// past [`CLIP_PARTS`] parts, where the middle decides. A closed branch's
/// runs across its start are one.
fn inside_runs(br: &Branch, surfaces: [&Surface; 2]) -> Vec<[f64; 2]> {
    // Each domain widened by a billionth of its width: a pcurve along a
    // domain edge (where carrying the surfaces on is needed at all) lies
    // on it within rounding, and is inside.
    let widened = |d: [f64; 2]| {
        let w = d[1] - d[0];
        let e = 1e-9 * if w.is_finite() { w.max(1.0) } else { 1.0 };
        [d[0] - e, d[1] + e]
    };
    let limits: [_; 2] = core::array::from_fn(|k| {
        let [du, dv] = surfaces[k].domain();
        ([widened(du), widened(dv)], surfaces[k].periods())
    });
    let box_of = |k: usize, [t0, t1]: [f64; 2]| {
        let (c, [m0, m1]) = &br.pcurves[k];
        let (s0, s1) = (m0 + m1 * t0, m0 + m1 * t1);
        let r = [s0.min(s1), s0.max(s1)];
        let b = match c {
            Curve2::Nurbs2(n) => n.clipped_bounds(r),
            c => c.bounds(r),
        };
        // Cutting pieces rounds: grow by a little more than it can.
        let (lo, hi) = (b.min(), b.max());
        let g = 1e-12 * lo.distance(hi).max(1.0);
        [lo.x - g, lo.y - g, hi.x + g, hi.y + g]
    };
    // `Some(true)` all inside, `Some(false)` all outside (in some
    // direction), `None` undecided.
    let judge = |range: [f64; 2]| -> Option<bool> {
        let mut inside = true;
        for (k, &([du, dv], (pu, pv))) in limits.iter().enumerate() {
            let [ulo, vlo, uhi, vhi] = box_of(k, range);
            for (lo, hi, d, period) in [(ulo, uhi, du, pu), (vlo, vhi, dv, pv)] {
                if period.is_some() {
                    continue;
                }
                if hi < d[0] || lo > d[1] {
                    return Some(false);
                }
                if lo < d[0] || hi > d[1] {
                    inside = false;
                }
            }
        }
        inside.then_some(true)
    };
    let at_middle = |[a, b]: [f64; 2]| -> bool {
        let t = (a + b) / 2.0;
        (0..2).all(|k| {
            let ([du, dv], (pu, pv)) = limits[k];
            let (c, [m0, m1]) = &br.pcurves[k];
            let p = c.point(m0 + m1 * t);
            (pu.is_some() || (p.x >= du[0] && p.x <= du[1]))
                && (pv.is_some() || (p.y >= dv[0] && p.y <= dv[1]))
        })
    };
    let mut kept: Vec<[f64; 2]> = Vec::new();
    let mut stack = vec![(br.range, 0u32)];
    let mut parts = 0;
    // Depth first, the lower half first: kept parts come in order.
    while let Some((r, depth)) = stack.pop() {
        parts += 1;
        let keep = match judge(r) {
            Some(x) => x,
            None if depth >= CLIP_DEPTH || parts > CLIP_PARTS => at_middle(r),
            None => {
                let m = (r[0] + r[1]) / 2.0;
                if m <= r[0] || m >= r[1] {
                    at_middle(r)
                } else {
                    stack.push(([m, r[1]], depth + 1));
                    stack.push(([r[0], m], depth + 1));
                    continue;
                }
            }
        };
        if keep {
            match kept.last_mut() {
                Some(last) if last[1] == r[0] => last[1] = r[1],
                _ => kept.push(r),
            }
        }
    }
    // A closed branch: the run ending at its end goes on into the one at
    // its start.
    if kept.len() > 1
        && br.curve.period().is_some()
        && kept[0][0] == br.range[0]
        && kept[kept.len() - 1][1] == br.range[1]
    {
        let first = kept.remove(0);
        let last = kept.last_mut().expect("two runs");
        last[1] += first[1] - first[0];
    }
    kept
}

/// Where two surfaces meet, one at least with no implicit form: the
/// general path for two such, else the trace in the other's parameters
/// against the implicit form, falling back on the general path (which
/// traces a tangency along a curve the other can't follow).
fn traced_any(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    match (Implicit::of(a).is_some(), Implicit::of(b).is_some()) {
        (false, false) => general_surface(a, b, region),
        _ => sheet_surface(a, b, region).or_else(|e| match e {
            Failure::Uncertain => general_surface(a, b, region),
            e => Err(e),
        }),
    }
}

/// A surface carried past its bounded domain edges, exactly: the same
/// surface over its own domain, so what is found on it there is found on
/// the surface. A NURBS past each clamped end that doesn't collapse, by
/// its end pieces' width over `part` ([`crate::geom::NurbsSurface::extended`]); a sweep
/// along its profile by the profile's range over `part` (its path can't
/// be carried on without changing its frame); an extrusion along its
/// curve, a revolution along its curve short of the axis. `None` for an
/// analytic surface or a pipe, or one with no such edge.
pub(super) fn extended(s: &Surface, part: f64) -> Option<Surface> {
    let n = match s {
        Surface::Nurbs(n) => n,
        Surface::Sweep(w) => {
            let r = w.profile_range();
            let m = (r[1] - r[0]) / part;
            let (curve, range) = extended_curve(w.profile(), r, [m, m])?;
            let swept = crate::geom::Sweep::new(
                (curve, range),
                (w.path().clone(), w.path_range()),
                *w.start(),
                w.law(),
                w.twist(),
                w.scale(),
            )
            .ok()?;
            return Some(Surface::Sweep(Arc::new(swept)));
        }
        Surface::Extrusion(e) => {
            let r = e.curve().domain();
            if !(r[0].is_finite() && r[1].is_finite()) || e.curve().period().is_some() {
                return None;
            }
            let m = (r[1] - r[0]) / part;
            let (curve, _) = extended_curve(e.curve(), r, [m, m])?;
            let out = crate::geom::Extrusion::from_unit(curve, e.dir()).ok()?;
            return Some(Surface::Extrusion(Arc::new(out)));
        }
        Surface::Revolution(rv) => {
            let r = rv.curve().domain();
            if rv.curve().period().is_some() {
                return None;
            }
            let poles = rv.poles();
            let w = (r[1] - r[0]) / part;
            let m = [
                if poles[0] { 0.0 } else { w },
                if poles[1] { 0.0 } else { w },
            ];
            if m == [0.0, 0.0] {
                return None;
            }
            let (curve, _) = extended_curve(rv.curve(), r, m)?;
            let out = crate::geom::Revolution::new(*rv.frame(), curve).ok()?;
            return Some(Surface::Revolution(Arc::new(out)));
        }
        _ => return None,
    };
    let rows = crate::mesh::uv::collapsed_rows(s);
    let mut out = (**n).clone();
    let mut changed = false;
    for (k, dir) in [crate::geom::Direction::U, crate::geom::Direction::V]
        .into_iter()
        .enumerate()
    {
        let kv = &out.knots()[k];
        if kv.is_periodic() {
            continue;
        }
        let [d0, d1] = kv.domain();
        let mut distinct: Vec<f64> = kv.flat().to_vec();
        distinct.dedup();
        let first = distinct.get(1).copied().unwrap_or(d1) - d0;
        let last = d1 - distinct[distinct.len().saturating_sub(2)];
        let margins = [
            if rows[k][0] { 0.0 } else { first / part },
            if rows[k][1] { 0.0 } else { last / part },
        ];
        if margins == [0.0, 0.0] {
            continue;
        }
        if let Some(e) = out.extended(dir, margins) {
            out = e;
            changed = true;
        }
    }
    changed.then(|| Surface::Nurbs(Arc::new(out)))
}

/// A NURBS, extrusion or revolution carried exactly past its bounded
/// domain edges by `margins` (per direction, before and after, in its
/// parameters; zero for no change), as [`extended`] carries it: a
/// NURBS's end pieces' own polynomials, a swept surface's curve. Never
/// past a collapsed row or a revolution's pole, nor along a periodic
/// direction. `None` where nothing changes or a margin can't be taken.
pub(crate) fn carried_by(s: &Surface, margins: [[f64; 2]; 2]) -> Option<Surface> {
    let some = |m: [f64; 2]| m != [0.0, 0.0];
    match s {
        Surface::Nurbs(n) => {
            let rows = crate::mesh::uv::collapsed_rows(s);
            let mut out = (**n).clone();
            let mut changed = false;
            for (k, dir) in [crate::geom::Direction::U, crate::geom::Direction::V]
                .into_iter()
                .enumerate()
            {
                if out.knots()[k].is_periodic() {
                    continue;
                }
                let m = [
                    if rows[k][0] { 0.0 } else { margins[k][0] },
                    if rows[k][1] { 0.0 } else { margins[k][1] },
                ];
                if !some(m) {
                    continue;
                }
                out = out.extended(dir, m)?;
                changed = true;
            }
            changed.then(|| Surface::Nurbs(Arc::new(out)))
        }
        Surface::Extrusion(e) => {
            let r = e.curve().domain();
            if !some(margins[0]) || e.curve().period().is_some() || !r.iter().all(|x| x.is_finite())
            {
                return None;
            }
            let (curve, _) = extended_curve(e.curve(), r, margins[0])?;
            let out = crate::geom::Extrusion::from_unit(curve, e.dir()).ok()?;
            Some(Surface::Extrusion(Arc::new(out)))
        }
        Surface::Revolution(rv) => {
            let r = rv.curve().domain();
            let poles = rv.poles();
            let m = [
                if poles[0] { 0.0 } else { margins[1][0] },
                if poles[1] { 0.0 } else { margins[1][1] },
            ];
            if !some(m) || rv.curve().period().is_some() {
                return None;
            }
            let (curve, _) = extended_curve(rv.curve(), r, m)?;
            let out = crate::geom::Revolution::new(*rv.frame(), curve).ok()?;
            Some(Surface::Revolution(Arc::new(out)))
        }
        _ => None,
    }
}

/// `curve` over `range` carried on by `margins` past either end: a line
/// as it is, a conic over a longer arc while it is shorter than a turn, a
/// clamped spline past its domain ([`crate::geom::NurbsCurve::extended`]) where the
/// range reaches it.
fn extended_curve(curve: &Curve, range: [f64; 2], m: [f64; 2]) -> Option<(Curve, [f64; 2])> {
    let r = [range[0] - m[0], range[1] + m[1]];
    match curve {
        Curve::Line(_) => Some((curve.clone(), r)),
        Curve::Nurbs(n) if n.period().is_none() => {
            let [d0, d1] = n.domain();
            let past = [(d0 - r[0]).max(0.0), (r[1] - d1).max(0.0)];
            if past == [0.0, 0.0] {
                return Some((curve.clone(), r));
            }
            Some((Curve::Nurbs(Arc::new(n.extended(past)?)), r))
        }
        _ => match curve.period() {
            Some(p) if r[1] - r[0] < p => Some((curve.clone(), r)),
            _ => None,
        },
    }
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

/// Where two surfaces with no implicit form meet inside `region`: traced
/// in their four parameters ([`super::general`]), cut where branches pass
/// a pole of either, and fitted with a pcurve on each.
fn general_surface(a: &Surface, b: &Surface, region: &Box3) -> Result<SurfaceHit, Failure> {
    let topo = general::trace(a, b, region)?;
    let mut chains = topo.branches;
    for pole in poles(a).into_iter().chain(poles(b)) {
        if a.distance(pole) <= LINEAR_RESOLUTION && b.distance(pole) <= LINEAR_RESOLUTION {
            chains = cut_at(chains, pole);
        }
    }
    let mut points = topo.points;
    let mut branches = fitted(&chains, [a, b], false, &mut points)?;
    for t in &topo.tangents {
        branches.push(tangent_branch(t, [a, b])?);
    }
    Ok(SurfaceHit::Meet { branches, points })
}

/// A curve along which two surfaces touch, as a branch: a smooth pcurve
/// on the first through the traced parameters (cubic Hermite by chord
/// length, tangents by central differences), the first surface along it
/// fitted with a certified bound ([`fit_along`]), and its pcurve on the
/// second fitted by projection with a bound up to the touch rule's reach
/// (`100 ×` [`LINEAR_RESOLUTION`]), which also bounds the distance
/// between them there. The branch is tangent, its tolerance the larger
/// bound.
fn tangent_branch(t: &TangentTrace, [a, b]: [&Surface; 2]) -> Result<Branch, Failure> {
    let uv: Vec<Point2> = t.params.iter().map(|x| Point2::new(x[0], x[1])).collect();
    let pts: Vec<Point3> = uv.iter().map(|q| a.point(q.x, q.y)).collect();
    let n = uv.len();
    let mut s = vec![0.0];
    for w in pts.windows(2) {
        s.push(s[s.len() - 1] + w[0].distance(w[1]));
    }
    if n < 2 || s[n - 1] <= LINEAR_RESOLUTION {
        return Err(Failure::Uncertain);
    }
    let d: Vec<Vector2> = (0..n)
        .map(|i| {
            let (lo, hi) = match (i, t.closed) {
                (0, false) => (0, 1),
                (i, false) if i == n - 1 => (n - 2, n - 1),
                (i, _) => (i.saturating_sub(1), (i + 1).min(n - 1)),
            };
            (uv[hi] - uv[lo]) / (s[hi] - s[lo])
        })
        .collect();
    let mut ctrl = vec![uv[0]];
    for i in 0..n - 1 {
        let h = s[i + 1] - s[i];
        ctrl.push(uv[i] + d[i] * (h / 3.0));
        ctrl.push(uv[i + 1] - d[i + 1] * (h / 3.0));
        ctrl.push(uv[i + 1]);
    }
    let mut mults = vec![3; n];
    mults[0] = 4;
    mults[n - 1] = 4;
    let kv = KnotVector::clamped(3, &s, &mults).map_err(|_| Failure::Uncertain)?;
    let pc = NurbsCurve2::new(kv, ctrl, None).map_err(|_| Failure::Uncertain)?;
    let lifted = fit_along(
        a,
        &Curve2::Nurbs2(Arc::new(pc)),
        [0.0, 1.0],
        [0.0, s[n - 1]],
    )?;
    let curve = Curve::Nurbs(Arc::new(lifted.curve));
    let range = curve.domain();
    let (pb, bound) = fit_pcurve_within(b, &curve, range, 100.0 * LINEAR_RESOLUTION, 20_000)?;
    let identity = [0.0, 1.0];
    Ok(Branch {
        curve,
        range,
        tangent: true,
        tolerance: lifted.tolerance.max(bound),
        pcurves: [
            (Curve2::Nurbs2(Arc::new(lifted.pcurve)), identity),
            (Curve2::Nurbs2(Arc::new(pb)), identity),
        ],
    })
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
                // A branch already ending at the pole (a pole's zone in
                // the trace ends it there) leaves nothing on that side.
                if part.len() >= 2 && part.iter().any(|&p| p != pole) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Frame, Plane, Vector3};

    /// A gently curved spline patch over `[0, 1]²` in x and y, and the
    /// plane `x = 1` through its domain edge `u = 1`: the curve where they
    /// meet lies along that edge, where the trace can't isolate it, so the
    /// patch is carried past its edges, beyond the corners too.
    fn patch_and_edge_plane() -> (Surface, Surface) {
        let k = KnotVector::clamped(2, &[0.0, 1.0], &[3, 3]).unwrap();
        let mut points = Vec::new();
        for i in 0..3 {
            for j in 0..3 {
                let (x, y) = (f64::from(i) / 2.0, f64::from(j) / 2.0);
                let z = if j == 1 { 0.1 } else { 0.0 };
                points.push(Point3::new(x, y, z));
            }
        }
        let patch = crate::geom::NurbsSurface::new([k.clone(), k], points, None).unwrap();
        let frame = Frame::new(
            Point3::new(1.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        (
            Surface::Nurbs(Arc::new(patch)),
            Surface::Plane(Plane::new(frame)),
        )
    }

    #[test]
    fn branches_never_leave_either_surface() {
        let (patch, plane) = patch_and_edge_plane();
        let region = Box3::new(Point3::new(-1.0, -1.0, -1.0), Point3::new(2.0, 2.0, 1.0));
        for (a, b, k) in [(&patch, &plane, 0), (&plane, &patch, 1)] {
            let Ok(SurfaceHit::Meet { branches, .. }) = surface_surface(a, b, &region) else {
                panic!("the patch and the plane meet");
            };
            assert!(!branches.is_empty());
            for br in &branches {
                let (c, [m0, m1]) = &br.pcurves[k];
                for i in 0..=64 {
                    let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(i) / 64.0;
                    let p = c.point(m0 + m1 * t);
                    // In the domain, but for the clip's slack (a billionth
                    // of it): within resolution on this unit patch.
                    let r = LINEAR_RESOLUTION;
                    for x in [p.x, p.y] {
                        assert!((-r..=1.0 + r).contains(&x), "({}, {}) at {t}", p.x, p.y);
                    }
                }
                // Along the whole edge, not short of it.
                let ends = [br.range[0], br.range[1]].map(|t| c.point(m0 + m1 * t).y);
                assert!(ends.iter().any(|&y| y < 1e-6) && ends.iter().any(|&y| y > 1.0 - 1e-6));
            }
        }
    }
}
