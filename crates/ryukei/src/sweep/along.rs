//! Sweeps along a path: a profile carried along a chain of curves.
//!
//! Each path segment is a leg. A leg along a line with no twist or scale
//! moves the profile as an extrude does, and one round a circle whose axis
//! lies in the section's plane as a revolve does: their sides are the
//! analytic or swept surfaces those make, their laterals lines and arcs.
//! Any other leg carries the profile in the path's moving frame: each
//! side is a [`Sweep`], each lateral fitted along the side's edge in its
//! parameters (certified, its bound the edge's tolerance). Legs meet at
//! sections: the profile moved rigidly to where the frame has taken it
//! and scaled, exact curves both legs' sides share. The body is built as a
//! capped sweep in one stretch per leg (`build::capped`).

use super::build::{self, Keys, LoopBuilt, LoopPlan, LumpBuilt, LumpPlan, Ring, SegmentPlan};
use super::build::{Span, VertexPlan};
use super::capped::{self, Motion};
use super::extrude::Straight;
use super::names::Namer;
use super::path::{PathSegment, ValidPath};
use super::revolve::Turn;
use super::{Lump, Path, Segment, SweepError, ValidLoop, ValidProfile, place};
use crate::geom::path_sweep::Carry;
use crate::geom::{
    Axis, Circle, Curve, Curve2, Ellipse, Frame, FrameLaw, Line, Point2, Point3, Rigid, Surface,
    Sweep, Vector3,
};
use crate::name::{Journal, Name, OpKey, SweepRole};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION, Tolerance};
use crate::topo::{Body, EntityId, FaceId, Sense};
use crate::{Error, check};
use std::sync::Arc;

/// How a sweep along a path carries its profile: by `law`, turning by
/// `twist` radians about the path from start to end and growing by
/// `scale` (end size over start), each spread over the path in proportion
/// to its length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlongOptions {
    pub law: FrameLaw,
    pub twist: f64,
    pub scale: f64,
}

impl Default for AlongOptions {
    /// Rotation-minimizing, no twist, no scale.
    fn default() -> Self {
        Self {
            law: FrameLaw::Minimal,
            twist: 0.0,
            scale: 1.0,
        }
    }
}

/// Sweeps `profile` along `path`: one solid per lump of the profile.
///
/// The profile is carried in a frame that starts as its own frame and
/// moves along the path as `opts.law` says (see [`FrameLaw`]); its plane
/// must be more than [`ANGULAR_RESOLUTION`] from the path's start tangent
/// (`ProfileAlongPath`), and it is scaled about the point where the
/// path's start meets its plane along its normal. Refusals, in order: the
/// path's (`EmptyPath`, `BadPathSegment`, `PathNotG1`); a twist that isn't
/// finite (`NonFinite`); a scale that isn't positive (`BadScale`); a
/// closed path, which comes with the next commit (`NotClosing`); then
/// `ProfileAlongPath`; then for each path segment in order a frame law
/// undefined along it (`FrameUndefined`) or a profile that would fold
/// (`PathTooCurved`); then, once the body is built, two faces that meet
/// away from their shared edges (`SelfIntersecting`) or whose meeting
/// can't be certified (`Uncertain`), the first pair in face order.
///
/// Faces: a side per profile segment and path segment, a cap at each end.
/// Names come from `key` and the caller's keys ([`SweepRole`]): a side
/// from its profile segment and its path segment, a section between two
/// legs' sides from both sides, a lateral from the profile segments
/// either side of its vertex and its path segment, a vertex at a joint
/// from those and the path segments either side; caps, cap edges and
/// end vertices as an extrude's. Editing the path's shape, the twist or
/// the scale renames nothing.
pub fn along(
    key: OpKey,
    profile: &ValidProfile,
    path: &Path,
    opts: &AlongOptions,
) -> Result<(Body, Journal), Error> {
    let path = ValidPath::new(path)?;
    if !opts.twist.is_finite() {
        return Err(Error::NonFinite);
    }
    if !(opts.scale.is_finite() && opts.scale > 0.0) {
        return Err(SweepError::BadScale.into());
    }
    if path.closed {
        return Err(SweepError::NotClosing.into());
    }
    let first = &path.segments[0];
    let t0 = first
        .curve
        .eval(first.range[0])
        .d1
        .normalize()
        .ok_or(Error::Degenerate)?;
    let frame = profile.frame();
    let along = frame.z().dot(t0);
    if along.abs() <= ANGULAR_RESOLUTION {
        return Err(SweepError::ProfileAlongPath.into());
    }
    // The scale's centre: where the path's start meets the profile's
    // plane along its normal.
    let c0 = first.curve.point(first.range[0]);
    let o = c0 - frame.z().get() * frame.to_local(c0).z;
    let legs = legs(profile, &path, opts, o)?;
    let mut b = Body::new();
    let mut keys = Keys::new(key);
    let namer = Namer::new("along", key);
    let carried = Carried {
        frame,
        o,
        sign: along.signum(),
        legs: &legs,
        law: opts.law,
    };
    for lump in profile.lumps() {
        carried.sweep(&mut b, &mut keys, &namer, lump)?;
    }
    let journal = namer.finish(&mut b)?;
    self_check(&b)?;
    Ok((b, journal))
}

