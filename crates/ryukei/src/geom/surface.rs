use super::curve::{check_length, ring_box};
use super::{
    Box3, Frame, Mirror, Point2, Point3, PrincipalCurvatures, Projection, Rigid, SurfaceEval,
    UnitVector3, Vector2, Vector3, polar_angle,
};
use super::{Curve2, Extrusion, NurbsSurface, Offset, Pipe, Revolution, Sweep};
use crate::Error;
use crate::math;
use crate::tol::ANGULAR_RESOLUTION;
use core::f64::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

/// A surface in model space, evaluated by parameters `(u, v)`.
///
/// The analytic surfaces are fast paths with closed-form distances; a
/// NURBS surface, and a swept surface that keeps its generating curve, is
/// shared behind an `Arc`, so cloning one is cheap. Each analytic natural
/// normal points away from the axis (or along `z` for a plane), a NURBS's
/// is along `du × dv`, a swept surface's is described with its type, and
/// a pipe's points away from its spine; a face says whether it uses that
/// normal or its reverse.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Surface {
    Plane(Plane),
    Cylinder(Cylinder),
    Cone(Cone),
    Sphere(Sphere),
    Torus(Torus),
    Nurbs(Arc<NurbsSurface>),
    Extrusion(Arc<Extrusion>),
    Revolution(Arc<Revolution>),
    Pipe(Arc<Pipe>),
    Sweep(Arc<Sweep>),
    Offset(Arc<Offset>),
}

/// How a mirrored surface ([`Surface::mirrored`]) maps its parameter
/// plane: the mirrored point of `(u, v)` is the new surface's point at
/// `apply((u, v))`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamFlip {
    /// `(u, v)` stays `(u, v)`. The natural normal then turns against the
    /// mirrored geometry, so a face on the surface flips its sense.
    Kept,
    /// `u` becomes `at - u`; the natural normal is mirrored with the
    /// geometry.
    U(f64),
    /// `v` becomes `at - v`; likewise.
    V(f64),
}

impl ParamFlip {
    pub fn apply(self, p: Point2) -> Point2 {
        match self {
            Self::Kept => p,
            Self::U(at) => Point2::new(at - p.x, p.y),
            Self::V(at) => Point2::new(p.x, at - p.y),
        }
    }

    /// A direction in the parameter plane, mapped.
    pub fn apply_vector(self, d: Vector2) -> Vector2 {
        match self {
            Self::Kept => d,
            Self::U(_) => Vector2::new(-d.x, d.y),
            Self::V(_) => Vector2::new(d.x, -d.y),
        }
    }

    /// Whether the natural normal turns against the mirrored geometry's.
    pub fn flips_normal(self) -> bool {
        self == Self::Kept
    }
}

/// The frame's xy plane: `origin + u x + v y`. The normal is `z`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    frame: Frame,
}

/// `origin + r (cos u x + sin u y) + v z`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cylinder {
    frame: Frame,
    radius: f64,
}

/// A single cone nappe, widening towards `+z`:
/// `origin + (r + v tan α)(cos u x + sin u y) + v z`.
///
/// `r` is the radius in the frame's xy plane and `α` the half-angle. The
/// apex is at `v = -r / tan α`, and `v` never goes below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    frame: Frame,
    radius: f64,
    half_angle: f64,
    sin: f64,
    cos: f64,
}

/// `origin + r (cos v (cos u x + sin u y) + sin v z)`, with `v` the
/// latitude in `[-π/2, π/2]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    frame: Frame,
    radius: f64,
}

/// A ring torus about `z`:
/// `origin + (R + r cos v)(cos u x + sin u y) + r sin v z`, with `R > r`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Torus {
    frame: Frame,
    major: f64,
    minor: f64,
}

impl Plane {
    pub fn new(frame: Frame) -> Self {
        Self { frame }
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }
}

impl Cylinder {
    pub fn new(frame: Frame, radius: f64) -> Result<Self, Error> {
        Ok(Self {
            frame,
            radius: check_length(radius)?,
        })
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }
}

impl Cone {
    /// Fails with `Degenerate` unless the half-angle is strictly between 0
    /// and π/2 by more than [`ANGULAR_RESOLUTION`].
    pub fn new(frame: Frame, radius: f64, half_angle: f64) -> Result<Self, Error> {
        let radius = check_length(radius)?;
        if !half_angle.is_finite() {
            return Err(Error::NonFinite);
        }
        if half_angle <= ANGULAR_RESOLUTION || half_angle >= FRAC_PI_2 - ANGULAR_RESOLUTION {
            return Err(Error::Degenerate);
        }
        let (sin, cos) = math::sin_cos(half_angle);
        Ok(Self {
            frame,
            radius,
            half_angle,
            sin,
            cos,
        })
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }

    pub fn half_angle(&self) -> f64 {
        self.half_angle
    }

    /// The `v` of the apex.
    pub fn apex_v(&self) -> f64 {
        -self.radius * self.cos / self.sin
    }
}

impl Sphere {
    pub fn new(frame: Frame, radius: f64) -> Result<Self, Error> {
        Ok(Self {
            frame,
            radius: check_length(radius)?,
        })
    }

    pub fn radius(&self) -> f64 {
        self.radius
    }
}

impl Torus {
    /// Fails with `Degenerate` unless `major` exceeds `minor` by at least
    /// [`crate::tol::LINEAR_RESOLUTION`]: horn and spindle tori are not
    /// supported.
    pub fn new(frame: Frame, major: f64, minor: f64) -> Result<Self, Error> {
        let major = check_length(major)?;
        let minor = check_length(minor)?;
        check_length(major - minor)?;
        Ok(Self {
            frame,
            major,
            minor,
        })
    }

