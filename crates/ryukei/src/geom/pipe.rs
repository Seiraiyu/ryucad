//! The pipe: the surface a ball of fixed radius sweeps along a spine
//! curve, the exact surface of a rolling-ball fillet.

use super::bound2::{IVec3, trig_box};
use super::jet::{self, JVec, Jet, Num};
use super::nurbs::{TURN, turn_knots};
use super::sheet::Sheet;
use super::{
    Box3, Circle, Curve, Curve2, Ellipse, Frame, KnotVector, Line, NurbsCurve, NurbsSurface,
    Point3, Projection, Reparam, Rigid, Surface, SurfaceBound, SurfaceEval, UnitVector3, Vector3,
};
use crate::Error;
use crate::interval::Interval;
use crate::math;
use core::f64::consts::{PI, TAU};
use std::sync::OnceLock;

/// `P(s, θ) = c(s) + r (cos θ e₁(s) + sin θ e₂(s))`: a circle of radius
/// `r` about each point of a spine curve `c`, in the plane normal to it.
///
/// The frame is anchored on the face the ball rolls on. On an analytic
/// surface `e₁` is the direction from the spine towards its foot on the
/// anchor; on any other, the direction from the spine towards the rail,
/// the point `S(p(s))` of the anchor at a pcurve `p` (see
/// [`Pipe::on_rail`]). Either is made perpendicular to the spine's
/// tangent `t`, and `e₂ = e₁ × t`. So `θ = 0` is where a ball centred on the spine touches
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
    /// The rail on the anchor `e₁` points towards, for a pipe anchored on
    /// any surface; `None` for an analytic anchor's foot.
    rail: Option<Rail>,
    /// The sign of the spine's signed distance from an analytic anchor:
    /// `1.0` on the side its natural normal points to.
    side: f64,
    /// An ellipse spine's exact NURBS form, for projection.
    flat: Option<(NurbsCurve, Reparam)>,
    bounds: Box3,
    /// The frame's enclosures over each of [`Pipe::stretches`], made on
    /// first use.
    frames: Vec<OnceLock<Option<Vec<FrameBox>>>>,
}

impl PartialEq for Pipe {
    fn eq(&self, other: &Self) -> bool {
        (&self.spine, self.radius, &self.anchor, &self.rail)
            == (&other.spine, other.radius, &other.anchor, &other.rail)
    }
}

/// Where a pipe anchored on any surface touches it: the anchor at a
/// pcurve's parameter `map[0] + map[1] s` for the spine's `s`.
#[derive(Clone, Debug)]
struct Rail {
    pcurve: Curve2,
    map: [f64; 2],
    /// The pcurve as a curve in the plane `z = 0`, for its derivatives'
    /// enclosures.
    lifted: Curve,
}

impl PartialEq for Rail {
    fn eq(&self, o: &Self) -> bool {
        (&self.pcurve, self.map) == (&o.pcurve, o.map)
    }
}

/// A curve of the parameter plane as a curve in the plane `z = 0` of
/// model space, with the same parameter.
pub(crate) fn lift(c: &Curve2) -> Result<Curve, Error> {
    let p3 = |p: super::Point2| Point3::new(p.x, p.y, 0.0);
    let v3 = |v: super::Vector2| Vector3::new(v.x, v.y, 0.0);
    let frame = |f: &super::Frame2| {
        let ccw = f.x().x * f.y().y - f.x().y * f.y().x > 0.0;
        let z = if ccw { 1.0 } else { -1.0 };
        Frame::new(p3(f.origin()), Vector3::new(0.0, 0.0, z), v3(f.x()))
    };
    Ok(match c {
        Curve2::Line2(l) => Curve::Line(Line::new(p3(l.origin()), v3(l.dir()))?),
        Curve2::Circle2(k) => Curve::Circle(Circle::new(frame(k.frame())?, k.radius())?),
        Curve2::Ellipse2(e) => {
            Curve::Ellipse(Ellipse::new(frame(e.frame())?, e.major(), e.minor())?)
        }
        Curve2::Nurbs2(n) => Curve::Nurbs(std::sync::Arc::new(NurbsCurve::new(
            n.knots().clone(),
            n.control_points().map(p3).collect(),
            Some(n.weights().to_vec()),
        )?)),
    })
}

