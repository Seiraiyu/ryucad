//! Swept surfaces that keep their generating curve: an extrusion moves a
//! planar curve along a direction, a revolution turns a curve about an
//! axis in its plane.

use super::bound2::CurveBound;
use super::bound2::SLACK;
use super::{
    Box3, Curve, Frame, NurbsCurve, Point3, Projection, Reparam, Rigid, SurfaceBound, SurfaceEval,
    UnitVector3, Vector3, polar_angle,
};
use crate::Error;
use crate::math;
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION};
use core::f64::consts::TAU;

/// `S(u, v) = C(u) + v d`: a planar curve `C` moved along a unit direction
/// `d` that is not in the curve's plane.
///
/// `u` is the curve's parameter (periodic when the curve is) and `v` the
/// distance along `d`, unbounded like a cylinder's height. The natural
/// normal is along `C'(u) × d`, so a circle extruded along its frame's
/// `z` has a cylinder's outward normal.
///
/// Projection reduces to the curve: the best `v` for each `u` is
/// `(p - C(u)) · d`, so projecting onto the surface is projecting onto the
/// curve as seen along `d`. That view is built once, at construction; it
/// takes no part in equality.
#[derive(Clone, Debug)]
pub struct Extrusion {
    curve: Curve,
    dir: UnitVector3,
    normal: UnitVector3,
    /// `|d · n|`: the sine of the angle between `d` and the curve's plane.
    lean: f64,
    flat: Flat,
}

/// The curve as seen along the direction, for projection.
#[derive(Clone, Debug)]
enum Flat {
    /// The direction is the plane's normal, bit for bit: a point moved
    /// along it into the plane through `origin` projects onto the curve
    /// itself.
    Along { origin: Point3 },
    /// The curve's exact NURBS form with every control point moved along
    /// the direction onto the plane through `origin` perpendicular to it,
    /// and stored relative to `origin`; `reparam` relates its parameter to
    /// the curve's.
    Nurbs {
        origin: Point3,
        curve: NurbsCurve,
        reparam: Reparam,
    },
}

impl PartialEq for Extrusion {
    fn eq(&self, other: &Self) -> bool {
        (&self.curve, self.dir) == (&other.curve, other.dir)
    }
}

/// `v / |v|`, for a vector known not to vanish whatever its units (an
/// area, a cross product of unit vectors); `None` if it does.
fn direction(v: Vector3) -> Option<UnitVector3> {
    let len = v.length();
    (len > 0.0 && len.is_finite()).then(|| {
        let u = v / len;
        // Rounding can leave a few ulps; renormalizing once more is exact
        // enough for `new_unchecked`.
        UnitVector3::new_unchecked(u / u.length())
    })
}

/// The mean of some points.
fn centroid(points: &[Point3]) -> Point3 {
    let sum = points.iter().fold(Vector3::ZERO, |s, &p| s + p.to_vector());
    Point3::ORIGIN + sum / points.len() as f64
}

/// The plane of a NURBS's control points, as its unit normal and the
/// control points' centroid.
///
/// Two candidate normals: Newell's, over the closed control polygon, and
/// the cross product of the point farthest from the centroid with the
/// point farthest from the line through both. Newell's fits best when the
/// polygon encloses area; the second survives polygons whose signed area
/// cancels (an S-shaped curve). The candidate with the smaller misfit wins
/// (Newell's on a tie). Fails with `Degenerate` when every control point
/// is within [`LINEAR_RESOLUTION`] of one line, or some control point is
/// farther than that from the chosen plane.
fn nurbs_plane(n: &NurbsCurve) -> Result<(UnitVector3, Point3), Error> {
    let points: Vec<Point3> = n.control_points().collect();
    let o = centroid(&points);
    let rel: Vec<Vector3> = points.iter().map(|&p| p - o).collect();
    let far = |from: &dyn Fn(Vector3) -> f64| {
        rel.iter()
            .copied()
            .fold((Vector3::ZERO, 0.0), |(best, d), r| {
                let dr = from(r);
                if dr > d { (r, dr) } else { (best, d) }
            })
    };
    let (a, reach) = far(&|r: Vector3| r.length());
    if reach <= LINEAR_RESOLUTION {
        return Err(Error::Degenerate);
    }
    let axis = a / reach;
    let (b, off) = far(&|r: Vector3| (r - axis * r.dot(axis)).length());
    if off <= LINEAR_RESOLUTION {
        return Err(Error::Degenerate);
    }
    let mut newell = Vector3::ZERO;
    for (i, &p) in rel.iter().enumerate() {
        let q = rel[(i + 1) % rel.len()];
        newell += Vector3::new(
            (p.y - q.y) * (p.z + q.z),
            (p.z - q.z) * (p.x + q.x),
            (p.x - q.x) * (p.y + q.y),
        );
    }
    let misfit = |n: UnitVector3| rel.iter().map(|r| r.dot(n.get()).abs()).fold(0.0, f64::max);
    let best = [direction(newell), direction(a.cross(b))]
        .into_iter()
        .flatten()
        .map(|n| (n, misfit(n)))
        .reduce(|x, y| if y.1 < x.1 { y } else { x })
        .ok_or(Error::Degenerate)?;
    if best.1 > LINEAR_RESOLUTION {
        return Err(Error::Degenerate);
    }
    Ok((best.0, o))
}