    pub fn major(&self) -> f64 {
        self.major
    }

    pub fn minor(&self) -> f64 {
        self.minor
    }
}

impl Surface {
    /// The frame an analytic surface sits in; a NURBS or a swept surface
    /// has none.
    pub fn frame(&self) -> Option<&Frame> {
        match self {
            Self::Plane(s) => Some(&s.frame),
            Self::Cylinder(s) => Some(&s.frame),
            Self::Cone(s) => Some(&s.frame),
            Self::Sphere(s) => Some(&s.frame),
            Self::Torus(s) => Some(&s.frame),
            Self::Nurbs(_)
            | Self::Extrusion(_)
            | Self::Revolution(_)
            | Self::Pipe(_)
            | Self::Sweep(_)
            | Self::Offset(_) => None,
        }
    }

    pub fn point(&self, u: f64, v: f64) -> Point3 {
        match self {
            Self::Plane(s) => s.frame.to_world(u, v, 0.0),
            Self::Cylinder(s) => {
                s.frame.origin() + s.frame.radial(u) * s.radius + s.frame.z().get() * v
            }
            Self::Cone(s) => {
                let r = s.radius + v * s.sin / s.cos;
                s.frame.origin() + s.frame.radial(u) * r + s.frame.z().get() * v
            }
            Self::Sphere(s) => {
                let (sv, cv) = math::sin_cos(v);
                s.frame.origin() + (s.frame.radial(u) * cv + s.frame.z().get() * sv) * s.radius
            }
            Self::Torus(s) => {
                let (sv, cv) = math::sin_cos(v);
                s.frame.origin()
                    + s.frame.radial(u) * (s.major + s.minor * cv)
                    + s.frame.z().get() * (s.minor * sv)
            }
            Self::Nurbs(n) => n.point(u, v),
            Self::Extrusion(e) => e.point(u, v),
            Self::Revolution(r) => r.point(u, v),
            Self::Pipe(p) => p.point(u, v),
            Self::Sweep(s) => s.point(u, v),
            Self::Offset(o) => o.point(u, v),
        }
    }

    /// The point and its first and second partial derivatives.
    pub fn eval(&self, u: f64, v: f64) -> SurfaceEval {
        let zero = Vector3::ZERO;
        match self {
            Self::Plane(s) => SurfaceEval {
                point: s.frame.to_world(u, v, 0.0),
                du: s.frame.x().get(),
                dv: s.frame.y().get(),
                duu: zero,
                duv: zero,
                dvv: zero,
            },
            Self::Cylinder(s) => {
                let (radial, along) = s.frame.radial_and_turn(u);
                SurfaceEval {
                    point: s.frame.origin() + radial * s.radius + s.frame.z().get() * v,
                    du: along * s.radius,
                    dv: s.frame.z().get(),
                    duu: radial * -s.radius,
                    duv: zero,
                    dvv: zero,
                }
            }
            Self::Cone(s) => {
                let (radial, along) = s.frame.radial_and_turn(u);
                let slope = s.sin / s.cos;
                let r = s.radius + v * s.sin / s.cos;
                SurfaceEval {
                    point: s.frame.origin() + radial * r + s.frame.z().get() * v,
                    du: along * r,
                    dv: radial * slope + s.frame.z().get(),
                    duu: radial * -r,
                    duv: along * slope,
                    dvv: zero,
                }
            }
            Self::Sphere(s) => {
                let (radial, along) = s.frame.radial_and_turn(u);
                let z = s.frame.z().get();
                let (sv, cv) = math::sin_cos(v);
                let out = radial * cv + z * sv;
                SurfaceEval {
                    point: s.frame.origin() + out * s.radius,
                    du: along * (s.radius * cv),
                    dv: (z * cv - radial * sv) * s.radius,
                    duu: radial * -(s.radius * cv),
                    duv: along * -(s.radius * sv),
                    dvv: out * -s.radius,
                }
            }
            Self::Torus(s) => {
                let (radial, along) = s.frame.radial_and_turn(u);
                let z = s.frame.z().get();
                let (sv, cv) = math::sin_cos(v);
                let ring = s.major + s.minor * cv;
                SurfaceEval {
                    point: s.frame.origin() + radial * ring + z * (s.minor * sv),
                    du: along * ring,
                    dv: (z * cv - radial * sv) * s.minor,
                    duu: radial * -ring,
                    duv: along * -(s.minor * sv),
                    dvv: (radial * cv + z * sv) * -s.minor,
                }
            }
            Self::Nurbs(n) => n.eval(u, v),
            Self::Extrusion(e) => e.eval(u, v),
            Self::Revolution(r) => r.eval(u, v),
            Self::Pipe(p) => p.eval(u, v),
            Self::Sweep(s) => s.eval(u, v),
            Self::Offset(o) => o.eval(u, v),
        }
    }

    /// The point of the surface at a pcurve's parameter `t`.
    pub fn point_at(&self, c: &Curve2, t: f64) -> Point3 {
        let q = c.point(t);
        self.point(q.x, q.y)
    }

