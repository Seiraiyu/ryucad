//! Silhouette branches fitted with certified bounds (design 326, plan
//! 10.A): see `fit_contour`.

use super::Failure;
use super::fit::fit_along;
use super::pipe::{Cell, Field, Form, signed};
use super::ss_trace::Traced;
use crate::geom::nurbs::BezierCurve;
use crate::geom::sheet::Sheet;
use crate::geom::{Curve2, KnotVector, NurbsCurve, NurbsCurve2, Point2, Surface, Vector2, Vector3};
use crate::interval::Interval;
use crate::math;
use crate::tol::LINEAR_RESOLUTION;
use crate::work::{Meter, Scope};
use std::sync::Arc;

/// The most times the traced points are densified by sampling before the
/// first certified fit, and the most points that may give.
const SAMPLE_ROUNDS: usize = 16;
const MAX_POINTS: usize = 1 << 14;

/// The most times spans are halved and the fit run again.
const ROUNDS: usize = 8;

/// The deepest a span of the lifted pcurve is halved while certifying.
const DEPTH: u32 = 10;

/// A fitted silhouette branch: the curve, its pcurve on the surface by
/// the same parameter, and how far it may lie from the true silhouette.
#[derive(Clone, Debug)]
pub(crate) struct ContourFit {
    pub(crate) curve: NurbsCurve,
    pub(crate) pcurve: NurbsCurve2,
    pub(crate) tolerance: f64,
}

/// Fits a branch `trace_contour` traced on `surface` seen along
/// `towards`.
pub(crate) fn fit_contour(
    surface: &Surface,
    towards: Vector3,
    traced: &Traced,
    m: &Meter,
) -> Result<ContourFit, Failure> {
    m.capped(Scope::Fit, || fitted(surface, towards, traced, m))
}

fn fitted(
    surface: &Surface,
    towards: Vector3,
    traced: &Traced,
    m: &Meter,
) -> Result<ContourFit, Failure> {
    let field = Field {
        sheet: Sheet::of(surface),
        form: Form::Contour(towards),
    };
    let anchors = unwrapped(surface, &traced.params, traced.closed);
    if anchors.len() < 2 {
        return Err(Failure::Uncertain);
    }
    let mut pts = presampled(surface, &field, anchors.clone(), m)?;
    let mut best: Option<ContourFit> = None;
    for _ in 0..ROUNDS {
        let pc = hermite(&field, &pts)?;
        let range = pc.domain();
        let lifted = fit_along(
            surface,
            &Curve2::Nurbs2(Arc::new(pc.clone())),
            [0.0, 1.0],
            range,
            m,
        )?;
        let spans = bounds(&field, &lifted.pcurve, &anchors, m)?;
        let worst = spans.iter().map(|s| s.1).fold(0.0, f64::max);
        let fit = ContourFit {
            curve: lifted.curve,
            pcurve: lifted.pcurve,
            tolerance: (lifted.tolerance + worst).max(LINEAR_RESOLUTION),
        };
        if worst <= LINEAR_RESOLUTION {
            return Ok(fit);
        }
        best = Some(fit);
        match refined(&field, &pc, &pts, &spans) {
            Some(p) => pts = p,
            None => break,
        }
    }
    match best {
        Some(f) if f.tolerance <= 10.0 * LINEAR_RESOLUTION => Ok(f),
        _ => Err(Failure::Uncertain),
    }
}