/// The unit normal of a curve's plane and a point of that plane. Fails
/// with `Degenerate` for a line, or a NURBS that is straight or not
/// planar (see [`nurbs_plane`]).
fn curve_plane(curve: &Curve) -> Result<(UnitVector3, Point3), Error> {
    match curve {
        Curve::Line(_) => Err(Error::Degenerate),
        Curve::Circle(c) => Ok((c.frame().z(), c.frame().origin())),
        Curve::Ellipse(e) => Ok((e.frame().z(), e.frame().origin())),
        Curve::Nurbs(n) => nurbs_plane(n),
    }
}

/// `r` with its component along `d` removed.
fn across(r: Vector3, d: Vector3) -> Vector3 {
    r - d * r.dot(d)
}

fn same_bits(a: UnitVector3, b: UnitVector3) -> bool {
    let (a, b) = (a.get(), b.get());
    [(a.x, b.x), (a.y, b.y), (a.z, b.z)]
        .iter()
        .all(|(x, y)| x.to_bits() == y.to_bits())
}

/// The rotation of a rigid transform alone. Composing with the reverse
/// translation is exact: the identity matrix multiplies exactly, and
/// `t + -t` is zero.
fn rotation_of(r: &Rigid) -> Rigid {
    Rigid::translation(-r.translation_part()).then_after(r)
}

impl Extrusion {
    /// The curve moved along `dir`, which is normalized.
    ///
    /// Fails with `NonFinite` for a NaN or infinite direction, or
    /// `Degenerate`: for a zero direction; for a line (use a plane); for a
    /// curve that is not planar (a NURBS whose control points are within
    /// [`LINEAR_RESOLUTION`] of one line, or not all within it of one
    /// plane); and for a direction within [`ANGULAR_RESOLUTION`] of the
    /// curve's plane (`|d · n| <= sin(ANGULAR_RESOLUTION)`). Fails with
    /// `OutsideSizeBox` in the rare case that the curve seen along `d`,
    /// centred on its control points' centroid, leaves the size box.
    pub fn new(curve: Curve, dir: Vector3) -> Result<Self, Error> {
        Vector3::try_new(dir.x, dir.y, dir.z)?;
        let d = dir.normalize().ok_or(Error::Degenerate)?;
        let (normal, origin) = curve_plane(&curve)?;
        let lean = d.dot(normal).abs();
        if lean <= math::sin(ANGULAR_RESOLUTION) {
            return Err(Error::Degenerate);
        }
        let flat = if same_bits(d, normal) || same_bits(d, -normal) {
            Flat::Along { origin }
        } else {
            let (n, reparam) = curve.to_nurbs(curve.domain())?;
            let points: Vec<Point3> = n.control_points().collect();
            let origin = centroid(&points);
            let flat = points
                .iter()
                .map(|&q| Point3::ORIGIN + across(q - origin, d.get()))
                .collect();
            Flat::Nurbs {
                origin,
                curve: NurbsCurve::new(n.knots().clone(), flat, Some(n.weights().to_vec()))?,
                reparam,
            }
        };
        Ok(Self {
            curve,
            dir: d,
            normal,
            lean,
            flat,
        })
    }

