//! Fitting intersection curves and pcurves, with certified tolerances.
//!
//! A branch is fitted by cubic Hermite interpolation through samples on
//! both surfaces, each with its tangent: in model space by chord length,
//! and on each surface by the same parameter, so the curve and its two
//! pcurves share one. Each span is then bounded:
//!
//! - its distance from each surface through that surface's implicit form
//!   along the span ([`Implicit::on_piece`] and
//!   [`Implicit::distance_bound`]);
//! - the distance between a pcurve's image and the curve at the same
//!   parameter, `g(x) = S(P(x)) - C(x)`, by Taylor's theorem on pieces of
//!   the span: `|g| <= |g(m)| + |g'(m)| r + M r² / 2`, `m` the piece's
//!   middle, `r` its half-width and `M` a certified bound on `|g''|` from
//!   the surface's derivative bounds and the hulls of the pcurve's and
//!   the curve's derivatives.
//!
//! A span whose bound exceeds [`LINEAR_RESOLUTION`] gets a new sample in
//! its middle, put on both surfaces by Newton's method, until every span
//! passes. The worst bound, never below resolution, is the tolerance; a
//! branch that can't get within `10 × LINEAR_RESOLUTION` is
//! [`Failure::Uncertain`]. A pcurve for an exact curve is fitted the same
//! way, by the curve's own parameter.

use super::implicit::Implicit;
use super::ss_trace::Traced;
use super::{Failure, SLACK};
use crate::geom::chain3::Piece3;
use crate::geom::{
    BezierCurve, Curve, Hom, KnotVector, NurbsCurve, NurbsCurve2, Point2, Point3, Surface, Vector2,
    Vector3,
};
use crate::geom::{cos_abs_max, cos_range};
use crate::math;
use crate::tol::LINEAR_RESOLUTION;

/// The most samples a fit may use.
const MAX_SAMPLES: usize = 20_000;

/// How many rounds of splitting spans a fit may take.
const MAX_ROUNDS: usize = 40;

/// The most a sample's tangent may turn from the last one's, in radians,
/// before a sample goes between them.
const MAX_TURN: f64 = 0.25;

/// How many times a span may be halved while bounding it.
const MAX_HALVINGS: u32 = 12;

/// A fitted branch: the curve, its pcurve on each surface by the same
/// parameter, and its certified tolerance.
#[derive(Clone, Debug)]
pub(crate) struct Fitted {
    pub(crate) curve: NurbsCurve,
    pub(crate) pcurves: [NurbsCurve2; 2],
    pub(crate) tolerance: f64,
}

/// A point of the curve, with its derivative by the fit's parameter, and
/// the same on each surface's parameters.
#[derive(Clone, Copy, Debug)]
struct Sample {
    s: f64,
    p: Point3,
    d: Vector3,
    uv: [Point2; 2],
    duv: [Vector2; 2],
}

/// `x` moved by whole periods to lie within half a period of `near`.
fn unwrap(near: f64, x: f64, period: Option<f64>) -> f64 {
    match period {
        Some(p) => x + ((near - x) / p).round() * p,
        None => x,
    }
}

/// The parameters of the point of `s` nearest `p`, each periodic one
/// unwrapped next to `near`.
fn uv_near(s: &Surface, p: Point3, near: Option<Point2>) -> Point2 {
    let (u, v) = s.project(p).param;
    match near {
        None => Point2::new(u, v),
        Some(q) => {
            let (pu, pv) = s.periods();
            Point2::new(unwrap(q.x, u, pu), unwrap(q.y, v, pv))
        }
    }
}

/// The parameter velocity on `s` at `uv` of a curve with velocity `d`:
/// the least-squares solution of `S_u du + S_v dv = d`. `None` where the
/// partials are dependent (a pole, an apex).
fn uv_velocity(s: &Surface, uv: Point2, d: Vector3) -> Option<Vector2> {
    let e = s.eval(uv.x, uv.y);
    let (a, b, c) = (e.du.dot(e.du), e.du.dot(e.dv), e.dv.dot(e.dv));
    let det = a * c - b * b;
    if det <= 1e-24 * a.max(c).max(1e-300) * a.max(c).max(1e-300) {
        return None;
    }
    let (x, y) = (e.du.dot(d), e.dv.dot(d));
    Some(Vector2::new((c * x - b * y) / det, (a * y - b * x) / det))
}

