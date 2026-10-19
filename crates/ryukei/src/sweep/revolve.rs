//! Revolve: a profile turned about an axis in its plane.

use super::build::{self, Keys, Sided, TurnPlan, TurnSegment, TurnVertex};
use super::capped::{self, Motion};
use super::names::Namer;
use super::{Lump, Segment, SweepError, ValidProfile, place};
use crate::Error;
use crate::geom::{
    Axis, Circle, Cone, Curve, Curve2, Cylinder, Frame, NurbsCurve, NurbsCurve2, Plane, Point2,
    Point3, Revolution, Rigid, Sphere, Surface, Torus, UnitVector3, Vector2,
};
use crate::math;
use crate::name::{Journal, OpKey, SweepRole};
use crate::tol::{ANGULAR_RESOLUTION, LINEAR_RESOLUTION, Tolerance};
use crate::topo::{Body, EntityId};
use core::f64::consts::TAU;
use std::sync::Arc;

/// Revolves `profile` about `axis` from angle `start` to `end` (radians,
/// counterclockwise about the axis's direction; angle 0 is the profile's
/// own plane): one solid per lump of the profile.
///
/// The axis must lie in the profile's plane (`AxisNotInPlane`), and
/// `end - start` in `(0, 2π]` (`BadAngle`); within [`ANGULAR_RESOLUTION`]
/// of 2π it is a full turn. The profile may touch the axis at a segment's
/// ends, and segments may lie along it; it may not cross it
/// (`CrossesAxis`) or touch it inside a segment (`TouchesAxisInside`). A
/// full turn has no caps: holes, and the stretches of a loop between
/// two contacts with the axis that another of its stretches spans,
/// enclose cavities, which become void shells. A full turn can't touch
/// the axis at a lone vertex (`PinchedOnAxis`). The arc swept by the profile's farthest point must
/// be at least `10 × LINEAR_RESOLUTION` long (`ShortSpan`), and the solid
/// must stay in the size box (`OutsideSizeBox`).
///
/// Faces are planes, cylinders, cones, spheres and ring tori where the
/// segment gives one exactly, and a [`Revolution`] otherwise. A vertex on
/// the axis is a vertex of the body only where a side meets the axis at
/// an angle (an apex), not where it closes smoothly (a pole). Names come
/// from `key` and the profile's keys.
pub fn revolve(
    key: OpKey,
    profile: &ValidProfile,
    axis: Axis,
    start: f64,
    end: f64,
) -> Result<(Body, Journal), Error> {
    let turn = Turn::new(profile, axis, start, end)?;
    let mut b = Body::new();
    let mut keys = Keys::new(key);
    let namer = Namer::new("revolve", key);
    for lump in profile.lumps() {
        if turn.full {
            whole(&mut b, &mut keys, &namer, profile.frame(), lump, &turn)?;
        } else {
            capped::sweep(&mut b, &mut keys, &namer, profile.frame(), lump, &turn)?;
        }
    }
    let journal = namer.finish(&mut b)?;
    Ok((b, journal))
}

/// Where a segment lies against the axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Contact {
    Off,
    On,
}

/// A revolve's motion, and the axis in the profile's plane.
struct Turn {
    /// The axis in model space, moved into the profile's plane.
    origin: Point3,
    dir: UnitVector3,
    /// The axis in the profile's plane, and the unit direction from it
    /// towards the profile.
    origin2: Point2,
    dir2: Vector2,
    out2: Vector2,
    start: f64,
    end: f64,
    full: bool,
    sign: f64,
    /// The angle the sides' surfaces start their own angles from: opposite
    /// the middle of a partial turn, so that no face meets a surface's
    /// seam.
    phase: f64,
}