    pub fn curve(&self) -> &Curve {
        &self.curve
    }

    pub fn dir(&self) -> UnitVector3 {
        self.dir
    }

    /// The unit normal of the curve's plane (for a circle or an ellipse,
    /// its frame's `z`).
    pub fn normal(&self) -> UnitVector3 {
        self.normal
    }

    pub(super) fn point(&self, u: f64, v: f64) -> Point3 {
        self.curve.point(u) + self.dir.get() * v
    }

    pub(super) fn eval(&self, u: f64, v: f64) -> SurfaceEval {
        let c = self.curve.eval(u);
        let d = self.dir.get();
        SurfaceEval {
            point: c.point + d * v,
            du: c.d1,
            dv: d,
            duu: c.d2,
            duv: Vector3::ZERO,
            dvv: Vector3::ZERO,
        }
    }

    /// The direction of `T × d`, with `T` the curve's unit tangent; `None`
    /// where that vanishes. `|T × d| >= |d · n| > 0` for a tangent in the
    /// plane, so it never vanishes for a nonzero tangent.
    pub(super) fn surface_normal(&self, u: f64) -> Option<UnitVector3> {
        direction(self.curve.tangent(u)?.get().cross(self.dir.get()))
    }

    pub(super) fn domain(&self) -> [[f64; 2]; 2] {
        [self.curve.domain(), [f64::NEG_INFINITY, f64::INFINITY]]
    }

    /// The nearest point: the curve seen along `d` projected on (globally,
    /// as [`Curve::project`] or [`NurbsCurve::project`] does), then the
    /// best `v` for that `u`. A periodic `u` is in `[start, start +
    /// period)`.
    pub(super) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        let d = self.dir.get();
        let u = match &self.flat {
            Flat::Along { origin } => self.curve.project(p - d * (p - *origin).dot(d)).param,
            Flat::Nurbs {
                origin,
                curve,
                reparam,
            } => {
                let s = curve.project(Point3::ORIGIN + across(p - *origin, d)).param;
                let u = reparam.from_nurbs(s);
                match self.curve.period() {
                    Some(period) if u >= self.curve.domain()[0] + period => u - period,
                    _ => u,
                }
            }
        };
        let v = (p - self.curve.point(u)).dot(d);
        let point = self.point(u, v);
        Projection {
            param: (u, v),
            point,
            distance: point.distance(p),
        }
    }

    /// `Suu = C''` and the other second derivatives vanish. The unit
    /// normal `N = T × d / |T × d|` turns at most `|T'| / |T × d|`, and
    /// `|T × d| >= |d · n|`, so it turns along `u` at most the curve's
    /// turning rate over `|d · n|`.
    pub(super) fn derivative_bound(&self, [ru, _]: [[f64; 2]; 2]) -> SurfaceBound {
        let c = piecewise_bound(&self.curve, ru);
        SurfaceBound {
            duu: c.d2,
            duv: 0.0,
            dvv: 0.0,
            turn_u: c.turn / self.lean * SLACK,
            turn_v: 0.0,
        }
    }

    /// The extrusion moved by a rigid transform; parameters are unchanged.
    /// The view along the direction is turned with it rather than built
    /// again, so this never fails.
    pub(super) fn transformed(&self, r: &Rigid) -> Self {
        let flat = match &self.flat {
            Flat::Along { origin } => Flat::Along {
                origin: r.apply_point(*origin),
            },
            Flat::Nurbs {
                origin,
                curve,
                reparam,
            } => Flat::Nurbs {
                origin: r.apply_point(*origin),
                curve: curve.transformed(&rotation_of(r)),
                reparam: *reparam,
            },
        };
        let (dir, normal) = (r.apply_unit(self.dir), r.apply_unit(self.normal));
        Self {
            curve: self.curve.transformed(r),
            dir,
            normal,
            lean: dir.dot(normal).abs(),
            flat,
        }
    }
}