/// Bounds on `|S_u|` and `|S_v|` over a parameter box, in closed form.
fn first_bound(s: &Surface, [_, rv]: [[f64; 2]; 2]) -> (f64, f64) {
    let k = 1.0 + 1e-12;
    let (su, sv) = match s {
        Surface::Plane(_) => (1.0, 1.0),
        Surface::Cylinder(c) => (c.radius(), 1.0),
        Surface::Cone(c) => {
            let (sin, cos) = math::sin_cos(c.half_angle());
            let rho = |v: f64| (c.radius() + v * sin / cos).abs();
            (rho(rv[0]).max(rho(rv[1])), 1.0 / cos)
        }
        Surface::Sphere(c) => (c.radius() * cos_abs_max(rv), c.radius()),
        Surface::Torus(t) => (t.major() + t.minor() * cos_range(rv).1, t.minor()),
        _ => (f64::INFINITY, f64::INFINITY),
    };
    (su * k, sv * k)
}

/// The control points of a cubic Hermite span, `a + d h / 3` and
/// `b - e h / 3` between its ends.
fn hermite<V>(a: V, da: V, b: V, db: V, h: f64) -> [V; 4]
where
    V: Copy
        + core::ops::Add<Output = V>
        + core::ops::Sub<Output = V>
        + core::ops::Mul<f64, Output = V>,
{
    [a, a + da * (h / 3.0), b - db * (h / 3.0), b]
}

/// A polynomial Bézier piece from its control points.
fn piece<V: Copy>(pts: [V; 4], range: [f64; 2]) -> BezierCurve<V> {
    BezierCurve {
        range,
        pts: pts.iter().map(|&p| Hom { p, w: 1.0 }).collect(),
    }
}

/// The piece over local `[x0, x1]`, reparameterized to `[0, 1]`.
fn sub<V: crate::geom::Coords>(p: &BezierCurve<V>, x0: f64, x1: f64) -> BezierCurve<V> {
    let mut out = p.clone();
    if x1 < 1.0 {
        out = out.split(x1)[0].clone();
    }
    if x0 > 0.0 {
        out = out.split(x0 / x1)[1].clone();
    }
    out
}

/// The value and first derivative of a polynomial Bézier piece at `x`,
/// by de Casteljau's algorithm.
fn eval<V: crate::geom::Coords>(p: &BezierCurve<V>, x: f64) -> (V, V) {
    let mut q: Vec<V> = p.pts.iter().map(|h| h.p).collect();
    let n = q.len() - 1;
    for k in 1..n {
        for i in 0..=n - k {
            q[i] = q[i] * (1.0 - x) + q[i + 1] * x;
        }
    }
    ((q[0] * (1.0 - x) + q[1] * x), (q[1] - q[0]) * n as f64)
}

/// The largest component magnitudes of a polynomial piece's first and
/// second derivatives over its local `[0, 1]`, from their control hulls.
fn derivative_hulls(p: &BezierCurve<Vector2>) -> ([f64; 2], [f64; 2]) {
    let q: Vec<Vector2> = p.pts.iter().map(|h| h.p).collect();
    let n = (q.len() - 1) as f64;
    let d1: Vec<Vector2> = q.windows(2).map(|w| (w[1] - w[0]) * n).collect();
    let d2: Vec<Vector2> = d1.windows(2).map(|w| (w[1] - w[0]) * (n - 1.0)).collect();
    let mag = |v: &[Vector2]| {
        v.iter().fold([0.0f64; 2], |m, x| {
            [m[0].max(x.x.abs()), m[1].max(x.y.abs())]
        })
    };
    (mag(&d1), mag(&d2))
}

/// The largest magnitude of a polynomial 3D piece's second derivative
/// over its local `[0, 1]`.
fn second_hull(p: &Piece3) -> f64 {
    let q: Vec<Vector3> = p.pts.iter().map(|h| h.p).collect();
    let n = (q.len() - 1) as f64;
    let d1: Vec<Vector3> = q.windows(2).map(|w| (w[1] - w[0]) * n).collect();
    d1.windows(2)
        .map(|w| ((w[1] - w[0]) * (n - 1.0)).length())
        .fold(0.0, f64::max)
}

