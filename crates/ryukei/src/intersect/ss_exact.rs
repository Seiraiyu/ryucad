//! Closed-form surface–surface intersections.
//!
//! - **Coaxial pairs.** Two surfaces of revolution about one axis meet in
//!   circles about it, where their meridians (the curves they turn in a
//!   half-plane through the axis) cross: lines for planes across the
//!   axis, cylinders and cones, circles for spheres and tori. A plane is
//!   such a surface about any of its normals, and a sphere about any line
//!   through its centre, so a plane across an axis, a sphere centred on
//!   one, and two spheres are all coaxial pairs.
//! - **Plane–plane** is a line; **plane–cylinder** two lines, a tangent
//!   line or an ellipse; **plane–cone** an ellipse, or a parabola or
//!   hyperbola as an exact rational quadratic, or rulings through the
//!   apex; a plane through a torus's axis cuts two circles.
//! - **Cylinder–cylinder** with parallel axes meet in lines; with equal
//!   radii and crossing axes in two ellipses, in the planes that bisect
//!   the axes. Two cones sharing an apex meet in rulings.
//!
//! Tangency follows explicit rules: meridians, sections or cones whose
//! gap is within [`LINEAR_RESOLUTION`] (or whose angles agree within
//! [`ANGULAR_RESOLUTION`]) touch, and the branch's tolerance covers the
//! gap. Everything else is left to tracing.

use super::SLACK;
use super::coincide::{off_axis, off_plane, parallel};
use super::ss::Raw;
use crate::geom::{
    Box3, Circle, Cone, Curve, Ellipse, Frame, KnotVector, Line, NurbsCurve, Point3, Surface,
    Vector3,
};
use crate::math;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION, in_size_box};
use core::f64::consts::{FRAC_PI_4, PI, TAU};
use std::sync::Arc;

/// The closed form for this pair, if it has one: its branches and
/// points.
pub(crate) fn exact(a: &Surface, b: &Surface, region: &Box3) -> Option<(Vec<Raw>, Vec<Point3>)> {
    let (a, b) = if rank(a) <= rank(b) { (a, b) } else { (b, a) };
    let mut out = Out::new(region);
    if let Some(axis) = common_axis(a, b) {
        meridians(a, b, &axis, &mut out);
        return Some(out.finish());
    }
    let (fa, fb) = (a.frame()?, b.frame()?);
    match (a, b) {
        (Surface::Plane(_), Surface::Plane(_)) => plane_plane(fa, fb, &mut out),
        (Surface::Plane(_), Surface::Cylinder(c)) => plane_cylinder(fa, fb, c.radius(), &mut out),
        (Surface::Plane(_), Surface::Cone(c)) => plane_cone(fa, fb, c, &mut out),
        (Surface::Plane(_), Surface::Torus(t)) => {
            let n = fa.z();
            let through = n.dot(fb.z()).abs() <= ANGULAR_RESOLUTION
                && off_plane(fa, fb.origin()) <= LINEAR_RESOLUTION;
            if !through {
                return None;
            }
            let gap = off_plane(fa, fb.origin());
            let m = fb.z().get().cross(n.get()).normalize()?.get();
            for side in [1.0, -1.0] {
                let centre = fb.origin() + m * (side * t.major());
                let frame = Frame::new(centre, n.get(), m * side).ok()?;
                out.circle(frame, t.minor(), false, gap);
            }
        }
        (Surface::Cylinder(x), Surface::Cylinder(y)) => {
            if parallel(fa.z(), fb.z()) {
                parallel_cylinders(fa, x.radius(), fb, y.radius(), &mut out);
            } else {
                crossing_cylinders(fa, x.radius(), fb, y.radius(), &mut out)?;
            }
        }
        (Surface::Cone(x), Surface::Cone(y)) => apex_cones(fa, x, fb, y, &mut out)?,
        _ => return None,
    }
    Some(out.finish())
}

/// The order pairs are taken in: plane, cylinder, cone, sphere, torus.
fn rank(s: &Surface) -> u8 {
    match s {
        Surface::Plane(_) => 0,
        Surface::Cylinder(_) => 1,
        Surface::Cone(_) => 2,
        Surface::Sphere(_) => 3,
        _ => 4,
    }
}

fn tolerance(gap: f64) -> f64 {
    LINEAR_RESOLUTION.max(gap + SLACK)
}

/// The branches and points found so far, kept to those meeting the
/// region.
struct Out {
    region: Box3,
    branches: Vec<Raw>,
    points: Vec<Point3>,
}

impl Out {
    fn new(region: &Box3) -> Self {
        Self {
            region: region.grow(LINEAR_RESOLUTION),
            branches: Vec::new(),
            points: Vec::new(),
        }
    }

    fn finish(self) -> (Vec<Raw>, Vec<Point3>) {
        (self.branches, self.points)
    }