/// Refuses a body whose faces meet away from their shared edges, naming
/// the first pair in face order.
fn self_check(b: &Body) -> Result<(), Error> {
    let names = |a: FaceId, c: FaceId| {
        let (x, y) = (b.faces[a].name, b.faces[c].name);
        if x <= y { [x, y] } else { [y, x] }
    };
    match check::check_intersections(b).first() {
        None => Ok(()),
        Some(check::Fault::FacesCross { a, b: c } | check::Fault::FacesOverlap { a, b: c }) => {
            Err(SweepError::SelfIntersecting {
                faces: names(*a, *c),
            }
            .into())
        }
        Some(check::Fault::CannotCheck { a, b: c }) => Err(SweepError::Uncertain {
            faces: names(*a, *c),
        }
        .into()),
        Some(_) => Err(Error::Degenerate),
    }
}

/// Where the profile is at a section: moved by `rigid` from where it
/// lies, then scaled by `sigma` about the centre's place, in a frame
/// whose axes are `axes` (twist included).
#[derive(Clone, Copy, Debug)]
struct Joint {
    rigid: Rigid,
    sigma: f64,
    axes: [Vector3; 3],
}

impl Joint {
    fn centre(&self, o: Point3) -> Point3 {
        self.rigid.apply_point(o)
    }

    /// A point of the profile where it lies, at this section.
    fn point(&self, o: Point3, p: Point3) -> Point3 {
        let c = self.centre(o);
        c + (self.rigid.apply_point(p) - c) * self.sigma
    }

    /// A profile segment over `range` placed at this section.
    fn span(
        &self,
        o: Point3,
        frame: &Frame,
        c: &Curve2,
        range: [f64; 2],
    ) -> Result<(Curve, [f64; 2]), Error> {
        let placed = place::span(frame, c, range, &self.rigid)?;
        scaled(placed, self.centre(o), self.sigma)
    }
}

/// A placed curve and its interval scaled by `k` about `c`: a line's
/// parameter is its length, so its interval scales too; a conic's or a
/// spline's parameter is kept.
fn scaled(
    (curve, [a, b]): (Curve, [f64; 2]),
    c: Point3,
    k: f64,
) -> Result<(Curve, [f64; 2]), Error> {
    if k == 1.0 {
        return Ok((curve, [a, b]));
    }
    let at = |p: Point3| c + (p - c) * k;
    Ok(match curve {
        Curve::Line(l) => (
            Curve::Line(Line::from_unit(at(l.origin()), l.dir())?),
            [a * k, b * k],
        ),
        Curve::Circle(ci) => {
            let f = ci.frame();
            let f = Frame::from_axes(at(f.origin()), f.z(), f.x())?;
            (Curve::Circle(Circle::new(f, ci.radius() * k)?), [a, b])
        }
        Curve::Ellipse(e) => {
            let f = e.frame();
            let f = Frame::from_axes(at(f.origin()), f.z(), f.x())?;
            (
                Curve::Ellipse(Ellipse::new(f, e.major() * k, e.minor() * k)?),
                [a, b],
            )
        }
        Curve::Nurbs(n) => (Curve::Nurbs(Arc::new(n.mapped(at))), [a, b]),
    })
}