impl Turn {
    fn new(profile: &ValidProfile, axis: Axis, start: f64, end: f64) -> Result<Self, Error> {
        if !(start.is_finite() && end.is_finite()) {
            return Err(Error::NonFinite);
        }
        let span = end - start;
        if span <= 0.0 || span > TAU + ANGULAR_RESOLUTION {
            return Err(SweepError::BadAngle.into());
        }
        let frame = profile.frame();
        let n = frame.z().get();
        let local = frame.to_local(axis.origin());
        let d = axis.dir().get();
        if local.z.abs() > LINEAR_RESOLUTION || d.dot(n).abs() > ANGULAR_RESOLUTION {
            return Err(SweepError::AxisNotInPlane.into());
        }
        let dir2 = Vector2::new(d.dot(frame.x().get()), d.dot(frame.y().get()));
        let dir2 = dir2 / dir2.length();
        let origin2 = Point2::new(local.x, local.y);
        let mut turn = Self {
            origin: place::point(frame, origin2),
            dir: UnitVector3::new_unchecked(place::vector(frame, dir2)),
            origin2,
            dir2,
            out2: Vector2::new(-dir2.y, dir2.x),
            start,
            end,
            full: span >= TAU - ANGULAR_RESOLUTION,
            sign: 1.0,
            phase: 0.0,
        };
        if !turn.full {
            turn.phase = (start + end) / 2.0 + core::f64::consts::PI;
        }
        let side = turn.side_of(profile)?;
        turn.out2 = turn.out2 * side;
        // Turning by a positive angle moves the profile along ω × r, which
        // is the frame's normal times cross(dir2, r): the side's sign.
        turn.sign = side;
        for lump in profile.lumps() {
            for lp in core::iter::once(&lump.outer).chain(&lump.holes) {
                for s in &lp.segments {
                    turn.contact(s)?;
                }
            }
        }
        let reach = turn.reach(profile);
        if !turn.full && span * reach < 10.0 * LINEAR_RESOLUTION {
            return Err(SweepError::ShortSpan.into());
        }
        turn.check_extent(profile, reach)?;
        Ok(turn)
    }

    /// The signed distance of a profile point from the axis, positive on
    /// the profile's side once that is known.
    fn offset(&self, p: Point2) -> f64 {
        self.out2.dot(p - self.origin2)
    }

    /// The side of the axis the profile is on: the sign of the first
    /// vertex, or failing that the first segment's middle, off the axis.
    fn side_of(&self, profile: &ValidProfile) -> Result<f64, Error> {
        let mut points = Vec::new();
        for lump in profile.lumps() {
            points.extend(lump.outer.vertices.iter().copied());
            for s in &lump.outer.segments {
                points.push(s.curve.point((s.t0 + s.t1) / 2.0));
            }
        }
        points
            .into_iter()
            .map(|p| self.offset(p))
            .find(|d| d.abs() > LINEAR_RESOLUTION)
            .map(f64::signum)
            .ok_or_else(|| {
                SweepError::CrossesAxis {
                    segment: profile.lumps()[0].outer.segments[0].key,
                }
                .into()
            })
    }

    /// The farthest the profile gets from the axis.
    fn reach(&self, profile: &ValidProfile) -> f64 {
        let mut far = 0.0f64;
        for lump in profile.lumps() {
            for s in &lump.outer.segments {
                let bx = s.curve.bounds([s.t0.min(s.t1), s.t0.max(s.t1)]);
                let (lo, hi) = (bx.min(), bx.max());
                for (x, y) in [(lo.x, lo.y), (hi.x, lo.y), (lo.x, hi.y), (hi.x, hi.y)] {
                    far = far.max(self.offset(Point2::new(x, y)).abs());
                }
            }
        }
        far
    }

    /// Fails unless the cylinder about the axis that holds the profile's
    /// box is inside the size box.
    fn check_extent(&self, profile: &ValidProfile, reach: f64) -> Result<(), Error> {
        let frame = profile.frame();
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for lump in profile.lumps() {
            for s in &lump.outer.segments {
                let bx = s.curve.bounds([s.t0.min(s.t1), s.t0.max(s.t1)]);
                for (x, y) in [
                    (bx.min().x, bx.min().y),
                    (bx.max().x, bx.max().y),
                    (bx.min().x, bx.max().y),
                    (bx.max().x, bx.min().y),
                ] {
                    let h = self.dir2.dot(Point2::new(x, y) - self.origin2);
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
            }
        }
        let (z, x) = (self.dir.get(), place::vector(frame, self.out2));
        let y = z.cross(x);
        for h in [lo, hi] {
            for (a, c) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let p = self.origin + z * h + x * (a * reach) + y * (c * reach);
                Point3::try_new(p.x, p.y, p.z)
                    .map_err(|_| Error::from(SweepError::OutsideSizeBox))?;
            }
        }
        Ok(())
    }