/// The traced parameters moved by whole periods to follow each other, a
/// closed branch closed again, a pole's free parameter taken from its
/// neighbour (its own is arbitrary), and repeats dropped.
fn unwrapped(surface: &Surface, params: &[[f64; 2]], closed: bool) -> Vec<Point2> {
    let (pu, pv) = surface.periods();
    let near = |x: f64, prev: f64, p: Option<f64>| match p {
        Some(p) => x + p * ((prev - x) / p).round(),
        None => x,
    };
    let mut out: Vec<Point2> = Vec::with_capacity(params.len() + 1);
    for &[u, v] in params {
        let q = match out.last() {
            Some(prev) => Point2::new(near(u, prev.x, pu), near(v, prev.y, pv)),
            None => Point2::new(u, v),
        };
        out.push(q);
    }
    let rows = crate::mesh::uv::collapsed_rows(surface);
    let domain = surface.domain();
    let n = out.len();
    for i in 0..n {
        for k in 0..2 {
            for e in 0..2 {
                let at = if k == 0 { out[i].x } else { out[i].y };
                if !rows[k][e] || at != domain[k][e] || n < 2 {
                    continue;
                }
                let j = if i == 0 { 1 } else { i - 1 };
                if k == 0 {
                    out[i].y = out[j].y;
                } else {
                    out[i].x = out[j].x;
                }
            }
        }
    }
    if closed && let Some(&first) = out.first() {
        let last = out[out.len() - 1];
        out.push(Point2::new(
            near(first.x, last.x, pu),
            near(first.y, last.y, pv),
        ));
    }
    out.dedup();
    out
}

/// The traced points densified until the Hermite spline through them
/// follows the silhouette at sampled points (a quarter, a half and three
/// quarters along each span, each moved onto `F = 0` by Newton's method)
/// within an eighth of [`LINEAR_RESOLUTION`] in space: a sample in the
/// middle of every span that doesn't. Sampled only, to start the certified
/// fit near the silhouette (a fit through the trace's crossings alone
/// strays tens of microns between them, and certifying a stray span
/// halves it to the bottom); the bound is still `bounds`'.
fn presampled(
    surface: &Surface,
    field: &Field,
    mut pts: Vec<Point2>,
    m: &Meter,
) -> Result<Vec<Point2>, Failure> {
    for _ in 0..SAMPLE_ROUNDS {
        let pc = hermite(field, &pts)?;
        let k = pc.knots().knots().to_vec();
        let mut out = vec![pts[0]];
        let mut added = false;
        for i in 0..pts.len() - 1 {
            let (a, b) = (k[i], k[i + 1]);
            let mut far = false;
            for f in [0.25, 0.5, 0.75] {
                // A sample moved onto the silhouette.
                m.charge(1)?;
                let q = pc.point(a + (b - a) * f);
                let on = newton(field, q);
                if surface.point(q.x, q.y).distance(surface.point(on.x, on.y))
                    > LINEAR_RESOLUTION / 8.0
                {
                    far = true;
                    break;
                }
            }
            if far && b - a > 1e-12 * (1.0 + a.abs()) && pts.len() + 1 < MAX_POINTS {
                out.push(newton(field, pc.point((a + b) / 2.0)));
                added = true;
            }
            out.push(pts[i + 1]);
        }
        if !added {
            break;
        }
        pts = out;
    }
    Ok(pts)
}

/// A cubic Hermite spline through `pts` by chord length in the parameter
/// plane, each tangent square to `∇F` (the chord's way where `∇F`
/// vanishes: a pole or a singular end).
fn hermite(field: &Field, pts: &[Point2]) -> Result<NurbsCurve2, Failure> {
    let n = pts.len();
    let mut s = vec![0.0];
    for w in pts.windows(2) {
        s.push(s[s.len() - 1] + (w[1] - w[0]).length());
    }
    let d: Vec<Vector2> = (0..n)
        .map(|i| {
            let chord = pts[(i + 1).min(n - 1)] - pts[i.saturating_sub(1)];
            let g = field.at(pts[i].x, pts[i].y).d;
            let along = Vector2::new(-g[1], g[0]);
            let len = along.length();
            let unit = if len > 0.0 && len.is_finite() {
                along / len
            } else {
                chord / chord.length()
            };
            if unit.dot(chord) < 0.0 { -unit } else { unit }
        })
        .collect();
    let mut ctrl = vec![pts[0]];
    for i in 0..n - 1 {
        let h = s[i + 1] - s[i];
        ctrl.push(pts[i] + d[i] * (h / 3.0));
        ctrl.push(pts[i + 1] - d[i + 1] * (h / 3.0));
        ctrl.push(pts[i + 1]);
    }
    let mut mults = vec![3; n];
    mults[0] = 4;
    mults[n - 1] = 4;
    let kv = KnotVector::clamped(3, &s, &mults).map_err(|_| Failure::Uncertain)?;
    NurbsCurve2::new(kv, ctrl, None).map_err(|_| Failure::Uncertain)
}