impl Rail {
    /// The pcurve's point and first two derivatives by the spine's `s`, as
    /// `(u, v)` pairs.
    fn pcurve_at(&self, s: f64) -> [[f64; 2]; 3] {
        let [m0, m1] = self.map;
        let d = self.lifted.jet3(m0 + m1 * s);
        [
            [d[0].x, d[0].y],
            [d[1].x * m1, d[1].y * m1],
            [d[2].x * m1 * m1, d[2].y * m1 * m1],
        ]
    }

    /// The rail point as jets in `s`, from the anchor's jets at the
    /// pcurve's point and the pcurve's derivatives: the chain rule.
    fn compose<T: Num>(s: JVec<T>, q: [[T; 2]; 3]) -> JVec<T> {
        let two = T::of(2.0);
        let [_, [u1, v1], [u2, v2]] = q;
        core::array::from_fn(|k| {
            let j = s[k];
            Jet::of_s(
                j.v,
                j.d[0] * u1 + j.d[1] * v1,
                j.h[0] * u1 * u1
                    + two * j.h[1] * u1 * v1
                    + j.h[2] * v1 * v1
                    + j.d[0] * u2
                    + j.d[1] * v2,
            )
        })
    }

    fn jets_at(&self, anchor: &Surface, s: f64) -> JVec<f64> {
        let q = self.pcurve_at(s);
        let j = Sheet::of(anchor).jets_at(q[0][0], q[0][1]);
        Self::compose(j, q)
    }

    /// Enclosures of the rail's jets over a stretch of spine.
    fn jet_box(&self, anchor: &Surface, [a, b]: [f64; 2]) -> Option<JVec<Interval>> {
        let [m0, m1] = self.map;
        let (ta, tb) = (m0 + m1 * a, m0 + m1 * b);
        let d = self.lifted.jet_box([ta.min(tb), ta.max(tb)]);
        let uv = [[d[0][0].lo(), d[0][0].hi()], [d[0][1].lo(), d[0][1].hi()]];
        let j = Sheet::of(anchor).jet_box(uv)?;
        let k = Interval::point(m1);
        let q = [
            [d[0][0], d[0][1]],
            [d[1][0] * k, d[1][1] * k],
            [d[2][0] * k * k, d[2][1] * k * k],
        ];
        Some(Self::compose(j, q))
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
pub(crate) fn anchor_normal<T: Num>(anchor: &Surface, c: JVec<T>) -> Option<JVec<T>> {
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
            rail: None,
            side: dist.signum(),
            flat,
            bounds,
            frames: Vec::new(),
        };
        let pipe = Self {
            frames: pipe.stretches().iter().map(|_| OnceLock::new()).collect(),
            ..pipe
        };
        pipe.certify()?;
        Ok(pipe)
    }