/// `S(u, v) = o + ρ(v) radial(u) + h(v) z`: a curve in the half-plane
/// `y = 0, x >= 0` of a frame, turned about the frame's `z` axis through
/// its origin `o`, with `ρ(v) = (C(v) - o) · x` and `h(v) = (C(v) - o) ·
/// z`.
///
/// `u` is the angle from the frame's `x`, counterclockwise about `z`
/// (period 2π, as [`Frame::radial`]); `v` is the curve's parameter. The
/// natural normal is the direction of `h' radial(u) - ρ' z`: along
/// `du × dv` where the curve is off the axis, and its limit where it
/// meets it, so it agrees with a cone's or a sphere's.
///
/// The curve touches the axis only at a pole: an end of a clamped curve's
/// domain within [`LINEAR_RESOLUTION`] of the axis is a collapsed row of
/// the surface. Every other point of the curve is certified farther than
/// [`LINEAR_RESOLUTION`] from it (see [`Revolution::new`]).
#[derive(Clone, Debug)]
pub struct Revolution {
    frame: Frame,
    curve: Curve,
    /// Whether each end of the curve's domain is on the axis.
    poles: [bool; 2],
    bounds: Box3,
}

impl PartialEq for Revolution {
    fn eq(&self, other: &Self) -> bool {
        (&self.frame, &self.curve) == (&other.frame, &other.curve)
    }
}

/// Subdivisions allowed while certifying that a Bézier piece keeps clear
/// of the axis.
const AXIS_DEPTH: u32 = 32;

/// A Bézier control value of the distance from the axis, `ρ`, and its
/// weight.
type RhoW = (f64, f64);

/// True when a rational Bézier piece, as `(ρ, w)` control values, is
/// certified farther than [`LINEAR_RESOLUTION`] from the axis everywhere
/// except where it leaves the axis at an exempt end (`exempt[k]`: the end
/// is a pole). With positive weights `ρ` is a convex combination of the
/// control values, so the piece is clear when every control value but an
/// exempt end's is. Otherwise it is halved, up to [`AXIS_DEPTH`] times: a
/// piece end on the curve within resolution is a touch, and so is a piece
/// still uncertain at the limit. A piece exempt at both ends (a curve of
/// one piece between two poles) is halved first, so that its middle is
/// tested.
fn clear_of_axis(pts: &[RhoW], exempt: [bool; 2], depth: u32) -> bool {
    let last = pts.len() - 1;
    let both = exempt == [true, true];
    if !both {
        let free = |i: usize| (i == 0 && exempt[0]) || (i == last && exempt[1]);
        if (0..=last).all(|i| free(i) || pts[i].0 > LINEAR_RESOLUTION) {
            return true;
        }
        if (!exempt[0] && pts[0].0 <= LINEAR_RESOLUTION)
            || (!exempt[1] && pts[last].0 <= LINEAR_RESOLUTION)
        {
            return false;
        }
    }
    if depth == 0 {
        return false;
    }
    let (a, b) = halve(pts);
    clear_of_axis(&a, [exempt[0], false], depth - 1)
        && clear_of_axis(&b, [false, exempt[1]], depth - 1)
}

/// De Casteljau's split at the middle, in homogeneous form.
fn halve(pts: &[RhoW]) -> (Vec<RhoW>, Vec<RhoW>) {
    let mut row: Vec<RhoW> = pts.iter().map(|&(r, w)| (r * w, w)).collect();
    let mut left = vec![row[0]];
    let mut right = vec![row[row.len() - 1]];
    while row.len() > 1 {
        row = row
            .windows(2)
            .map(|p| ((p[0].0 + p[1].0) / 2.0, (p[0].1 + p[1].1) / 2.0))
            .collect();
        left.push(row[0]);
        right.push(row[row.len() - 1]);
    }
    right.reverse();
    let euclid = |v: Vec<RhoW>| v.into_iter().map(|(rw, w)| (rw / w, w)).collect();
    (euclid(left), euclid(right))
}

/// Checks that a circle or an ellipse with semi-axes `a` along its frame's
/// `x` and `b` along its `y` lies in `frame`'s xz plane within
/// [`LINEAR_RESOLUTION`] and is everywhere farther than that from the
/// axis, in closed form: along a unit vector `n` the conic's points reach
/// `c · n ± hypot(a x_c · n, b y_c · n)`.
fn conic_off_axis(frame: &Frame, conic: &Frame, a: f64, b: f64) -> Result<(), Error> {
    let c = frame.to_local(conic.origin());
    let reach = |n: Vector3| math::hypot(a * conic.x().get().dot(n), b * conic.y().get().dot(n));
    let off_plane = c.y.abs() + reach(frame.y().get());
    let nearest = c.x - reach(frame.x().get());
    if off_plane > LINEAR_RESOLUTION || nearest <= LINEAR_RESOLUTION {
        return Err(Error::Degenerate);
    }
    Ok(())
}