/// The curve a pcurve is checked against, on a span's local parameter
/// `x`: its point and derivative, and a bound on its second derivative
/// over a stretch.
trait Target {
    fn at(&self, x: f64) -> (Point3, Vector3);
    fn second(&self, x0: f64, x1: f64) -> f64;
}

/// A span of the fitted curve itself.
struct SpanTarget<'a>(&'a Piece3);

impl Target for SpanTarget<'_> {
    fn at(&self, x: f64) -> (Point3, Vector3) {
        let (p, d) = eval(self.0, x);
        (Point3::ORIGIN + p, d)
    }

    fn second(&self, x0: f64, x1: f64) -> f64 {
        second_hull(&sub(self.0, x0, x1))
    }
}

/// An exact curve over a span `[t0, t1]` of its parameter.
struct CurveTarget<'a> {
    curve: &'a Curve,
    t: [f64; 2],
}

impl Target for CurveTarget<'_> {
    fn at(&self, x: f64) -> (Point3, Vector3) {
        let [t0, t1] = self.t;
        let e = self.curve.eval(t0 + (t1 - t0) * x);
        (e.point, e.d1 * (t1 - t0))
    }

    fn second(&self, x0: f64, x1: f64) -> f64 {
        let [t0, t1] = self.t;
        let h = t1 - t0;
        self.curve.derivative_bound([t0 + h * x0, t0 + h * x1]).d2 * h * h
    }
}

/// A certified bound on `|S(P(x)) - C(x)|` over the span, halving it
/// where the bound is too loose.
fn pcurve_bound(s: &Surface, p: &BezierCurve<Vector2>, c: &dyn Target, target: f64) -> f64 {
    fn go(
        s: &Surface,
        p: &BezierCurve<Vector2>,
        c: &dyn Target,
        [x0, x1]: [f64; 2],
        target: f64,
        depth: u32,
    ) -> f64 {
        let part = sub(p, x0, x1);
        let (lo, hi) = part.hull();
        let ranges = [[lo.x, hi.x], [lo.y, hi.y]];
        let second = s.derivative_bound(ranges);
        let (su, sv) = first_bound(s, ranges);
        let w = x1 - x0;
        let (d1, d2) = derivative_hulls(&part);
        let (u1, v1) = (d1[0] / w, d1[1] / w);
        let (u2, v2) = (d2[0] / (w * w), d2[1] / (w * w));
        let m2 = second.duu * u1 * u1
            + 2.0 * second.duv * u1 * v1
            + second.dvv * v1 * v1
            + su * u2
            + sv * v2
            + c.second(x0, x1);
        let m = (x0 + x1) / 2.0;
        let (q, dq) = eval(p, m);
        let e = s.eval(q.x, q.y);
        let (cp, cd) = c.at(m);
        let g = (e.point - cp).length();
        let dg = (e.du * dq.x + e.dv * dq.y - cd).length();
        let r = w / 2.0;
        let bound = (g + dg * r + m2 * r * r / 2.0) * (1.0 + 1e-9) + SLACK;
        // Halving shrinks the Taylor terms, never the error at the middle:
        // where that alone is most of the target, the span must be split
        // instead.
        if bound <= target || depth == 0 || g > target / 2.0 {
            return bound;
        }
        go(s, p, c, [x0, m], target, depth - 1).max(go(s, p, c, [m, x1], target, depth - 1))
    }
    go(s, p, c, [0.0, 1.0], target, MAX_HALVINGS)
}

/// A certified bound on the distance of a span from a surface, halving
/// it where the bound is too loose.
fn surface_bound(imp: &Implicit, span: &Piece3, target: f64, depth: u32) -> f64 {
    let (poly, _) = imp.on_piece(span);
    let mut local = imp.local(span.pts[0].p);
    for h in &span.pts[1..] {
        let l = imp.local(h.p);
        for k in 0..3 {
            local[k] = local[k].hull(l[k]);
        }
    }
    let bound = imp.distance_bound(poly.range().mag(), local);
    if bound <= target || depth == 0 {
        return bound;
    }
    // Where the span's middle alone is most of the target away, halving
    // won't bring it in: the span must be split instead.
    let mid = Point3::ORIGIN + span.split(0.5)[1].pts[0].p;
    let here = imp.distance_bound(imp.value_at(mid).mag(), imp.local(mid.to_vector()));
    if here > target / 2.0 {
        return bound.max(here);
    }
    let [a, b] = span.split(0.5);
    surface_bound(imp, &a, target, depth - 1).max(surface_bound(imp, &b, target, depth - 1))
}