    fn inside(&self, p: Point3) -> bool {
        let (lo, hi) = (self.region.min(), self.region.max());
        (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && (lo.z..=hi.z).contains(&p.z)
    }

    fn point(&mut self, p: Point3) {
        if self.inside(p) {
            self.points.push(p);
        }
    }

    /// The line through `origin` along `dir` (from `origin` on, for a
    /// ray), as far as it crosses the region.
    fn line(&mut self, origin: Point3, dir: Vector3, ray: bool, tangent: bool, gap: f64) {
        let Some(dir) = dir.normalize() else { return };
        let d = dir.get();
        let (lo, hi) = (self.region.min(), self.region.max());
        let (mut t0, mut t1) = (if ray { 0.0 } else { f64::NEG_INFINITY }, f64::INFINITY);
        for (o, d, lo, hi) in [
            (origin.x, d.x, lo.x, hi.x),
            (origin.y, d.y, lo.y, hi.y),
            (origin.z, d.z, lo.z, hi.z),
        ] {
            if d.abs() < 1e-300 {
                if o < lo || o > hi {
                    return;
                }
                continue;
            }
            let (a, b) = ((lo - o) / d, (hi - o) / d);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
        if t1 - t0 <= LINEAR_RESOLUTION {
            return;
        }
        let Ok(line) = Line::new(origin + d * t0, d) else {
            return;
        };
        self.branches.push(Raw {
            curve: Curve::Line(line),
            range: [0.0, t1 - t0],
            tangent,
            tolerance: tolerance(gap),
        });
    }

    fn closed(&mut self, curve: Curve, tangent: bool, gap: f64) {
        if curve.bounds([0.0, TAU]).intersects(&self.region) {
            self.branches.push(Raw {
                curve,
                range: [0.0, TAU],
                tangent,
                tolerance: tolerance(gap),
            });
        }
    }

    fn circle(&mut self, frame: Frame, r: f64, tangent: bool, gap: f64) {
        if let Ok(c) = Circle::new(frame, r) {
            self.closed(Curve::Circle(c), tangent, gap);
        }
    }
}

/// An axis both surfaces turn about, as a frame whose `z` is the axis,
/// when there is one.
fn common_axis(a: &Surface, b: &Surface) -> Option<Frame> {
    let axial = |s: &Surface| {
        matches!(
            s,
            Surface::Cylinder(_) | Surface::Cone(_) | Surface::Torus(_)
        )
    };
    let (fa, fb) = (*a.frame()?, *b.frame()?);
    let on_axis = |f: &Frame, p: Point3| off_axis(f, p) <= LINEAR_RESOLUTION;
    match (a, b) {
        (Surface::Plane(_), Surface::Plane(_)) => None,
        (Surface::Plane(_), Surface::Sphere(_)) => {
            Frame::new(fb.origin(), fa.z().get(), fa.x().get()).ok()
        }
        (Surface::Plane(_), s) if axial(s) => parallel(fa.z(), fb.z()).then_some(fb),
        (Surface::Sphere(_), Surface::Sphere(_)) => {
            let z = (fb.origin() - fa.origin())
                .normalize()
                .map_or(fa.z().get(), |u| u.get());
            Frame::new(fa.origin(), z, fa.x().get())
                .or_else(|_| Frame::new(fa.origin(), z, fa.y().get()))
                .ok()
        }
        (Surface::Sphere(_), s) if axial(s) => on_axis(&fb, fa.origin()).then_some(fb),
        (s, Surface::Sphere(_)) if axial(s) => on_axis(&fa, fb.origin()).then_some(fa),
        (s, t) if axial(s) && axial(t) => {
            (parallel(fa.z(), fb.z()) && on_axis(&fa, fb.origin()) && on_axis(&fb, fa.origin()))
                .then_some(fa)
        }
        _ => None,
    }
}

/// A surface's meridian in the half-plane `(ρ, z)` about an axis frame.
#[derive(Clone, Copy, Debug)]
enum Meridian {
    /// `p + t d`, `d` a unit vector, for every `t` or (a ray) `t >= 0`.
    Line {
        p: [f64; 2],
        d: [f64; 2],
        ray: bool,
    },
    Circle {
        c: [f64; 2],
        r: f64,
    },
}

fn meridian(s: &Surface, axis: &Frame) -> Option<Meridian> {
    let f = s.frame()?;
    let height = |p: Point3| (p - axis.origin()).dot(axis.z().get());
    Some(match s {
        Surface::Plane(_) => Meridian::Line {
            p: [0.0, height(f.origin())],
            d: [1.0, 0.0],
            ray: true,
        },
        Surface::Cylinder(c) => Meridian::Line {
            p: [c.radius(), 0.0],
            d: [0.0, 1.0],
            ray: false,
        },
        Surface::Cone(c) => {
            let apex = f.origin() + f.z().get() * c.apex_v();
            let (sin, cos) = math::sin_cos(c.half_angle());
            let up = if f.z().dot(axis.z()) > 0.0 { 1.0 } else { -1.0 };
            Meridian::Line {
                p: [0.0, height(apex)],
                d: [sin, up * cos],
                ray: true,
            }
        }
        Surface::Sphere(c) => Meridian::Circle {
            c: [0.0, height(f.origin())],
            r: c.radius(),
        },
        Surface::Torus(t) => Meridian::Circle {
            c: [t.major(), height(f.origin())],
            r: t.minor(),
        },
        _ => return None,
    })
}

/// Where two meridians cross: `(point, tangent, gap)`.
fn cross_meridians(a: Meridian, b: Meridian) -> Vec<([f64; 2], bool, f64)> {
    let cross = |u: [f64; 2], v: [f64; 2]| u[0] * v[1] - u[1] * v[0];
    let sub = |u: [f64; 2], v: [f64; 2]| [u[0] - v[0], u[1] - v[1]];
    let len = |u: [f64; 2]| math::hypot(u[0], u[1]);
    let along = |p: [f64; 2], d: [f64; 2], t: f64| [p[0] + d[0] * t, p[1] + d[1] * t];
    let mut out = Vec::new();
    match (a, b) {
        (
            Meridian::Line { p, d, ray },
            Meridian::Line {
                p: q,
                d: e,
                ray: ray2,
            },
        ) => {
            let det = cross(d, e);
            if det.abs() <= ANGULAR_RESOLUTION {
                return out;
            }
            let w = sub(q, p);
            let (t, s) = (cross(w, e) / det, cross(w, d) / det);
            if (!ray || t >= -LINEAR_RESOLUTION) && (!ray2 || s >= -LINEAR_RESOLUTION) {
                out.push((along(p, d, t), false, 0.0));
            }
        }
        (Meridian::Line { p, d, ray }, Meridian::Circle { c, r })
        | (Meridian::Circle { c, r }, Meridian::Line { p, d, ray }) => {
            let q = sub(p, c);
            let b = d[0] * q[0] + d[1] * q[1];
            let dist = cross(q, d).abs();
            if dist > r + LINEAR_RESOLUTION {
                return out;
            }
            let gap = (dist - r).abs();
            let ts: Vec<(f64, bool)> = if gap <= LINEAR_RESOLUTION {
                vec![(-b, true)]
            } else {
                let h = math::sqrt(r * r - dist * dist);
                vec![(-b - h, false), (-b + h, false)]
            };
            for (t, tangent) in ts {
                if !ray || t >= -LINEAR_RESOLUTION {
                    out.push((along(p, d, t), tangent, if tangent { gap } else { 0.0 }));
                }
            }
        }
        (Meridian::Circle { c, r }, Meridian::Circle { c: k, r: s }) => {
            let v = sub(k, c);
            let d = len(v);
            if d <= LINEAR_RESOLUTION
                || d > r + s + LINEAR_RESOLUTION
                || d < (r - s).abs() - LINEAR_RESOLUTION
            {
                return out;
            }
            let u = [v[0] / d, v[1] / d];
            let outer = (d - (r + s)).abs();
            let inner = (d - (r - s).abs()).abs();
            if outer <= LINEAR_RESOLUTION {
                out.push((along(c, u, r), true, outer));
            } else if inner <= LINEAR_RESOLUTION {
                let side = if r >= s { r } else { -r };
                out.push((along(c, u, side), true, inner));
            } else {
                let a = (d * d + r * r - s * s) / (2.0 * d);
                let h = math::sqrt((r * r - a * a).max(0.0));
                let m = along(c, u, a);
                out.push(([m[0] - u[1] * h, m[1] + u[0] * h], false, 0.0));
                out.push(([m[0] + u[1] * h, m[1] - u[0] * h], false, 0.0));
            }
        }
    }
    out
}

/// A coaxial pair: circles about the axis where the meridians cross, and
/// points where they cross on the axis.
fn meridians(a: &Surface, b: &Surface, axis: &Frame, out: &mut Out) {
    let (Some(ma), Some(mb)) = (meridian(a, axis), meridian(b, axis)) else {
        return;
    };
    for ([rho, z], tangent, gap) in cross_meridians(ma, mb) {
        let centre = axis.origin() + axis.z().get() * z;
        if rho < -LINEAR_RESOLUTION {
            continue;
        }
        if rho <= LINEAR_RESOLUTION {
            out.point(centre);
            continue;
        }
        if let Ok(frame) = Frame::new(centre, axis.z().get(), axis.x().get()) {
            out.circle(frame, rho, tangent, gap);
        }
    }
}

fn plane_plane(f: &Frame, g: &Frame, out: &mut Out) {
    let (n1, n2) = (f.z().get(), g.z().get());
    let d = n1.cross(n2);
    let len2 = d.length_squared();
    if d.length() <= ANGULAR_RESOLUTION {
        return;
    }
    let (h1, h2) = (
        n1.dot(f.origin().to_vector()),
        n2.dot(g.origin().to_vector()),
    );
    let p = Point3::ORIGIN + (n2.cross(d) * h1 + d.cross(n1) * h2) / len2;
    out.line(p, d, false, false, 0.0);
}

/// A plane and a cylinder whose axis is not along its normal.
fn plane_cylinder(f: &Frame, g: &Frame, r: f64, out: &mut Out) {
    let (n, z) = (f.z().get(), g.z().get());
    let cz = n.dot(z);
    let delta = n.dot(g.origin() - f.origin());
    if cz.abs() <= ANGULAR_RESOLUTION {
        let foot = g.origin() - n * delta;
        if delta.abs() > r + LINEAR_RESOLUTION {
            return;
        }
        let gap = (delta.abs() - r).abs();
        if gap <= LINEAR_RESOLUTION {
            out.line(foot, z, false, true, gap);
            return;
        }
        let w = math::sqrt(r * r - delta * delta);
        let Some(m) = n.cross(z).normalize() else {
            return;
        };
        out.line(foot + m.get() * w, z, false, false, 0.0);
        out.line(foot - m.get() * w, z, false, false, 0.0);
        return;
    }
    if let Some(e) = oblique_section(f, g, r) {
        out.closed(Curve::Ellipse(e), false, 0.0);
    }
}

/// The ellipse a plane cuts across a cylinder's axis: centred where the
/// axis meets the plane, with the cylinder's radius across the axis and
/// `r / |n·z|` along the plane's steepest slope.
fn oblique_section(f: &Frame, g: &Frame, r: f64) -> Option<Ellipse> {
    let (n, z) = (f.z().get(), g.z().get());
    let cz = n.dot(z);
    let centre = g.origin() + z * (n.dot(f.origin() - g.origin()) / cz);
    let m = n.cross(z).normalize()?.get();
    let e = n.cross(m);
    let frame = Frame::new(centre, n, e).ok()?;
    Ellipse::new(frame, r / cz.abs(), r).ok()
}

/// Cylinders with parallel axes: the sections across the axes are
/// circles, and each point where they cross is a line along the axes.
fn parallel_cylinders(f: &Frame, r1: f64, g: &Frame, r2: f64, out: &mut Out) {
    let local = f.to_local(g.origin());
    let c1 = Meridian::Circle {
        c: [0.0, 0.0],
        r: r1,
    };
    let c2 = Meridian::Circle {
        c: [local.x, local.y],
        r: r2,
    };
    for ([x, y], tangent, gap) in cross_meridians(c1, c2) {
        out.line(f.to_world(x, y, 0.0), f.z().get(), false, tangent, gap);
    }
}

/// Cylinders of equal radius whose axes cross: two ellipses, in the
/// planes through the crossing that bisect the axes. Other crossing
/// cylinders have no closed form.
fn crossing_cylinders(f: &Frame, r1: f64, g: &Frame, r2: f64, out: &mut Out) -> Option<()> {
    if (r1 - r2).abs() > LINEAR_RESOLUTION {
        return None;
    }
    let (z1, z2) = (f.z().get(), g.z().get());
    let w = z1.dot(z2);
    let d = g.origin() - f.origin();
    let (a, b) = (d.dot(z1), d.dot(z2));
    let den = 1.0 - w * w;
    let (s, t) = ((a - w * b) / den, (w * a - b) / den);
    let (p, q) = (f.origin() + z1 * s, g.origin() + z2 * t);
    if p.distance(q) > LINEAR_RESOLUTION {
        return None;
    }
    let o = p + (q - p) * 0.5;
    for normal in [z1 - z2, z1 + z2] {
        let plane = Frame::new(o, normal, z1.cross(normal)).ok()?;
        if let Some(e) = oblique_section(&plane, f, r1) {
            out.closed(Curve::Ellipse(e), false, p.distance(q));
        }
    }
    Some(())
}

/// Two cones sharing an apex, not coaxial: the rulings along directions
/// `d` with `d·z1 = cos α1` and `d·z2 = cos α2`, or the apex alone.
fn apex_cones(f: &Frame, x: &Cone, g: &Frame, y: &Cone, out: &mut Out) -> Option<()> {
    let apex = |f: &Frame, c: &Cone| f.origin() + f.z().get() * c.apex_v();
    let (a1, a2) = (apex(f, x), apex(g, y));
    if a1.distance(a2) > LINEAR_RESOLUTION {
        return None;
    }
    let (z1, z2) = (f.z().get(), g.z().get());
    let w = z1.dot(z2);
    let (c1, c2) = (math::cos(x.half_angle()), math::cos(y.half_angle()));
    let den = 1.0 - w * w;
    let (a, b) = ((c1 - w * c2) / den, (c2 - w * c1) / den);
    let e = z1.cross(z2).normalize()?.get();
    let c2sq = 1.0 - (a * a + b * b + 2.0 * a * b * w);
    let base = z1 * a + z2 * b;
    if c2sq < -ANGULAR_RESOLUTION {
        out.point(a1);
    } else if c2sq <= ANGULAR_RESOLUTION * ANGULAR_RESOLUTION {
        out.line(a1, base, true, true, a1.distance(a2));
    } else {
        let c = math::sqrt(c2sq);
        out.line(a1, base + e * c, true, false, a1.distance(a2));
        out.line(a1, base - e * c, true, false, a1.distance(a2));
    }
    Some(())
}

/// A plane and a cone whose axis is not along its normal. Each ruling
/// `apex + ρ d(u)`, `d(u) = cos α z + sin α radial(u)`, meets the plane
/// `n·(p - apex) = h` at `ρ = h / (n·d(u))`, so the curve is
/// `P(u) = apex + d(u) h / (n·d(u))` wherever `n·d(u)` has the sign of
/// `h`: all round for an ellipse, an open stretch for a parabola or a
/// hyperbola's one branch on this nappe. Through the apex it is rulings.
fn plane_cone(f: &Frame, g: &Frame, cone: &Cone, out: &mut Out) {
    let n = f.z().get();
    let (x, y, z) = (g.x().get(), g.y().get(), g.z().get());
    let apex = g.origin() + z * cone.apex_v();
    let h = n.dot(f.origin() - apex);
    let (sa, ca) = math::sin_cos(cone.half_angle());
    // n·d(u) = A + M cos(u - φ).
    let (a, b, c) = (ca * n.dot(z), sa * n.dot(x), sa * n.dot(y));
    let m = math::hypot(b, c);
    let phi = math::atan2(c, b);
    let ruling = |u: f64| z * ca + g.radial(u) * sa;
    if h.abs() <= LINEAR_RESOLUTION {
        let gap = h.abs();
        if m < a.abs() * (1.0 - ANGULAR_RESOLUTION) {
            out.point(apex);
        } else if m <= a.abs() * (1.0 + ANGULAR_RESOLUTION) {
            let u = if a > 0.0 { phi + PI } else { phi };
            out.line(apex, ruling(u), true, true, gap);
        } else {
            let psi = math::acos(-a / m);
            out.line(apex, ruling(phi + psi), true, false, gap);
            out.line(apex, ruling(phi - psi), true, false, gap);
        }
        return;
    }
    let s = h.signum();
    let closed = m < a.abs() && s * a > 0.0;
    if closed && let Some(e) = cone_ellipse(n, z, apex, h, ca) {
        out.closed(Curve::Ellipse(e), false, 0.0);
        return;
    }
    let range = if m <= a.abs() {
        if s * a <= 0.0 {
            return;
        }
        [phi - PI, phi + PI]
    } else {
        let psi = math::acos((-a / m).clamp(-1.0, 1.0));
        if s > 0.0 {
            [phi - psi, phi + psi]
        } else {
            [phi + psi, phi + TAU - psi]
        }
    };
    let conic = Conic {
        apex,
        frame: *g,
        sa,
        ca,
        n,
        h,
    };
    for run in conic.runs(range, out) {
        if let Some(curve) = conic.nurbs(run) {
            let domain = curve.domain();
            out.branches.push(Raw {
                curve: Curve::Nurbs(Arc::new(curve)),
                range: domain,
                tangent: false,
                tolerance: LINEAR_RESOLUTION,
            });
        }
    }
}

/// The closed section of a cone, from the conic in the plane's
/// coordinates: with `e1` the axis's direction in the plane, `e2` across
/// it and `(s, t)` coordinates from the apex's foot `apex + h n`, the cone
/// `|p|² = K (p·z)²`, `K = 1 / cos² α`, is
/// `t² + λ (s - s0)² = R` with `λ = 1 - K g²`, `g = e1·z`,
/// `s0 = K h c g / λ` and `R = λ s0² - h² (1 - K c²)`, `c = n·z`.
fn cone_ellipse(n: Vector3, z: Vector3, apex: Point3, h: f64, ca: f64) -> Option<Ellipse> {
    let c = n.dot(z);
    let along = z - n * c;
    let g = along.length();
    let e1 = along / g;
    let e2 = n.cross(e1);
    let k = 1.0 / (ca * ca);
    let lambda = 1.0 - k * g * g;
    if lambda <= 0.0 {
        return None;
    }
    let s0 = k * h * c * g / lambda;
    let r = lambda * s0 * s0 - h * h * (1.0 - k * c * c);
    // On this nappe: the centre above the apex.
    if r <= 0.0 || h * c + s0 * g <= 0.0 {
        return None;
    }
    let (a1, a2) = (math::sqrt(r / lambda), math::sqrt(r));
    let centre = apex + n * h + e1 * s0;
    if !in_size_box(centre) || a1.max(a2) > crate::tol::SIZE_BOX_HALF {
        return None;
    }
    let (x, major, minor) = if a1 >= a2 { (e1, a1, a2) } else { (e2, a2, a1) };
    Ellipse::new(Frame::new(centre, n, x).ok()?, major, minor).ok()
}

/// An open plane–cone section, `P(u) = apex + d(u) h / (n·d(u))`.
struct Conic {
    apex: Point3,
    frame: Frame,
    sa: f64,
    ca: f64,
    n: Vector3,
    h: f64,
}

/// Samples along an open section when cutting it to the region.
const RUN_SAMPLES: usize = 1024;

/// The most pieces an open section's NURBS may be halved into before it
/// is given up.
const MAX_HALVINGS: usize = 256;

impl Conic {
    /// `d` at a point `(cx, cy)` of the plane of the unit circle (affine,
    /// so it holds off the circle too).
    fn dir(&self, cx: f64, cy: f64) -> Vector3 {
        self.frame.z().get() * self.ca
            + (self.frame.x().get() * cx + self.frame.y().get() * cy) * self.sa
    }

    fn point(&self, u: f64) -> Point3 {
        let (s, c) = math::sin_cos(u);
        let d = self.dir(c, s);
        self.apex + d * (self.h / self.n.dot(d))
    }

    /// The stretches of `(u0, u1)` (open: the curve runs to infinity at
    /// its ends) where the curve is in the region, each widened by a
    /// sample and kept inside the size box.
    fn runs(&self, [u0, u1]: [f64; 2], out: &Out) -> Vec<[f64; 2]> {
        let step = (u1 - u0) / RUN_SAMPLES as f64;
        let at = |k: usize| u0 + step * (k as f64 + 0.5);
        let inside: Vec<bool> = (0..RUN_SAMPLES)
            .map(|k| out.inside(self.point(at(k))))
            .collect();
        let mut runs = Vec::new();
        let mut k = 0;
        while k < RUN_SAMPLES {
            if !inside[k] {
                k += 1;
                continue;
            }
            let first = k;
            while k < RUN_SAMPLES && inside[k] {
                k += 1;
            }
            // Widened by a step, but never past the range's ends: there
            // the section runs off to infinity and beyond them it is on
            // the other nappe.
            let (a, b) = (at(first), at(k - 1));
            runs.push([
                self.widen(a, (u0 - a).max(-step)),
                self.widen(b, (u1 - b).min(step)),
            ]);
        }
        runs
    }

    /// `u + step`, or as far towards it as keeps the point in the size
    /// box.
    fn widen(&self, u: f64, step: f64) -> f64 {
        let mut s = step;
        for _ in 0..60 {
            if in_size_box(self.point(u + s)) {
                return u + s;
            }
            s /= 2.0;
        }
        u
    }

    /// The curve over `[u0, u1]` as a rational quadratic NURBS whose knots
    /// are the angles where its pieces meet: each piece is the image of a
    /// circular arc in the half-angle form, through the map
    /// `c -> apex + d(c) h / (n·d(c))`, which is projective in `c`, so it
    /// keeps the form exact. Pieces are halved until every weight is
    /// positive and every control point is in the size box.
    fn nurbs(&self, [u0, u1]: [f64; 2]) -> Option<NurbsCurve> {
        let count = ((u1 - u0) / FRAC_PI_4).ceil().max(1.0) as usize;
        let mut breaks: Vec<f64> = (0..=count)
            .map(|i| u0 + (u1 - u0) * i as f64 / count as f64)
            .collect();
        breaks[count] = u1;
        let sign = self.h.signum();
        // Near an asymptote the pieces must shrink with their distance from
        // it, so a section close to the apex takes many halvings.
        for _ in 0..MAX_HALVINGS {
            let mut points = Vec::new();
            let mut weights = Vec::new();
            let mut bad = None;
            for (i, w) in breaks.windows(2).enumerate() {
                let (a, b) = (w[0], w[1]);
                let half = (b - a) / 2.0;
                let mid = a + half;
                let (cw, mw) = (math::cos(half), 1.0);
                let (sa_, ca_) = math::sin_cos(a);
                let (sm, cm) = math::sin_cos(mid);
                let (sb, cb) = math::sin_cos(b);
                let ctl = [(ca_, sa_, mw), (cm / cw, sm / cw, cw), (cb, sb, mw)];
                for (k, &(cx, cy, wk)) in ctl.iter().enumerate() {
                    if i > 0 && k == 0 {
                        continue;
                    }
                    let d = self.dir(cx, cy);
                    let den = self.n.dot(d);
                    let weight = den * wk * sign;
                    let p = self.apex + d * (self.h / den);
                    if weight <= 0.0 || !in_size_box(p) {
                        bad = Some(i);
                    }
                    points.push(p);
                    weights.push(weight);
                }
            }
            match bad {
                Some(i) => {
                    let m = (breaks[i] + breaks[i + 1]) / 2.0;
                    breaks.insert(i + 1, m);
                }
                None => {
                    let mut mults = vec![2; breaks.len()];
                    mults[0] = 3;
                    *mults.last_mut()? = 3;
                    let knots = KnotVector::clamped(2, &breaks, &mults).ok()?;
                    return NurbsCurve::new(knots, points, Some(weights)).ok();
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::ss::{Branch, SurfaceHit, surface_surface};
    use super::*;
    use crate::geom::{Cylinder, Plane, Sphere, Torus};

    fn region() -> Box3 {
        Box3::new(
            Point3::new(-10.0, -10.0, -10.0),
            Point3::new(10.0, 10.0, 10.0),
        )
    }

    fn frame(o: (f64, f64, f64), z: (f64, f64, f64)) -> Frame {
        Frame::new(
            Point3::new(o.0, o.1, o.2),
            Vector3::new(z.0, z.1, z.2),
            Vector3::new(1.0, 0.2, 0.1),
        )
        .unwrap()
    }

    fn plane(o: (f64, f64, f64), z: (f64, f64, f64)) -> Surface {
        Surface::Plane(Plane::new(frame(o, z)))
    }

    fn cylinder(o: (f64, f64, f64), z: (f64, f64, f64), r: f64) -> Surface {
        Surface::Cylinder(Cylinder::new(frame(o, z), r).unwrap())
    }

    fn cone(o: (f64, f64, f64), z: (f64, f64, f64), r: f64, a: f64) -> Surface {
        Surface::Cone(Cone::new(frame(o, z), r, a).unwrap())
    }

    fn sphere(o: (f64, f64, f64), r: f64) -> Surface {
        Surface::Sphere(Sphere::new(frame(o, (0.0, 0.0, 1.0)), r).unwrap())
    }

    fn torus(o: (f64, f64, f64), z: (f64, f64, f64), big: f64, small: f64) -> Surface {
        Surface::Torus(Torus::new(frame(o, z), big, small).unwrap())
    }

    /// The branches and points, after checking that every branch lies on
    /// both surfaces and inside its tolerance.
    fn meet(a: &Surface, b: &Surface) -> (Vec<Branch>, Vec<Point3>) {
        let hit = surface_surface(a, b, &region()).unwrap();
        let SurfaceHit::Meet { branches, points } = hit else {
            panic!("coincident")
        };
        for br in &branches {
            for k in 0..=64 {
                let t = br.range[0] + (br.range[1] - br.range[0]) * f64::from(k) / 64.0;
                let p = br.curve.point(t);
                for s in [a, b] {
                    assert!(
                        s.distance(p) <= br.tolerance,
                        "{br:?} off {s:?} by {}",
                        s.distance(p)
                    );
                }
            }
        }
        for &p in &points {
            assert!(a.distance(p) <= 1e-8 && b.distance(p) <= 1e-8);
        }
        // The same curves either way round (lines may run the other way).
        let SurfaceHit::Meet {
            branches: back,
            points: back_points,
        } = surface_surface(b, a, &region()).unwrap()
        else {
            panic!("coincident one way only")
        };
        assert_eq!(kinds(&back), kinds(&branches));
        assert_eq!(back_points.len(), points.len());
        (branches, points)
    }

    fn kinds(b: &[Branch]) -> Vec<(&'static str, bool)> {
        b.iter()
            .map(|b| {
                let k = match b.curve {
                    Curve::Line(_) => "line",
                    Curve::Circle(_) => "circle",
                    Curve::Ellipse(_) => "ellipse",
                    _ => "nurbs",
                };
                (k, b.tangent)
            })
            .collect()
    }

    #[test]
    fn planes() {
        let (b, _) = meet(
            &plane((0.0, 0.0, 1.0), (0.0, 0.0, 1.0)),
            &plane((2.0, 0.0, 0.0), (1.0, 0.0, 0.3)),
        );
        assert_eq!(kinds(&b), [("line", false)]);
        let (b, _) = meet(
            &plane((0.0, 0.0, 1.0), (0.0, 0.0, 1.0)),
            &plane((0.0, 0.0, 2.0), (0.0, 0.0, 1.0)),
        );
        assert!(b.is_empty());
        assert_eq!(
            surface_surface(
                &plane((0.0, 0.0, 1.0), (0.0, 0.0, 1.0)),
                &plane((3.0, 1.0, 1.0), (0.0, 0.0, -1.0)),
                &region()
            ),
            Ok(SurfaceHit::Coincident { same: false })
        );
    }

    #[test]
    fn plane_and_cylinder() {
        let c = cylinder((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 2.0);
        let (b, _) = meet(&plane((0.0, 0.0, 1.0), (0.0, 0.0, 1.0)), &c);
        assert_eq!(kinds(&b), [("circle", false)]);
        let (b, _) = meet(&plane((0.0, 0.0, 1.0), (0.0, 0.5, 1.0)), &c);
        assert_eq!(kinds(&b), [("ellipse", false)]);
        let (b, _) = meet(&plane((1.0, 0.0, 0.0), (1.0, 0.0, 0.0)), &c);
        assert_eq!(kinds(&b), [("line", false), ("line", false)]);
        let (b, _) = meet(&plane((2.0 + 0.5e-8, 0.0, 0.0), (1.0, 0.0, 0.0)), &c);
        assert_eq!(kinds(&b), [("line", true)]);
        let (b, _) = meet(&plane((2.1, 0.0, 0.0), (1.0, 0.0, 0.0)), &c);
        assert!(b.is_empty());
    }

    #[test]
    fn plane_and_cone() {
        let k = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, FRAC_PI_4 / 1.5);
        let section = |o, z| kinds(&meet(&plane(o, z), &k).0);
        assert_eq!(
            section((0.0, 0.0, 2.0), (0.0, 0.0, 1.0)),
            [("circle", false)]
        );
        assert_eq!(
            section((0.0, 0.0, 2.0), (0.0, 0.3, 1.0)),
            [("ellipse", false)]
        );
        // Steeper than the cone's side: a hyperbola's branch on this nappe.
        assert_eq!(
            section((0.0, 0.0, 2.0), (0.0, 1.0, 0.2)),
            [("nurbs", false)]
        );
        // As steep as the side: a parabola.
        let alpha = FRAC_PI_4 / 1.5;
        let (s, c) = math::sin_cos(alpha);
        assert_eq!(section((0.0, 0.0, 2.0), (0.0, c, -s)), [("nurbs", false)]);
        // Below the apex, nothing; the plane through the apex across the
        // axis touches it there.
        assert!(section((0.0, 0.0, -9.0), (0.0, 0.2, 1.0)).is_empty());
        let apex_v = -1.0 / math::tan(alpha);
        let (b, p) = meet(&plane((0.0, 0.0, apex_v), (0.0, 0.2, 1.0)), &k);
        assert!(b.is_empty() && p.len() == 1);
        // Through the apex and steeper: two rulings.
        assert_eq!(
            section((0.0, 0.0, apex_v), (0.0, 1.0, 0.1)),
            [("line", false), ("line", false)]
        );
    }

    #[test]
    fn a_plane_just_off_the_apex_cuts_a_whole_hyperbola() {
        // Steeper than the side and 4 mm from the apex: the branch runs
        // out to its asymptotes, nearly two rulings, and still comes back
        // whole (property-test case: it was once dropped).
        let k = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, 0.8);
        let apex_v = -1.0 / math::tan(0.8);
        let (b, _) = meet(&plane((0.0, 0.0, apex_v - 0.004), (0.0, 1.0, 0.3)), &k);
        assert_eq!(kinds(&b), [("nurbs", false)]);
        let br = &b[0];
        // It runs up both rulings it nearly is, out to the region's edge.
        let apex = Point3::new(0.0, 0.0, apex_v);
        let far = [br.range[0], br.range[1]].map(|t| br.curve.point(t));
        assert!(far.iter().all(|p| p.distance(apex) > 2.0), "{far:?}");
        assert!(far[0].x * far[1].x < 0.0, "{far:?}");
    }

    #[test]
    fn coaxial_pairs_meet_in_circles() {
        let z = (0.0, 0.0, 1.0);
        let sp = sphere((0.0, 0.0, 1.0), 2.0);
        assert_eq!(
            kinds(&meet(&plane((0.0, 0.0, 2.0), (0.3, 0.1, 1.0)), &sp).0),
            [("circle", false)]
        );
        let (b, p) = meet(&plane((0.0, 0.0, 3.0), z), &sp);
        assert!(b.is_empty() && p.len() == 1);
        assert_eq!(
            kinds(&meet(&cylinder((0.0, 0.0, 0.0), z, 1.0), &sp).0),
            [("circle", false), ("circle", false)]
        );
        assert_eq!(
            kinds(
                &meet(
                    &cone((0.0, 0.0, 0.0), z, 1.0, 0.7),
                    &torus((0.0, 0.0, 1.0), (0.0, 0.0, -1.0), 3.0, 1.0)
                )
                .0
            )
            .len(),
            2
        );
        assert_eq!(
            kinds(&meet(&sphere((0.0, 0.0, 0.0), 2.0), &sphere((1.0, 1.0, 0.0), 2.0)).0),
            [("circle", false)]
        );
        let (b, p) = meet(&sphere((0.0, 0.0, 0.0), 1.0), &sphere((3.0, 0.0, 0.0), 2.0));
        assert!(b.is_empty() && p.len() == 1);
        // A cylinder tangent inside a torus's hole.
        assert_eq!(
            kinds(
                &meet(
                    &cylinder((0.0, 0.0, 0.0), z, 2.0),
                    &torus((0.0, 0.0, 0.0), z, 3.0, 1.0)
                )
                .0
            ),
            [("circle", true)]
        );
    }

    #[test]
    fn cylinders() {
        let z = (0.0, 0.0, 1.0);
        let a = cylinder((0.0, 0.0, 0.0), z, 1.0);
        assert_eq!(
            kinds(&meet(&a, &cylinder((1.5, 0.0, 0.0), z, 1.0)).0),
            [("line", false), ("line", false)]
        );
        assert_eq!(
            kinds(&meet(&a, &cylinder((2.0, 0.0, 0.0), z, 1.0)).0),
            [("line", true)]
        );
        // Equal radii, axes crossing at right angles: the Steinmetz pair.
        assert_eq!(
            kinds(&meet(&a, &cylinder((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), 1.0)).0),
            [("ellipse", false), ("ellipse", false)]
        );
        // Unequal radii have no closed form: traced, two loops.
        assert_eq!(
            kinds(&meet(&a, &cylinder((0.0, 0.0, 0.0), (1.0, 0.0, 0.0), 0.5)).0),
            [("nurbs", false), ("nurbs", false)]
        );
    }

    #[test]
    fn a_plane_through_a_torus_axis_and_cones_sharing_an_apex() {
        let t = torus((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 3.0, 1.0);
        assert_eq!(
            kinds(&meet(&plane((0.0, 0.0, 0.0), (1.0, 1.0, 0.0)), &t).0),
            [("circle", false), ("circle", false)]
        );
        // Cones with one apex, (0, 0, -1), and axes 45° apart: two rulings.
        let a = cone((0.0, 0.0, 0.0), (0.0, 0.0, 1.0), 1.0, FRAC_PI_4);
        let axis = Vector3::new(1.0, 0.0, 1.0).normalize().unwrap().get();
        let o = Point3::new(0.0, 0.0, -1.0) + axis;
        let shared = cone((o.x, o.y, o.z), (1.0, 0.0, 1.0), 1.0, FRAC_PI_4);
        assert_eq!(
            kinds(&meet(&a, &shared).0),
            [("line", false), ("line", false)]
        );
    }
}
