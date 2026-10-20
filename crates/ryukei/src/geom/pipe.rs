//! The pipe: the surface a ball of fixed radius sweeps along a spine
//! curve, the exact surface of a rolling-ball fillet.

use super::bound2::{IVec3, trig_box};
use super::jet::{self, JVec, Jet, Num};
use super::{
    Box3, Curve, NurbsCurve, Point3, Projection, Reparam, Rigid, Surface, SurfaceBound,
    SurfaceEval, UnitVector3, Vector3,
};
use crate::Error;
use crate::interval::Interval;
use crate::math;
use core::f64::consts::{PI, TAU};

/// `P(s, θ) = c(s) + r (cos θ e₁(s) + sin θ e₂(s))`: a circle of radius
/// `r` about each point of a spine curve `c`, in the plane normal to it.
///
/// The frame is anchored on an analytic surface, the face the ball rolls
/// on: `e₁` is the direction from the spine towards its foot on the
/// anchor, made perpendicular to the spine's tangent `t`, and
/// `e₂ = e₁ × t`. So `θ = 0` is where a ball centred on the spine touches
/// the anchor, the seam `θ = ±π` is opposite it, and the natural normal,
/// `cos θ e₁ + sin θ e₂`, points away from the spine along `P_s × P_θ`.
/// `s` is the spine's parameter (periodic when the spine is) and `θ` runs
/// over `[-π, π]`.
///
/// A pipe is built only where it is regular, certified over the whole
/// spine (see [`Pipe::new`]). A spine along a line or a circle gives a
/// cylinder or a torus, which are used instead.
#[derive(Clone, Debug)]
pub struct Pipe {
    spine: Curve,
    radius: f64,
    anchor: Surface,
    /// The sign of the spine's signed distance from the anchor: `1.0` on
    /// the side its natural normal points to.
    side: f64,
    /// An ellipse spine's exact NURBS form, for projection.
    flat: Option<(NurbsCurve, Reparam)>,
    bounds: Box3,
}

impl PartialEq for Pipe {
    fn eq(&self, other: &Self) -> bool {
        (&self.spine, self.radius, &self.anchor) == (&other.spine, other.radius, &other.anchor)
    }
}

/// The deepest a stretch of spine is halved while certifying a pipe.
const MAX_DEPTH: u32 = 12;

/// Constant jets from a vector.
fn constant<T: Num>(v: Vector3) -> JVec<T> {
    v.to_array().map(|x| Jet::constant(T::of(x)))
}

/// The anchor's natural normal at the foot of `c`, a point off it, as a
/// field over jets: the gradient of its signed distance. `None` where it
/// may not be defined (on an axis, at a sphere's centre, on a torus's core
/// circle) or for a surface that can't anchor a pipe.
fn anchor_normal<T: Num>(anchor: &Surface, c: JVec<T>) -> Option<JVec<T>> {
    let f = anchor.frame()?;
    let (x, y, z) = (
        constant(f.x().get()),
        constant(f.y().get()),
        constant(f.z().get()),
    );
    let q = jet::sub(c, constant(f.origin().to_vector()));
    let (lx, ly) = (jet::dot(q, x), jet::dot(q, y));
    let radial = || -> Option<JVec<T>> {
        let inv = (lx * lx + ly * ly).sqrt()?.recip()?;
        Some(jet::times(
            jet::add(jet::times(x, lx), jet::times(y, ly)),
            inv,
        ))
    };
    match anchor {
        Surface::Plane(_) => Some(z),
        Surface::Cylinder(_) => radial(),
        Surface::Sphere(_) => jet::unit(q),
        Surface::Cone(k) => {
            let (sin, cos) = math::sin_cos(k.half_angle());
            Some(jet::add(
                jet::times(radial()?, Jet::constant(T::of(cos))),
                jet::times(z, Jet::constant(T::of(-sin))),
            ))
        }
        Surface::Torus(t) => jet::unit(jet::sub(
            q,
            jet::times(radial()?, Jet::constant(T::of(t.major()))),
        )),
        _ => None,
    }
}