/// Each knot span of the lifted pcurve and its certified distance from
/// the silhouette in space.
fn bounds(
    field: &Field,
    pc: &NurbsCurve2,
    anchors: &[Point2],
    m: &Meter,
) -> Result<Vec<([f64; 2], f64)>, Failure> {
    let k = pc.knots().knots();
    let mut out = Vec::with_capacity(k.len());
    for w in k.windows(2) {
        out.push((
            [w[0], w[1]],
            span(field, pc, [w[0], w[1]], anchors, DEPTH, m)?,
        ));
    }
    Ok(out)
}

fn span(
    field: &Field,
    pc: &NurbsCurve2,
    [a, b]: [f64; 2],
    anchors: &[Point2],
    depth: u32,
    m: &Meter,
) -> Result<f64, Failure> {
    // A piece of a pcurve certified.
    m.charge(1)?;
    let bx = pc.clipped_bounds([a, b]);
    let cell: Cell = [[bx.min().x, bx.max().x], [bx.min().y, bx.max().y]];
    let by_along = pc
        .pieces()
        .iter()
        .filter_map(|p| p.clipped([a, b]).map(|c| (c, p.range != [a, b])))
        .map(|(c, cut)| along(field, &c, cut))
        .try_fold(0.0f64, |w, d| d.map(|d| w.max(d)));
    // The second-order bound settles nearly every piece; the box bounds
    // are for pieces it can't (a pole, a singular end).
    let here = match by_along {
        Some(d) if d <= LINEAR_RESOLUTION => Some(d),
        _ => [
            by_along,
            certified(field, cell),
            extent(field, cell, anchors),
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min),
    };
    if let Some(d) = here
        && (d <= LINEAR_RESOLUTION || depth == 0)
    {
        return Ok(d);
    }
    let mid = (a + b) / 2.0;
    if depth == 0 || !(a < mid && mid < b) {
        return here.ok_or(Failure::Uncertain);
    }
    let left = span(field, pc, [a, mid], anchors, depth - 1, m)?;
    let right = span(field, pc, [mid, b], anchors, depth - 1, m)?;
    Ok(left.max(right))
}