/// Checks a NURBS against `frame` as [`Revolution::new`] describes, and
/// returns which ends of its domain are poles.
fn nurbs_off_axis(frame: &Frame, n: &NurbsCurve) -> Result<[bool; 2], Error> {
    let local: Vec<Vector3> = n.control_points().map(|p| frame.to_local(p)).collect();
    if local
        .iter()
        .any(|q| q.y.abs() > LINEAR_RESOLUTION || q.x < -LINEAR_RESOLUTION)
    {
        return Err(Error::Degenerate);
    }
    let poles = match n.period() {
        Some(_) => [false, false],
        None => [
            local[0].x <= LINEAR_RESOLUTION,
            local[local.len() - 1].x <= LINEAR_RESOLUTION,
        ],
    };
    let (o, x) = (frame.origin().to_vector(), frame.x().get());
    let pieces = n.pieces();
    let last = pieces.len() - 1;
    for (i, piece) in pieces.iter().enumerate() {
        let pts: Vec<RhoW> = piece.pts.iter().map(|h| ((h.p - o).dot(x), h.w)).collect();
        let exempt = [i == 0 && poles[0], i == last && poles[1]];
        if !clear_of_axis(&pts, exempt, AXIS_DEPTH) {
            return Err(Error::Degenerate);
        }
    }
    Ok(poles)
}

/// The box of the solid of revolution of `b` about `frame`'s axis: the
/// cylinder about the axis through `b`'s farthest corner, over the
/// heights of its corners. Distance from the axis is convex and height
/// linear, so over a box both are greatest (and height least) at a
/// corner.
fn revolved_box(frame: &Frame, b: &Box3) -> Box3 {
    let (mut reach, mut lo, mut hi) = (0.0f64, f64::INFINITY, f64::NEG_INFINITY);
    for p in b.corners() {
        let q = frame.to_local(p);
        reach = reach.max(math::sqrt(q.x * q.x + q.y * q.y));
        lo = lo.min(q.z);
        hi = hi.max(q.z);
    }
    let z = frame.z().get();
    let across = |zi: f64| reach * math::sqrt((1.0 - zi * zi).max(0.0));
    let d = Vector3::new(across(z.x), across(z.y), across(z.z));
    let (a, c) = (frame.origin() + z * lo, frame.origin() + z * hi);
    Box3::new(a.min(c) - d, a.max(c) + d)
}

impl Revolution {
    /// The curve turned about `frame`'s `z` axis. The curve must lie in
    /// the frame's xz half-plane with `x >= 0`.
    ///
    /// Fails with `Degenerate`: for a line (use a plane, a cylinder or a
    /// cone); for a curve farther than [`LINEAR_RESOLUTION`] from the xz
    /// plane, or reaching below `x = -LINEAR_RESOLUTION` (a NURBS: any
    /// control point; a circle or an ellipse: in closed form); and for a
    /// curve that touches the axis inside its domain. The rule: only an
    /// end of a clamped curve's domain may be within
    /// [`LINEAR_RESOLUTION`] of the axis (it becomes a pole), and the rest
    /// of the curve must be certified farther away, where it is not
    /// leaving such an end. A circle or an ellipse has no ends, so its
    /// nearest point (in closed form) must be farther. A NURBS is certified
    /// on its Bézier pieces: every control point but a pole's must be
    /// farther, or the piece is halved and tried again, up to 32 times;
    /// a piece end within resolution, or a piece still uncertain at the
    /// limit, is a touch. So is a curve that leaves a pole tangent to the
    /// axis, which never certifies.
    pub fn new(frame: Frame, curve: Curve) -> Result<Self, Error> {
        let poles = match &curve {
            Curve::Line(_) => return Err(Error::Degenerate),
            Curve::Circle(c) => {
                conic_off_axis(&frame, c.frame(), c.radius(), c.radius())?;
                [false, false]
            }
            Curve::Ellipse(e) => {
                conic_off_axis(&frame, e.frame(), e.major(), e.minor())?;
                [false, false]
            }
            Curve::Nurbs(n) => nurbs_off_axis(&frame, n)?,
        };
        let bounds = revolved_box(&frame, &curve.bounds(curve.domain()));
        Ok(Self {
            frame,
            curve,
            poles,
            bounds,
        })
    }

    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    pub fn curve(&self) -> &Curve {
        &self.curve
    }