/// The anchor's signed distance at `c` (positive on its natural normal's
/// side), and for a cone, how far the point's foot along its generator is
/// from the apex: the distance holds only while that is positive.
fn anchor_distance<T: Num>(anchor: &Surface, c: [T; 3]) -> Option<(T, T)> {
    let f = anchor.frame()?;
    let o = f.origin().to_vector().to_array();
    let q: [T; 3] = core::array::from_fn(|k| c[k] - T::of(o[k]));
    let along = |a: Vector3| {
        let a = a.to_array();
        q[0] * T::of(a[0]) + q[1] * T::of(a[1]) + q[2] * T::of(a[2])
    };
    let (lx, ly, lz) = (along(f.x().get()), along(f.y().get()), along(f.z().get()));
    let rho = || (lx * lx + ly * ly).sqrt();
    let one = T::of(1.0);
    Some(match anchor {
        Surface::Plane(_) => (lz, one),
        Surface::Cylinder(k) => (rho()? - T::of(k.radius()), one),
        Surface::Sphere(k) => (
            (lx * lx + ly * ly + lz * lz).sqrt()? - T::of(k.radius()),
            one,
        ),
        Surface::Torus(t) => {
            let w = rho()? - T::of(t.major());
            ((w * w + lz * lz).sqrt()? - T::of(t.minor()), one)
        }
        Surface::Cone(k) => {
            let (sin, cos) = math::sin_cos(k.half_angle());
            let w = rho()? - T::of(k.radius());
            let apex = T::of(k.radius() / sin);
            (
                w * T::of(cos) - lz * T::of(sin),
                w * T::of(sin) + lz * T::of(cos) + apex,
            )
        }
        _ => return None,
    })
}

/// Jets of a spine point and of its derivative, from the point and its
/// first three derivatives.
fn spine_jets<T: Num>(d: [[T; 3]; 4]) -> (JVec<T>, JVec<T>) {
    (
        core::array::from_fn(|k| Jet::of_s(d[0][k], d[1][k], d[2][k])),
        core::array::from_fn(|k| Jet::of_s(d[1][k], d[2][k], d[3][k])),
    )
}

impl Pipe {
    /// The pipe of `radius` about `spine`, anchored on `anchor`.
    ///
    /// Fails with `NonFinite`, or `Degenerate`: for a spine that is a line
    /// or a circle (use a cylinder or a torus), an anchor that is not
    /// analytic, a radius below [`crate::tol::LINEAR_RESOLUTION`], or a
    /// pipe that can't be certified regular over the spine's whole domain.
    /// That is checked on stretches of the spine (its Bézier pieces, or
    /// eighths of an ellipse), halved up to 12 times, in interval
    /// arithmetic: the spine's speed is positive; it stays on one side of
    /// the anchor, off the anchor's axis, centre or core circle, and over
    /// a cone's nappe rather than its apex; its tangent is never along the
    /// anchor's normal, so the frame is defined; and `r` times its
    /// curvature stays below 1, so the pipe never folds.
    pub fn new(spine: Curve, radius: f64, anchor: Surface) -> Result<Self, Error> {
        if !radius.is_finite() {
            return Err(Error::NonFinite);
        }
        if radius < crate::tol::LINEAR_RESOLUTION {
            return Err(Error::Degenerate);
        }
        if anchor.frame().is_none() {
            return Err(Error::Degenerate);
        }
        let flat = match &spine {
            Curve::Line(_) | Curve::Circle(_) => return Err(Error::Degenerate),
            Curve::Ellipse(e) => Some(e.to_nurbs()?),
            Curve::Nurbs(_) => None,
        };
        let [d0, _] = spine.domain();
        let (dist, _) =
            anchor_distance(&anchor, spine.point(d0).to_array()).ok_or(Error::Degenerate)?;
        if dist == 0.0 {
            return Err(Error::Degenerate);
        }
        let bounds = spine.bounds(spine.domain()).grow(radius * (1.0 + 1e-12));
        let pipe = Self {
            spine,
            radius,
            anchor,
            side: dist.signum(),
            flat,
            bounds,
        };
        pipe.certify()?;
        Ok(pipe)
    }