    /// Where a segment lies against the axis, certified from the hulls of
    /// its rational Bézier pieces: off it (perhaps touching it at its
    /// ends), or along it. Fails where it crosses the axis, or comes
    /// within [`LINEAR_RESOLUTION`] of it away from its ends.
    fn contact(&self, s: &Segment) -> Result<Contact, Error> {
        let piece = nurbs_part(s)?;
        let d: Vec<f64> = piece.control_points().map(|p| self.offset(p)).collect();
        if d.iter().all(|x| x.abs() <= LINEAR_RESOLUTION) {
            return Ok(Contact::On);
        }
        let ends = [
            self.offset(s.curve.point(s.t0)).abs() <= LINEAR_RESOLUTION,
            self.offset(s.curve.point(s.t1)).abs() <= LINEAR_RESOLUTION,
        ];
        // The piece runs by increasing parameter: its first end is the
        // segment's start when t0 < t1.
        let ends = if s.t0 < s.t1 {
            ends
        } else {
            [ends[1], ends[0]]
        };
        self.clear(s, &piece, ends, 0)?;
        Ok(Contact::Off)
    }

    /// Certifies that a part of a segment stays on the profile's side, more
    /// than [`LINEAR_RESOLUTION`] from the axis except at an end that
    /// touches it, subdividing where the hull is not enough.
    fn clear(
        &self,
        s: &Segment,
        piece: &NurbsCurve2,
        ends: [bool; 2],
        depth: u32,
    ) -> Result<(), Error> {
        let d: Vec<f64> = piece.control_points().map(|p| self.offset(p)).collect();
        let last = d.len() - 1;
        let cross = SweepError::CrossesAxis { segment: s.key };
        if d[0] < -LINEAR_RESOLUTION || d[last] < -LINEAR_RESOLUTION {
            return Err(cross.into());
        }
        // A touching end may sit on the axis; the rest of the hull must be
        // strictly on the profile's side, so the curve leaves the axis
        // there and doesn't come back within the piece.
        let inner = &d[usize::from(ends[0])..=last - usize::from(ends[1])];
        let floor = if ends[0] || ends[1] {
            0.0
        } else {
            LINEAR_RESOLUTION
        };
        if inner.iter().all(|&x| x > floor) {
            return Ok(());
        }
        let [a, b] = piece.domain();
        if depth >= 48 || (b - a) / 2.0 < LINEAR_RESOLUTION {
            return Err(if d.iter().any(|&x| x < -LINEAR_RESOLUTION) {
                cross.into()
            } else {
                SweepError::TouchesAxisInside { segment: s.key }.into()
            });
        }
        let (left, right) = piece.split((a + b) / 2.0)?;
        self.clear(s, &left, [ends[0], false], depth + 1)?;
        self.clear(s, &right, [false, ends[1]], depth + 1)
    }

    /// Whether a profile point is on the axis.
    fn on_axis(&self, p: Point2) -> bool {
        self.offset(p).abs() <= LINEAR_RESOLUTION
    }

    /// The rigid turn by `angle` about the axis.
    fn rotation(&self, angle: f64) -> Rigid {
        let o = self.origin.to_vector();
        Rigid::translation(o)
            .then_after(&Rigid::rotation(self.dir, angle))
            .then_after(&Rigid::translation(-o))
    }

    /// The axis frame the sides' surfaces turn in: `z` along the axis, `x`
    /// from it towards the profile turned to the phase angle.
    fn frame(&self, frame: &Frame) -> Result<Frame, Error> {
        let out = place::vector(frame, self.out2);
        let x = self.rotation(self.phase).apply_vector(out);
        Frame::new(self.origin, self.dir.get(), x)
    }

    /// The point of the axis nearest a profile point, in model space.
    fn foot(&self, frame: &Frame, p: Point2) -> Point3 {
        let h = self.dir2.dot(p - self.origin2);
        place::point(frame, self.origin2 + self.dir2 * h)
    }

    /// The circle a profile point sweeps, with `z` along the axis (or
    /// against it, `back`) and angle 0 at the point itself.
    fn circle(&self, frame: &Frame, p: Point2, back: bool) -> Result<Circle, Error> {
        let at = place::point(frame, p);
        let centre = self.foot(frame, p);
        let z = if back {
            -self.dir.get()
        } else {
            self.dir.get()
        };
        Circle::new(Frame::new(centre, z, at - centre)?, at.distance(centre))
    }