/// The distance in space from every point of a polynomial piece of the
/// pcurve to the silhouette, to second order in the piece's length: `G =
/// F ∘ q` is bounded over the piece by Taylor's formula about each end
/// (`G` and `G'` at the end, where the fit put the piece on `F = 0` and
/// square to `∇F`, and `G'' = q'ᵀ H q' + ∇F · q''` over the piece's hull,
/// `q'` and `q''` by the hulls of its hodographs), and that size is turned
/// into a distance along one partial of one sign as `certified` does. A
/// box bound alone is first order (`|F|` over a piece's box is about
/// `|∇F|` times its width), so it needed pieces as short as the
/// resolution. `cut`: the piece was cut from a longer one, so its control
/// points are within rounding of the curve's, which the bound covers.
fn along(field: &Field, piece: &BezierCurve<Vector2>, cut: bool) -> Option<f64> {
    if piece.pts.iter().any(|h| h.w != 1.0) || piece.pts.len() < 2 {
        return None;
    }
    let c: Vec<[Interval; 2]> = piece
        .pts
        .iter()
        .map(|h| [Interval::point(h.p.x), Interval::point(h.p.y)])
        .collect();
    let size = piece
        .pts
        .iter()
        .map(|h| h.p.x.abs().max(h.p.y.abs()))
        .fold(1.0, f64::max);
    let slack = if cut { 64.0 * f64::EPSILON * size } else { 0.0 };
    let n = c.len() - 1;
    let deg = Interval::point(n as f64);
    let hull = |v: &[[Interval; 2]]| -> Option<[Interval; 2]> {
        let mut out = *v.first()?;
        for x in &v[1..] {
            out = [out[0].hull(x[0]), out[1].hull(x[1])];
        }
        Some(out)
    };
    let diff =
        |a: [Interval; 2], b: [Interval; 2], k: Interval| [(b[0] - a[0]) * k, (b[1] - a[1]) * k];
    let d1: Vec<[Interval; 2]> = c.windows(2).map(|w| diff(w[0], w[1], deg)).collect();
    let dd = if n >= 2 {
        let k = Interval::point((n - 1) as f64);
        hull(
            &d1.windows(2)
                .map(|w| diff(w[0], w[1], k))
                .collect::<Vec<_>>(),
        )?
    } else {
        [Interval::point(0.0); 2]
    };
    let dq = hull(&d1)?;
    let grow = |x: Interval| Interval::new(x.lo() - slack, x.hi() + slack);
    let bx = hull(&c)?;
    let cell: Cell = {
        let (x, y) = (grow(bx[0])?, grow(bx[1])?);
        [[x.lo(), x.hi()], [y.lo(), y.hi()]]
    };
    let e = field.enclose(cell)?;
    let two = Interval::point(2.0);
    let g2 = e.h[0] * dq[0].square()
        + two * e.h[1] * dq[0] * dq[1]
        + e.h[2] * dq[1].square()
        + e.d[0] * dd[0]
        + e.d[1] * dd[1];
    let end = |p: [Interval; 2], d: [Interval; 2]| -> Option<f64> {
        let (x, y) = (grow(p[0])?, grow(p[1])?);
        let j = field.enclose([[x.lo(), x.hi()], [y.lo(), y.hi()]])?;
        let g1 = j.d[0] * d[0] + j.d[1] * d[1];
        Some(j.v.mag() + g1.mag() / 2.0 + g2.mag() / 8.0)
    };
    let eps = end(c[0], d1[0])?.max(end(c[n], d1[n - 1])?) * (1.0 + 8.0 * f64::EPSILON);
    if !eps.is_finite() {
        return None;
    }
    let mut best: Option<f64> = None;
    for k in 0..2 {
        if !signed(e.d[k]) {
            continue;
        }
        let mut reach = eps / least(e.d[k]);
        for _ in 0..3 {
            let mut grown = cell;
            grown[k] = [cell[k][0] - reach, cell[k][1] + reach];
            let g = field.enclose(grown)?;
            if !signed(g.d[k]) {
                break;
            }
            let next = eps / least(g.d[k]);
            if next <= reach {
                let p = field.sheet.jet_box(grown)?;
                let speed = length_hi([p[0].d[k], p[1].d[k], p[2].d[k]]);
                let d = next * speed;
                best = Some(best.map_or(d, |b| b.min(d)));
                break;
            }
            reach = next;
        }
    }
    best
}

/// The least of `|x|` over an interval of one sign.
fn least(x: Interval) -> f64 {
    x.lo().abs().min(x.hi().abs())
}

/// An upper bound on the length of a vector of enclosures.
fn length_hi(v: [Interval; 3]) -> f64 {
    let s: f64 = v.iter().map(|x| x.mag() * x.mag()).sum();
    math::sqrt(s) * (1.0 + 4.0 * f64::EPSILON)
}

/// The distance in space from every point over `cell` to the silhouette,
/// by `F`'s size and one partial of one sign (see the module).
fn certified(field: &Field, cell: Cell) -> Option<f64> {
    let e = field.enclose(cell)?;
    let eps = e.v.mag();
    let mut best: Option<f64> = None;
    for k in 0..2 {
        if !signed(e.d[k]) {
            continue;
        }
        let mut reach = eps / least(e.d[k]);
        for _ in 0..3 {
            let mut grown = cell;
            grown[k] = [cell[k][0] - reach, cell[k][1] + reach];
            let g = field.enclose(grown)?;
            if !signed(g.d[k]) {
                break;
            }
            let next = eps / least(g.d[k]);
            if next <= reach {
                let p = field.sheet.jet_box(grown)?;
                let speed = length_hi([p[0].d[k], p[1].d[k], p[2].d[k]]);
                let d = next * speed;
                best = Some(best.map_or(d, |b| b.min(d)));
                break;
            }
            reach = next;
        }
    }
    best
}