    /// Whether each end of the curve's domain lies on the axis: those `v`
    /// rows collapse to a point. Never for a periodic curve.
    pub fn poles(&self) -> [bool; 2] {
        self.poles
    }

    /// 1 when the curve is periodic (the surface closes up like a torus),
    /// 0 when it is clamped with both ends on the axis (like a sphere),
    /// otherwise `None`.
    pub(super) fn closed_genus(&self) -> Option<u32> {
        if self.curve.period().is_some() {
            Some(1)
        } else if self.poles == [true, true] {
            Some(0)
        } else {
            None
        }
    }

    pub(super) fn bounds(&self) -> Box3 {
        self.bounds
    }

    /// `(ρ, h)` of a point.
    fn meridian(&self, p: Point3) -> (f64, f64) {
        let r = p - self.frame.origin();
        (r.dot(self.frame.x().get()), r.dot(self.frame.z().get()))
    }

    pub(super) fn point(&self, u: f64, v: f64) -> Point3 {
        let (rho, h) = self.meridian(self.curve.point(v));
        self.frame.origin() + self.frame.radial(u) * rho + self.frame.z().get() * h
    }

    pub(super) fn eval(&self, u: f64, v: f64) -> SurfaceEval {
        let c = self.curve.eval(v);
        let (x, z) = (self.frame.x().get(), self.frame.z().get());
        let (rho, h) = self.meridian(c.point);
        let (d1r, d1h) = (c.d1.dot(x), c.d1.dot(z));
        let (d2r, d2h) = (c.d2.dot(x), c.d2.dot(z));
        let (radial, turn) = self.frame.radial_and_turn(u);
        SurfaceEval {
            point: self.frame.origin() + radial * rho + z * h,
            du: turn * rho,
            dv: radial * d1r + z * d1h,
            duu: radial * -rho,
            duv: turn * d1r,
            dvv: radial * d2r + z * d2h,
        }
    }

    /// The direction of `h' radial(u) - ρ' z`, from the curve's unit
    /// tangent; `None` where the curve has none.
    pub(super) fn surface_normal(&self, u: f64, v: f64) -> Option<UnitVector3> {
        let t = self.curve.tangent(v)?.get();
        let z = self.frame.z().get();
        direction(self.frame.radial(u) * t.dot(z) - z * t.dot(self.frame.x().get()))
    }

    pub(super) fn domain(&self) -> [[f64; 2]; 2] {
        [[0.0, TAU], self.curve.domain()]
    }

    /// The nearest point. A point turned into the frame's xz half-plane
    /// (by its angle about the axis, 0 on the axis) projects onto the
    /// curve, globally, as [`Curve::project`] does; `u` is that angle, in
    /// `[0, 2π)`. For `ρ >= 0` no other angle is nearer: at an angle `θ`
    /// away the squared distance grows by `2 ρ ρ_p (1 - cos θ)`.
    pub(super) fn project(&self, p: Point3) -> Projection<(f64, f64), Point3> {
        let q = self.frame.to_local(p);
        let u = polar_angle(q.y, q.x);
        let rho = math::sqrt(q.x * q.x + q.y * q.y);
        let v = self.curve.project(self.frame.to_world(rho, 0.0, q.z)).param;
        let point = self.point(u, v);
        Projection {
            param: (u, v),
            point,
            distance: point.distance(p),
        }
    }