    /// The natural unit normal at `(u, v)`. Always `Some` for the analytic
    /// surfaces: at a cone's apex it is the limit along the generator at
    /// angle `u`, and at a sphere's poles it is the axis. For a NURBS see
    /// [`NurbsSurface::normal`]. An extrusion's is the direction of
    /// `T × d`, `T` the curve's unit tangent: `None` only where the curve
    /// has no tangent. A revolution's is the direction of
    /// `h' radial(u) - ρ' z` (see [`Revolution`]), its limit at a pole;
    /// `None` likewise.
    pub fn normal(&self, u: f64, v: f64) -> Option<UnitVector3> {
        Some(match self {
            Self::Nurbs(n) => return n.normal(u, v),
            Self::Extrusion(e) => return e.surface_normal(u),
            Self::Revolution(r) => return r.surface_normal(u, v),
            Self::Pipe(p) => return p.normal(u, v),
            Self::Sweep(s) => return s.normal(u, v),
            Self::Offset(o) => return o.normal(u, v),
            Self::Plane(s) => s.frame.z(),
            Self::Cylinder(s) => UnitVector3::new_unchecked(s.frame.radial(u)),
            Self::Cone(s) => {
                UnitVector3::new_unchecked(s.frame.radial(u) * s.cos - s.frame.z().get() * s.sin)
            }
            Self::Sphere(Sphere { frame, .. }) | Self::Torus(Torus { frame, .. }) => {
                let (sv, cv) = math::sin_cos(v);
                UnitVector3::new_unchecked(frame.radial(u) * cv + frame.z().get() * sv)
            }
        })
    }

    /// The principal curvatures against the natural normal (see
    /// [`SurfaceEval::principal_curvatures`]). `None` where `du × dv`
    /// vanishes: a sphere's poles, a cone's apex.
    pub fn principal_curvatures(&self, u: f64, v: f64) -> Option<PrincipalCurvatures> {
        match self {
            Self::Nurbs(n) => n.principal_curvatures(u, v),
            _ => self.eval(u, v).principal_curvatures(self.normal(u, v)?),
        }
    }

    /// The parameter ranges `[[u0, u1], [v0, v1]]`. Open directions are
    /// unbounded; a cone's `v` starts at its apex; a NURBS has its knot
    /// domains; an extrusion has its curve's domain in `u`, a revolution
    /// in `v`.
    pub fn domain(&self) -> [[f64; 2]; 2] {
        let all = [f64::NEG_INFINITY, f64::INFINITY];
        let turn = [0.0, TAU];
        match self {
            Self::Plane(_) => [all, all],
            Self::Cylinder(_) => [turn, all],
            Self::Cone(s) => [turn, [s.apex_v(), f64::INFINITY]],
            Self::Sphere(_) => [turn, [-FRAC_PI_2, FRAC_PI_2]],
            Self::Torus(_) => [turn, turn],
            Self::Nurbs(n) => n.domain(),
            Self::Extrusion(e) => e.domain(),
            Self::Revolution(r) => r.domain(),
            Self::Pipe(p) => p.domain(),
            Self::Sweep(s) => s.domain(),
            Self::Offset(o) => o.domain(),
        }
    }

    /// The parameter periods in `u` and `v`.
    pub fn periods(&self) -> (Option<f64>, Option<f64>) {
        match self {
            Self::Plane(_) => (None, None),
            Self::Cylinder(_) | Self::Cone(_) | Self::Sphere(_) => (Some(TAU), None),
            Self::Torus(_) => (Some(TAU), Some(TAU)),
            Self::Nurbs(n) => n.periods(),
            Self::Extrusion(e) => (e.curve().period(), None),
            Self::Revolution(r) => (Some(TAU), r.curve().period()),
            Self::Pipe(p) => p.periods(),
            Self::Sweep(s) => s.periods(),
            Self::Offset(o) => o.periods(),
        }
    }

    /// For a surface that closes up on itself, the genus of that closed
    /// surface (sphere 0, torus 1). `None` for open surfaces (an extrusion
    /// is one), and for every NURBS: its closed genus isn't worked out. A
    /// revolution of a periodic curve has genus 1, and of a clamped one
    /// with both ends on the axis genus 0; other revolutions are open.
    /// Only closed surfaces may carry a face with no loops.
    pub fn closed_genus(&self) -> Option<u32> {
        match self {
            Self::Sphere(_) => Some(0),
            Self::Torus(_) => Some(1),
            Self::Revolution(r) => r.closed_genus(),
            Self::Pipe(p) => p.closed_genus(),
            Self::Sweep(s) => s.closed_genus(),
            Self::Nurbs(n) => n.closed_genus(),
            Self::Offset(o) => o.closed_genus(),
            Self::Plane(_) | Self::Cylinder(_) | Self::Cone(_) | Self::Extrusion(_) => None,
        }
    }

    /// The distance from `p` to the nearest point of the surface.
    pub fn distance(&self, p: Point3) -> f64 {
        // `p` in an analytic surface's frame, and its distance from the
        // frame's axis.
        let local = |f: &Frame| {
            let q = f.to_local(p);
            (q, math::sqrt(q.x * q.x + q.y * q.y))
        };
        match self {
            Self::Plane(s) => s.frame.to_local(p).z.abs(),
            Self::Cylinder(s) => (local(&s.frame).1 - s.radius).abs(),
            Self::Cone(s) => {
                let (q, rho) = local(&s.frame);
                // In the half-plane through the axis and p: the generator
                // runs from (r, 0) in direction (sin α, cos α).
                let (w, h) = (rho - s.radius, q.z);
                if w * s.sin + h * s.cos >= -s.radius / s.sin {
                    (w * s.cos - h * s.sin).abs()
                } else {
                    let dh = h - s.apex_v();
                    math::sqrt(rho * rho + dh * dh)
                }
            }
            Self::Sphere(s) => {
                let (q, rho) = local(&s.frame);
                (math::sqrt(rho * rho + q.z * q.z) - s.radius).abs()
            }
            Self::Torus(s) => {
                let (q, rho) = local(&s.frame);
                let w = rho - s.major;
                (math::sqrt(w * w + q.z * q.z) - s.minor).abs()
            }
            Self::Nurbs(n) => n.distance(p),
            Self::Extrusion(e) => e.project(p).distance,
            Self::Revolution(r) => r.project(p).distance,
            Self::Pipe(s) => s.project(p).distance,
            Self::Sweep(s) => s.project(p).distance,
            Self::Offset(o) => o.project(p).distance,
        }
    }