/// Fits a traced branch where surfaces `a` and `b` meet.
pub(crate) fn fit_branch(surfaces: [&Surface; 2], traced: &Traced) -> Result<Fitted, Failure> {
    let imps = [
        Implicit::of(surfaces[0]).ok_or(Failure::Unsupported)?,
        Implicit::of(surfaces[1]).ok_or(Failure::Unsupported)?,
    ];
    let branch = Branch { surfaces, imps };
    let mut points = traced.points.clone();
    if traced.closed {
        points.push(points[0]);
    }
    let mut samples = branch.initial(&points)?;
    branch.refine_turns(&mut samples)?;
    let mut bounds: Vec<Option<f64>> = vec![None; samples.len() - 1];
    for round in 0.. {
        rechord(&mut samples);
        let mut worst = 0.0f64;
        let mut split = Vec::new();
        for i in 0..samples.len() - 1 {
            let b = match bounds[i] {
                Some(b) => b,
                None => {
                    let b = branch.span_bound(&samples[i], &samples[i + 1]);
                    bounds[i] = Some(b);
                    b
                }
            };
            worst = worst.max(b);
            if b > LINEAR_RESOLUTION && samples[i].p.distance(samples[i + 1].p) > LINEAR_RESOLUTION
            {
                split.push(i);
            }
        }
        if split.is_empty() || round == MAX_ROUNDS || samples.len() + split.len() > MAX_SAMPLES {
            if worst > 10.0 * LINEAR_RESOLUTION {
                return Err(Failure::Uncertain);
            }
            return branch.build(&samples, worst.max(LINEAR_RESOLUTION));
        }
        for &i in split.iter().rev() {
            let mid = branch.between(&samples[i], &samples[i + 1])?;
            samples.insert(i + 1, mid);
            bounds[i] = None;
            bounds.insert(i + 1, None);
        }
    }
    unreachable!("the loop returns")
}

/// Recomputes the chord-length parameter.
fn rechord(samples: &mut [Sample]) {
    let mut s = 0.0;
    samples[0].s = 0.0;
    for i in 1..samples.len() {
        s += samples[i].p.distance(samples[i - 1].p);
        samples[i].s = s;
    }
}

struct Branch<'a> {
    surfaces: [&'a Surface; 2],
    imps: [Implicit; 2],
}