/// The extent in space of the surface over `cell`, when the cell holds a
/// traced point: that point is on the silhouette.
fn extent(field: &Field, cell: Cell, anchors: &[Point2]) -> Option<f64> {
    let inside = |p: &Point2| {
        cell[0][0] <= p.x && p.x <= cell[0][1] && cell[1][0] <= p.y && p.y <= cell[1][1]
    };
    if !anchors.iter().any(inside) {
        return None;
    }
    let p = field.sheet.jet_box(cell)?;
    let across = (0..3)
        .map(|k| {
            let w = p[k].v.hi() - p[k].v.lo();
            w * w
        })
        .sum::<f64>();
    Some(math::sqrt(across) * (1.0 + 4.0 * f64::EPSILON))
}

/// `pts` with a sample in the middle of every span a bad piece reaches,
/// moved onto `F = 0`; `None` when there is none to add.
fn refined(
    field: &Field,
    pc: &NurbsCurve2,
    pts: &[Point2],
    spans: &[([f64; 2], f64)],
) -> Option<Vec<Point2>> {
    let k = pc.knots().knots();
    let mut out = vec![pts[0]];
    let mut added = false;
    for i in 0..pts.len() - 1 {
        let (a, b) = (k[i], k[i + 1]);
        let bad = spans
            .iter()
            .any(|(r, d)| *d > LINEAR_RESOLUTION && r[0] < b && r[1] > a);
        if bad && b - a > 1e-12 * (1.0 + a.abs()) {
            out.push(newton(field, pc.point((a + b) / 2.0)));
            added = true;
        }
        out.push(pts[i + 1]);
    }
    added.then_some(out)
}

/// Newton's method onto `F = 0` along `∇F`, in floating point.
fn newton(field: &Field, mut q: Point2) -> Point2 {
    for _ in 0..12 {
        let j = field.at(q.x, q.y);
        let g2 = j.d[0] * j.d[0] + j.d[1] * j.d[1];
        if !(g2 > 0.0 && g2.is_finite()) {
            break;
        }
        let step = Vector2::new(j.d[0], j.d[1]) * (-j.v / g2);
        q = q + step;
        if step.length() <= 1e-16 * (1.0 + q.x.abs() + q.y.abs()) {
            break;
        }
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Box3, Frame, Point3, Torus};

    #[test]
    fn a_traced_torus_silhouette_fits_within_its_bound() {
        let torus = Surface::Torus(Torus::new(Frame::WORLD, 1.0, 0.3).unwrap());
        let t = Vector3::new(1.0, 0.2, 1.0);
        let t = t / t.length();
        let region = Box3::new(Point3::new(-2.0, -2.0, -2.0), Point3::new(2.0, 2.0, 2.0));
        // unmetered: a unit test
        let m = Meter::free();
        let topo = super::super::sheet_trace::trace_contour(&torus, t, &region, &m).unwrap();
        for b in &topo.branches {
            let fit = fit_contour(&torus, t, b, &m).unwrap();
            assert!(
                fit.tolerance <= 10.0 * LINEAR_RESOLUTION,
                "{}",
                fit.tolerance
            );
            let [d0, d1] = fit.curve.domain();
            for i in 0..=64 {
                let s = d0 + (d1 - d0) * i as f64 / 64.0;
                let q = fit.pcurve.point(s);
                let p = fit.curve.point(s);
                assert!(torus.point(q.x, q.y).distance(p) <= fit.tolerance);
                // Square to the view within the bound's angle at a 0.3 m tube.
                assert!(torus.normal(q.x, q.y).unwrap().get().dot(t).abs() <= 1e-6);
            }
        }
    }
}