    /// The surface a segment off the axis sweeps.
    fn surface(&self, frame: &Frame, s: &Segment) -> Result<Surface, Error> {
        let axis = self.frame(frame)?;
        Ok(match &s.curve {
            Curve2::Line2(l) => {
                let along = l.dir().dot(self.dir2);
                let away = l.dir().dot(self.out2);
                let (p, q) = (s.curve.point(s.t0), s.curve.point(s.t1));
                // The end farther from the axis: a cone's radius there is
                // never zero.
                let far = if self.offset(p) >= self.offset(q) {
                    p
                } else {
                    q
                };
                let base = Frame::new(self.foot(frame, far), self.dir.get(), axis.x().get())?;
                if away.abs() <= ANGULAR_RESOLUTION {
                    Surface::Cylinder(Cylinder::new(base, self.offset(far))?)
                } else if along.abs() <= ANGULAR_RESOLUTION {
                    Surface::Plane(Plane::new(base))
                } else {
                    // Widening towards +z: up the axis when the line moves
                    // away from it going up.
                    let z = if along * away > 0.0 {
                        self.dir.get()
                    } else {
                        -self.dir.get()
                    };
                    let f = Frame::new(self.foot(frame, far), z, axis.x().get())?;
                    let half = math::atan2(away.abs(), along.abs());
                    Surface::Cone(Cone::new(f, self.offset(far), half)?)
                }
            }
            Curve2::Circle2(c) => {
                let centre = c.frame().origin();
                let major = self.offset(centre);
                let f = Frame::new(self.foot(frame, centre), self.dir.get(), axis.x().get())?;
                if major.abs() <= LINEAR_RESOLUTION {
                    Surface::Sphere(Sphere::new(f, c.radius())?)
                } else if let Ok(t) = Torus::new(f, major, c.radius()) {
                    Surface::Torus(t)
                } else {
                    self.revolution(frame, &axis, s)?
                }
            }
            _ => self.revolution(frame, &axis, s)?,
        })
    }

    /// A revolution of the segment's whole curve, or of just the segment's
    /// part (a clamped NURBS) when the whole curve reaches the axis.
    fn revolution(&self, frame: &Frame, axis: &Frame, s: &Segment) -> Result<Surface, Error> {
        // The curve turned to the phase angle, into the frame's xz plane.
        let turn = self.rotation(self.phase);
        let whole = place::curve(frame, &s.curve)?.transformed(&turn);
        if let Ok(r) = Revolution::new(*axis, whole) {
            return Ok(Surface::Revolution(Arc::new(r)));
        }
        let part = nurbs_part(s)?;
        let points = part
            .control_points()
            .map(|p| place::point(frame, p))
            .collect();
        let curve = NurbsCurve::new(part.knots().clone(), points, Some(part.weights().to_vec()))?;
        let r = Revolution::new(*axis, Curve::Nurbs(Arc::new(curve.transformed(&turn))))?;
        Ok(Surface::Revolution(Arc::new(r)))
    }

    /// Whether a side meets the axis at a right angle at a segment's end
    /// on it (a pole, where the body has no vertex) rather than at an
    /// angle (an apex).
    fn smooth(&self, s: &Segment, at_start: bool) -> bool {
        let t = if at_start { s.t0 } else { s.t1 };
        let d1 = s.curve.eval(t).d1;
        let len = d1.length();
        len > 0.0 && (d1.dot(self.dir2) / len).abs() <= ANGULAR_RESOLUTION
    }
}

/// A segment's part as a clamped NURBS in the profile plane, running by
/// increasing parameter.
fn nurbs_part(s: &Segment) -> Result<NurbsCurve2, Error> {
    let (lo, hi) = (s.t0.min(s.t1), s.t0.max(s.t1));
    let (n, reparam) = s.curve.to_nurbs([lo, hi])?;
    n.part([reparam.to_nurbs(lo), reparam.to_nurbs(hi)])
}

impl Motion for Turn {
    fn at(&self, end: bool) -> Rigid {
        self.rotation(if end { self.end } else { self.start })
    }

    fn sign(&self) -> f64 {
        self.sign
    }

    fn fixed(&self, p: Point2) -> bool {
        self.on_axis(p)
    }