/// How a leg moves the profile.
enum Kind {
    Straight(Straight),
    Turn(Turn),
    Carried,
}

/// One path segment's stretch of the sweep.
struct Leg {
    segment: PathSegment,
    kind: Kind,
    start: Joint,
    end: Joint,
    /// The leg's twist, and its end's scale over its start's.
    twist: f64,
    ratio: f64,
}

/// The rotation taking the frame `from` to `to`, as a matrix: `Σⱼ toⱼ
/// fromⱼᵀ`.
fn rotation(from: &[Vector3; 3], to: &[Vector3; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for (f, t) in from.iter().zip(to) {
        let (f, t) = (f.to_array(), t.to_array());
        for (r, row) in m.iter_mut().enumerate() {
            for (c, x) in row.iter_mut().enumerate() {
                *x += t[r] * f[c];
            }
        }
    }
    m
}

fn rotate(m: &[[f64; 3]; 3], v: Vector3) -> Vector3 {
    let v = v.to_array();
    let row = |r: usize| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2];
    Vector3::new(row(0), row(1), row(2))
}

/// Plans the legs, in path order, each from the joint the previous one
/// ends at.
fn legs(
    profile: &ValidProfile,
    path: &ValidPath,
    opts: &AlongOptions,
    o: Point3,
) -> Result<Vec<Leg>, Error> {
    let lengths = path.lengths();
    let total: f64 = lengths.iter().sum();
    let frame = profile.frame();
    let first = &path.segments[0];
    let undefined = |s: &PathSegment| Error::from(SweepError::FrameUndefined { segment: s.key });
    let axes = Carry::build(
        first.curve.clone(),
        first.range,
        frame.x().get(),
        opts.law,
        0.0,
    )
    .ok()
    .and_then(|c| c.axes_at(first.range[0]))
    .ok_or_else(|| undefined(first))?;
    let mut joint = Joint {
        rigid: Rigid::IDENTITY,
        sigma: 1.0,
        axes,
    };
    let mut done = 0.0;
    let mut out = Vec::with_capacity(path.segments.len());
    for (s, &len) in path.segments.iter().zip(&lengths) {
        let twist = opts.twist * len / total;
        done += len;
        let sigma = 1.0 + (opts.scale - 1.0) * done / total;
        let ratio = sigma / joint.sigma;
        let plain = twist.abs() <= ANGULAR_RESOLUTION && opts.scale == 1.0;
        let kind = if plain {
            analytic(profile, s, &joint, opts.law)?
        } else {
            None
        };
        let (kind, end) = match kind {
            Some((kind, motion)) => {
                let rigid = motion.then_after(&joint.rigid);
                let m = motion.rotation_matrix();
                let end = Joint {
                    rigid,
                    sigma: 1.0,
                    axes: joint.axes.map(|a| rotate(&m, a)),
                };
                (kind, end)
            }
            None => {
                let carry = Carry::new(s.curve.clone(), s.range, joint.axes[0], opts.law, twist)
                    .map_err(|_| undefined(s))?;
                let [a, b] = s.range;
                let axes = carry.axes_at(b).ok_or_else(|| undefined(s))?;
                // The centre rides with the frame from the leg's start.
                let c = joint.centre(o);
                let rel = c - s.curve.point(a);
                let k = joint.axes.map(|f| f.dot(rel));
                let end_centre = axes
                    .iter()
                    .zip(k)
                    .fold(s.curve.point(b), |p, (f, x)| p + *f * x);
                let m = rotation(&joint.axes, &axes);
                let motion =
                    Rigid::from_parts(m, end_centre.to_vector() - rotate(&m, c.to_vector()));
                let end = Joint {
                    rigid: motion.then_after(&joint.rigid),
                    sigma,
                    axes,
                };
                (Kind::Carried, end)
            }
        };
        out.push(Leg {
            segment: s.clone(),
            kind,
            start: joint,
            end,
            twist,
            ratio,
        });
        joint = end;
    }
    Ok(out)
}