    pub fn spine(&self) -> &Curve {
        &self.spine
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }

    pub fn anchor(&self) -> &Surface {
        &self.anchor
    }

    /// The stretches the spine is first cut into for certifying.
    fn stretches(&self) -> Vec<[f64; 2]> {
        match &self.spine {
            Curve::Nurbs(n) => n.pieces().iter().map(|p| p.range).collect(),
            _ => (0..8)
                .map(|i| [TAU * f64::from(i) / 8.0, TAU * f64::from(i + 1) / 8.0])
                .collect(),
        }
    }

    fn certify(&self) -> Result<(), Error> {
        let mut stack: Vec<([f64; 2], u32)> =
            self.stretches().into_iter().map(|r| (r, 0)).collect();
        while let Some((range, depth)) = stack.pop() {
            match self.regular_over(range) {
                Some(true) => {}
                Some(false) => return Err(Error::Degenerate),
                None if depth < MAX_DEPTH => {
                    let m = (range[0] + range[1]) / 2.0;
                    stack.push(([range[0], m], depth + 1));
                    stack.push(([m, range[1]], depth + 1));
                }
                None => return Err(Error::Degenerate),
            }
        }
        Ok(())
    }

    /// Whether the pipe is regular over a stretch of spine: `Some(false)`
    /// where the spine is certified to cross to the anchor's other side,
    /// `None` where rounding can't tell.
    fn regular_over(&self, range: [f64; 2]) -> Option<bool> {
        let d = self.spine.jet_box(range);
        let (dist, foot) = anchor_distance(&self.anchor, d[0])?;
        let side = dist * Interval::point(self.side);
        if side.hi() < 0.0 {
            return Some(false);
        }
        if side.lo() <= 0.0 || foot.lo() <= 0.0 {
            return None;
        }
        let (c, cp) = spine_jets(d);
        self.frame(c, cp)?;
        let dot = |a: IVec3, b: IVec3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let speed2 = dot(d[1], d[1]);
        let cross: IVec3 = [
            d[1][1] * d[2][2] - d[1][2] * d[2][1],
            d[1][2] * d[2][0] - d[1][0] * d[2][2],
            d[1][0] * d[2][1] - d[1][1] * d[2][0],
        ];
        // r κ < 1, with κ = |c' × c''| / |c'|³: r² |c' × c''|² < |c'|⁶.
        let r2 = Interval::point(self.radius).square();
        let lhs = r2 * dot(cross, cross);
        let rhs = speed2 * speed2 * speed2;
        (speed2.lo() > 0.0 && lhs.hi() < rhs.lo()).then_some(true)
    }

    /// The frame `(e₁, e₂)` at a spine point and its derivative, as jets.
    fn frame<T: Num>(&self, c: JVec<T>, cp: JVec<T>) -> Option<(JVec<T>, JVec<T>)> {
        let n = anchor_normal(&self.anchor, c)?;
        let toward = jet::times(n, Jet::constant(T::of(-self.side)));
        let w = jet::sub(
            jet::times(toward, jet::dot(cp, cp)),
            jet::times(cp, jet::dot(toward, cp)),
        );
        let e1 = jet::unit(w)?;
        let e2 = jet::unit(jet::cross(e1, cp))?;
        Some((e1, e2))
    }

    /// The point and the natural normal as jets, from the spine's point
    /// and derivatives and `(cos θ, sin θ)`.
    pub(crate) fn jets<T: Num>(&self, d: [[T; 3]; 4], cos: T, sin: T) -> Option<[JVec<T>; 2]> {
        let (c, cp) = spine_jets(d);
        let (e1, e2) = self.frame(c, cp)?;
        let ct = Jet::of_t(cos, -sin, -cos);
        let st = Jet::of_t(sin, cos, -sin);
        let around = jet::add(jet::times(e1, ct), jet::times(e2, st));
        let p = jet::add(c, around.map(|x| x.scale(self.radius)));
        Some([p, around])
    }