    fn lateral(
        &self,
        frame: &Frame,
        p: Point2,
        from_end: bool,
    ) -> Result<(Curve, [f64; 2]), Error> {
        let c = self.circle(frame, p, from_end)?;
        let interval = if from_end {
            [-self.end, -self.start]
        } else {
            [self.start, self.end]
        };
        Ok((Curve::Circle(c), interval))
    }

    fn side(&self, frame: &Frame, s: &Segment) -> Result<Option<Surface>, Error> {
        match self.contact(s)? {
            Contact::On => Ok(None),
            Contact::Off => Ok(Some(self.surface(frame, s)?)),
        }
    }
}

/// A full turn of one lump. Each loop turns into shells: a ring of sides
/// if the loop never touches the axis, or one chain of sides between each
/// stretch along the axis and the next. The outer loop's chain that spans
/// the others along the axis (or its ring) is the lump's outer shell, a
/// new solid region; its other chains, and every hole, enclose cavities
/// and become void shells in that region.
fn whole(
    b: &mut Body,
    keys: &mut Keys,
    namer: &Namer,
    frame: &Frame,
    lump: &Lump,
    turn: &Turn,
) -> Result<(), Error> {
    let region = shells(b, keys, namer, frame, &lump.outer, turn, None)?;
    for hole in &lump.holes {
        shells(b, keys, namer, frame, hole, turn, Some(region))?;
    }
    Ok(())
}

/// The shells a full turn of one loop makes: into `region`, or for an
/// outer loop (`None`) its outer shell in a new solid region first, then
/// its cavities there. Returns the region.
fn shells(
    b: &mut Body,
    keys: &mut Keys,
    namer: &Namer,
    frame: &Frame,
    lp: &super::ValidLoop,
    turn: &Turn,
    region: Option<crate::topo::RegionId>,
) -> Result<crate::topo::RegionId, Error> {
    let n = lp.segments.len();
    let on: Vec<bool> = lp
        .segments
        .iter()
        .map(|s| turn.contact(s).map(|c| c == Contact::On))
        .collect::<Result<_, _>>()?;
    let at_axis: Vec<bool> = lp.vertices.iter().map(|&p| turn.on_axis(p)).collect();
    // A vertex on the axis must end a stretch along it.
    for i in 0..n {
        let prev = (i + n - 1) % n;
        if at_axis[i] && !on[prev] && !on[i] {
            return Err(SweepError::PinchedOnAxis {
                segment: lp.segments[i].key,
            }
            .into());
        }
    }
    if on.iter().all(|&x| x) {
        // A loop with no area; a valid profile has none.
        return Err(Error::Degenerate);
    }
    // Stretches along the axis: the segments that start one. Each chain
    // runs from just after a stretch to the start of the next.
    let starts: Vec<usize> = (0..n).filter(|&i| on[i] && !on[(i + n - 1) % n]).collect();
    let mut chains: Vec<(usize, usize)> = starts
        .iter()
        .map(|&s| {
            let mut first = s;
            while on[first] {
                first = (first + 1) % n;
            }
            let count = (0..n).take_while(|&k| !on[(first + k) % n]).count();
            (first, count)
        })
        .collect();
    let chain = !chains.is_empty();
    if !chain {
        chains.push((0, n));
    }
    // The chain spanning the most of the axis encloses the others.
    if region.is_none() && chains.len() > 1 {
        let axial = |i: usize| turn.dir2.dot(lp.vertices[i % n] - turn.origin2);
        let span = |&(first, count): &(usize, usize)| (axial(first + count) - axial(first)).abs();
        let outer = (0..chains.len())
            .max_by(|&x, &y| {
                span(&chains[x])
                    .total_cmp(&span(&chains[y]))
                    .then(y.cmp(&x))
            })
            .expect("a chain");
        chains.swap(0, outer);
    }
    let mut region = region;
    for (first, count) in chains {
        let built = turn_shell(b, keys, namer, frame, lp, turn, region, chain, first, count)?;
        region.get_or_insert(built);
    }
    Ok(region.expect("a shell"))
}

