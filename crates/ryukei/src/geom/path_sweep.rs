//! The sweep: a profile curve carried along a path in a moving frame,
//! the exact surface of a sweep along a path.

mod frame;

pub(crate) use frame::Carry;
pub use frame::FrameLaw;

use super::bound2::IVec3;
use super::jet::{self, JVec, Jet, Num};
use super::sheet::{Sheet, nearest};
use super::{
    Box3, Curve, Frame, KnotVector, Mirror, NurbsSurface, Point3, Projection, Reparam, Rigid,
    SurfaceBound, SurfaceEval, UnitVector3, Vector3,
};
use crate::Error;
use crate::interval::Interval;

/// The most times a span along the path is halved while fitting a
/// sweep's NURBS form.
const MAX_FIT_DEPTH: u32 = 20;

/// The pieces a span's second derivatives are enclosed over while
/// bounding a Hermite fit.
const HERMITE_PIECES: u32 = 8;

/// The deepest a cell is halved while certifying a sweep regular.
const MAX_DEPTH: u32 = 16;

/// The most cells certifying a sweep looks at before it gives up.
const MAX_CELLS: usize = 1 << 14;

/// `P(u, s) = c(s) + Σⱼ (aⱼ + σ(s) bⱼ(u)) fⱼ(s)`: a profile curve `Q`
/// carried along a path `c` by a moving frame `(f₁, f₂, f₃) = (f₁, f₂,
/// t)` (see [`FrameLaw`]), scaled about a centre `o` by `σ`.
///
/// The frame starts as `start` says: the profile is placed as it lies,
/// and `aⱼ = fⱼ(s₀) · (o - c(s₀))` and `bⱼ(u) = fⱼ(s₀) · (Q(u) - o)` are
/// where the centre and the profile sit in the frame at the start, with
/// `o` the start frame's origin; a minimal frame starts from the start
/// frame's `x` made perpendicular to the tangent. So the section at each
/// `s` is the profile moved rigidly with the frame, scaled about the
/// centre's place by `σ(s)`, which runs linearly from 1 at the start to
/// `scale` at the end, as the twist runs from 0 to `twist`. `u` is the
/// profile's parameter over its range and `s` the path's (periodic where
/// the profile, or the path, is a whole period of a periodic curve), and
/// the natural normal is along `P_u × P_s`.
///
/// A sweep is built only where it is regular, certified over its whole
/// domain (see [`Sweep::new`]); it is exact by its definition, never a
/// fit. A path along a line or a circle with no twist or scale gives an
/// extrusion or a revolution, which sweeps along paths use instead.
#[derive(Clone, Debug)]
pub struct Sweep {
    profile: Curve,
    profile_range: [f64; 2],
    start: Frame,
    law: FrameLaw,
    scale: f64,
    carry: Carry,
    /// The frame's axes at the start, and the centre's place in them.
    axes0: [Vector3; 3],
    a: [f64; 3],
    bounds: Box3,
}

impl PartialEq for Sweep {
    fn eq(&self, o: &Self) -> bool {
        (
            &self.profile,
            self.profile_range,
            self.carry.path(),
            self.carry.range(),
            self.start,
            self.law,
            self.carry.twist(),
            self.scale,
        ) == (
            &o.profile,
            o.profile_range,
            o.carry.path(),
            o.carry.range(),
            o.start,
            o.law,
            o.carry.twist(),
            o.scale,
        )
    }
}

/// Whether `range` is a whole period of `c`.
fn whole(c: &Curve, range: [f64; 2]) -> bool {
    c.period() == Some(range[1] - range[0])
}