    /// The pipe of `radius` about `spine`, anchored on any surface: the
    /// ball touches `anchor` at the rail `anchor(pcurve(map[0] + map[1]
    /// s))` for the spine's `s`, and `e₁` points from the spine towards
    /// it. The rail point depends on `s` alone, so the frame is a function
    /// of `s` as on an analytic anchor, and the pipe's bounds and NURBS
    /// form work alike.
    ///
    /// Fails with `NonFinite`, or `Degenerate`: for a spine that is a line
    /// or a circle, a radius below [`crate::tol::LINEAR_RESOLUTION`], a
    /// pcurve the anchor can't be enclosed over, or a pipe that can't be
    /// certified regular over the spine's whole domain, as [`Pipe::new`]
    /// certifies it, the frame's direction towards the rail never along
    /// the spine's tangent.
    pub fn on_rail(
        spine: Curve,
        radius: f64,
        anchor: Surface,
        pcurve: Curve2,
        map: [f64; 2],
    ) -> Result<Self, Error> {
        if !radius.is_finite() || !map.iter().all(|x| x.is_finite()) {
            return Err(Error::NonFinite);
        }
        if radius < crate::tol::LINEAR_RESOLUTION || map[1] == 0.0 {
            return Err(Error::Degenerate);
        }
        let flat = match &spine {
            Curve::Line(_) | Curve::Circle(_) => return Err(Error::Degenerate),
            Curve::Ellipse(e) => Some(e.to_nurbs()?),
            Curve::Nurbs(_) => None,
        };
        let lifted = lift(&pcurve)?;
        let bounds = spine.bounds(spine.domain()).grow(radius * (1.0 + 1e-12));
        let pipe = Self {
            spine,
            radius,
            anchor,
            rail: Some(Rail {
                pcurve,
                map,
                lifted,
            }),
            side: 1.0,
            flat,
            bounds,
            frames: Vec::new(),
        };
        let pipe = Self {
            frames: pipe.stretches().iter().map(|_| OnceLock::new()).collect(),
            ..pipe
        };
        pipe.certify()?;
        Ok(pipe)
    }