/// A leg's extrude or revolve, with the rigid motion from its start to its
/// end: along a line, or round a circle whose axis lies in the section's
/// plane where the frame law turns with it. `None` where the leg needs a
/// sweep; a turn the profile would cross or touch the axis of is
/// `PathTooCurved`.
fn analytic(
    profile: &ValidProfile,
    s: &PathSegment,
    joint: &Joint,
    law: FrameLaw,
) -> Result<Option<(Kind, Rigid)>, Error> {
    let [a, b] = s.range;
    let moved = profile.moved(&joint.rigid);
    let frame = moved.frame();
    match &s.curve {
        Curve::Line(l) => {
            let straight = Straight::new(frame, l.dir().get(), 0.0, b - a)?;
            let motion = straight.at(true);
            Ok(Some((Kind::Straight(straight), motion)))
        }
        Curve::Circle(c) => {
            let z = c.frame().z();
            let n = frame.z();
            let along_law = match law {
                FrameLaw::Minimal => true,
                FrameLaw::Binormal(d) => d.get().cross(z.get()).length() <= ANGULAR_RESOLUTION,
            };
            let in_plane = z.dot(n).abs() <= ANGULAR_RESOLUTION
                && frame.to_local(c.frame().origin()).z.abs() <= LINEAR_RESOLUTION;
            if !(along_law && in_plane) {
                return Ok(None);
            }
            let too_curved = || Error::from(SweepError::PathTooCurved { segment: s.key });
            let axis = Axis::new(c.frame().origin(), z.get())?;
            let turn = Turn::new(&moved, axis, 0.0, b - a).map_err(|e| match e {
                Error::InvalidSweep(
                    SweepError::CrossesAxis { .. }
                    | SweepError::TouchesAxisInside { .. }
                    | SweepError::PinchedOnAxis { .. },
                ) => too_curved(),
                e => e,
            })?;
            // A profile touching the axis would pinch the body there.
            for lump in moved.lumps() {
                for lp in capped::loops(lump) {
                    for (s2, &v) in lp.segments.iter().zip(&lp.vertices) {
                        if turn.fixed(v) || !matches!(turn.side(frame, s2), Ok(Some(_))) {
                            return Err(too_curved());
                        }
                    }
                }
            }
            let motion = turn.at(true);
            Ok(Some((Kind::Turn(turn), motion)))
        }
        _ => Ok(None),
    }
}

/// What every lump is swept with.
struct Carried<'a> {
    frame: &'a Frame,
    o: Point3,
    sign: f64,
    legs: &'a [Leg],
    law: FrameLaw,
}

/// A segment of a loop in path order, its range the way the path runs.
struct Ordered<'a> {
    segment: &'a Segment,
    range: [f64; 2],
}

/// A loop in path order: each path segment's index in the loop, and each
/// path vertex's (the start of the path segment of the same number).
struct Order {
    segments: Vec<usize>,
    vertices: Vec<usize>,
}