impl Branch<'_> {
    /// Samples at the traced points, with tangents along the chain.
    fn initial(&self, points: &[Point3]) -> Result<Vec<Sample>, Failure> {
        // Points a hair apart (a crossing on a cell's corner seen from
        // both sides) are one.
        let mut pts: Vec<Point3> = Vec::with_capacity(points.len());
        for &p in points {
            if pts
                .last()
                .is_none_or(|q: &Point3| q.distance(p) > LINEAR_RESOLUTION)
            {
                pts.push(p);
            }
        }
        let n = pts.len();
        if n < 2 {
            return Err(Failure::Uncertain);
        }
        let mut out: Vec<Sample> = Vec::with_capacity(n);
        for i in 0..n {
            let (chord, inward) = if i + 1 < n {
                (pts[i + 1] - pts[i], pts[i + 1] - pts[i])
            } else {
                (pts[i] - pts[i - 1], pts[i - 1] - pts[i])
            };
            let prev = out.last().map(|s: &Sample| s.uv);
            out.push(self.sample(pts[i], chord, prev, Some(inward))?);
        }
        Ok(out)
    }

    /// A sample at `p` on both surfaces, its tangent along the surfaces'
    /// normals' cross product, turned to agree with `along`, or `along`
    /// itself where the normals are parallel (a singular point).
    ///
    /// At a pole or an apex, where a surface's parameters have no
    /// derivative, they are read a little way towards `inward` (into the
    /// branch, for its end samples) and carried back.
    fn sample(
        &self,
        p: Point3,
        along: Vector3,
        near: Option<[Point2; 2]>,
        inward: Option<Vector3>,
    ) -> Result<Sample, Failure> {
        let (_, ga) = self.imps[0].eval_grad(p);
        let (_, gb) = self.imps[1].eval_grad(p);
        let cross = ga.cross(gb);
        let d = match cross.normalize() {
            Some(t) if cross.length() > 1e-8 * ga.length() * gb.length() => {
                let t = t.get();
                if t.dot(along) < 0.0 { -t } else { t }
            }
            _ => along.normalize().ok_or(Failure::Uncertain)?.get(),
        };
        let mut uv = [Point2::ORIGIN; 2];
        let mut duv = [Vector2::ZERO; 2];
        for k in 0..2 {
            uv[k] = uv_near(self.surfaces[k], p, near.map(|n| n[k]));
            match uv_velocity(self.surfaces[k], uv[k], d) {
                Some(v) => duv[k] = v,
                None => {
                    let dir = inward.ok_or(Failure::Uncertain)?;
                    let sign = if dir.dot(d) >= 0.0 { 1.0 } else { -1.0 };
                    let eps = 1e-7 * (1.0 + p.to_vector().length());
                    let q = p + d * (sign * eps);
                    let uq = uv_near(self.surfaces[k], q, near.map(|n| n[k]));
                    let v = uv_velocity(self.surfaces[k], uq, d).ok_or(Failure::Uncertain)?;
                    uv[k] = uq - v * (sign * eps);
                    duv[k] = v;
                }
            }
        }
        Ok(Sample {
            s: 0.0,
            p,
            d,
            uv,
            duv,
        })
    }

    /// Adds samples where the tangent turns too fast between two.
    fn refine_turns(&self, samples: &mut Vec<Sample>) -> Result<(), Failure> {
        let mut i = 0;
        while i + 1 < samples.len() {
            let (a, b) = (&samples[i], &samples[i + 1]);
            let turn = math::acos(a.d.dot(b.d).clamp(-1.0, 1.0));
            // A span shorter than resolution is not split: there is
            // nothing between its ends to put a sample on.
            if turn > MAX_TURN && a.p.distance(b.p) > LINEAR_RESOLUTION {
                if samples.len() >= MAX_SAMPLES {
                    return Err(Failure::Uncertain);
                }
                let mid = self.between(a, b)?;
                samples.insert(i + 1, mid);
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// A new sample between two: the Hermite span's middle, put on both
    /// surfaces by Newton's method across the chord.
    fn between(&self, a: &Sample, b: &Sample) -> Result<Sample, Failure> {
        let h = a.p.distance(b.p);
        let chord = b.p - a.p;
        let mut x = a.p + chord * 0.5 + (a.d - b.d) * (h / 8.0);
        let start = x;
        let along = chord.normalize().ok_or(Failure::Uncertain)?.get();
        for _ in 0..50 {
            let (fa, ga) = self.imps[0].eval_grad(x);
            let (fb, gb) = self.imps[1].eval_grad(x);
            let fc = along.dot(x - start);
            // Solve [ga; gb; along] dx = -(fa, fb, fc).
            let rows = [ga, gb, along];
            let rhs = [-fa, -fb, -fc];
            let det = rows[0].dot(rows[1].cross(rows[2]));
            if det == 0.0 || !det.is_finite() {
                return Err(Failure::Uncertain);
            }
            let dx = (rows[1].cross(rows[2]) * rhs[0]
                + rows[2].cross(rows[0]) * rhs[1]
                + rows[0].cross(rows[1]) * rhs[2])
                / det;
            x += dx;
            if dx.length() <= 1e-15 * (1.0 + x.to_vector().length()) {
                break;
            }
        }
        if self.surfaces[0].distance(x) > LINEAR_RESOLUTION
            || self.surfaces[1].distance(x) > LINEAR_RESOLUTION
        {
            return Err(Failure::Uncertain);
        }
        self.sample(x, chord, Some(a.uv), None)
    }

    /// The certified bound of the span between two samples.
    fn span_bound(&self, a: &Sample, b: &Sample) -> f64 {
        let h = b.s - a.s;
        let span = piece(
            hermite(a.p.to_vector(), a.d, b.p.to_vector(), b.d, h),
            [a.s, b.s],
        );
        let mut worst = 0.0f64;
        for k in 0..2 {
            worst = worst.max(surface_bound(
                &self.imps[k],
                &span,
                LINEAR_RESOLUTION,
                MAX_HALVINGS,
            ));
            let p2 = piece(
                hermite(
                    a.uv[k].to_vector(),
                    a.duv[k],
                    b.uv[k].to_vector(),
                    b.duv[k],
                    h,
                ),
                [a.s, b.s],
            );
            worst = worst.max(pcurve_bound(
                self.surfaces[k],
                &p2,
                &SpanTarget(&span),
                LINEAR_RESOLUTION,
            ));
        }
        worst
    }

    fn build(&self, samples: &[Sample], tolerance: f64) -> Result<Fitted, Failure> {
        let knots: Vec<f64> = samples.iter().map(|s| s.s).collect();
        let mut mults = vec![3; knots.len()];
        mults[0] = 4;
        mults[knots.len() - 1] = 4;
        let kv = KnotVector::clamped(3, &knots, &mults).map_err(|_| Failure::Uncertain)?;
        let mut pts3 = vec![samples[0].p];
        let mut pts2 = [vec![samples[0].uv[0]], vec![samples[0].uv[1]]];
        for w in samples.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let h = b.s - a.s;
            let c = hermite(a.p.to_vector(), a.d, b.p.to_vector(), b.d, h);
            pts3.extend(c[1..].iter().map(|&v| Point3::ORIGIN + v));
            for (k, pts) in pts2.iter_mut().enumerate() {
                let c = hermite(
                    a.uv[k].to_vector(),
                    a.duv[k],
                    b.uv[k].to_vector(),
                    b.duv[k],
                    h,
                );
                pts.extend(c[1..].iter().map(|&v| Point2::ORIGIN + v));
            }
        }
        let curve = NurbsCurve::new(kv.clone(), pts3, None).map_err(|_| Failure::Uncertain)?;
        let [p0, p1] = pts2;
        let pcurves = [
            NurbsCurve2::new(kv.clone(), p0, None).map_err(|_| Failure::Uncertain)?,
            NurbsCurve2::new(kv, p1, None).map_err(|_| Failure::Uncertain)?,
        ];
        Ok(Fitted {
            curve,
            pcurves,
            tolerance,
        })
    }
}

/// A pcurve on `surface` for `curve` over `[t0, t1]`, by the curve's own
/// parameter, and the certified bound on how far its image strays from
/// the curve (at least [`LINEAR_RESOLUTION`]).
pub(crate) fn fit_pcurve(
    surface: &Surface,
    curve: &Curve,
    [t0, t1]: [f64; 2],
) -> Result<(NurbsCurve2, f64), Failure> {
    let at = |t: f64, near: Option<Point2>| -> Result<(f64, Point2, Vector2), Failure> {
        let e = curve.eval(t);
        let uv = uv_near(surface, e.point, near);
        if let Some(duv) = uv_velocity(surface, uv, e.d1) {
            return Ok((t, uv, duv));
        }
        // At a pole or an apex, where the parameters have no derivative:
        // those of a point a little way in, carried back along their
        // derivative there.
        let dt = (t1 - t0) * 1e-6 * if t < (t0 + t1) / 2.0 { 1.0 } else { -1.0 };
        let e = curve.eval(t + dt);
        let uv = uv_near(surface, e.point, near);
        let duv = uv_velocity(surface, uv, e.d1).ok_or(Failure::Uncertain)?;
        Ok((t, uv - duv * dt, duv))
    };
    // Each sample is unwrapped next to the one before, so no step may
    // turn a periodic parameter by a quarter period or more: a longer
    // step could unwrap the wrong way round and leave a jump of a whole
    // period that refining never closes.
    let (pu, pv) = surface.periods();
    let small = |a: Point2, b: Point2| {
        pu.is_none_or(|p| (b.x - a.x).abs() < p / 4.0)
            && pv.is_none_or(|p| (b.y - a.y).abs() < p / 4.0)
    };
    let n = 8;
    let mut samples = vec![at(t0, None)?];
    let mut ahead: Vec<f64> = (1..=n)
        .rev()
        .map(|i| t0 + (t1 - t0) * i as f64 / n as f64)
        .collect();
    while let Some(t) = ahead.pop() {
        let last = *samples.last().expect("one sample");
        let s = at(t, Some(last.1))?;
        if small(last.1, s.1) || samples.len() + ahead.len() > MAX_SAMPLES {
            samples.push(s);
        } else {
            ahead.push(t);
            ahead.push((last.0 + t) / 2.0);
        }
    }
    let bound_of = |a: &(f64, Point2, Vector2), b: &(f64, Point2, Vector2)| {
        let h = b.0 - a.0;
        let p2 = piece(
            hermite(a.1.to_vector(), a.2, b.1.to_vector(), b.2, h),
            [a.0, b.0],
        );
        pcurve_bound(
            surface,
            &p2,
            &CurveTarget {
                curve,
                t: [a.0, b.0],
            },
            LINEAR_RESOLUTION,
        )
    };
    let mut bounds: Vec<Option<f64>> = vec![None; samples.len() - 1];
    for round in 0.. {
        let mut worst = 0.0f64;
        let mut split = Vec::new();
        for i in 0..samples.len() - 1 {
            let b = *bounds[i].get_or_insert_with(|| bound_of(&samples[i], &samples[i + 1]));
            worst = worst.max(b);
            if b > LINEAR_RESOLUTION {
                split.push(i);
            }
        }
        if split.is_empty() || round == MAX_ROUNDS || samples.len() + split.len() > MAX_SAMPLES {
            if worst > 10.0 * LINEAR_RESOLUTION {
                return Err(Failure::Uncertain);
            }
            let knots: Vec<f64> = samples.iter().map(|s| s.0).collect();
            let mut mults = vec![3; knots.len()];
            mults[0] = 4;
            mults[knots.len() - 1] = 4;
            let kv = KnotVector::clamped(3, &knots, &mults).map_err(|_| Failure::Uncertain)?;
            let mut pts = vec![samples[0].1];
            for w in samples.windows(2) {
                let h = w[1].0 - w[0].0;
                let c = hermite(w[0].1.to_vector(), w[0].2, w[1].1.to_vector(), w[1].2, h);
                pts.extend(c[1..].iter().map(|&v| Point2::ORIGIN + v));
            }
            let pc = NurbsCurve2::new(kv, pts, None).map_err(|_| Failure::Uncertain)?;
            return Ok((pc, worst.max(LINEAR_RESOLUTION)));
        }
        for &i in split.iter().rev() {
            let t = (samples[i].0 + samples[i + 1].0) / 2.0;
            let mid = at(t, Some(samples[i].1))?;
            samples.insert(i + 1, mid);
            bounds[i] = None;
            bounds.insert(i + 1, None);
        }
    }
    unreachable!("the loop returns")
}

#[cfg(test)]
mod tests {
    use super::super::ss_trace::trace;
    use super::*;
    use crate::geom::{Box3, Cylinder, Ellipse, Frame, Plane, Sphere, Torus};

    fn frame(o: (f64, f64, f64), z: (f64, f64, f64)) -> Frame {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(1.0, 0.2, 0.1),
        )
        .unwrap()
    }

    /// The fit lies on both surfaces and its pcurves on it, within its
    /// tolerance, at many parameters.
    fn sound(a: &Surface, b: &Surface, f: &Fitted) {
        assert!(f.tolerance >= LINEAR_RESOLUTION && f.tolerance <= 10.0 * LINEAR_RESOLUTION);
        let [s0, s1] = f.curve.domain();
        for k in 0..=1000 {
            let s = s0 + (s1 - s0) * f64::from(k) / 1000.0;
            let p = f.curve.point(s);
            assert!(
                a.distance(p) <= f.tolerance && b.distance(p) <= f.tolerance,
                "at {s}"
            );
            for (surf, pc) in [(a, &f.pcurves[0]), (b, &f.pcurves[1])] {
                let q = pc.point(s);
                assert!(
                    surf.point(q.x, q.y).distance(p) <= f.tolerance,
                    "pcurve at {s}"
                );
            }
        }
    }

    #[test]
    fn traced_branches_fit_within_resolution() {
        let region = Box3::new(Point3::new(-6.0, -6.0, -6.0), Point3::new(6.0, 6.0, 6.0));
        let pairs = [
            (
                Surface::Cylinder(
                    Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 2.0).unwrap(),
                ),
                Surface::Cylinder(
                    Cylinder::new(frame((0.0, 0.3, 0.0), (1.0, 0.0, 0.0)), 1.0).unwrap(),
                ),
            ),
            (
                Surface::Plane(Plane::new(frame((0.0, 0.0, 0.3), (0.2, 0.1, 1.0)))),
                Surface::Torus(
                    Torus::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 3.0, 1.0).unwrap(),
                ),
            ),
            (
                Surface::Cylinder(
                    Cylinder::new(frame((0.0, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap(),
                ),
                Surface::Sphere(Sphere::new(frame((1.2, 0.0, 0.0), (0.0, 0.0, 1.0)), 1.0).unwrap()),
            ),
        ];
        for (a, b) in pairs {
            let topo = trace(&a, &b, &region).unwrap();
            assert!(!topo.branches.is_empty());
            for br in &topo.branches {
                let f = fit_branch([&a, &b], br).unwrap();
                sound(&a, &b, &f);
                if br.closed {
                    let [s0, s1] = f.curve.domain();
                    assert!(f.curve.point(s0).distance(f.curve.point(s1)) <= 1e-12);
                }
            }
        }
    }

    #[test]
    fn a_pcurve_for_an_oblique_ellipse_on_a_cylinder() {
        let cyl = Surface::Cylinder(Cylinder::new(Frame::WORLD, 1.0).unwrap());
        let f = Frame::new(
            Point3::ORIGIN,
            Vector3::new(0.0, 0.6, 1.0),
            Vector3::new(0.0, 1.0, -0.6),
        )
        .unwrap();
        let cz = 1.0 / Vector3::new(0.0, 0.6, 1.0).length();
        let e = Curve::Ellipse(Ellipse::new(f, 1.0 / cz, 1.0).unwrap());
        let (pc, tol) = fit_pcurve(&cyl, &e, [0.0, core::f64::consts::TAU]).unwrap();
        assert!(tol <= 10.0 * LINEAR_RESOLUTION);
        for k in 0..=500 {
            let t = core::f64::consts::TAU * f64::from(k) / 500.0;
            let q = pc.point(t);
            assert!(cyl.point(q.x, q.y).distance(e.point(t)) <= tol);
        }
        // Once round the cylinder: the pcurve winds a whole period in u.
        let (a, b) = (pc.point(0.0), pc.point(core::f64::consts::TAU));
        assert!(((b.x - a.x).abs() - core::f64::consts::TAU).abs() < 1e-9);
    }

    #[test]
    fn a_pcurve_round_a_cone_far_from_its_axis_does_not_jump_a_period() {
        // A plane cutting a frustum's cone obliquely, as a block's side
        // does: round the far side of its ellipse, u turns more than half a
        // period between the first coarse samples.
        let cone = Surface::Cone(
            crate::geom::Cone::new(
                Frame::new(
                    Point3::new(
                        -0.008608865348833917,
                        -0.08458643353936435,
                        -0.5818371950286672,
                    ),
                    Vector3::new(
                        -0.014640514631474661,
                        -0.14385042252113564,
                        -0.9894911375408151,
                    ),
                    Vector3::new(
                        0.9943270645299778,
                        0.10217408365665531,
                        -0.029565949538618703,
                    ),
                )
                .unwrap(),
                0.6382232064294462,
                0.30070550424513104,
            )
            .unwrap(),
        );
        let plane = Surface::Plane(Plane::new(
            Frame::new(
                Point3::new(
                    -0.34557856925691477,
                    -0.1495225089465062,
                    -0.3751246083485936,
                ),
                Vector3::new(
                    -0.7905403965321923,
                    -0.47628647730274276,
                    0.38496372944638296,
                ),
                Vector3::new(
                    -0.28374551106380286,
                    0.8419254014805647,
                    0.45896634222231497,
                ),
            )
            .unwrap(),
        ));
        let region = Box3::new(Point3::new(-2.0, -2.0, -2.0), Point3::new(2.0, 2.0, 2.0));
        let super::super::ss::SurfaceHit::Meet { branches, .. } =
            super::super::ss::surface_surface(&plane, &cone, &region).unwrap()
        else {
            panic!("the plane cuts the cone");
        };
        let br = &branches[0];
        // Most of a turn, wrapping past the end of the branch's period.
        let range = [2.8968530636395937, 9.154404442385939];
        let (pc, tol) = fit_pcurve(&cone, &br.curve, range).unwrap();
        assert!(tol <= 10.0 * LINEAR_RESOLUTION);
        for k in 0..=1000 {
            let t = range[0] + (range[1] - range[0]) * f64::from(k) / 1000.0;
            let q = pc.point(t);
            assert!(
                cone.point(q.x, q.y).distance(br.curve.point(t)) <= tol,
                "at {t}"
            );
        }
    }
}