    /// The rail a pipe anchored on any surface rolls along: the pcurve on
    /// its anchor and the map from the spine's parameter to the pcurve's.
    pub fn rail(&self) -> Option<(&Curve2, [f64; 2])> {
        self.rail.as_ref().map(|r| (&r.pcurve, r.map))
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
        if self.rail.is_none() {
            let (dist, foot) = anchor_distance(&self.anchor, d[0])?;
            let side = dist * Interval::point(self.side);
            if side.hi() < 0.0 {
                return Some(false);
            }
            if side.lo() <= 0.0 || foot.lo() <= 0.0 {
                return None;
            }
        }
        let (c, cp) = spine_jets(d);
        self.frame(c, cp, self.rail_box(range)?)?;
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

    /// For a pipe anchored on a rail, the direction from the spine to the
    /// rail at `s`, `R - c`, as jets in `s`; `None` for an analytic anchor.
    fn rail_at(&self, s: f64) -> Option<JVec<f64>> {
        let r = self.rail.as_ref()?.jets_at(&self.anchor, s);
        let (c, _) = spine_jets(self.spine.jet3(s).map(Vector3::to_array));
        Some(jet::sub(r, c))
    }

    /// Enclosures of [`Pipe::rail_at`] over a stretch of spine:
    /// `Some(None)` for an analytic anchor, `None` where they can't be had.
    ///
    /// The rail and the spine move together, so enclosing each over the
    /// stretch and subtracting loses the direction between them. The mean
    /// value form about the stretch's middle `m` keeps it: `D(s) ∈ D(m) +
    /// D'(stretch) [-h/2, h/2]` for `D = R - c` and its first derivative,
    /// the second enclosed directly.
    fn rail_box(&self, [a, b]: [f64; 2]) -> Option<Option<JVec<Interval>>> {
        let Some(rail) = &self.rail else {
            return Some(None);
        };
        let m = (a + b) / 2.0;
        let diff = |range: [f64; 2]| -> Option<[IVec3; 3]> {
            let r = rail.jet_box(&self.anchor, range)?;
            let d = self.spine.jet_box(range);
            Some([
                core::array::from_fn(|k| r[k].v - d[0][k]),
                core::array::from_fn(|k| r[k].d[0] - d[1][k]),
                core::array::from_fn(|k| r[k].h[0] - d[2][k]),
            ])
        };
        let (at, over) = (diff([m, m])?, diff([a, b])?);
        let half = Interval::new(a - m, b - m)?;
        Some(Some(core::array::from_fn(|k| {
            Jet::of_s(
                at[0][k] + over[1][k] * half,
                at[1][k] + over[2][k] * half,
                over[2][k],
            )
        })))
    }

    /// The frame `(e₁, e₂)` at a spine point and its derivative, as jets,
    /// with the direction to the rail there ([`Pipe::rail_at`]) for a pipe
    /// anchored on a rail.
    fn frame<T: Num>(
        &self,
        c: JVec<T>,
        cp: JVec<T>,
        rail: Option<JVec<T>>,
    ) -> Option<(JVec<T>, JVec<T>)> {
        let toward = match rail {
            Some(r) => r,
            None => {
                let n = anchor_normal(&self.anchor, c)?;
                jet::times(n, Jet::constant(T::of(-self.side)))
            }
        };
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
    pub(crate) fn jets<T: Num>(
        &self,
        d: [[T; 3]; 4],
        rail: Option<JVec<T>>,
        cos: T,
        sin: T,
    ) -> Option<[JVec<T>; 2]> {
        let (c, cp) = spine_jets(d);
        let (e1, e2) = self.frame(c, cp, rail)?;
        let ct = Jet::of_t(cos, -sin, -cos);
        let st = Jet::of_t(sin, cos, -sin);
        let around = jet::add(jet::times(e1, ct), jet::times(e2, st));
        let p = jet::add(c, around.map(|x| x.scale(self.radius)));
        Some([p, around])
    }

    /// Jets in floating point at `(s, θ)`. A clamped spine's parameter is
    /// taken within its domain if the frame fails outside it.
    pub(crate) fn jets_at(&self, s: f64, t: f64) -> [JVec<f64>; 2] {
        let (sin, cos) = math::sin_cos(t);
        let at = |s: f64| {
            let d = self.spine.jet3(s).map(Vector3::to_array);
            self.jets(d, self.rail_at(s), cos, sin)
        };
        at(s)
            .or_else(|| {
                let [d0, d1] = self.spine.domain();
                at(s.clamp(d0, d1))
            })
            .expect("a certified pipe has a frame on its spine")
    }

    pub(crate) fn point(&self, s: f64, t: f64) -> Point3 {
        let [p, _] = self.jets_at(s, t);
        Point3::new(p[0].v, p[1].v, p[2].v)
    }

    pub(crate) fn eval(&self, s: f64, t: f64) -> SurfaceEval {
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

    pub(crate) fn normal(&self, s: f64, t: f64) -> Option<UnitVector3> {
        let [_, n] = self.jets_at(s, t);
        Vector3::new(n[0].v, n[1].v, n[2].v).normalize()
    }

    pub(crate) fn domain(&self) -> [[f64; 2]; 2] {
        [self.spine.domain(), [-PI, PI]]
    }

    pub(crate) fn periods(&self) -> (Option<f64>, Option<f64>) {
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
    pub(crate) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
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

    /// The pipe reflected by `m`: its spine reflected with its parameter
    /// kept, about its anchor reflected. The anchor's foot is reflected
    /// with it, so `e₁` is too, and `e₂ = e₁ × t` turns the other way:
    /// `θ` becomes `-θ`. The anchor's natural normal is mirrored with it,
    /// so the spine stays on the same side.
    pub(super) fn mirrored(&self, m: &super::Mirror) -> Result<Self, Error> {
        let spine = self.spine.mirrored(m)?;
        let (anchor, flip) = self.anchor.mirrored(m)?;
        // The rail's pcurve maps as the anchor's parameters do.
        let rail = match &self.rail {
            None => None,
            Some(r) => {
                let pcurve = r.pcurve.mirrored(flip);
                Some(Rail {
                    lifted: lift(&pcurve)?,
                    pcurve,
                    map: r.map,
                })
            }
        };
        Ok(Self {
            bounds: spine
                .bounds(spine.domain())
                .grow(self.radius * (1.0 + 1e-12)),
            flat: match &spine {
                Curve::Ellipse(e) => Some(e.to_nurbs()?),
                _ => None,
            },
            spine,
            radius: self.radius,
            anchor,
            rail,
            side: self.side,
            frames: self.frames.iter().map(|_| OnceLock::new()).collect(),
        })
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
            rail: self.rail.clone(),
            side: self.side,
            frames: self.frames.iter().map(|_| OnceLock::new()).collect(),
        }
    }

    /// The pipe about the same spine with its radius grown by `d`: the
    /// offset by `d` along the natural normal, with the same parameters.
    pub(super) fn offset(&self, d: f64) -> Result<Self, Error> {
        match &self.rail {
            None => Self::new(self.spine.clone(), self.radius + d, self.anchor.clone()),
            Some(r) => Self::on_rail(
                self.spine.clone(),
                self.radius + d,
                self.anchor.clone(),
                r.pcurve.clone(),
                r.map,
            ),
        }
    }

    /// Enclosures of the point and natural normal jets over a parameter
    /// box: the spine's derivatives over `s` and `cos θ`, `sin θ` over
    /// `θ`. `None` where the frame can't be certified over the box.
    pub(crate) fn jet_box(&self, [rs, rt]: [[f64; 2]; 2]) -> Option<[JVec<Interval>; 2]> {
        let d = self.spine.jet_box(rs);
        let (cos, sin) = trig_box(rt);
        self.jets(d, self.rail_box(rs)?, cos, sin)
    }

    /// Bounds over a parameter box, from the frame's enclosures over the
    /// stretches of spine it covers ([`Frames`]): with `n = cos θ e₁ + sin
    /// θ e₂`, `Pss = c'' + r nss`, `Psθ = r nsθ` and `|Pθθ| = r`; the unit
    /// normal `n` turns by `|ns|` along `s` and `1` along `θ`. A clamped
    /// spine's range is cut to its domain first.
    pub(super) fn derivative_bound(&self, [rs, rt]: [[f64; 2]; 2]) -> SurfaceBound {
        let (cos, sin) = trig_box(rt);
        let r = Interval::point(self.radius);
        let mut out = SurfaceBound {
            duu: 0.0,
            duv: 0.0,
            dvv: self.radius * (1.0 + 1e-12),
            turn_u: 0.0,
            turn_v: 1.0 + 1e-12,
        };
        let covered = self.frames_over(rs, &mut |f: &FrameBox| {
            let ns = turn(f.e1d, f.e2d, cos, sin);
            let nst = turn(f.e2d, f.e1d.map(|x| -x), cos, sin);
            let nss = turn(f.e1dd, f.e2dd, cos, sin);
            out.duu = out
                .duu
                .max(length(core::array::from_fn(|k| f.c2[k] + r * nss[k])));
            out.duv = out.duv.max(length(nst.map(|x| r * x)));
            out.turn_u = out.turn_u.max(length(ns));
        });
        if covered { out } else { INFINITE_BOUND }
    }

    /// Bounds on `|P_s|` and `|P_θ|` over a parameter box (`r` along
    /// `θ`), from the frame's enclosures as [`Pipe::derivative_bound`]
    /// takes them.
    pub(crate) fn first_bound(&self, [rs, rt]: [[f64; 2]; 2]) -> (f64, f64) {
        let (cos, sin) = trig_box(rt);
        let r = Interval::point(self.radius);
        let mut su = 0.0f64;
        let covered = self.frames_over(rs, &mut |f: &FrameBox| {
            let ns = turn(f.e1d, f.e2d, cos, sin);
            su = su.max(length(core::array::from_fn(|k| f.c1[k] + r * ns[k])));
        });
        if covered {
            (su, self.radius * (1.0 + 1e-12))
        } else {
            (f64::INFINITY, f64::INFINITY)
        }
    }

    /// A NURBS within `tolerance` (at least
    /// [`crate::tol::LINEAR_RESOLUTION`]) of the pipe over `[s0, s1]` of
    /// its spine, all the way round, and the certified bound on how far it
    /// strays: the parameters are the pipe's along `s` and
    /// [`Reparam::Angle`] round it. A pipe has no exact rational form
    /// ([`Surface::to_nurbs`] is `Degenerate`); this is for export.
    ///
    /// Each section is a circle, exactly the rational quadratic turn of
    /// eight control points `Qₖ(s) = c(s) + r (aₖ e₁(s) + bₖ e₂(s))` with
    /// fixed weights, so the surface is `Σ Rₖ(θ) Qₖ(s)` with a rational
    /// basis that sums to one. Along `s` each `Qₖ` is a cubic Hermite
    /// spline through its values and derivatives at the ends of spans
    /// (the spine's stretches, halved), so the surface strays by at most
    /// the worst `|Q̃ₖ - Qₖ|`. That vanishes at both ends of a span, so it
    /// is at most `h² / 8` times a bound on `Q̃ₖ'' - Qₖ''` over the span:
    /// `Q̃ₖ''` is linear in `s`, and `Qₖ''` is enclosed from the spine's
    /// and the frame's second derivatives over the span. A span is halved
    /// until that is within the tolerance (or its frame can be enclosed),
    /// up to 20 times; past that the
    /// call fails with `Degenerate`, as it does for a range outside a
    /// clamped spine's domain or longer than a closed one's period.
    pub fn to_nurbs_within(
        &self,
        [s0, s1]: [f64; 2],
        tolerance: f64,
    ) -> Result<(NurbsSurface, [Reparam; 2], f64), Error> {
        let tolerance = tolerance.max(crate::tol::LINEAR_RESOLUTION);
        let [d0, d1] = self.spine.domain();
        let ok = match self.spine.period() {
            Some(p) => s1 - s0 <= p,
            None => d0 <= s0 && s1 <= d1,
        };
        if !(s0 < s1 && ok) {
            return Err(Error::Degenerate);
        }
        // The spine's stretches over the range, halved until every span's
        // bound is within the tolerance.
        let mut cuts: Vec<f64> = vec![s0];
        for [a, b] in self.stretches() {
            for k in -1i32..=1 {
                let shift = self.spine.period().map_or(0.0, |p| p * f64::from(k));
                for x in [a + shift, b + shift] {
                    if x > s0 && x < s1 {
                        cuts.push(x);
                    }
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
            match self.hermite_bound(span) {
                Some(bound) if bound <= tolerance => {
                    worst = worst.max(bound);
                    done.push(span);
                    continue;
                }
                _ => {}
            }
            if depth < 20 {
                let m = (span[0] + span[1]) / 2.0;
                spans.push(([span[0], m], depth + 1));
                spans.push(([m, span[1]], depth + 1));
            } else {
                return Err(Error::Degenerate);
            }
        }
        done.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let mut knots = vec![done[0][0]];
        knots.extend(done.iter().map(|s| s[1]));
        let mut mults = vec![3; knots.len()];
        mults[0] = 4;
        *mults.last_mut().expect("two knots") = 4;
        let along = KnotVector::clamped(3, &knots, &mults)?;
        // Rows along s, each the eight points of a section's turn.
        let mut rows: Vec<[Point3; 8]> = vec![self.section(knots[0]).0];
        for span in &done {
            let h = span[1] - span[0];
            let (a, da) = self.section(span[0]);
            let (b, db) = self.section(span[1]);
            rows.push(core::array::from_fn(|k| a[k] + da[k] * (h / 3.0)));
            rows.push(core::array::from_fn(|k| b[k] - db[k] * (h / 3.0)));
            rows.push(b);
        }
        let points = rows.iter().flat_map(|r| r.iter().copied()).collect();
        let weights = rows
            .iter()
            .flat_map(|_| TURN.iter().map(|&(.., w)| w))
            .collect();
        let surface = NurbsSurface::new([along, turn_knots()?], points, Some(weights))?;
        Ok((surface, [Reparam::Identity, Reparam::Angle], worst))
    }

    /// The eight control points of the section at `s` and their
    /// derivatives by `s`.
    fn section(&self, s: f64) -> ([Point3; 8], [Vector3; 8]) {
        let (c, cp) = spine_jets(self.spine.jet3(s).map(Vector3::to_array));
        let (e1, e2) = self
            .frame(c, cp, self.rail_at(s))
            .expect("a certified pipe has a frame on its spine");
        let v = |j: &JVec<f64>, f: &dyn Fn(&Jet<f64>) -> f64| {
            Vector3::new(f(&j[0]), f(&j[1]), f(&j[2]))
        };
        let (o, o1) = (v(&c, &|j| j.v), v(&c, &|j| j.d[0]));
        let (x, x1) = (v(&e1, &|j| j.v), v(&e1, &|j| j.d[0]));
        let (y, y1) = (v(&e2, &|j| j.v), v(&e2, &|j| j.d[0]));
        let r = self.radius;
        (
            TURN.map(|(a, b, _)| Point3::ORIGIN + o + (x * a + y * b) * r),
            TURN.map(|(a, b, _)| o1 + (x1 * a + y1 * b) * r),
        )
    }

    /// The bound on how far the Hermite spans of the sections' control
    /// points stray from them over `span`: `h² / 8` times the worst
    /// `|Q̃ₖ'' - Qₖ''|` over it. `None` where the frame can't be enclosed.
    fn hermite_bound(&self, span: [f64; 2]) -> Option<f64> {
        let h = span[1] - span[0];
        let f = self.frame_over(span)?;
        let (a, da) = self.section(span[0]);
        let (b, db) = self.section(span[1]);
        let r = Interval::point(self.radius);
        let mut worst = 0.0f64;
        for k in 0..8 {
            let (ck, sk, _) = TURN[k];
            // The Hermite cubic's second derivative at its ends: 6 (P₀ -
            // 2 P₁ + P₂) / h² and 6 (P₁ - 2 P₂ + P₃) / h².
            let p = [
                a[k].to_vector(),
                a[k].to_vector() + da[k] * (h / 3.0),
                b[k].to_vector() - db[k] * (h / 3.0),
                b[k].to_vector(),
            ];
            let s0 = (p[0] - p[1] * 2.0 + p[2]) * (6.0 / (h * h));
            let s1 = (p[1] - p[2] * 2.0 + p[3]) * (6.0 / (h * h));
            let (s0, s1) = (s0.to_array(), s1.to_array());
            let e: IVec3 = core::array::from_fn(|i| {
                let fitted = Interval::new(s0[i].min(s1[i]), s0[i].max(s1[i])).expect("ordered");
                let true_ = f.c2[i]
                    + r * (Interval::point(ck) * f.e1dd[i] + Interval::point(sk) * f.e2dd[i]);
                fitted - true_
            });
            worst = worst.max(length(e));
        }
        Some(h * h / 8.0 * worst * (1.0 + 1e-9) + crate::intersect::SLACK)
    }

    /// Calls `each` with every stretch of [`Frames`] that `[a, b]` meets
    /// (cut to the domain of a clamped spine, wrapped round a periodic
    /// one); `false` where some stretch has no certified frame.
    fn frames_over(&self, [a, b]: [f64; 2], each: &mut dyn FnMut(&FrameBox)) -> bool {
        let [d0, d1] = self.spine.domain();
        let ranges: Vec<[f64; 2]> = match self.spine.period() {
            None => vec![[a.clamp(d0, d1), b.clamp(d0, d1)]],
            Some(p) if b - a >= p => vec![[d0, d1]],
            Some(p) => {
                let a0 = d0 + (a - d0).rem_euclid(p);
                let b0 = a0 + (b - a);
                if b0 <= d1 {
                    vec![[a0, b0]]
                } else {
                    vec![[a0, d1], [d0, b0 - p]]
                }
            }
        };
        let stretches = self.stretches();
        let mut ok = true;
        for [lo, hi] in ranges {
            for (i, &[s0, s1]) in stretches.iter().enumerate() {
                if s1 < lo || s0 > hi {
                    continue;
                }
                match self.frames[i].get_or_init(|| self.tabulate([s0, s1])) {
                    Some(frames) => {
                        for f in frames {
                            if f.range[1] >= lo && f.range[0] <= hi {
                                each(f);
                            }
                        }
                    }
                    None => ok = false,
                }
            }
        }
        ok
    }

    /// The frame's enclosures over a stretch of spine, halved where they
    /// are loose; `None` where the frame can't be certified.
    fn tabulate(&self, range: [f64; 2]) -> Option<Vec<FrameBox>> {
        let mut out = Vec::new();
        let mut stack = vec![(range, 0u32)];
        while let Some((range, depth)) = stack.pop() {
            let f = self.frame_over(range);
            let tight = f.as_ref().is_some_and(|f| f.tight(self.radius));
            if tight || depth == FRAME_DEPTH {
                out.push(f?);
                continue;
            }
            let m = (range[0] + range[1]) / 2.0;
            // Pushed in reverse, so the stretches come out in order.
            stack.push(([m, range[1]], depth + 1));
            stack.push(([range[0], m], depth + 1));
        }
        Some(out)
    }

    /// The frame's enclosures over one stretch of spine.
    fn frame_over(&self, range: [f64; 2]) -> Option<FrameBox> {
        let d = self.spine.jet_box(range);
        let (c, cp) = spine_jets(d);
        let (e1, e2) = self.frame(c, cp, self.rail_box(range)?)?;
        Some(FrameBox {
            range,
            c1: d[1],
            c2: d[2],
            e1d: e1.map(|j| j.d[0]),
            e2d: e2.map(|j| j.d[0]),
            e1dd: e1.map(|j| j.h[0]),
            e2dd: e2.map(|j| j.h[0]),
        })
    }
}

/// How many times a stretch of spine is halved, at most, while
/// tabulating its frame.
const FRAME_DEPTH: u32 = 10;

/// Enclosures over a stretch of spine of what a pipe's derivatives are
/// made of: the spine's first and second derivatives and the frame's
/// first and second derivatives by `s`. The frame depends on `s` alone,
/// so these bound the pipe's derivatives over any range of `θ`.
#[derive(Clone, Debug)]
struct FrameBox {
    range: [f64; 2],
    c1: IVec3,
    c2: IVec3,
    e1d: IVec3,
    e2d: IVec3,
    e1dd: IVec3,
    e2dd: IVec3,
}

impl FrameBox {
    /// Whether every enclosure is narrow next to its own size, or to the
    /// scale of its kind (the spine's speed over the radius for the
    /// frame), so that halving would gain little.
    fn tight(&self, radius: f64) -> bool {
        let speed = length(self.c1);
        let narrow = |v: IVec3, scale: f64| {
            let width = length(v.map(|x| Interval::point(x.hi() - x.lo())));
            width <= 0.125 * (length(v) + scale)
        };
        let k = speed / radius;
        narrow(self.c1, speed)
            && narrow(self.c2, speed * k)
            && narrow(self.e1d, k)
            && narrow(self.e2d, k)
            && narrow(self.e1dd, k * k)
            && narrow(self.e2dd, k * k)
    }
}

/// `cos θ a + sin θ b` over enclosures of `cos θ` and `sin θ`.
fn turn(a: IVec3, b: IVec3, cos: Interval, sin: Interval) -> IVec3 {
    core::array::from_fn(|k| cos * a[k] + sin * b[k])
}

/// An upper bound on the length of any vector in a box.
fn length(v: IVec3) -> f64 {
    let sum = v[0].square() + v[1].square() + v[2].square();
    sum.sqrt().map_or(f64::INFINITY, Interval::hi) * (1.0 + 1e-12)
}

const INFINITE_BOUND: SurfaceBound = SurfaceBound {
    duu: f64::INFINITY,
    duv: f64::INFINITY,
    dvv: f64::INFINITY,
    turn_u: f64::INFINITY,
    turn_v: f64::INFINITY,
};

#[cfg(test)]
pub(crate) mod tests;