    /// The nearest point of the surface to `p`, in closed form. Angles are
    /// in `[0, 2π)`; where the nearest point is not unique (on an axis, at a
    /// centre), angle 0 is chosen. For a NURBS see [`NurbsSurface::project`].
    /// An extrusion projects onto its curve as seen along its direction,
    /// and a revolution onto its curve after turning the point into the
    /// curve's half-plane, with the guarantee of [`NurbsSurface::project`].
    pub fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        let frame = match self {
            Self::Nurbs(n) => return n.project(p),
            Self::Extrusion(e) => return e.project(p),
            Self::Revolution(r) => return r.project(p),
            Self::Pipe(s) => return s.project(p),
            Self::Sweep(s) => return s.project(p),
            Self::Offset(o) => return o.project(p),
            Self::Plane(Plane { frame })
            | Self::Cylinder(Cylinder { frame, .. })
            | Self::Cone(Cone { frame, .. })
            | Self::Sphere(Sphere { frame, .. })
            | Self::Torus(Torus { frame, .. }) => frame,
        };
        let (u, v) = self.params_of(frame, p);
        Projection {
            param: (u, v),
            point: self.point(u, v),
            distance: self.distance(p),
        }
    }

    /// A box holding the surface. The open surfaces (plane, cylinder,
    /// cone, extrusion) are unbounded and give the size box; a NURBS gives
    /// its hull bounds, and a revolution the box of a cylinder about its
    /// axis holding its curve's bounds.
    pub fn bounds(&self) -> Box3 {
        match self {
            Self::Plane(_) | Self::Cylinder(_) | Self::Cone(_) | Self::Extrusion(_) => {
                crate::tol::size_box()
            }
            Self::Sphere(s) => {
                let r = Vector3::new(s.radius, s.radius, s.radius);
                Box3::new(s.frame.origin() - r, s.frame.origin() + r)
            }
            Self::Torus(s) => ring_box(&s.frame, s.major, s.minor),
            Self::Nurbs(n) => n.bounds(),
            Self::Revolution(r) => r.bounds(),
            Self::Pipe(p) => p.bounds(),
            Self::Sweep(s) => s.bounds(),
            Self::Offset(o) => o.bounds(),
        }
    }

    /// The parameters of the nearest point of an analytic surface sitting
    /// in `frame`.
    fn params_of(&self, frame: &Frame, p: Point3) -> (f64, f64) {
        let q = frame.to_local(p);
        let rho = math::sqrt(q.x * q.x + q.y * q.y);
        let u = polar_angle(q.y, q.x);
        match self {
            Self::Plane(_) => (q.x, q.y),
            Self::Cylinder(_) => (u, q.z),
            Self::Cone(s) => {
                let slant = (rho - s.radius) * s.sin + q.z * s.cos;
                (u, (slant * s.cos).max(s.apex_v()))
            }
            Self::Sphere(_) => (u, math::atan2(q.z, rho)),
            Self::Torus(s) => (u, polar_angle(q.z, rho - s.major)),
            Self::Nurbs(_)
            | Self::Extrusion(_)
            | Self::Revolution(_)
            | Self::Pipe(_)
            | Self::Sweep(_)
            | Self::Offset(_) => self.project(p).param,
        }
    }

    /// The surface moved by `d` along its natural normal, with the same
    /// parameters: the offset's point at `(u, v)` is `S(u, v) + d N(u, v)`.
    /// A plane shifts, a cylinder's or sphere's radius and a torus's minor
    /// radius grow by `d`, and a cone keeps its axis and half-angle with
    /// its frame moved by `-d sin α` along the axis and its radius there
    /// grown by `d cos α`. A pipe's radius grows by `d` about the same
    /// spine.
    ///
    /// A NURBS, an extrusion or a revolution gives an [`Offset`] over its
    /// whole domain, and an offset one by the distances' sum; `None` for
    /// a sweep, which has no offset yet. Fails with `NonFinite`, or
    /// `Degenerate` where the offset collapses: a radius that would fall
    /// below [`crate::tol::LINEAR_RESOLUTION`] (for a cone, its radius in
    /// its moved frame's plane), a torus's minor radius that would reach
    /// its major, or an [`Offset`] that can't be certified regular.
    pub fn offset(&self, d: f64) -> Option<Result<Self, Error>> {
        self.offset_within(d, self.domain())
    }

    /// [`Surface::offset`], but an [`Offset`] only over `domain`, a box of
    /// the surface's parameters (an analytic surface or a pipe ignores
    /// it): a face's, which may be regular where the whole surface isn't.
    pub fn offset_within(&self, d: f64, domain: [[f64; 2]; 2]) -> Option<Result<Self, Error>> {
        if !d.is_finite() {
            return Some(Err(Error::NonFinite));
        }
        Some(match self {
            // Moving a frame by a translation keeps its axes bit for bit.
            Self::Plane(s) => Ok(Self::Plane(Plane::new(
                s.frame
                    .transformed(&Rigid::translation(s.frame.z().get() * d)),
            ))),
            Self::Cylinder(s) => Cylinder::new(s.frame, s.radius + d).map(Self::Cylinder),
            Self::Sphere(s) => Sphere::new(s.frame, s.radius + d).map(Self::Sphere),
            Self::Torus(s) => Torus::new(s.frame, s.major, s.minor + d).map(Self::Torus),
            Self::Cone(s) => {
                let f = s
                    .frame
                    .transformed(&Rigid::translation(s.frame.z().get() * -(d * s.sin)));
                Cone::new(f, s.radius + d * s.cos, s.half_angle).map(Self::Cone)
            }
            Self::Pipe(p) => p.offset(d).map(|p| Self::Pipe(Arc::new(p))),
            Self::Nurbs(_) | Self::Extrusion(_) | Self::Revolution(_) | Self::Offset(_) => {
                Offset::new(self.clone(), d, domain).map(|o| Self::Offset(Arc::new(o)))
            }
            Self::Sweep(_) => return None,
        })
    }

    /// The surface moved by a rigid transform. Parameters are unchanged.
    pub fn transformed(&self, r: &Rigid) -> Self {
        let mut out = self.clone();
        let frame = match &mut out {
            Self::Plane(s) => &mut s.frame,
            Self::Cylinder(s) => &mut s.frame,
            Self::Cone(s) => &mut s.frame,
            Self::Sphere(s) => &mut s.frame,
            Self::Torus(s) => &mut s.frame,
            Self::Nurbs(n) => return Self::Nurbs(Arc::new(n.transformed(r))),
            Self::Extrusion(e) => return Self::Extrusion(Arc::new(e.transformed(r))),
            Self::Revolution(s) => return Self::Revolution(Arc::new(s.transformed(r))),
            Self::Pipe(p) => return Self::Pipe(Arc::new(p.transformed(r))),
            Self::Sweep(s) => return Self::Sweep(Arc::new(s.transformed(r))),
            Self::Offset(o) => return Self::Offset(Arc::new(o.transformed(r))),
        };
        *frame = frame.transformed(r);
        out
    }

    /// The surface reflected by `m`, and how its parameters map. An
    /// analytic surface takes [`Mirror::frame_reversed`], so its natural
    /// normal is mirrored with it: a plane's `v` and the others' angle `u`
    /// run the other way (`u` becomes `2π - u`, staying in its period). A
    /// revolution does the same and a pipe's angle `v` is negated; a NURBS
    /// or an extrusion keeps its parameters, its natural normal turning
    /// against the mirrored geometry. Fails with `OutsideSizeBox` when a
    /// frame's origin leaves the size box.
    pub fn mirrored(&self, m: &Mirror) -> Result<(Self, ParamFlip), Error> {
        let mut out = self.clone();
        let (frame, flip) = match &mut out {
            Self::Plane(s) => (&mut s.frame, ParamFlip::V(0.0)),
            Self::Cylinder(s) => (&mut s.frame, ParamFlip::U(TAU)),
            Self::Cone(s) => (&mut s.frame, ParamFlip::U(TAU)),
            Self::Sphere(s) => (&mut s.frame, ParamFlip::U(TAU)),
            Self::Torus(s) => (&mut s.frame, ParamFlip::U(TAU)),
            Self::Nurbs(n) => {
                let n = n.mapped(|p| m.apply_point(p));
                return Ok((Self::Nurbs(Arc::new(n)), ParamFlip::Kept));
            }
            Self::Extrusion(e) => {
                return Ok((Self::Extrusion(Arc::new(e.mirrored(m)?)), ParamFlip::Kept));
            }
            Self::Revolution(r) => {
                return Ok((
                    Self::Revolution(Arc::new(r.mirrored(m)?)),
                    ParamFlip::U(TAU),
                ));
            }
            Self::Pipe(p) => return Ok((Self::Pipe(Arc::new(p.mirrored(m)?)), ParamFlip::V(0.0))),
            Self::Sweep(s) => return Ok((Self::Sweep(Arc::new(s.mirrored(m)?)), ParamFlip::Kept)),
            Self::Offset(o) => {
                let (o, flip) = o.mirrored(m)?;
                return Ok((Self::Offset(Arc::new(o)), flip));
            }
        };
        *frame = m.frame_reversed(frame)?;
        Ok((out, flip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vector3;
    use core::f64::consts::{FRAC_PI_4, PI};

    fn frame() -> Frame {
        Frame::new(
            Point3::new(1.0, 2.0, 3.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap()
    }

    fn all() -> [Surface; 5] {
        let f = frame();
        [
            Surface::Plane(Plane::new(f)),
            Surface::Cylinder(Cylinder::new(f, 2.0).unwrap()),
            Surface::Cone(Cone::new(f, 2.0, FRAC_PI_4).unwrap()),
            Surface::Sphere(Sphere::new(f, 2.0).unwrap()),
            Surface::Torus(Torus::new(f, 3.0, 1.0).unwrap()),
        ]
    }

    fn near(a: Point3, b: Point3) -> bool {
        a.distance(b) <= 1e-14
    }

    #[test]
    fn mirrors_map_parameters() {
        let m = Mirror::new(
            Point3::new(0.5, -1.0, 2.0),
            Vector3::new(1.0, 0.3, -0.6).normalize().unwrap(),
        );
        let nurbs = all()[3].to_nurbs([[0.0, PI], [-1.0, 1.0]]).unwrap().0;
        let surfaces = all().into_iter().chain([Surface::Nurbs(Arc::new(nurbs))]);
        for s in surfaces {
            let (r, flip) = s.mirrored(&m).unwrap();
            for (u, v) in [(0.3, 0.2), (2.0, -0.7), (5.5, 0.9)] {
                let q = flip.apply(Point2::new(u, v));
                let (p, mp) = (s.point(u, v), r.point(q.x, q.y));
                assert!(m.apply_point(p).distance(mp) < 1e-12, "{s:?} at {u} {v}");
                let (n, mn) = (s.normal(u, v).unwrap(), r.normal(q.x, q.y).unwrap());
                let sign = if flip.flips_normal() { -1.0 } else { 1.0 };
                let d = m.apply_vector(n.get()) * sign - mn.get();
                assert!(d.length() < 1e-12, "{s:?}: {flip:?}");
            }
        }
    }

    fn cone_apex_v(s: &Surface) -> f64 {
        match s {
            Surface::Cone(c) => c.apex_v(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn evaluation_at_known_parameters() {
        let [plane, cyl, cone, sphere, torus] = all();
        assert!(near(plane.point(1.0, -1.0), Point3::new(2.0, 1.0, 3.0)));
        assert!(near(cyl.point(0.0, 1.0), Point3::new(3.0, 2.0, 4.0)));
        assert!(near(cone.point(0.0, 1.0), Point3::new(4.0, 2.0, 4.0)));
        assert!(near(cone.point(1.0, -2.0), Point3::new(1.0, 2.0, 1.0)));
        assert!(near(sphere.point(PI, 0.0), Point3::new(-1.0, 2.0, 3.0)));
        assert!(near(
            sphere.point(0.0, FRAC_PI_2),
            Point3::new(1.0, 2.0, 5.0)
        ));
        assert!(near(torus.point(0.0, 0.0), Point3::new(5.0, 2.0, 3.0)));
        assert!(near(torus.point(0.0, PI), Point3::new(3.0, 2.0, 3.0)));
        assert!(near(
            torus.point(FRAC_PI_2, FRAC_PI_2),
            Point3::new(1.0, 5.0, 4.0)
        ));
    }

    #[test]
    fn natural_normals() {
        let [plane, cyl, cone, sphere, torus] = all();
        let close = |n: Option<UnitVector3>, v: Vector3| (n.unwrap().get() - v).length() <= 1e-15;
        assert!(close(plane.normal(5.0, 5.0), Vector3::new(0.0, 0.0, 1.0)));
        assert!(close(cyl.normal(PI, 7.0), Vector3::new(-1.0, 0.0, 0.0)));
        let h = core::f64::consts::FRAC_1_SQRT_2;
        assert!(close(cone.normal(0.0, 0.0), Vector3::new(h, 0.0, -h)));
        // At the apex: the limit along the generator.
        let apex = cone_apex_v(&cone);
        assert!(close(cone.normal(0.0, apex), Vector3::new(h, 0.0, -h)));
        assert!(close(
            sphere.normal(0.0, -FRAC_PI_2),
            Vector3::new(0.0, 0.0, -1.0)
        ));
        assert!(close(torus.normal(0.0, PI), Vector3::new(-1.0, 0.0, 0.0)));
    }

    #[test]
    fn eval_agrees_with_point_and_normal() {
        for s in all() {
            for (u, v) in [(0.0, 0.0), (0.5, 0.3), (2.0, -0.7), (4.0, 1.2)] {
                let e = s.eval(u, v);
                assert_eq!(e.point, s.point(u, v), "{s:?}");
                let n = e.normal().unwrap().get();
                assert!((n - s.normal(u, v).unwrap().get()).length() <= 1e-15);
            }
        }
    }

    #[test]
    fn cylinder_derivatives() {
        let [_, cyl, ..] = all();
        let e = cyl.eval(FRAC_PI_2, 3.0);
        let close = |a: Vector3, b: Vector3| (a - b).length() <= 1e-15;
        assert!(close(e.du, Vector3::new(-2.0, 0.0, 0.0)));
        assert!(close(e.dv, Vector3::new(0.0, 0.0, 1.0)));
        assert!(close(e.duu, Vector3::new(0.0, -2.0, 0.0)));
        assert_eq!((e.duv, e.dvv), (Vector3::ZERO, Vector3::ZERO));
    }

    #[test]
    fn principal_curvatures_in_closed_form() {
        let [plane, cyl, cone, sphere, torus] = all();
        let k = |s: &Surface, u, v| {
            let k = s.principal_curvatures(u, v).unwrap();
            (k.k1, k.k2)
        };
        let near = |(a, b): (f64, f64), (c, d): (f64, f64)| {
            (a - c).abs() <= 1e-14 && (b - d).abs() <= 1e-14
        };
        assert_eq!(k(&plane, 1.0, 2.0), (0.0, 0.0));
        // Outward normals: convex directions curve away, so they are
        // negative.
        assert!(near(k(&cyl, 1.0, 2.0), (0.0, -0.5)));
        assert!(near(k(&sphere, 1.0, 0.5), (-0.5, -0.5)));
        // A cone at radius 4 (v = 2): the circle direction curves by
        // cos α / ρ.
        assert!(near(k(&cone, 1.0, 2.0), (0.0, -math::cos(FRAC_PI_4) / 4.0)));
        // The outer equator of the torus: tube 1/r, ring 1/(R + r).
        assert!(near(k(&torus, 0.0, 0.0), (-0.25, -1.0)));
        // The inner equator is saddle-shaped.
        assert!(near(k(&torus, 0.0, PI), (0.5, -1.0)));
        // No curvatures where du vanishes: a sphere's pole, a cone's apex.
        assert_eq!(sphere.principal_curvatures(0.0, FRAC_PI_2), None);
        assert_eq!(cone.principal_curvatures(0.0, cone_apex_v(&cone)), None);
    }

    #[test]
    fn points_along_a_pcurve() {
        use crate::geom::{Curve2, Line2, Point2, Vector2};
        let [_, cyl, ..] = all();
        let along =
            Curve2::Line2(Line2::new(Point2::new(0.0, 1.0), Vector2::new(1.0, 0.0)).unwrap());
        for t in [0.0, 1.0, 4.0] {
            assert_eq!(cyl.point_at(&along, t), cyl.point(t, 1.0));
        }
    }

    #[test]
    fn the_nurbs_variant() {
        let [_, _, _, sphere, _] = all();
        let (n, _) = sphere.to_nurbs([[0.0; 2]; 2]).unwrap();
        let s = Surface::Nurbs(std::sync::Arc::new(n.clone()));
        assert_eq!(s.frame(), None);
        assert_eq!(sphere.frame(), Some(&frame()));
        assert_eq!(s.point(0.3, 0.2), n.point(0.3, 0.2));
        assert_eq!(s.eval(0.3, 0.2), n.eval(0.3, 0.2));
        assert_eq!(s.normal(0.3, 0.2), n.normal(0.3, 0.2));
        assert_eq!(
            s.principal_curvatures(0.3, 0.2),
            n.principal_curvatures(0.3, 0.2)
        );
        assert_eq!(s.domain(), n.domain());
        assert_eq!(s.periods(), n.periods());
        // Periodic one way with both ends of the other collapsed: a
        // sphere's topology.
        assert_eq!(s.closed_genus(), Some(0));
        let p = Point3::new(5.0, 1.0, -2.0);
        assert_eq!(s.project(p), n.project(p));
        assert_eq!(s.distance(p), n.distance(p));
        assert_eq!(s.bounds(), n.bounds());
        let r = Rigid::translation(Vector3::new(0.0, 0.0, 1.0));
        assert!(near(
            s.transformed(&r).point(0.3, 0.2),
            s.point(0.3, 0.2) + Vector3::new(0.0, 0.0, 1.0)
        ));
        let (same, rp) = s.to_nurbs([[0.0; 2]; 2]).unwrap();
        assert_eq!(same, n);
        assert_eq!(rp, [crate::geom::Reparam::Identity; 2]);
    }

    #[test]
    fn domains() {
        let [plane, cyl, cone, sphere, torus] = all();
        let inf = f64::INFINITY;
        assert_eq!(plane.domain(), [[-inf, inf], [-inf, inf]]);
        assert_eq!(cyl.domain(), [[0.0, TAU], [-inf, inf]]);
        assert_eq!(cone.domain(), [[0.0, TAU], [cone_apex_v(&cone), inf]]);
        assert_eq!(sphere.domain(), [[0.0, TAU], [-FRAC_PI_2, FRAC_PI_2]]);
        assert_eq!(torus.domain(), [[0.0, TAU], [0.0, TAU]]);
    }

    #[test]
    fn closed_form_distances() {
        let [plane, cyl, cone, sphere, torus] = all();
        let centre = Point3::new(1.0, 2.0, 3.0);
        assert_eq!(plane.distance(Point3::new(9.0, 9.0, 1.0)), 2.0);
        assert_eq!(cyl.distance(centre), 2.0);
        assert_eq!(cyl.distance(Point3::new(1.0, 7.0, -40.0)), 3.0);
        assert_eq!(sphere.distance(centre), 2.0);
        assert_eq!(torus.distance(centre), 2.0);
        assert_eq!(torus.distance(Point3::new(4.0, 2.0, 3.0)), 1.0);
        // The apex is at v = -2, i.e. (1, 2, 1). Below it, the nearest
        // point is the apex itself.
        let apex = Point3::new(1.0, 2.0, 1.0);
        assert!(cone.distance(apex) <= 1e-15);
        assert!((cone.distance(Point3::new(1.0, 2.0, -2.0)) - 3.0).abs() <= 1e-15);
        // Inside the cone on the axis, above the apex: perpendicular to
        // the generator, (h - apex) sin α.
        let d = cone.distance(Point3::new(1.0, 2.0, 3.0));
        assert!((d - 2.0 * math::sin(FRAC_PI_4)).abs() <= 1e-15);
    }

    #[test]
    fn params_round_trip() {
        for s in all() {
            for (u, v) in [(0.0, 0.0), (0.5, 0.3), (2.0, -0.7), (4.0, 1.2), (6.0, 0.9)] {
                let p = s.point(u, v);
                assert!(s.distance(p) <= 1e-14, "{s:?} at {u}, {v}");
                let foot = s.project(p);
                let (pu, pv) = foot.param;
                assert!(near(s.point(pu, pv), p), "{s:?} at {u}, {v}");
                assert_eq!(foot.point, s.point(pu, pv));
                assert_eq!(foot.distance, s.distance(p));
            }
        }
    }

    #[test]
    fn projections_of_off_surface_points_land_on_the_nearest_point() {
        let [_, cyl, cone, sphere, torus] = all();
        let p = Point3::new(1.0, 7.0, 4.0);
        let (u, v) = cyl.project(p).param;
        assert_eq!((u, v), (FRAC_PI_2, 1.0));
        let (u, v) = sphere.project(Point3::new(1.0, 2.0, 9.0)).param;
        assert_eq!((u, v), (0.0, FRAC_PI_2));
        let (_, v) = torus.project(Point3::new(3.5, 2.0, 3.0)).param;
        assert_eq!(v, PI);
        // Below the apex the nearest point is the apex.
        let (_, v) = cone.project(Point3::new(1.0, 2.0, -5.0)).param;
        assert_eq!(v, cone_apex_v(&cone));
    }

    #[test]
    fn bounds() {
        let [plane, cyl, cone, sphere, torus] = all();
        for open in [plane, cyl, cone] {
            assert_eq!(open.bounds(), crate::tol::size_box());
        }
        let b = sphere.bounds();
        assert_eq!(b.min(), Point3::new(-1.0, 0.0, 1.0));
        assert_eq!(b.max(), Point3::new(3.0, 4.0, 5.0));
        let b = torus.bounds();
        assert_eq!(b.min(), Point3::new(-3.0, -2.0, 2.0));
        assert_eq!(b.max(), Point3::new(5.0, 6.0, 4.0));
    }

    #[test]
    fn periods_and_closedness() {
        let [plane, cyl, cone, sphere, torus] = all();
        assert_eq!(plane.periods(), (None, None));
        assert_eq!(cyl.periods(), (Some(TAU), None));
        assert_eq!(cone.periods(), (Some(TAU), None));
        assert_eq!(sphere.periods(), (Some(TAU), None));
        assert_eq!(torus.periods(), (Some(TAU), Some(TAU)));
        assert_eq!(plane.closed_genus(), None);
        assert_eq!(cyl.closed_genus(), None);
        assert_eq!(cone.closed_genus(), None);
        assert_eq!(sphere.closed_genus(), Some(0));
        assert_eq!(torus.closed_genus(), Some(1));
    }

    #[test]
    fn offsets_keep_their_parameters() {
        for s in all() {
            for d in [0.3, -0.2] {
                let o = s.offset(d).unwrap().unwrap();
                for (u, v) in [(0.0, 0.0), (0.5, 0.3), (2.0, -0.7), (4.0, 1.2)] {
                    let n = s.normal(u, v).unwrap().get();
                    let want = s.point(u, v) + n * d;
                    assert!(o.point(u, v).distance(want) <= 1e-14, "{s:?} by {d}");
                    assert!((o.normal(u, v).unwrap().get() - n).length() <= 1e-15);
                }
            }
        }
        // Collapses.
        let [_, cyl, cone, sphere, torus] = all();
        assert_eq!(cyl.offset(-2.0), Some(Err(Error::Degenerate)));
        assert_eq!(sphere.offset(-3.0), Some(Err(Error::Degenerate)));
        assert_eq!(torus.offset(2.0), Some(Err(Error::Degenerate)));
        assert_eq!(torus.offset(-1.0), Some(Err(Error::Degenerate)));
        // The cone's radius in its moved frame: 2 + d cos 45°.
        assert_eq!(cone.offset(-3.0), Some(Err(Error::Degenerate)));
        assert_eq!(cyl.offset(f64::NAN), Some(Err(Error::NonFinite)));
        let (n, _) = sphere.to_nurbs([[0.0; 2]; 2]).unwrap();
        // A NURBS sphere's poles have no certified normal: its offset over
        // the whole domain is refused, over a band it is an offset.
        let n = Surface::Nurbs(std::sync::Arc::new(n));
        assert_eq!(n.offset(0.1), Some(Err(Error::Degenerate)));
        let band = n
            .offset_within(0.1, [[0.0, TAU], [-1.0, 1.0]])
            .unwrap()
            .unwrap();
        assert!(matches!(band, Surface::Offset(_)));
    }

    #[test]
    fn transformed_moves_the_frame() {
        let r = Rigid::translation(Vector3::new(0.0, 0.0, 10.0));
        for s in all() {
            let t = s.transformed(&r);
            assert!(near(
                t.point(0.5, 0.25),
                s.point(0.5, 0.25) + Vector3::new(0.0, 0.0, 10.0)
            ));
        }
    }

    #[test]
    fn constructors_reject_bad_input() {
        let f = Frame::WORLD;
        assert_eq!(Cylinder::new(f, 0.0), Err(Error::Degenerate));
        assert_eq!(Sphere::new(f, f64::NAN), Err(Error::NonFinite));
        assert_eq!(Cone::new(f, 1.0, 0.0), Err(Error::Degenerate));
        assert_eq!(Cone::new(f, 1.0, FRAC_PI_2), Err(Error::Degenerate));
        assert_eq!(Cone::new(f, 1.0, -0.5), Err(Error::Degenerate));
        assert_eq!(Cone::new(f, 1.0, f64::NAN), Err(Error::NonFinite));
        assert_eq!(Torus::new(f, 1.0, 1.0), Err(Error::Degenerate));
        assert_eq!(Torus::new(f, 1.0, 2.0), Err(Error::Degenerate));
        assert!(Torus::new(f, 1.0, 0.5).is_ok());
        let c = Cone::new(f, 2.0, FRAC_PI_4).unwrap();
        assert_eq!((c.radius(), c.half_angle()), (2.0, FRAC_PI_4));
        assert!((c.apex_v() + 2.0).abs() <= 1e-15);
    }
}