    /// Jets in floating point at `(s, θ)`. A clamped spine's parameter is
    /// taken within its domain if the frame fails outside it.
    fn jets_at(&self, s: f64, t: f64) -> [JVec<f64>; 2] {
        let (sin, cos) = math::sin_cos(t);
        let at = |s: f64| {
            let d = self.spine.jet3(s).map(Vector3::to_array);
            self.jets(d, cos, sin)
        };
        at(s)
            .or_else(|| {
                let [d0, d1] = self.spine.domain();
                at(s.clamp(d0, d1))
            })
            .expect("a certified pipe has a frame on its spine")
    }

    pub(super) fn point(&self, s: f64, t: f64) -> Point3 {
        let [p, _] = self.jets_at(s, t);
        Point3::new(p[0].v, p[1].v, p[2].v)
    }

    pub(super) fn eval(&self, s: f64, t: f64) -> SurfaceEval {
        let [p, _] = self.jets_at(s, t);
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

    pub(super) fn normal(&self, s: f64, t: f64) -> Option<UnitVector3> {
        let [_, n] = self.jets_at(s, t);
        Vector3::new(n[0].v, n[1].v, n[2].v).normalize()
    }

    pub(super) fn domain(&self) -> [[f64; 2]; 2] {
        [self.spine.domain(), [-PI, PI]]
    }

    pub(super) fn periods(&self) -> (Option<f64>, Option<f64>) {
        (self.spine.period(), Some(TAU))
    }

    /// 1 about a closed spine (a tube, like a torus); otherwise open.
    pub(super) fn closed_genus(&self) -> Option<u32> {
        self.spine.period().map(|_| 1)
    }

    pub(super) fn bounds(&self) -> Box3 {
        self.bounds
    }

    /// The nearest point, with the guarantee of [`NurbsCurve::project`].
    ///
    /// The pipe's normals pass through the spine at right angles to it, so
    /// at the nearest point (where `p - P` is along the normal) `s` is a
    /// parameter where the distance from `p` to the spine is stationary,
    /// unless it is at an end of a clamped spine, where the pipe ends in a
    /// circle. At each such `s` the nearest point of the circle there is
    /// along `p - c(s)` seen in the circle's plane: angle `θ` from its
    /// components along `e₁` and `e₂`. The least distance among them wins,
    /// ties going to the lowest parameter.
    pub(super) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        let mut ss = match (&self.spine, &self.flat) {
            (Curve::Nurbs(n), _) => n.stationary(p),
            (_, Some((n, reparam))) => n
                .stationary(p)
                .into_iter()
                .map(|t| reparam.from_nurbs(t).rem_euclid(TAU))
                .collect(),
            _ => unreachable!("a pipe's spine is a NURBS or an ellipse"),
        };
        if self.spine.period().is_none() {
            let [d0, d1] = self.spine.domain();
            ss.push(d0);
            ss.push(d1);
        }
        ss.sort_by(f64::total_cmp);
        let mut feet: Vec<Projection<(f64, f64), Point3>> = Vec::with_capacity(ss.len());
        for s in ss {
            let [p0, _] = self.jets_at(s, 0.0);
            let [p1, _] = self.jets_at(s, PI / 2.0);
            let c = self.spine.point(s);
            // e₁ and e₂ from the points at θ = 0 and θ = π/2.
            let e1 = (Point3::new(p0[0].v, p0[1].v, p0[2].v) - c) / self.radius;
            let e2 = (Point3::new(p1[0].v, p1[1].v, p1[2].v) - c) / self.radius;
            let d = p - c;
            let (x, y) = (d.dot(e1), d.dot(e2));
            let t = if x == 0.0 && y == 0.0 {
                0.0
            } else {
                math::atan2(y, x)
            };
            let point = self.point(s, t);
            feet.push(Projection {
                param: (s, t),
                point,
                distance: point.distance(p),
            });
        }
        let least = feet
            .iter()
            .map(|f| f.distance)
            .fold(f64::INFINITY, f64::min);
        feet.into_iter()
            .find(|f| f.distance <= least + crate::tol::LINEAR_RESOLUTION)
            .expect("a spine has at least one stationary point or end")
    }