impl Carried<'_> {
    /// The frame of the section at `joint` (unscaled; the plane is the
    /// same).
    fn section_frame(&self, joint: &Joint) -> Frame {
        self.frame.transformed(&joint.rigid)
    }

    /// A loop's segments and vertices in path order. Paths run
    /// counterclockwise about the motion: an outer loop its own way when
    /// the motion is along the profile's normal, a hole the other way; an
    /// outer loop of one closed curve running clockwise is placed run
    /// backwards. Returns them with the loop's index of each, and whether
    /// a closed loop's curve runs counterclockwise about the motion.
    fn order<'b>(&self, lp: &'b ValidLoop, outer: bool) -> (Vec<Ordered<'b>>, Order, bool) {
        let n = lp.segments.len();
        if capped::closed(lp) {
            let s = &lp.segments[0];
            let (lo, hi) = (s.t0.min(s.t1), s.t0.max(s.t1));
            let ccw = capped::curve_ccw(lp, outer) == (self.sign > 0.0);
            let range = if outer && !ccw { [hi, lo] } else { [lo, hi] };
            let order = Order {
                segments: vec![0],
                vertices: vec![0],
            };
            let one = Ordered { segment: s, range };
            return (vec![one], order, ccw || outer);
        }
        let fwd = outer == (self.sign > 0.0);
        let mut segs = Vec::with_capacity(n);
        let mut order = Order {
            segments: Vec::with_capacity(n),
            vertices: Vec::with_capacity(n),
        };
        for k in 0..n {
            let j = if fwd { k } else { n - 1 - k };
            let s = &lp.segments[j];
            let range = if fwd { [s.t0, s.t1] } else { [s.t1, s.t0] };
            segs.push(Ordered { segment: s, range });
            order.segments.push(j);
            order.vertices.push(if fwd { k } else { (n - k) % n });
        }
        (segs, order, true)
    }

    /// The side a leg sweeps from a segment, with its sense, and the
    /// placed span it starts from.
    fn side(&self, leg: &Leg, s: &Ordered) -> Result<(Surface, Sense), Error> {
        let frame = self.section_frame(&leg.start);
        let surface = match &leg.kind {
            Kind::Straight(m) => m.side(&frame, s.segment)?,
            Kind::Turn(m) => m.side(&frame, s.segment)?,
            Kind::Carried => {
                let span = leg
                    .start
                    .span(self.o, self.frame, &s.segment.curve, s.range)?;
                let start = Frame::new(
                    leg.start.centre(self.o),
                    leg.start.axes[2],
                    leg.start.axes[0],
                )?;
                let sweep = Sweep::new(
                    span,
                    (leg.segment.curve.clone(), leg.segment.range),
                    start,
                    self.law,
                    leg.twist,
                    leg.ratio,
                )
                .map_err(|_| SweepError::PathTooCurved {
                    segment: leg.segment.key,
                })?;
                Some(Surface::Sweep(Arc::new(sweep)))
            }
        }
        .ok_or(Error::Degenerate)?;
        // The sense whose normal points out of the solid where the
        // segment's middle crosses the leg's start.
        let (p, out) = capped::outward(&frame, s.segment);
        let at = leg
            .start
            .point(self.o, leg.start.rigid.inverse().apply_point(p));
        let (u, v) = surface.project(at).param;
        let sense = match surface.normal(u, v) {
            Some(n) if n.get().dot(out) < 0.0 => Sense::Reversed,
            _ => Sense::Forward,
        };
        Ok((surface, sense))
    }

    /// The lateral a leg sweeps from a path vertex, the start of segment
    /// `s`, whose side is `side`, and the tolerance it needs beyond the
    /// profile's.
    fn lateral(
        &self,
        leg: &Leg,
        start: (Curve, [f64; 2]),
        side: &Surface,
        p: Point2,
    ) -> Result<((Curve, [f64; 2]), f64), Error> {
        let frame = self.section_frame(&leg.start);
        match &leg.kind {
            Kind::Straight(m) => Ok((m.lateral(&frame, p, false)?, 0.0)),
            Kind::Turn(m) => Ok((m.lateral(&frame, p, false)?, 0.0)),
            Kind::Carried => {
                let Surface::Sweep(sweep) = side else {
                    return Err(Error::Degenerate);
                };
                let (curve, tolerance) = sweep
                    .rail(start.1[0], leg.segment.range, LINEAR_RESOLUTION)
                    .map_err(|_| SweepError::PathTooCurved {
                        segment: leg.segment.key,
                    })?;
                Ok((
                    (Curve::Nurbs(Arc::new(curve)), leg.segment.range),
                    tolerance,
                ))
            }
        }
    }

    /// Plans, builds, tolerances and names one lump.
    fn sweep(
        &self,
        b: &mut Body,
        keys: &mut Keys,
        namer: &Namer,
        lump: &Lump,
    ) -> Result<(), Error> {
        let mut plans = Vec::new();
        let mut extra = Vec::new();
        let mut orders = Vec::new();
        for (l, lp) in capped::loops(lump).enumerate() {
            let (segs, order, ccw) = self.order(lp, l == 0);
            let verts = &order.vertices;
            let n = segs.len();
            let first = &self.legs[0].start;
            let span = |b: &mut Body, j: &Joint, s: &Ordered| -> Result<Span, Error> {
                let (curve, interval) = j.span(self.o, self.frame, &s.segment.curve, s.range)?;
                Ok(Span {
                    curve: b.add_curve(curve),
                    interval,
                })
            };
            let at = |j: &Joint, k: usize| {
                j.point(self.o, place::point(self.frame, lp.vertices[verts[k]]))
            };
            let mut near = Vec::with_capacity(n);
            let mut points = Vec::with_capacity(n);
            for (k, seg) in segs.iter().enumerate() {
                near.push(span(b, first, seg)?);
                points.push(at(first, k));
            }
            let mut rings = Vec::with_capacity(self.legs.len());
            let mut fits = Vec::with_capacity(self.legs.len());
            for leg in self.legs {
                let mut segments = Vec::with_capacity(n);
                let mut sides = Vec::with_capacity(n);
                for s in &segs {
                    let (surface, sense) = self.side(leg, s)?;
                    sides.push(surface.clone());
                    segments.push(SegmentPlan {
                        far: span(b, &leg.end, s)?,
                        side: Some((b.add_surface(surface), sense)),
                    });
                }
                let mut vertices = Vec::with_capacity(n);
                let mut fit = Vec::with_capacity(n);
                for k in 0..n {
                    let lateral = if capped::closed(lp) {
                        fit.push(0.0);
                        None
                    } else {
                        let start = leg.start.span(
                            self.o,
                            self.frame,
                            &segs[k].segment.curve,
                            segs[k].range,
                        )?;
                        let (made, tol) =
                            self.lateral(leg, start, &sides[k], lp.vertices[verts[k]])?;
                        fit.push(tol);
                        Some(Span {
                            curve: b.add_curve(made.0),
                            interval: made.1,
                        })
                    };
                    vertices.push(VertexPlan {
                        far: at(&leg.end, k),
                        lateral,
                    });
                }
                rings.push(Ring { segments, vertices });
                fits.push(fit);
            }
            plans.push(LoopPlan {
                near,
                points,
                rings,
                closed: capped::closed(lp),
                ccw,
            });
            extra.push(fits);
            orders.push(order);
        }
        let last = &self.legs[self.legs.len() - 1].end;
        let near_cap = b.add_surface(capped::cap(self.frame, &Rigid::IDENTITY, -self.sign)?);
        let far_cap = b.add_surface(capped::cap(self.frame, &last.rigid, self.sign)?);
        let mut plans = plans.into_iter();
        let outer = plans.next().ok_or(Error::Degenerate)?;
        let plan = LumpPlan {
            outer,
            holes: plans.collect(),
            near_cap: (near_cap, Sense::Forward),
            far_cap: (far_cap, Sense::Forward),
        };
        let built = build::capped(b, keys, &plan)?;
        self.tolerances(b, lump, &orders, &extra, &built)?;
        self.name(b, namer, lump, &orders, &built);
        Ok(())
    }

    /// Each profile vertex's tolerance, scaled with its section, on its
    /// body vertices and laterals, which also take their fits' bounds.
    fn tolerances(
        &self,
        b: &mut Body,
        lump: &Lump,
        orders: &[Order],
        extra: &[Vec<Vec<f64>>],
        built: &LumpBuilt,
    ) -> Result<(), Error> {
        let made = core::iter::once(&built.outer).chain(&built.holes);
        for (l, (lp, m)) in capped::loops(lump).zip(made).enumerate() {
            for (k, &i) in orders[l]
                .vertices
                .iter()
                .enumerate()
                .take(m.near_vertices.len())
            {
                let gap = lp.tolerances[i];
                let mut grow = |v, sigma: f64| -> Result<(), Error> {
                    let tol = Tolerance::new(gap * sigma.max(1.0))?;
                    if b.vertices[v].tolerance.get() < tol.get() {
                        b.vertices[v].tolerance = tol;
                    }
                    Ok(())
                };
                grow(m.near_vertices[k], 1.0)?;
                for (r, ring) in m.rings.iter().enumerate() {
                    let leg = &self.legs[r];
                    grow(ring.far_vertices[k], leg.end.sigma)?;
                    if let Some(e) = ring.laterals[k] {
                        let sigma = leg.start.sigma.max(leg.end.sigma).max(1.0);
                        let tol = Tolerance::new(gap * sigma + extra[l][r][k])?;
                        b.edges[e].tolerance = tol;
                    }
                }
            }
        }
        Ok(())
    }

    /// Names a built lump from the profile's and the path's keys.
    fn name(&self, b: &mut Body, namer: &Namer, lump: &Lump, orders: &[Order], built: &LumpBuilt) {
        let outer = namer.loop_(b, lump.outer.key.0);
        let start = namer.name(
            b,
            EntityId::Face(built.near_cap),
            SweepRole::StartCap,
            &[outer],
        );
        let end = namer.name(
            b,
            EntityId::Face(built.far_cap),
            SweepRole::EndCap,
            &[outer],
        );
        let paths: Vec<Name> = self
            .legs
            .iter()
            .map(|l| namer.path_segment(b, l.segment.key.0))
            .collect();
        let made = core::iter::once(&built.outer).chain(&built.holes);
        for (l, (lp, m)) in capped::loops(lump).zip(made).enumerate() {
            self.name_loop(b, namer, lp, &orders[l], m, [start, end], &paths);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn name_loop(
        &self,
        b: &mut Body,
        namer: &Namer,
        lp: &ValidLoop,
        order: &Order,
        m: &LoopBuilt,
        caps: [Name; 2],
        paths: &[Name],
    ) {
        let n = lp.segments.len();
        let segs: Vec<Name> = lp
            .segments
            .iter()
            .map(|s| namer.segment(b, s.key.0))
            .collect();
        let last = m.rings.len() - 1;
        for (k, &j) in order.segments.iter().enumerate() {
            let seg = segs[j];
            let sides: Vec<Name> = m
                .rings
                .iter()
                .enumerate()
                .map(|(r, ring)| {
                    let face = ring.sides[k].expect("every leg sweeps a side");
                    namer.name(b, EntityId::Face(face), SweepRole::Side, &[seg, paths[r]])
                })
                .collect();
            namer.name(
                b,
                EntityId::Edge(m.near[k]),
                SweepRole::CapEdge,
                &[caps[0], sides[0]],
            );
            namer.name(
                b,
                EntityId::Edge(m.rings[last].far[k]),
                SweepRole::CapEdge,
                &[caps[1], sides[last]],
            );
            for r in 0..last {
                namer.name(
                    b,
                    EntityId::Edge(m.rings[r].far[k]),
                    SweepRole::Section,
                    &[sides[r], sides[r + 1]],
                );
            }
        }
        for (k, &i) in order
            .vertices
            .iter()
            .enumerate()
            .take(m.near_vertices.len())
        {
            // The profile vertex between the loop's segments i - 1 and i.
            let pair = [segs[(i + n - 1) % n], segs[i]];
            namer.name(
                b,
                EntityId::Vertex(m.near_vertices[k]),
                SweepRole::StartVertex,
                &pair,
            );
            for (r, ring) in m.rings.iter().enumerate() {
                if let Some(e) = ring.laterals[k] {
                    namer.name(
                        b,
                        EntityId::Edge(e),
                        SweepRole::Lateral,
                        &[pair[0], pair[1], paths[r]],
                    );
                }
                let v = EntityId::Vertex(ring.far_vertices[k]);
                if r == last {
                    namer.name(b, v, SweepRole::EndVertex, &pair);
                } else {
                    let parents = [pair[0], pair[1], paths[r], paths[r + 1]];
                    namer.name(b, v, SweepRole::JointVertex, &parents);
                }
            }
        }
    }
}