    /// A bound on `|C'|` over `[a, b]`: on each stretch between the
    /// curve's knots, where `C'` is continuous (it may jump at a knot of
    /// multiplicity `degree`), its value at the middle plus the bound on
    /// `|C''|` times the half-width.
    fn speed_bound(&self, range: [f64; 2]) -> f64 {
        cuts(&self.curve, range)
            .windows(2)
            .map(|w| {
                let c = self.curve.derivative_bound([w[0], w[1]]);
                let mid = (w[0] + w[1]) / 2.0;
                self.curve.eval(mid).d1.length() + c.d2 * (w[1] - w[0]) / 2.0
            })
            .fold(0.0, f64::max)
    }

    /// `Suu = -ρ radial` is bounded by the farthest reach of the curve's
    /// bounds from the axis; `Suv = ρ' turn` by a bound on `|C'|` (see
    /// `speed_bound`); `Svv = ρ'' radial + h'' z` by `|C''|`. The normal
    /// turns at most 1 per radian of `u` (`|dN/du| = |h'| / |C'|`) and
    /// along `v` as the curve's tangent does. A clamped curve's range is
    /// cut to its domain first.
    pub(super) fn derivative_bound(&self, [_, rv]: [[f64; 2]; 2]) -> SurfaceBound {
        let rv = match self.curve.period() {
            Some(_) => rv,
            None => {
                let [d0, d1] = self.curve.domain();
                [rv[0].clamp(d0, d1), rv[1].clamp(d0, d1)]
            }
        };
        let c = piecewise_bound(&self.curve, rv);
        let b = self.curve.bounds(rv);
        let reach = b
            .corners()
            .iter()
            .map(|&p| {
                let q = self.frame.to_local(p);
                math::sqrt(q.x * q.x + q.y * q.y)
            })
            .fold(0.0, f64::max);
        SurfaceBound {
            duu: reach * SLACK,
            duv: self.speed_bound(rv) * SLACK,
            dvv: c.d2,
            turn_u: SLACK,
            turn_v: c.turn,
        }
    }

    /// The revolution moved by a rigid transform; parameters are
    /// unchanged.
    pub(super) fn transformed(&self, r: &Rigid) -> Self {
        let (frame, curve) = (self.frame.transformed(r), self.curve.transformed(r));
        let bounds = revolved_box(&frame, &curve.bounds(curve.domain()));
        Self {
            frame,
            curve,
            poles: self.poles,
            bounds,
        }
    }
}

/// The parameters a curve's range is cut at for its bounds: its ends, the
/// curve's knots inside it (moved by whole periods on a periodic curve),
/// and each stretch between them in quarters.
fn cuts(curve: &Curve, [a, b]: [f64; 2]) -> Vec<f64> {
    let mut knots = vec![a];
    if let Curve::Nurbs(n) = curve {
        let ks = n.knots().knots();
        let (k0, period) = (ks[0], n.period().unwrap_or(0.0));
        let turns = if period > 0.0 {
            let first = ((a - k0) / period).floor() as i64;
            first..=((b - k0) / period).ceil() as i64
        } else {
            0..=0
        };
        for q in turns {
            knots.extend(
                ks.iter()
                    .map(|&k| k + q as f64 * period)
                    .filter(|&t| t > a && t < b),
            );
        }
    }
    knots.push(b);
    knots.sort_by(f64::total_cmp);
    let mut out = vec![a];
    for w in knots.windows(2) {
        for i in 1..=QUARTERS {
            out.push(w[0] + (w[1] - w[0]) * i as f64 / QUARTERS as f64);
        }
    }
    out
}

/// How many pieces each knot span is cut into for bounds.
const QUARTERS: usize = 4;

/// A curve's bounds over a range as the worst of its bounds over the
/// pieces `cuts` gives: still certified, and far tighter where one box
/// over a whole Bézier piece would let the derivative's interval reach
/// zero (a quarter circle's derivative turns through a right angle) and
/// the turning rate blow up.
fn piecewise_bound(curve: &Curve, range: [f64; 2]) -> CurveBound {
    if !(range[0].is_finite() && range[1].is_finite()) || range[0] >= range[1] {
        return curve.derivative_bound(range);
    }
    cuts(curve, range)
        .windows(2)
        .map(|w| curve.derivative_bound([w[0], w[1]]))
        .fold(CurveBound { d2: 0.0, turn: 0.0 }, |x, y| CurveBound {
            d2: x.d2.max(y.d2),
            turn: x.turn.max(y.turn),
        })
}

#[cfg(test)]
mod tests;