    /// The pipe moved by a rigid transform; parameters are unchanged.
    pub(super) fn transformed(&self, r: &Rigid) -> Self {
        let spine = self.spine.transformed(r);
        Self {
            bounds: spine
                .bounds(spine.domain())
                .grow(self.radius * (1.0 + 1e-12)),
            flat: match &spine {
                Curve::Ellipse(e) => Some(e.to_nurbs().expect("an ellipse converts exactly")),
                _ => None,
            },
            spine,
            radius: self.radius,
            anchor: self.anchor.transformed(r),
            side: self.side,
        }
    }

    /// The pipe about the same spine with its radius grown by `d`: the
    /// offset by `d` along the natural normal, with the same parameters.
    pub(super) fn offset(&self, d: f64) -> Result<Self, Error> {
        Self::new(self.spine.clone(), self.radius + d, self.anchor.clone())
    }

    /// Enclosures of the point and natural normal jets over a parameter
    /// box: the spine's derivatives over `s` and `cos θ`, `sin θ` over
    /// `θ`. `None` where the frame can't be certified over the box.
    pub(crate) fn jet_box(&self, [rs, rt]: [[f64; 2]; 2]) -> Option<[JVec<Interval>; 2]> {
        let d = self.spine.jet_box(rs);
        let (cos, sin) = trig_box(rt);
        self.jets(d, cos, sin)
    }

    /// Bounds over a parameter box, from the jets' enclosures: `Suu`,
    /// `Suv` and `Svv` are the point's second derivatives, and the unit
    /// normal turns as fast as its own derivatives (`1` along `θ`). A
    /// clamped spine's range is cut to its domain first.
    ///
    /// Interval jets over a long stretch of spine or a wide angle are
    /// loose (by orders of magnitude over a tenth of an ellipse), so a
    /// wide box is cut into pieces at most a 64th of the spine's domain
    /// long and `π/8` wide, and a piece whose bounds are more than twice
    /// the derivatives at its middle plus half their scale (or whose frame
    /// can't be certified at once) is halved along `s`, up to 10 times.
    /// The worst piece's bounds hold for the whole box.
    pub(super) fn derivative_bound(&self, [rs, rt]: [[f64; 2]; 2]) -> SurfaceBound {
        let [d0, d1] = self.spine.domain();
        let rs = match self.spine.period() {
            Some(_) => rs,
            None => [rs[0].clamp(d0, d1), rs[1].clamp(d0, d1)],
        };
        let pieces = |[a, b]: [f64; 2], step: f64, most: f64| {
            let n = ((b - a) / step).ceil().clamp(1.0, most) as usize;
            (0..n).map(move |i| {
                let at = |k: usize| a + (b - a) * k as f64 / n as f64;
                [at(i), if i + 1 == n { b } else { at(i + 1) }]
            })
        };
        let mut out = ZERO_BOUND;
        for s in pieces(rs, (d1 - d0) / 64.0, 64.0) {
            for t in pieces(rt, PI / 8.0, 16.0) {
                out = worse(out, self.bound_over(s, t, 10));
            }
        }
        out
    }