/// The product of an interval vector's coordinates with a vector's.
fn idot(a: IVec3, b: IVec3) -> Interval {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn icross(a: IVec3, b: IVec3) -> IVec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The box of a point's enclosures.
fn point_box(q: &JVec<Interval>) -> Box3 {
    Box3::new(
        Point3::new(q[0].v.lo(), q[1].v.lo(), q[2].v.lo()),
        Point3::new(q[0].v.hi(), q[1].v.hi(), q[2].v.hi()),
    )
}

/// The box holding a box's corners moved by `f`.
fn moved_box(b: &Box3, f: impl Fn(Point3) -> Point3) -> Box3 {
    let b = Box3::from_points(b.corners().map(f)).expect("eight corners");
    b.grow(1e-12 * (1.0 + b.max().to_vector().length()))
}

impl Sweep {
    /// The sweep of `profile` (a curve over a range of its parameter)
    /// along `path` (likewise), placed by `start`, by `law`, turning by
    /// `twist` radians about the path and growing by `scale` from start
    /// to end.
    ///
    /// Fails with `NonFinite`, or `Degenerate`: for a range that is empty,
    /// outside a clamped curve's domain or longer than a periodic one's
    /// period; a scale that isn't positive; a frame that can't be
    /// certified defined along the path ([`Carry::new`]); or a sweep that
    /// can't be certified regular over its whole domain. That is checked
    /// over the profile's pieces against the frame's stretches, halved up
    /// to 16 times (across the longer side in model space), at most
    /// 16,384 cells in all, in interval arithmetic: `P_u × P_s` never
    /// vanishes, so the sweep never folds or stalls (a profile along the
    /// path, or one wider than the path's radius of curvature, fails).
    pub fn new(
        profile: (Curve, [f64; 2]),
        path: (Curve, [f64; 2]),
        start: Frame,
        law: FrameLaw,
        twist: f64,
        scale: f64,
    ) -> Result<Self, Error> {
        let carry = Carry::new(path.0, path.1, start.x().get(), law, twist)?;
        let s = Self::with(profile, carry, start, law, scale)?;
        let bounds = s.certify()?;
        Ok(Self { bounds, ..s })
    }

    /// The sweep on a frame already made, before it is certified (its
    /// bounds the size box).
    fn with(
        (profile, profile_range): (Curve, [f64; 2]),
        carry: Carry,
        start: Frame,
        law: FrameLaw,
        scale: f64,
    ) -> Result<Self, Error> {
        let [u0, u1] = profile_range;
        if !(u0.is_finite() && u1.is_finite() && scale.is_finite()) {
            return Err(Error::NonFinite);
        }
        let ok = match profile.period() {
            Some(p) => u1 - u0 <= p,
            None => {
                let [d0, d1] = profile.domain();
                d0 <= u0 && u1 <= d1
            }
        };
        if !(u0 < u1 && ok && scale > 0.0) {
            return Err(Error::Degenerate);
        }
        let [s0, _] = carry.range();
        let axes0 = carry.axes_at(s0).ok_or(Error::Degenerate)?;
        let rel = start.origin() - carry.path().point(s0);
        let a = axes0.map(|f| f.dot(rel));
        Ok(Self {
            profile,
            profile_range,
            start,
            law,
            scale,
            carry,
            axes0,
            a,
            bounds: crate::tol::size_box(),
        })
    }

    pub fn profile(&self) -> &Curve {
        &self.profile
    }

    pub fn profile_range(&self) -> [f64; 2] {
        self.profile_range
    }

    pub fn path(&self) -> &Curve {
        self.carry.path()
    }

    pub fn path_range(&self) -> [f64; 2] {
        self.carry.range()
    }

    pub fn start(&self) -> &Frame {
        &self.start
    }

    pub fn law(&self) -> FrameLaw {
        self.law
    }

    pub fn twist(&self) -> f64 {
        self.carry.twist()
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The scale at `s` and its rate.
    fn sigma(&self, s: f64) -> (f64, f64) {
        let [s0, s1] = self.carry.range();
        let rate = (self.scale - 1.0) / (s1 - s0);
        (1.0 + rate * (s - s0), rate)
    }

    /// The point as jets in `(u, s)`, from the profile's point and first
    /// two derivatives, the path's point and frame as jets in `s`, and the
    /// scale with its rate.
    fn jets<T: Num>(
        &self,
        q: [[T; 3]; 3],
        (c, f): (JVec<T>, [JVec<T>; 3]),
        (sigma, rate): (T, f64),
    ) -> JVec<T> {
        let o = self.start.origin().to_vector().to_array().map(T::of);
        let rel: [T; 3] = core::array::from_fn(|k| q[0][k] - o[k]);
        let sj = Jet::of_t(sigma, T::of(rate), T::of(0.0));
        let mut p = c;
        for ((axis, &a), fj) in self.axes0.iter().zip(&self.a).zip(f) {
            let w = axis.to_array().map(T::of);
            let dot = |v: [T; 3]| v[0] * w[0] + v[1] * w[1] + v[2] * w[2];
            let b = Jet::of_s(dot(rel), dot(q[1]), dot(q[2]));
            let coef = Jet::constant(T::of(a)) + sj * b;
            p = jet::add(p, jet::times(fj, coef));
        }
        p
    }

    /// The point's jets at `(u, s)` in floating point.
    pub(crate) fn jets_at(&self, u: f64, s: f64) -> JVec<f64> {
        let q = self.profile.jet3(u).map(Vector3::to_array);
        let m = self
            .carry
            .jets_at(s)
            .or_else(|| {
                let [s0, s1] = self.carry.range();
                self.carry.jets_at(s.clamp(s0, s1))
            })
            .expect("a certified sweep has a frame on its path");
        let (sigma, rate) = self.sigma(s);
        self.jets([q[0], q[1], q[2]], m, (sigma, rate))
    }

    /// Enclosures of the point's jets over a parameter box; `None` where
    /// the frame can't be certified over it.
    pub(crate) fn jet_box(&self, [ru, rs]: [[f64; 2]; 2]) -> Option<JVec<Interval>> {
        if !(ru[0].is_finite() && ru[1].is_finite()) {
            return None;
        }
        let q = self.profile.jet_box(ru);
        let m = self.carry.jet_box(rs)?;
        let [s0, s1] = self.carry.range();
        let (a, rate) = self.sigma(rs[0].clamp(s0, s1));
        let (b, _) = self.sigma(rs[1].clamp(s0, s1));
        let sigma = Interval::new(a.min(b), a.max(b))?;
        Some(self.jets([q[0], q[1], q[2]], m, (sigma, rate)))
    }

    pub(crate) fn point(&self, u: f64, s: f64) -> Point3 {
        let p = self.jets_at(u, s);
        Point3::new(p[0].v, p[1].v, p[2].v)
    }

    pub(crate) fn eval(&self, u: f64, s: f64) -> SurfaceEval {
        let p = self.jets_at(u, s);
        let part = |f: &dyn Fn(&Jet<f64>) -> f64| Vector3::new(f(&p[0]), f(&p[1]), f(&p[2]));
        SurfaceEval {
            point: Point3::ORIGIN + part(&|j| j.v),
            du: part(&|j| j.d[0]),
            dv: part(&|j| j.d[1]),
            duu: part(&|j| j.h[0]),
            duv: part(&|j| j.h[1]),
            dvv: part(&|j| j.h[2]),
        }
    }

    /// The natural normal, along `P_u × P_s`.
    pub(crate) fn normal(&self, u: f64, s: f64) -> Option<UnitVector3> {
        let e = self.eval(u, s);
        e.du.cross(e.dv).normalize()
    }

    pub(crate) fn domain(&self) -> [[f64; 2]; 2] {
        [self.profile_range, self.carry.range()]
    }

    pub(crate) fn periods(&self) -> (Option<f64>, Option<f64>) {
        let [u0, u1] = self.profile_range;
        let [s0, s1] = self.carry.range();
        (
            whole(&self.profile, self.profile_range).then_some(u1 - u0),
            self.carry.closed().then_some(s1 - s0),
        )
    }

    /// 1 for a closed profile round a closed path (a tube, like a torus);
    /// otherwise open.
    pub(super) fn closed_genus(&self) -> Option<u32> {
        match self.periods() {
            (Some(_), Some(_)) => Some(1),
            _ => None,
        }
    }

    pub(super) fn bounds(&self) -> Box3 {
        self.bounds
    }

    /// The cells searches start from: the profile's pieces against the
    /// frame's stretches.
    pub(crate) fn cells(&self) -> Vec<[[f64; 2]; 2]> {
        let us = frame::breaks(&self.profile, self.profile_range);
        let ss = self.carry.stretches();
        us.windows(2)
            .flat_map(|u| ss.iter().map(move |&s| [[u[0], u[1]], s]))
            .collect()
    }

    /// Certifies the sweep regular over its domain and returns the box
    /// its cells' enclosures fill.
    fn certify(&self) -> Result<Box3, Error> {
        let mut stack: Vec<([[f64; 2]; 2], u32)> =
            self.cells().into_iter().map(|c| (c, 0)).collect();
        let mut seen = 0usize;
        let mut bounds: Option<Box3> = None;
        while let Some((cell, depth)) = stack.pop() {
            seen += 1;
            if seen > MAX_CELLS {
                return Err(Error::Degenerate);
            }
            let q = self.jet_box(cell);
            if let Some(q) = &q {
                let pu = q.map(|j| j.d[0]);
                let ps = q.map(|j| j.d[1]);
                let n = icross(pu, ps);
                if idot(n, n).lo() > 0.0 {
                    let b = point_box(q);
                    bounds = Some(bounds.map_or(b, |o| o.union(&b)));
                    continue;
                }
            }
            if depth == MAX_DEPTH {
                return Err(Error::Degenerate);
            }
            // Halve across the longer side in model space.
            let len = |k: usize| {
                let w = cell[k][1] - cell[k][0];
                q.as_ref().map_or(w, |q| {
                    let d = q.map(|j| j.d[k]);
                    w * crate::math::sqrt(idot(d, d).hi())
                })
            };
            let k = usize::from(len(1) > len(0));
            let m = (cell[k][0] + cell[k][1]) / 2.0;
            let (mut a, mut b) = (cell, cell);
            a[k][1] = m;
            b[k][0] = m;
            stack.push((a, depth + 1));
            stack.push((b, depth + 1));
        }
        let b = bounds.ok_or(Error::Degenerate)?;
        Ok(b.grow(1e-12 * (1.0 + b.max().to_vector().length())))
    }

    /// The nearest point, as [`nearest`] finds it from the sweep's cells.
    pub(crate) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        nearest(
            &Sheet::Sweep(self),
            self.domain(),
            self.periods(),
            &self.cells(),
            p,
        )
    }

    /// The sweep moved by a rigid transform; parameters are unchanged.
    /// Its frame is made again from the moved path, which is the frame
    /// moved, and not certified again.
    pub(super) fn transformed(&self, r: &Rigid) -> Self {
        let law = match self.law {
            FrameLaw::Minimal => FrameLaw::Minimal,
            FrameLaw::Binormal(b) => FrameLaw::Binormal(r.apply_unit(b)),
        };
        let start = self.start.transformed(r);
        let carry = Carry::build(
            self.carry.path().transformed(r),
            self.carry.range(),
            start.x().get(),
            law,
            self.carry.twist(),
        )
        .expect("a moved frame is defined where it was");
        let s = Self::with(
            (self.profile.transformed(r), self.profile_range),
            carry,
            start,
            law,
            self.scale,
        )
        .expect("a moved sweep is valid where it was");
        Self {
            bounds: moved_box(&self.bounds, |p| r.apply_point(p)),
            ..s
        }
    }

    /// The sweep reflected by `m`, with the same parameters: its profile,
    /// path and start reflected, the law's direction too, and the twist
    /// turning the other way. The frame's second axis (a minimal one's)
    /// or first (a binormal one's) is reflected and reversed, as is its
    /// part of the profile's place in it, so every point is reflected; the
    /// natural normal turns against the mirrored geometry.
    pub(super) fn mirrored(&self, m: &Mirror) -> Result<Self, Error> {
        let law = match self.law {
            FrameLaw::Minimal => FrameLaw::Minimal,
            FrameLaw::Binormal(b) => FrameLaw::Binormal(m.apply_unit(b)),
        };
        let start = m.frame(&self.start)?;
        let carry = Carry::build(
            self.carry.path().mirrored(m)?,
            self.carry.range(),
            start.x().get(),
            law,
            -self.carry.twist(),
        )?;
        let s = Self::with(
            (self.profile.mirrored(m)?, self.profile_range),
            carry,
            start,
            law,
            self.scale,
        )?;
        Ok(Self {
            bounds: moved_box(&self.bounds, |p| m.apply_point(p)),
            ..s
        })
    }

    /// A NURBS within `tolerance` (at least
    /// [`crate::tol::LINEAR_RESOLUTION`]) of the sweep over `[s0, s1]` of
    /// its path and its whole profile, and the certified bound on how far
    /// it strays: the parameters are the profile's NURBS form's along `u`
    /// (its own [`Reparam`]: a line's or a spline's, a circle's angle) and
    /// the sweep's along `s`. A sweep has no exact rational form
    /// ([`super::Surface::to_nurbs`] is `Degenerate`); this is for export.
    ///
    /// Each section is the profile moved rigidly and scaled, so it is the
    /// profile's NURBS with the same weights and its control points `Qₖ`
    /// carried: `Qₖ(s) = c(s) + Σⱼ (aⱼ + σ(s) bⱼₖ) fⱼ(s)`. Along `s` each is
    /// a cubic Hermite spline through its values and derivatives at the
    /// ends of spans (the frame's stretches, halved), and the surface
    /// strays by at most the worst `|Q̃ₖ - Qₖ|`, which is at most `h² / 8`
    /// times a bound on `Q̃ₖ'' - Qₖ''` over the span, enclosed from the
    /// frame's jets. A span is halved until that is within the tolerance,
    /// up to 20 times; past that, or for a range outside the path's (or
    /// longer than a closed path), the call fails with `Degenerate`.
    pub fn to_nurbs_within(
        &self,
        [s0, s1]: [f64; 2],
        tolerance: f64,
    ) -> Result<(NurbsSurface, [Reparam; 2], f64), Error> {
        let tolerance = tolerance.max(crate::tol::LINEAR_RESOLUTION);
        let [d0, d1] = self.carry.range();
        let ok = if self.carry.closed() {
            s1 - s0 <= d1 - d0
        } else {
            d0 <= s0 && s1 <= d1
        };
        if !(s0 < s1 && ok) {
            return Err(Error::Degenerate);
        }
        let (n, reparam) = self.profile.to_nurbs(self.profile_range)?;
        let n = if whole(&self.profile, self.profile_range) {
            n
        } else {
            n.part(self.profile_range.map(|t| reparam.to_nurbs(t)))?
        };
        let pts: Vec<Point3> = n.control_points().collect();
        let (along, rows, worst) = self.fit_carried(&pts, [s0, s1], tolerance)?;
        let count = rows[0].len();
        let points = rows.into_iter().flatten().collect();
        let weights = n
            .weights()
            .iter()
            .flat_map(|&w| core::iter::repeat_n(w, count))
            .collect();
        let surface = NurbsSurface::new([n.knots().clone(), along], points, Some(weights))?;
        Ok((surface, [reparam, Reparam::Identity], worst))
    }

    /// Cubic Hermite splines in `s` over `[s0, s1]` through the points
    /// `pts` carried along (see [`Sweep::to_nurbs_within`]): the knots,
    /// each point's control points, and the worst certified bound, within
    /// `tolerance` or `Degenerate`.
    fn fit_carried(
        &self,
        pts: &[Point3],
        [s0, s1]: [f64; 2],
        tolerance: f64,
    ) -> Result<(KnotVector, Vec<Vec<Point3>>, f64), Error> {
        let [d0, d1] = self.carry.range();
        // The frame's stretches over the range, halved until every span's
        // bound is within the tolerance.
        let mut cuts: Vec<f64> = vec![s0];
        let period = d1 - d0;
        for [a, _] in self.carry.stretches() {
            for k in -1i32..=1 {
                let shift = if self.carry.closed() {
                    period * f64::from(k)
                } else {
                    0.0
                };
                let x = a + shift;
                if x > s0 && x < s1 {
                    cuts.push(x);
                }
            }
        }
        cuts.push(s1);
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        let mut spans: Vec<([f64; 2], u32)> = cuts.windows(2).map(|w| ([w[0], w[1]], 0)).collect();
        let mut done: Vec<[f64; 2]> = Vec::new();
        let mut worst = 0.0f64;
        while let Some((span, depth)) = spans.pop() {
            match self.hermite_bound(pts, span) {
                Some(bound) if bound <= tolerance => {
                    worst = worst.max(bound);
                    done.push(span);
                    continue;
                }
                _ => {}
            }
            if depth == MAX_FIT_DEPTH {
                return Err(Error::Degenerate);
            }
            let m = (span[0] + span[1]) / 2.0;
            spans.push(([span[0], m], depth + 1));
            spans.push(([m, span[1]], depth + 1));
        }
        done.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let mut knots = vec![done[0][0]];
        knots.extend(done.iter().map(|s| s[1]));
        let mut mults = vec![3; knots.len()];
        mults[0] = 4;
        *mults.last_mut().expect("two knots") = 4;
        let along = KnotVector::clamped(3, &knots, &mults)?;
        // Each point's row along s: its value at the first knot, then each
        // span's inner Hermite points and its end.
        let mut rows: Vec<Vec<Point3>> = vec![Vec::new(); pts.len()];
        let first = self.carried(pts, knots[0]);
        for (row, (p, _)) in rows.iter_mut().zip(&first) {
            row.push(*p);
        }
        for span in &done {
            let h = span[1] - span[0];
            let a = self.carried(pts, span[0]);
            let b = self.carried(pts, span[1]);
            for (row, ((pa, da), (pb, db))) in rows.iter_mut().zip(a.iter().zip(&b)) {
                row.push(*pa + *da * (h / 3.0));
                row.push(*pb - *db * (h / 3.0));
                row.push(*pb);
            }
        }
        Ok((along, rows, worst))
    }

    /// The profile's control points carried to `s`, with their
    /// derivatives by `s`.
    fn carried(&self, pts: &[Point3], s: f64) -> Vec<(Point3, Vector3)> {
        let m = self
            .carry
            .jets_at(s)
            .expect("a certified sweep has a frame on its path");
        let sigma = self.sigma(s);
        let zero = [0.0; 3];
        pts.iter()
            .map(|q| {
                let p = self.jets([q.to_vector().to_array(), zero, zero], m, sigma);
                (
                    Point3::new(p[0].v, p[1].v, p[2].v),
                    Vector3::new(p[0].d[1], p[1].d[1], p[2].d[1]),
                )
            })
            .collect()
    }

    /// The bound on how far the Hermite spans of the carried control
    /// points stray from them over `span`: `h² / 8` times the worst
    /// `|Q̃ₖ'' - Qₖ''|` over it. `None` where the frame can't be enclosed.
    fn hermite_bound(&self, pts: &[Point3], span: [f64; 2]) -> Option<f64> {
        let h = span[1] - span[0];
        let [d0, d1] = self.carry.range();
        let a = self.carried(pts, span[0]);
        let b = self.carried(pts, span[1]);
        let zero = [Interval::point(0.0); 3];
        // Each point's Hermite cubic, by its Bézier control points.
        let cubics: Vec<[Vector3; 4]> = (0..pts.len())
            .map(|k| {
                [
                    a[k].0.to_vector(),
                    a[k].0.to_vector() + a[k].1 * (h / 3.0),
                    b[k].0.to_vector() - b[k].1 * (h / 3.0),
                    b[k].0.to_vector(),
                ]
            })
            .collect();
        // The cubic's point, derivative by `s` and second derivative at
        // `x` of the span.
        let at = |p: &[Vector3; 4], x: f64| {
            let y = 1.0 - x;
            let v = p[0] * (y * y * y)
                + p[1] * (3.0 * x * y * y)
                + p[2] * (3.0 * x * x * y)
                + p[3] * (x * x * x);
            let d =
                ((p[1] - p[0]) * (y * y) + (p[2] - p[1]) * (2.0 * x * y) + (p[3] - p[2]) * (x * x))
                    * (3.0 / h);
            let dd =
                ((p[2] - p[1] * 2.0 + p[0]) * y + (p[3] - p[2] * 2.0 + p[1]) * x) * (6.0 / (h * h));
            (v, d, dd)
        };
        // Over each eighth of the span, Taylor's bound about its middle:
        // `|e(m)| + |e'(m)| r + max |e''| r² / 2` for the error `e` of the
        // fit, with `max |e''|` enclosed over the eighth. As the eighths
        // shrink it tends to the error itself, never to the spread of the
        // second derivatives.
        let mut worst = 0.0f64;
        for piece in 0..HERMITE_PIECES {
            let x0 = f64::from(piece) / f64::from(HERMITE_PIECES);
            let x1 = f64::from(piece + 1) / f64::from(HERMITE_PIECES);
            let xm = (x0 + x1) / 2.0;
            let (sub, mid) = ([span[0] + h * x0, span[0] + h * x1], span[0] + h * xm);
            let r = h * (x1 - x0) / 2.0;
            let m = self.carry.jet_box(sub)?;
            let (sa, rate) = self.sigma(sub[0].clamp(d0, d1));
            let (sb, _) = self.sigma(sub[1].clamp(d0, d1));
            let sigma = Interval::new(sa.min(sb), sa.max(sb))?;
            let truth_mid = self.carried(pts, mid);
            for ((q, cubic), (qm, dm)) in pts.iter().zip(&cubics).zip(&truth_mid) {
                let (v, d, _) = at(cubic, xm);
                let g = (v - qm.to_vector()).length();
                let dg = (d - *dm).length();
                let qi = q.to_vector().to_array().map(Interval::point);
                let truth = self.jets([qi, zero, zero], m, (sigma, rate));
                let (_, _, f0) = at(cubic, x0);
                let (_, _, f1) = at(cubic, x1);
                let (f0, f1) = (f0.to_array(), f1.to_array());
                let e: IVec3 = core::array::from_fn(|i| {
                    let pad = 1e-12 * (f0[i].abs() + f1[i].abs());
                    let fitted = Interval::new(f0[i].min(f1[i]) - pad, f0[i].max(f1[i]) + pad)
                        .expect("ordered");
                    fitted - truth[i].h[2]
                });
                let m2 = crate::math::sqrt(idot(e, e).hi()) * (1.0 + 1e-12);
                worst = worst.max(g + dg * r + m2 * r * r / 2.0);
            }
        }
        Some(worst * (1.0 + 1e-9) + crate::intersect::SLACK)
    }

    /// Bounds over a parameter box, the worst of those from the
    /// enclosures over its parts between the profile's pieces and the
    /// frame's closed forms (see [`super::bound2::from_jets`]): one
    /// enclosure over many of a frame's samples is loose. A box cut into
    /// more than [`BOUND_PARTS`] parts is bounded whole.
    pub(super) fn derivative_bound(&self, [ru, rs]: [[f64; 2]; 2]) -> SurfaceBound {
        let split = |r: [f64; 2], cuts: Vec<f64>| -> Vec<[f64; 2]> {
            let mut at = vec![r[0]];
            at.extend(cuts.into_iter().filter(|&x| x > r[0] && x < r[1]));
            at.push(r[1]);
            at.windows(2).map(|w| [w[0], w[1]]).collect()
        };
        let us = split(ru, frame::breaks(&self.profile, self.profile_range));
        let ss = split(rs, self.carry.cuts());
        let parts: Vec<[[f64; 2]; 2]> = if us.len() * ss.len() > BOUND_PARTS {
            vec![[ru, rs]]
        } else {
            us.iter()
                .flat_map(|&u| ss.iter().map(move |&s| [u, s]))
                .collect()
        };
        let mut out = SurfaceBound {
            duu: 0.0,
            duv: 0.0,
            dvv: 0.0,
            turn_u: 0.0,
            turn_v: 0.0,
        };
        // A part over which the normal may turn by more than a radian is
        // halved across its turning side, a few times: the turning rates
        // are bounded from `|S_u × S_v|` from below, which a wide part
        // underestimates.
        let mut stack: Vec<([[f64; 2]; 2], u32)> = parts.into_iter().map(|c| (c, 0)).collect();
        let mut budget = BOUND_PARTS;
        while let Some((cell, depth)) = stack.pop() {
            let Some(q) = self.jet_box(cell) else {
                return super::bound2::INFINITE_BOUND;
            };
            let b = super::bound2::from_jets(&q);
            let (wu, ws) = (cell[0][1] - cell[0][0], cell[1][1] - cell[1][0]);
            let (tu, ts) = (b.turn_u * wu, b.turn_v * ws);
            let loose = (tu + ts).is_nan() || tu + ts > 1.0;
            if depth < BOUND_DEPTH && budget >= 2 && loose {
                budget -= 2;
                let k = usize::from(ts > tu || tu.is_nan() || ts.is_nan());
                let m = (cell[k][0] + cell[k][1]) / 2.0;
                let (mut lo, mut hi) = (cell, cell);
                lo[k][1] = m;
                hi[k][0] = m;
                stack.push((lo, depth + 1));
                stack.push((hi, depth + 1));
                continue;
            }
            out.duu = out.duu.max(b.duu);
            out.duv = out.duv.max(b.duv);
            out.dvv = out.dvv.max(b.dvv);
            out.turn_u = out.turn_u.max(b.turn_u);
            out.turn_v = out.turn_v.max(b.turn_v);
        }
        out
    }
}

/// How many times [`Sweep::derivative_bound`] halves a part.
const BOUND_DEPTH: u32 = 4;

/// The most parts [`Sweep::derivative_bound`] bounds a box over.
const BOUND_PARTS: usize = 64;

#[cfg(test)]
pub(crate) mod tests;