/// One shell of a full turn: the `count` sides from loop index `first`
/// on, as a chain between two axis contacts or a ring round the whole
/// loop. Returns its region.
#[allow(clippy::too_many_arguments)]
fn turn_shell(
    b: &mut Body,
    keys: &mut Keys,
    namer: &Namer,
    frame: &Frame,
    lp: &super::ValidLoop,
    turn: &Turn,
    region: Option<crate::topo::RegionId>,
    chain: bool,
    first: usize,
    count: usize,
) -> Result<crate::topo::RegionId, Error> {
    let n = lp.segments.len();
    // The order the chain or ring runs in: from just after a stretch
    // along the axis, round to the next. Paths run counterclockwise
    // about the reverse of the motion, which is the loop's own way when
    // the motion is against the profile's normal.
    let forward = turn.sign < 0.0;
    // Loop indices of the sides, in chain order.
    let sides: Vec<usize> = (0..count)
        .map(|k| {
            if forward {
                (first + k) % n
            } else {
                (first + count - 1 - k) % n
            }
        })
        .collect();
    // The profile vertices off the axis that the builder wants: for a
    // chain, the one between each side and the next; for a ring, the one
    // each side starts from. A ring of one closed segment has none.
    let start_of = |k: usize| {
        if forward {
            sides[k]
        } else {
            (sides[k] + 1) % n
        }
    };
    let verts: Vec<usize> = if chain {
        (1..count).map(start_of).collect()
    } else if n == 1 {
        Vec::new()
    } else {
        (0..count).map(start_of).collect()
    };
    let mut plan = TurnPlan {
        segments: Vec::with_capacity(count),
        vertices: Vec::with_capacity(verts.len()),
        ends: None,
        region,
    };
    for &i in &sides {
        let s = &lp.segments[i];
        let surface = turn.surface(frame, s)?;
        let sense = capped::sense_of(&surface, frame, s);
        let side: Sided = (b.add_surface(surface), sense);
        plan.segments.push(TurnSegment { side });
    }
    for &v in &verts {
        let p = lp.vertices[v];
        let c = turn.circle(frame, p, false)?;
        plan.vertices.push(TurnVertex {
            point: place::point(frame, p),
            circle: build::Span {
                curve: b.add_curve(Curve::Circle(c)),
                interval: [0.0, TAU],
            },
        });
    }
    if chain {
        // The chain's ends on the axis: where its first side starts and
        // its last side ends, in chain order.
        let (s0, s1) = (&lp.segments[sides[0]], &lp.segments[sides[count - 1]]);
        let (p, from_start) = if forward {
            (s0.t0, true)
        } else {
            (s0.t1, false)
        };
        let (q, to_end) = if forward {
            (s1.t1, true)
        } else {
            (s1.t0, false)
        };
        let a = place::point(frame, s0.curve.point(p));
        let z = place::point(frame, s1.curve.point(q));
        let apex_a = !turn.smooth(s0, from_start);
        let apex_z = !turn.smooth(s1, !to_end);
        plan.ends = Some([(a, apex_a), (z, apex_z)]);
    }
    let made = build::turned(b, keys, &plan)?;
    let region = made.region;
    // Tolerances and names.
    let segs: Vec<_> = lp
        .segments
        .iter()
        .map(|s| namer.segment(b, s.key.0))
        .collect();
    for (k, &i) in sides.iter().enumerate() {
        namer.name(
            b,
            EntityId::Face(made.sides[k]),
            SweepRole::Side,
            &[segs[i]],
        );
    }
    let pair = |i: usize| [segs[(i + n - 1) % n], segs[i]];
    for (k, &v) in verts.iter().enumerate() {
        let tol = Tolerance::new(lp.tolerances[v])?;
        b.vertices[made.vertices[k]].tolerance = tol;
        b.edges[made.circles[k]].tolerance = tol;
        namer.name(
            b,
            EntityId::Edge(made.circles[k]),
            SweepRole::Lateral,
            &pair(v),
        );
        namer.name(
            b,
            EntityId::Vertex(made.vertices[k]),
            SweepRole::Vertex,
            &pair(v),
        );
    }
    if chain {
        // The axis vertices at the chain's ends, in loop order.
        let (a, z) = if forward {
            (sides[0], (sides[count - 1] + 1) % n)
        } else {
            ((sides[0] + 1) % n, sides[count - 1])
        };
        for (apex, v) in made.apexes.into_iter().zip([a, z]) {
            if let Some(apex) = apex {
                namer.name(b, EntityId::Vertex(apex), SweepRole::Apex, &pair(v));
            }
        }
    }
    Ok(region)
}