    /// Bounds on `|P_s|` and `|P_θ|` over a parameter box (`r` along
    /// `θ`), from the jets' enclosures over pieces of it as
    /// [`Pipe::derivative_bound`] takes them.
    pub(crate) fn first_bound(&self, [rs, rt]: [[f64; 2]; 2]) -> (f64, f64) {
        let [d0, d1] = self.spine.domain();
        let rs = match self.spine.period() {
            Some(_) => rs,
            None => [rs[0].clamp(d0, d1), rs[1].clamp(d0, d1)],
        };
        let n = ((rs[1] - rs[0]) / ((d1 - d0) / 256.0))
            .ceil()
            .clamp(1.0, 256.0) as usize;
        let mut su = 0.0f64;
        for i in 0..n {
            let at = |k: usize| rs[0] + (rs[1] - rs[0]) * k as f64 / n as f64;
            let piece = [at(i), if i + 1 == n { rs[1] } else { at(i + 1) }];
            let Some([p, _]) = self.jet_box([piece, rt]) else {
                return (f64::INFINITY, f64::INFINITY);
            };
            let sum = p
                .iter()
                .fold(Interval::point(0.0), |acc, j| acc + j.d[0].square());
            su = su.max(sum.sqrt().map_or(f64::INFINITY, Interval::hi));
        }
        (su, self.radius * (1.0 + 1e-12))
    }

    fn bound_over(&self, rs: [f64; 2], rt: [f64; 2], depth: u32) -> SurfaceBound {
        let len = |f: &dyn Fn(&Jet<Interval>) -> Interval, v: &JVec<Interval>| {
            let sum = v
                .iter()
                .fold(Interval::point(0.0), |acc, j| acc + f(j).square());
            sum.sqrt().map_or(f64::INFINITY, Interval::hi)
        };
        let bound = self.jet_box([rs, rt]).map(|[p, n]| SurfaceBound {
            duu: len(&|j| j.h[0], &p),
            duv: len(&|j| j.h[1], &p),
            dvv: len(&|j| j.h[2], &p),
            turn_u: len(&|j| j.d[0], &n),
            turn_v: len(&|j| j.d[1], &n),
        });
        let loose = |b: &SurfaceBound| {
            let (ms, mt) = ((rs[0] + rs[1]) / 2.0, (rt[0] + rt[1]) / 2.0);
            let [p, n] = self.jets_at(ms, mt);
            let at = |f: &dyn Fn(&Jet<f64>) -> f64, v: &JVec<f64>| {
                math::sqrt(v.iter().map(|j| f(j) * f(j)).sum::<f64>())
            };
            let (duu, duv, dvv) = (
                at(&|j| j.h[0], &p),
                at(&|j| j.h[1], &p),
                at(&|j| j.h[2], &p),
            );
            let (turn, speed) = (at(&|j| j.d[0], &n), at(&|j| j.d[0], &p));
            // Each compared with twice its value at the middle, and half
            // the scale of its kind, so a value near zero there doesn't
            // demand an exact zero.
            let second = 0.5 * (duu + duv + dvv);
            b.duu > 2.0 * duu + second
                || b.duv > 2.0 * duv + second
                || b.turn_u > 2.0 * turn + 0.5 * (turn + speed / self.radius)
        };
        match bound {
            Some(b) if depth == 0 || rs[0] >= rs[1] || !loose(&b) => b,
            None if depth == 0 || rs[0] >= rs[1] => INFINITE_BOUND,
            _ => {
                let m = (rs[0] + rs[1]) / 2.0;
                worse(
                    self.bound_over([rs[0], m], rt, depth - 1),
                    self.bound_over([m, rs[1]], rt, depth - 1),
                )
            }
        }
    }
}

const ZERO_BOUND: SurfaceBound = SurfaceBound {
    duu: 0.0,
    duv: 0.0,
    dvv: 0.0,
    turn_u: 0.0,
    turn_v: 0.0,
};

const INFINITE_BOUND: SurfaceBound = SurfaceBound {
    duu: f64::INFINITY,
    duv: f64::INFINITY,
    dvv: f64::INFINITY,
    turn_u: f64::INFINITY,
    turn_v: f64::INFINITY,
};

/// The larger of two bounds, each way.
fn worse(a: SurfaceBound, b: SurfaceBound) -> SurfaceBound {
    SurfaceBound {
        duu: a.duu.max(b.duu),
        duv: a.duv.max(b.duv),
        dvv: a.dvv.max(b.dvv),
        turn_u: a.turn_u.max(b.turn_u),
        turn_v: a.turn_v.max(b.turn_v),
    }
}

#[cfg(test)]
mod tests;
