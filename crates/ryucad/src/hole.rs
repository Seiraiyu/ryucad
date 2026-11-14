//! Holes to ryukei tools. A hole's profile lies in the half-plane through
//! its axis: the bore, a counterbore or countersink at its mouth, and a
//! flat or drilled bottom, as lines from the axis out and back. It is
//! turned all round once, at the sketch's origin against its normal, and
//! placed at each point, so every point's tool is the same body under a
//! key of its own.
//!
//! A segment's key is SipHash of `ryucad.hole\0<part>\0<feature>\0<segment>`
//! (`top`, `near`, `counterbore_wall`, `counterbore_floor`, `countersink`,
//! `counterdrill_wall`, `counterdrill_floor`, `spotface_wall`,
//! `spotface_floor`, `wall` (the threaded length, when tapped), `taper_end`,
//! `drill_wall` (a blind tapped hole's drill below its thread),
//! `far_countersink`, `far_counterbore_floor`, `far_counterbore_wall`,
//! `bottom`, `tip`, `ball`, `axis`), never of a size, so resizing a hole renames
//! nothing and switching its form renames only what its new segments
//! touch. A point's tool key is `ryucad.tool\0<part>\0<feature>\0<point>`.

pub mod check;
pub mod derive;
pub mod exits;
pub mod spec;

use crate::doc::files::Feature;
use crate::doc::hole::{Bottom, Depth, Hole, HoleForm};
use crate::doc::id::Id;
use crate::doc::project::Part;
use crate::doc::sketch::Geometry;
use crate::error::{Code, Error, Result};
use crate::profile::Keys;
use crate::rebuild::{Built, Results};
use crate::sketch::Solved;
use core::f64::consts::TAU;
use derive::Sizes;
use ryukei::geom::{Axis, Circle2, Curve2, Frame, Frame2, Line2, Point2, Point3, Vector2, Vector3};
use ryukei::name::{OpKey, ToolKey};
use ryukei::sweep::{self, Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};
use serde_json::json;
use std::sync::Arc;

/// The key of one of a hole's profile segments.
pub fn segment_key(part: &Id, feature: &Id, segment: &str) -> u128 {
    let mut bytes = b"ryucad.hole\0".to_vec();
    for s in [part.as_str(), feature.as_str(), segment] {
        bytes.extend_from_slice(s.as_bytes());
        bytes.push(0);
    }
    bytes.pop();
    OpKey::from_bytes(&bytes).0
}

/// The tool key of a hole's point.
pub fn point_key(part: &Id, feature: &Id, point: &Id) -> ToolKey {
    let mut bytes = b"ryucad.tool\0".to_vec();
    for id in [part, feature, point] {
        bytes.extend_from_slice(id.as_str().as_bytes());
        bytes.push(0);
    }
    bytes.pop();
    ToolKey(OpKey::from_bytes(&bytes).0)
}

/// The points of `solved` (sketch `sketch`) that `listed` names, or every
/// point no curve uses, in natural order, in the part's frame; what for
/// errors names the reader (`hole h`). A listed ID that isn't a point is
/// `not_found` or `reference_wrong_kind` at `/points/<i>`. May be empty.
pub fn sketch_points(
    what: &str,
    owner: &Id,
    sketch: &Id,
    listed: &[Id],
    solved: &Solved,
) -> Result<Vec<(Id, Point3)>> {
    let s = &solved.sketch;
    let at = |m: [f64; 2]| solved.frame.apply_point(Point3::new(m[0], m[1], 0.0));
    if listed.is_empty() {
        return Ok(s
            .free_points()
            .into_iter()
            .filter_map(|p| match s.entities[p].geometry {
                Geometry::Point { m } => Some((p.clone(), at(m))),
                _ => None,
            })
            .collect());
    }
    listed
        .iter()
        .enumerate()
        .map(|(i, p)| match s.entities.get(p).map(|e| &e.geometry) {
            Some(Geometry::Point { m }) => Ok((p.clone(), at(*m))),
            Some(g) => Err(Error::new(
                Code::ReferenceWrongKind,
                format!("{what}: {p} is a {}, not a point", g.type_name()),
            )
            .with("feature", owner)
            .with("entity", p)
            .with("pointer", format!("/points/{i}"))),
            None => Err(Error::new(
                Code::NotFound,
                format!("{what}: sketch {sketch} has no {p}"),
            )
            .with("feature", owner)
            .with("entity", p)
            .with("pointer", format!("/points/{i}"))),
        })
        .collect()
}

/// The solved sketch a hole drills at, and the points it drills: those
/// it lists, else every point no curve uses, in natural order (`p2` before `p10`). A listed ID
/// that isn't a point of the sketch is `not_found` or
/// `reference_wrong_kind`; a sketch with no point to drill is
/// `invalid_param`.
pub fn points(id: &Id, h: &Hole, sketch: &Id, solved: &Solved) -> Result<Vec<(Id, Point3)>> {
    let out = sketch_points(&format!("hole {id}"), id, sketch, &h.points, solved)?;
    if out.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "hole {id}: sketch {sketch} has no point that no curve uses; list the points to drill"
            ),
        )
        .with("feature", id)
        .with("pointer", "/points"));
    }
    Ok(out)
}

/// How deep the bore goes from the sketch's plane, in meters: blind,
/// through all of its targets, or up to its target.
#[allow(clippy::too_many_arguments)]
fn depth(
    part: &Part,
    id: &Id,
    h: &Hole,
    s: &Sizes,
    from: Point3,
    dir: Vector3,
    targets: &[&Built],
    before: &dyn Results,
) -> Result<f64> {
    if let Some(t) = s.thread.as_ref().and_then(|t| t.depth.as_ref())
        && s.mouth_depth().is_some_and(|m| t.si <= m)
    {
        return Err(Error::new(
            Code::InvalidExtent,
            format!(
                "hole {id}: its thread ends at {}, inside its {}",
                t.text,
                h.form.type_name()
            ),
        )
        .with("feature", id)
        .with("pointer", "/thread/depth"));
    }
    let d = match &h.depth {
        Depth::Blind(q) => return Ok(q.meters()),
        Depth::Tip(q) => {
            let Bottom::Drill(a) = &h.bottom else {
                unreachable!("doc::hole refuses a tip depth with no drill point")
            };
            let shoulder = q.meters() - s.drill.si / 2.0 / ryukei::math::tan(a.radians() / 2.0);
            if shoulder <= s.mouth_depth().unwrap_or(0.0) {
                return Err(Error::new(
                    Code::InvalidExtent,
                    format!("hole {id}: its point alone is deeper than {q} to the tip"),
                )
                .with("feature", id)
                .with("pointer", "/depth/tip"));
            }
            return Ok(shoulder);
        }
        Depth::ThroughAll => crate::rebuild::through_all("hole", id, from, dir, false, targets)?.1,
        Depth::ThroughNext => unreachable!("through_next is built per place (hole::tool_at)"),
        Depth::UpTo { target, offset } => crate::rebuild::UpTo {
            ty: "hole",
            role: "depth",
            target,
            offset: offset.as_ref(),
            from,
            dir,
        }
        .length(part, id, before)?,
    };
    let at = match h.form {
        HoleForm::Countersink { .. } => "/countersink",
        HoleForm::Counterdrill { .. } => "/counterdrill/depth",
        HoleForm::Spotface { .. } => "/spotface/depth",
        _ => "/counterbore/depth",
    };
    if let Some(mouth) = s.mouth_depth()
        && mouth >= d
    {
        return Err(Error::new(
            Code::InvalidExtent,
            format!(
                "hole {id}: its {} reaches {mouth} m, past the {d} m the hole goes",
                h.form.type_name()
            ),
        )
        .with("feature", id)
        .with("pointer", at));
    }
    Ok(d)
}

/// A corner of the profile, `(radius, depth)` from the mouth on the axis
/// (negative depth above it), and the segment that leaves it: a line to
/// the next corner, or an arc about `arc`.
struct Corner {
    at: (f64, f64),
    name: &'static str,
    arc: Option<(f64, f64)>,
}

fn line(at: (f64, f64), name: &'static str) -> Corner {
    Corner {
        at,
        name,
        arc: None,
    }
}

/// The profile's corners: out along the top, down the mouth form (a near
/// chamfer first), the bore, a far side (Task 9), and across the bottom
/// or down the point or round the ball to the axis, and up the axis.
/// `depth` is the bore's shoulder; `up` how far a spotface's wall reaches
/// back above the mouth.
fn corners(
    id: &Id,
    h: &Hole,
    s: &Sizes,
    depth: f64,
    up: f64,
    exit: Option<f64>,
) -> Result<Vec<Corner>> {
    let r = s.drill.si / 2.0;
    // A tapered thread's wall is a cone, 1:16 on the diameter through the
    // gauge plane, to the thread's end; then an annulus in to the drill.
    let taper = s
        .thread
        .as_ref()
        .and_then(|t| Some((t.gauge.as_ref()?, t.taper?, t.depth.as_ref()?)));
    let radius_at = |z: f64| -> f64 {
        match taper {
            Some(((gd, gl), (a, b), _)) => {
                (gd.si - (z - gl.si) * f64::from(a) / f64::from(b)) / 2.0
            }
            None => r,
        }
    };
    let wall = |out: &mut Vec<Corner>, z: f64| {
        out.push(line((radius_at(z), z), "wall"));
        if let Some((_, _, end)) = taper {
            let re = radius_at(end.si);
            if re - r > ryukei::tol::LINEAR_RESOLUTION {
                out.push(line((re, end.si), "taper_end"));
            }
        }
    };
    if let Some((_, _, end)) = taper
        && radius_at(end.si) < r
    {
        return Err(Error::new(
            Code::InvalidExtent,
            format!(
                "hole {id}: its tapered wall narrows inside its drill at {} from the mouth",
                end.text
            ),
        )
        .with("feature", id)
        .with("pointer", "/thread/depth"));
    }
    let tan = |a: &derive::Size| ryukei::math::tan(a.si / 2.0);
    let top = if s.spotface.is_some() { -up } else { 0.0 };
    let mut out = vec![line((0.0, top), "top")];
    // A near chamfer at `z`, down to radius `under`: where it ends.
    let near = |out: &mut Vec<Corner>, z: f64, under: f64| -> f64 {
        match &s.near {
            Some((d, a)) => {
                let rn = d.si / 2.0;
                out.push(line((rn, z), "near"));
                z + (rn - under) / tan(a)
            }
            None => z,
        }
    };
    if let Some((d, cdepth)) = &s.counterbore {
        let rc = d.si / 2.0;
        let z = near(&mut out, 0.0, rc);
        out.push(line((rc, z), "counterbore_wall"));
        out.push(line((rc, cdepth.si), "counterbore_floor"));
        wall(&mut out, cdepth.si);
    } else if let Some((d, _)) = &s.countersink {
        let z = s.mouth_depth().expect("a countersink has a depth");
        out.push(line((d.si / 2.0, 0.0), "countersink"));
        wall(&mut out, z);
    } else if let Some((d, cdepth, a)) = &s.counterdrill {
        let rc = d.si / 2.0;
        let z = near(&mut out, 0.0, rc);
        out.push(line((rc, z), "counterdrill_wall"));
        out.push(line((rc, cdepth.si), "counterdrill_floor"));
        wall(&mut out, cdepth.si + (rc - r) / tan(a));
    } else if let Some((d, sdepth)) = &s.spotface {
        let rs = d.si / 2.0;
        out.push(line((rs, -up), "spotface_wall"));
        out.push(line((rs, sdepth.si), "spotface_floor"));
        let z = near(&mut out, sdepth.si, radius_at(sdepth.si));
        wall(&mut out, z);
    } else {
        let z = near(&mut out, 0.0, radius_at(0.0));
        wall(&mut out, z);
    }
    // A blind thread that stops short of its drill: the wall ends where
    // the thread does, and the drill's own wall goes on to the bottom.
    let mouth = s.thread_start();
    if h.depth.is_blind()
        && let Some(t) = s.thread.as_ref().and_then(|t| t.depth.as_ref())
        && t.si > mouth
        && t.si < depth
    {
        out.push(line((r, t.si), "drill_wall"));
    }
    match &h.bottom {
        Bottom::Flat => {
            let tan = |a: &derive::Size| ryukei::math::tan(a.si / 2.0);
            match (&s.far, exit) {
                (Some(derive::FarSizes::Countersink(d, a)), Some(e)) => {
                    let rf = d.si / 2.0;
                    out.push(line((r, e - (rf - r) / tan(a)), "far_countersink"));
                    // The cone runs on past the exit to the bore's end.
                    out.push(line((rf + (depth - e) * tan(a), depth), "bottom"));
                }
                (Some(derive::FarSizes::Counterbore(d, fd)), Some(e)) => {
                    let rf = d.si / 2.0;
                    out.push(line((r, e - fd.si), "far_counterbore_floor"));
                    out.push(line((rf, e - fd.si), "far_counterbore_wall"));
                    out.push(line((rf, depth), "bottom"));
                }
                _ => out.push(line((r, depth), "bottom")),
            }
            out.push(line((0.0, depth), "axis"));
        }
        Bottom::Drill(angle) => {
            out.push(line((r, depth), "tip"));
            out.push(line(
                (0.0, depth + r / ryukei::math::tan(angle.radians() / 2.0)),
                "axis",
            ));
        }
        Bottom::Ball => {
            out.push(Corner {
                at: (r, depth),
                name: "ball",
                arc: Some((0.0, depth)),
            });
            out.push(line((0.0, depth + r), "axis"));
        }
    }
    Ok(out)
}

/// How far a spotface's wall reaches back from `from` against `down`:
/// past every corner of the targets' boxes, by a hundredth of their depth
/// along it (never refused: a flat face has nothing above it).
fn reach_back(from: Point3, down: Vector3, targets: &[&Built]) -> f64 {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for b in targets {
        for f in b.body.faces() {
            let bx = ryukei::intersect::face_box(&b.body, f);
            let (a, z) = (bx.min(), bx.max());
            for (x, y, w) in [
                (a.x, a.y, a.z),
                (z.x, a.y, a.z),
                (a.x, z.y, a.z),
                (z.x, z.y, a.z),
                (a.x, a.y, z.z),
                (z.x, a.y, z.z),
                (a.x, z.y, z.z),
                (z.x, z.y, z.z),
            ] {
                let d = (Point3::new(x, y, w) - from).dot(down);
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
    }
    let margin = (hi - lo).max(1e-6) * 0.01;
    (-lo).max(0.0) + margin
}

/// The hole's tool: its profile turned all round its axis, at the
/// sketch's origin, drilling against the sketch's normal; and its frame's
/// origin, from which each point's tool is placed.
#[allow(clippy::too_many_arguments)]
pub fn tool(
    part: &Part,
    id: &Id,
    h: &Hole,
    s: &Sizes,
    solved: &Solved,
    targets: &[&Built],
    before: &dyn Results,
) -> Result<(Built, Point3)> {
    let (origin, down) = frame_of(solved)?;
    let d = depth(part, id, h, s, origin, down, targets, before)?;
    build_tool(part, id, h, s, solved, targets, d, None)
}

/// The sketch frame's origin and the direction a hole drills.
fn frame_of(solved: &Solved) -> Result<(Point3, Vector3)> {
    let sketch = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    Ok((sketch.origin(), -sketch.z().get()))
}

/// The hole's tool at the sketch's origin for a bore of depth `d`, and,
/// for a far side, the place's exit.
#[allow(clippy::too_many_arguments)]
fn build_tool(
    part: &Part,
    id: &Id,
    h: &Hole,
    s: &Sizes,
    solved: &Solved,
    targets: &[&Built],
    d: f64,
    exit: Option<f64>,
) -> Result<(Built, Point3)> {
    let sketch = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let (origin, down) = (sketch.origin(), -sketch.z().get());
    // The profile's plane holds the axis: x across, y down the hole.
    let frame = Frame::new(origin, sketch.y().get(), sketch.x().get()).map_err(|e| {
        Error::new(Code::Internal, "a hole's frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let up = if s.spotface.is_some() {
        reach_back(origin, down, targets)
    } else {
        0.0
    };
    let corners = corners(id, h, s, d, up, exit)?;
    let mut keys = Keys::default();
    let segments: Vec<Segment> = corners
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let next = &corners[(i + 1) % corners.len()];
            let (a, b) = (
                Point2::new(c.at.0, c.at.1),
                Point2::new(next.at.0, next.at.1),
            );
            let k = segment_key(&part.id, id, c.name);
            keys.segments.insert(
                k,
                Id::parse(c.name, "entity").expect("a segment's name is an ID"),
            );
            let (curve, t0, t1) = match c.arc {
                None => {
                    let line = Line2::through(a, b).expect("a hole's corners are apart");
                    (Curve2::Line2(line), 0.0, a.distance(b))
                }
                Some((cx, cy)) => {
                    let centre = Point2::new(cx, cy);
                    let x = Vector2::new(a.x - cx, a.y - cy);
                    let frame = Frame2::new(centre, x, true).expect("a ball has a radius");
                    let circle =
                        Circle2::new(frame, a.distance(centre)).expect("a ball has a radius");
                    let (dx, dy) = (b.x - cx, b.y - cy);
                    let mut t1 = ryukei::math::atan2(x.x * dy - x.y * dx, x.x * dx + x.y * dy);
                    if t1 <= 0.0 {
                        t1 += TAU;
                    }
                    (Curve2::Circle2(circle), 0.0, t1)
                }
            };
            Segment {
                key: SegmentKey(k),
                curve,
                t0,
                t1,
            }
        })
        .collect();
    let lk = segment_key(&part.id, id, "loop");
    keys.loops
        .insert(lk, Id::parse("loop", "entity").expect("an ID"));
    let made = ValidProfile::new(Profile {
        frame,
        loops: vec![Loop {
            key: LoopKey(lk),
            segments,
        }],
    })
    .and_then(|valid| {
        let axis = Axis::new(origin, down)?;
        let key = crate::rebuild::op_key(&part.id, id);
        let made = sweep::revolve(key, &valid, axis, 0.0, TAU)?;
        Ok((valid, made))
    });
    let (valid, made) = made.map_err(|e| {
        Error::new(
            Code::RebuildFailed,
            format!("hole {id}: its profile doesn't turn into a solid: {e}"),
        )
        .with("feature", id)
        .with("kernel", format!("{e:?}"))
    })?;
    // Origins name the segment, never a sketch: each point adds itself.
    let key = crate::rebuild::op_key(&part.id, id);
    let origins: crate::rebuild::Origins =
        crate::profile::origins(key, h.sketch.as_ref().unwrap_or(id), &valid, &keys)
            .into_iter()
            .map(|(n, o)| {
                let mut v = json!({"feature": id});
                if let Some(e) = o.get("entity") {
                    v["segment"] = e.clone();
                }
                if let Some(e) = o.get("entities") {
                    v["segments"] = e.clone();
                }
                (n, v)
            })
            .collect();
    let built = crate::rebuild::finish(id, "hole", made, origins)?;
    Ok((built, origin))
}

/// Whether a hole's tool depends on where each place's axis leaves its
/// targets: a far side, or through_next.
pub fn needs_exits(h: &Hole) -> bool {
    h.far.is_some() || h.depth == Depth::ThroughNext
}

/// The tools of a hole whose shape depends on each place's exit, built
/// at the sketch's origin one per distinct exit (equal within the linear
/// resolution, first come first kept), for the hole and its patterns.
pub(crate) struct ExitTools<'a> {
    part: &'a Part,
    id: &'a Id,
    h: &'a Hole,
    s: &'a Sizes,
    solved: &'a Solved,
    targets: &'a [&'a Built],
    origin: Point3,
    down: Vector3,
    /// through_all's depth; a hundredth of the targets' depth (through_next's margin).
    through: f64,
    margin: f64,
    pub tools: Vec<(f64, Arc<Built>)>,
}

impl<'a> ExitTools<'a> {
    pub fn new(
        part: &'a Part,
        id: &'a Id,
        h: &'a Hole,
        s: &'a Sizes,
        solved: &'a Solved,
        targets: &'a [&'a Built],
        before: &dyn Results,
    ) -> Result<ExitTools<'a>> {
        let (origin, down) = frame_of(solved)?;
        let (lo, hi) = crate::rebuild::span(origin, down, targets);
        let through = match h.depth {
            Depth::ThroughAll => depth(part, id, h, s, origin, down, targets, before)?,
            _ => 0.0,
        };
        Ok(ExitTools {
            part,
            id,
            h,
            s,
            solved,
            targets,
            origin,
            down,
            through,
            margin: (hi - lo).max(1e-6) * 0.01,
            tools: Vec::new(),
        })
    }

    pub fn origin(&self) -> Point3 {
        self.origin
    }

    pub fn down(&self) -> Vector3 {
        self.down
    }

    /// Where the axis through `at` along `axis` leaves the targets, from
    /// `at`: the first wall's far side for through_next, the last for
    /// through_all. `place` names the place in refusals.
    pub fn exit(
        &self,
        id: &Id,
        place: &str,
        at: Point3,
        axis: Vector3,
        work: &ryukei::work::Work,
    ) -> Result<f64> {
        let next = self.h.depth == Depth::ThroughNext;
        let bodies = self.targets.iter().map(|b| &b.body);
        match exits::over(bodies, at, axis, next, work) {
            Ok(Some(e)) => Ok(e),
            Ok(None) => Err(Error::new(
                Code::OperationNoEffect,
                format!("hole {id}: its axis at {place} meets none of its targets"),
            )
            .with("feature", id)
            .with("tool", id)
            .with("point", place)),
            Err(exits::Uncertain) => Err(Error::new(
                Code::RebuildFailed,
                format!("hole {id}: where its axis at {place} leaves its targets is uncertain"),
            )
            .with("feature", id)
            .with("point", place)),
        }
    }

    /// The index of the tool for `exit`: one built for an exit within the
    /// linear resolution, else a new one.
    pub fn tool(&mut self, exit: f64) -> Result<usize> {
        if let Some(i) = self
            .tools
            .iter()
            .position(|(e, _)| (e - exit).abs() <= ryukei::tol::LINEAR_RESOLUTION)
        {
            return Ok(i);
        }
        let d = match self.h.depth {
            Depth::ThroughNext => exit + self.margin,
            _ => self.through,
        };
        let far_top = match &self.s.far {
            Some(derive::FarSizes::Countersink(dia, a)) => {
                exit - (dia.si - self.s.drill.si) / 2.0 / ryukei::math::tan(a.si / 2.0)
            }
            Some(derive::FarSizes::Counterbore(_, fd)) => exit - fd.si,
            None => exit,
        };
        if far_top <= self.s.thread_start() {
            return Err(Error::new(
                Code::InvalidExtent,
                format!("hole {}: its far side reaches up past its mouth", self.id),
            )
            .with("feature", self.id)
            .with("pointer", "/far"));
        }
        let (b, _) = build_tool(
            self.part,
            self.id,
            self.h,
            self.s,
            self.solved,
            self.targets,
            d,
            Some(exit),
        )?;
        self.tools.push((exit, Arc::new(b)));
        Ok(self.tools.len() - 1)
    }
}

/// Whether a feature is a hole whose tool depends on its targets: through
/// all, through the next wall, a far side or a spotface (which reaches
/// past them).
pub fn through(f: &Feature) -> bool {
    matches!(f, Feature::Hole(h) if h.depth.is_through()
        || h.far.is_some()
        || matches!(h.form, HoleForm::Spotface { .. }))
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::doc::files::Feature;
    use crate::doc::id::Id;
    use crate::error::Code;
    use crate::rebuild::Built;
    use crate::session::Session;
    use crate::testing::call;
    use crate::units::Unit;
    use core::f64::consts::PI;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    fn id(s: &str) -> Id {
        Id::parse(s, "feature").unwrap()
    }

    /// Box `b` (20 × 10 × 5 mm) and a sketch `s` on its top face's plane
    /// holding points `p1` at (10, 5) and `p2` at (4, 5) mm, each fixed
    /// (`c1`, `c2`), and a line `l1` from `p3` to `p4` across the box's
    /// far end.
    pub(crate) fn drilled() -> (tempfile::TempDir, Session) {
        let (dir, mut s, _) = crate::sketch::tests::with_box();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "s",
                   "placement": {"origin": [0, 0, 5]}}),
        )
        .unwrap();
        let ops = json!([
            {"op": "add", "type": "point", "at": [10, 5]},
            {"op": "add", "type": "fix", "a": "p1"},
            {"op": "add", "type": "point", "at": [4, 5]},
            {"op": "add", "type": "fix", "a": "p2"},
            {"op": "add", "type": "point", "at": [18, 1]},
            {"op": "add", "type": "point", "at": [18, 9]},
            {"op": "add", "type": "line", "start": "p3", "end": "p4"},
        ]);
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": ops}),
        )
        .unwrap();
        (dir, s)
    }

    /// Writes hole `h` (its fields, in millimetres) after everything in
    /// part `p`, as a file would hold it.
    pub(crate) fn put(s: &mut Session, h: &str, fields: Value) {
        let mut v = fields;
        // Format 3, or the one the fields give (23 for intent).
        if v.get("format").is_none() {
            v["format"] = json!(3);
        }
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("hole");
        v["sketch"] = json!("s");
        if v.get("targets").is_none() {
            v["targets"] = json!(["b"]);
        }
        let f = Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap();
        let pid = id("p");
        let mut list = crate::part::order(s.project().part(&pid).unwrap()).unwrap();
        if !list.contains(&id(h)) {
            list.push(id(h));
        }
        crate::save::save(s, &pid, &[(&id(h), &f)], list).unwrap();
    }

    /// What hole `h` left of box `b`, or why it failed.
    fn after(s: &mut Session, h: &str) -> Result<Built, crate::error::Error> {
        let r = s.rebuilt(&id("p")).unwrap();
        let (_, out) = r.features.iter().find(|(f, _)| *f == id(h)).unwrap();
        out.clone()
            .map(|o| o.solid(&id("b")).expect("the box").clone())
    }

    /// The volume a body encloses, in mm³, meshed finely.
    fn volume(b: &Built) -> f64 {
        assert_eq!(ryukei::check::check_intersections(&b.body), []);
        let m =
            ryukei::mesh::tessellate(&b.body, &ryukei::mesh::MeshParams::new(2e-6, 0.1)).unwrap();
        assert!(m.faults.is_empty(), "{:?}", m.faults);
        ryukei_testkit::mesh::volume_and_area(&m).0 * 1e9
    }

    fn near(got: f64, want: f64) {
        assert!((got - want).abs() < 2e-4 * want, "{got} mm³ != {want} mm³");
    }

    #[test]
    fn a_blind_thread_splits_the_wall_where_it_ends() {
        let (_dir, mut s) = drilled();
        let tapped = |thread_depth: &str| {
            json!({"format": 23, "points": ["p1"], "form": "simple",
                   "thread": {"standard": "iso_metric", "size": "M3", "depth": thread_depth},
                   "depth": {"blind": "4 mm"}, "bottom": {"drill": "118 deg"}})
        };
        let segments = |b: &Built| -> BTreeSet<String> {
            b.origins
                .values()
                .filter(|o| o["feature"] == "h")
                .filter_map(|o| o["segment"].as_str().map(str::to_owned))
                .collect()
        };
        put(&mut s, "h", tapped("2.5 mm"));
        let b = after(&mut s, "h").unwrap();
        assert_eq!(
            segments(&b),
            ["drill_wall", "tip", "wall"].map(String::from).into()
        );
        // The same volume as one wall: a 2.5 mm bore (M3's rule) 4 mm
        // deep, and its point.
        let tip = 1.25 / ryukei::math::tan(59f64.to_radians());
        near(
            volume(&b),
            1000.0 - PI * 1.25 * 1.25 * 4.0 - PI * 1.25 * 1.25 * tip / 3.0,
        );
        let names = |b: &Built| b.origins.keys().cloned().collect::<BTreeSet<_>>();
        let before = names(&b);
        put(&mut s, "h", tapped("3 mm"));
        assert_eq!(
            names(&after(&mut s, "h").unwrap()),
            before,
            "a deeper thread renames nothing"
        );
        // Full depth: no split.
        put(&mut s, "h", tapped("full"));
        let b = after(&mut s, "h").unwrap();
        assert!(!segments(&b).contains("drill_wall"));
        // A thread that ends inside its counterbore is refused.
        let mut v = tapped("1 mm");
        v["form"] = json!("counterbore");
        v["counterbore"] = json!({"diameter": "6 mm", "depth": "1.5 mm"});
        put(&mut s, "h", v);
        let e = after(&mut s, "h").unwrap_err();
        assert_eq!(e.details["pointer"], "/thread/depth", "{e}");
    }

    #[test]
    fn holes_cut_their_forms_from_the_solid() {
        let (_dir, mut s) = drilled();
        // A cone of radius r and height h, and a frustum between radii.
        let cone = |r: f64, h: f64| PI * r * r * h / 3.0;
        let frustum = |a: f64, b: f64, h: f64| PI * h * (a * a + a * b + b * b) / 3.0;
        let tip = 2.0 / ryukei::math::tan(59f64.to_radians());
        for (fields, removed) in [
            (
                json!({"points": ["p1"], "form": "simple", "diameter": 4,
                       "depth": {"blind": 3}}),
                PI * 4.0 * 3.0,
            ),
            (
                json!({"points": ["p1"], "form": "simple", "diameter": 4,
                       "depth": "through_all"}),
                PI * 4.0 * 5.0,
            ),
            (
                json!({"points": ["p1"], "form": "simple", "diameter": 4,
                       "depth": {"blind": 3}, "bottom": {"drill": "118 deg"}}),
                PI * 4.0 * 3.0 + cone(2.0, tip),
            ),
            (
                json!({"points": ["p1"], "form": "counterbore", "diameter": 4,
                       "counterbore": {"diameter": 8, "depth": 1},
                       "depth": {"blind": 3}}),
                PI * 16.0 + PI * 4.0 * 2.0,
            ),
            (
                json!({"points": ["p1"], "form": "countersink", "diameter": 4,
                       "countersink": {"diameter": 8, "angle": 90},
                       "depth": "through_all"}),
                frustum(4.0, 2.0, 2.0) + PI * 4.0 * 3.0,
            ),
            // Both points, one tool each in one cut.
            (
                json!({"points": ["p1", "p2"], "form": "simple", "diameter": 2,
                       "depth": {"blind": 3}}),
                2.0 * PI * 3.0,
            ),
            // Every point no curve uses: the line's ends aren't drilled.
            (
                json!({"form": "simple", "diameter": 2, "depth": {"blind": 3}}),
                2.0 * PI * 3.0,
            ),
        ] {
            put(&mut s, "h", fields.clone());
            let b = after(&mut s, "h").unwrap_or_else(|e| panic!("{fields}: {e}"));
            near(volume(&b), 1000.0 - removed);
        }
    }

    #[test]
    fn up_to_a_face() {
        let (_dir, mut s) = drilled();
        // The box's bottom face, then 1 mm short of it.
        let r = s.rebuilt(&id("p")).unwrap();
        let b = r.output(&id("b")).unwrap().body().unwrap().clone();
        let bottom = b
            .body
            .faces()
            .find(|f| ryukei::intersect::face_box(&b.body, *f).max().z.abs() < 1e-12)
            .unwrap();
        let name = b.body.face(bottom).unwrap().name.to_string();
        let target = json!({"feature": "b", "name": name});
        for (offset, depth) in [(None, 5.0), (Some("-1 mm"), 4.0)] {
            let mut up = json!({"target": target});
            if let Some(o) = offset {
                up["offset"] = json!(o);
            }
            put(
                &mut s,
                "h",
                json!({"points": ["p1"], "form": "simple", "diameter": 4,
                       "depth": {"up_to": up}}),
            );
            near(
                volume(&after(&mut s, "h").unwrap()),
                1000.0 - PI * 4.0 * depth,
            );
        }
    }

    #[test]
    fn holes_that_wont_do() {
        let (_dir, mut s) = drilled();
        let simple = |points: Value| json!({"points": points, "form": "simple", "diameter": 2, "depth": {"blind": 3}});
        // A point off the box cuts nothing: the error names it.
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s",
                   "ops": [{"op": "add", "type": "point", "at": [40, 5]}]}),
        )
        .unwrap();
        put(&mut s, "h", simple(json!(["p1", "p5"])));
        let e = after(&mut s, "h").unwrap_err();
        assert_eq!(e.code, Code::OperationNoEffect, "{e}");
        assert_eq!(
            (&e.details["tool"], &e.details["point"]),
            (&json!("h"), &json!("p5"))
        );
        for (points, code, pointer) in [
            (json!(["p9"]), Code::NotFound, "/points/0"),
            (json!(["p1", "l1"]), Code::ReferenceWrongKind, "/points/1"),
        ] {
            put(&mut s, "h", simple(points.clone()));
            let e = after(&mut s, "h").unwrap_err();
            assert_eq!(
                (e.code, e.details["pointer"].as_str()),
                (code, Some(pointer)),
                "{points}: {e}"
            );
        }
        // A counterbore deeper than an up-to depth reaches: 1.5 mm, to
        // 3.5 mm short of the bottom.
        let r = s.rebuilt(&id("p")).unwrap();
        let b = r.output(&id("b")).unwrap().body().unwrap().clone();
        let bottom = b
            .body
            .faces()
            .find(|f| ryukei::intersect::face_box(&b.body, *f).max().z.abs() < 1e-12)
            .unwrap();
        let name = b.body.face(bottom).unwrap().name.to_string();
        put(
            &mut s,
            "h",
            json!({"points": ["p1"], "form": "counterbore", "diameter": 2,
                   "counterbore": {"diameter": 4, "depth": 2},
                   "depth": {"up_to": {"target": {"feature": "b", "name": name},
                                       "offset": "-3.5 mm"}}}),
        );
        let e = after(&mut s, "h").unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidExtent, Some("/counterbore/depth")),
            "{e}"
        );
        // Through all from a sketch on the box's bottom: a hole drills
        // against its sketch's normal, so the refusal says what a hole can
        // do about it (#92).
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "s", "placement": {"origin": [0, 0, 0]}}),
        )
        .unwrap();
        put(
            &mut s,
            "h",
            json!({"points": ["p1"], "form": "simple", "diameter": 2, "depth": "through_all"}),
        );
        let e = after(&mut s, "h").unwrap_err();
        assert_eq!(
            (e.code, e.details["pointer"].as_str()),
            (Code::InvalidExtent, Some("/depth")),
            "{e}"
        );
        assert!(!e.message.contains("reverse"), "{e}");
    }

    #[test]
    fn the_point_the_kernel_refuses_is_named() {
        let (_dir, mut s) = drilled();
        // A bore of radius 2 mm about (2, 5) touches the box's end face
        // along a line: the cut would leave it touching itself.
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s",
                   "ops": [{"op": "add", "type": "point", "at": [2, 5]},
                           {"op": "add", "type": "fix", "a": "p5"}]}),
        )
        .unwrap();
        put(
            &mut s,
            "h",
            json!({"points": ["p1", "p5"], "form": "simple", "diameter": 4,
                   "depth": {"blind": 3}}),
        );
        let e = after(&mut s, "h").unwrap_err();
        assert_eq!(e.code, Code::BooleanFailed, "{e}");
        assert_eq!(e.details["point"], "p5", "{e}");
    }

    #[test]
    fn resizing_and_moving_rename_nothing() {
        let (_dir, mut s) = drilled();
        let hole = |d: f64| {
            json!({"points": ["p1", "p2"], "form": "counterbore", "diameter": d,
                   "counterbore": {"diameter": 2.0 * d, "depth": 1},
                   "depth": {"blind": 3}})
        };
        put(&mut s, "h", hole(1.5));
        let before = after(&mut s, "h").unwrap();
        let names = |b: &Built| {
            let mut n: Vec<String> = b.origins.keys().cloned().collect();
            n.sort();
            n
        };
        put(&mut s, "h", hole(2.0));
        let resized = after(&mut s, "h").unwrap();
        assert_eq!(names(&resized), names(&before));
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s",
                   "ops": [{"op": "remove", "id": "c2"},
                           {"op": "move", "id": "p2", "at": [6, 4]},
                           {"op": "add", "type": "fix", "a": "p2"}]}),
        )
        .unwrap();
        let moved = after(&mut s, "h").unwrap();
        assert_eq!(names(&moved), names(&before));
        // Every face the hole made says which point and segment.
        let walls: Vec<&Value> = moved
            .origins
            .values()
            .filter(|o| o["feature"] == "h" && o.get("segment").is_some())
            .collect();
        let points: std::collections::BTreeSet<&str> =
            walls.iter().map(|o| o["point"].as_str().unwrap()).collect();
        assert_eq!(points, ["p1", "p2"].into());
        let segments: std::collections::BTreeSet<&str> = walls
            .iter()
            .map(|o| o["segment"].as_str().unwrap())
            .collect();
        assert_eq!(
            segments,
            ["counterbore_floor", "counterbore_wall", "wall", "bottom"].into()
        );
    }

    #[test]
    fn a_tapped_hole_builds_at_its_tap_drill_and_reports_its_spec() {
        let (dir, mut s) = drilled();
        put(
            &mut s,
            "h",
            json!({"format": 23, "points": ["p1"], "form": "simple",
                                "thread": {"standard": "iso_metric", "size": "M6"},
                                "depth": "through_all"}),
        );
        let b = after(&mut s, "h").unwrap();
        // A Ø5.0 bore through the 5 mm box.
        near(volume(&b), 1000.0 - PI * 2.5 * 2.5 * 5.0);
        // `feature.show` gives the report itself.
        let shown = call(&mut s, "feature.show", json!({"part": "p", "id": "h"})).unwrap();
        assert_eq!(shown["spec"], "M6x1.0-6H THRU");
        assert_eq!(shown["sizes"]["drill"]["value"], "5 mm");
        let r = call(&mut s, "rebuild", json!({"part": "p"})).unwrap();
        let entry = r["parts"][0]["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "h")
            .unwrap();
        assert_eq!(entry["spec"], "M6x1.0-6H THRU");
        // A table entry the hole reads moves its hash; one it doesn't, not.
        let hash = entry["hash"].clone();
        let write = |t: &Value| {
            std::fs::write(
                dir.path().join("hole_standards.json"),
                crate::doc::canonical::to_string(t),
            )
            .unwrap()
        };
        let mut t = crate::doc::fixtures::house_standards();
        t["threads"]["iso_metric"]
            .as_object_mut()
            .unwrap()
            .remove("M6");
        write(&t);
        s.reload().unwrap();
        let again = |s: &mut Session| {
            let r = call(s, "rebuild", json!({"part": "p"})).unwrap();
            r["parts"][0]["features"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["id"] == "h")
                .unwrap()["hash"]
                .clone()
        };
        assert_eq!(again(&mut s), hash);
        write(&crate::doc::fixtures::house_standards()); // M6's house tap drill, 5.1 mm
        s.reload().unwrap();
        assert_ne!(again(&mut s), hash);
        near(
            volume(&after(&mut s, "h").unwrap()),
            1000.0 - PI * 2.55 * 2.55 * 5.0,
        );
    }

    #[test]
    fn holes_from_a_spec_and_dry_runs() {
        let (_dir, mut s) = drilled();
        let added = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "hole", "id": "h",
            "sketch": "s", "points": ["p1"], "spec": "M6x1.0-6H THRU", "targets": ["b"]}),
        )
        .unwrap();
        let f = &added["feature"];
        assert_eq!(f["spec"], "M6x1.0-6H THRU");
        assert_eq!(f["thread"], json!({"standard": "iso_metric", "size": "M6"}));
        assert_eq!(f["sizes"]["drill"]["value"], "5 mm");
        assert!(f.get("diameter").is_none());
        // A field set by the spec and beside it is usage.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "spec": "M8 THRU", "depth": "through_all"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage, "{e}");
        // A dry run answers and writes nothing.
        let before = call(&mut s, "feature.show", json!({"part": "p", "id": "h"})).unwrap();
        let dry = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "spec": "M8x1.25-6H 10 DEEP, DRILL 12 DEEP", "dry_run": true}),
        )
        .unwrap();
        assert_eq!(dry["dry_run"], true);
        assert_eq!(dry["feature"]["sizes"]["drill"]["value"], "6.8 mm");
        assert_eq!(
            call(&mut s, "feature.show", json!({"part": "p", "id": "h"})).unwrap(),
            before
        );
        // Partial thread fields lay over the current thread.
        let edited = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h", "class": "5H"}),
        )
        .unwrap();
        assert_eq!(edited["feature"]["thread"]["class"], "5H");
        let edited = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h", "pitch": "0.75 mm"}),
        )
        .unwrap();
        assert_eq!(edited["feature"]["thread"]["pitch"], "0.75 mm");
        // What a table must give is refused when written, dry run or not.
        for dry_run in [true, false] {
            let e = call(
                &mut s,
                "feature.add",
                json!({"part": "p", "type": "hole", "id": "c",
                "sketch": "s", "points": ["p2"], "spec": "CBORE HOUSE_CAP M5 THRU",
                "targets": ["b"], "dry_run": dry_run}),
            )
            .unwrap_err();
            assert_eq!(e.details["reason"], "needs_project_table");
            assert_eq!(
                e.details["missing"],
                json!(["clearance.normal", "counterbore"])
            );
        }
        // "" takes a thread away: the hole needs its diameter again.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h", "thread": ""}),
        )
        .unwrap_err();
        assert_eq!(e.details["pointer"], "/diameter");
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h", "thread": "", "diameter": "5 mm"}),
        )
        .unwrap();
    }

    #[test]
    fn new_forms_depths_and_bottoms_cut_what_they_say() {
        let (_dir, mut s) = drilled();
        let cone = |r: f64, h: f64| PI * r * r * h / 3.0;
        let frustum = |a: f64, b: f64, h: f64| PI * h * (a * a + a * b + b * b) / 3.0;
        let t59 = ryukei::math::tan(59f64.to_radians());
        for (fields, removed) in [
            // A counterdrill: 8 mm to 1 mm, its 118 deg floor down to the
            // 4 mm bore, the bore through.
            (
                json!({"format": 25, "points": ["p1"], "form": "counterdrill", "diameter": 4,
                       "counterdrill": {"diameter": 8, "depth": 1}, "depth": "through_all"}),
                {
                    let floor = 2.0 / t59;
                    PI * 16.0 * 1.0 + frustum(4.0, 2.0, floor) + PI * 4.0 * (5.0 - 1.0 - floor)
                },
            ),
            // A spotface 0.5 mm deep: the top 0.5 mm within 8 mm goes.
            (
                json!({"format": 25, "points": ["p1"], "form": "spotface", "diameter": 4,
                       "spotface": {"diameter": 8, "depth": 0.5}, "depth": "through_all"}),
                PI * 16.0 * 0.5 + PI * 4.0 * 4.5,
            ),
            // A near chamfer, 6 mm at 90 deg: 1 mm down to the bore.
            (
                json!({"format": 25, "points": ["p1"], "form": "simple", "diameter": 4,
                       "near": {"diameter": 6}, "depth": "through_all"}),
                frustum(3.0, 2.0, 1.0) + PI * 4.0 * 4.0,
            ),
            // To the tip: 3 mm to the tip, the shoulder above it.
            (
                json!({"format": 25, "points": ["p1"], "form": "simple", "diameter": 4,
                       "depth": {"tip": 3}, "bottom": {"drill": "118 deg"}}),
                PI * 4.0 * (3.0 - 2.0 / t59) + cone(2.0, 2.0 / t59),
            ),
            // A ball bottom: 3 mm to the shoulder, then a hemisphere.
            (
                json!({"format": 25, "points": ["p1"], "form": "simple", "diameter": 3,
                       "depth": {"blind": 2}, "bottom": "ball"}),
                PI * 2.25 * 2.0 + 2.0 / 3.0 * PI * 1.5 * 1.5 * 1.5,
            ),
        ] {
            put(&mut s, "h", fields.clone());
            let b = after(&mut s, "h").unwrap_or_else(|e| panic!("{fields}: {e}"));
            near(volume(&b), 1000.0 - removed);
        }
        // Each new segment has its name, and none of them is a size.
        let segs = |b: &Built| -> BTreeSet<String> {
            b.origins
                .values()
                .filter(|o| o["feature"] == "h")
                .filter_map(|o| o["segment"].as_str().map(str::to_owned))
                .collect()
        };
        put(
            &mut s,
            "h",
            json!({"format": 25, "points": ["p1"], "form": "counterdrill",
            "diameter": 4, "counterdrill": {"diameter": 8, "depth": 1}, "near": {"diameter": 9},
            "depth": {"blind": 2.5}, "bottom": "ball"}),
        );
        assert_eq!(
            segs(&after(&mut s, "h").unwrap()),
            [
                "ball",
                "counterdrill_floor",
                "counterdrill_wall",
                "near",
                "wall"
            ]
            .map(String::from)
            .into()
        );
        // A tip depth no deeper than the point is refused.
        put(
            &mut s,
            "h",
            json!({"format": 25, "points": ["p1"], "form": "simple", "diameter": 4,
            "depth": {"tip": 1}, "bottom": {"drill": "118 deg"}}),
        );
        assert_eq!(
            after(&mut s, "h").unwrap_err().details["pointer"],
            "/depth/tip"
        );
    }

    /// Box b (z 0 to 5 mm), a 10 x 10 x 5 mm box joined under its left
    /// half (z -5 to 0), and `p5` at (16, 5) on the sketch beside `p2`
    /// (4, 5): p2's axis leaves at 10 mm, p5's at 5 mm.
    fn stepped() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = drilled();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "under",
            "operation": "join", "targets": ["b"], "params": {"x": 10, "y": 10, "z": 5},
            "placement": {"origin": [0, 0, -5]}}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "s", "ops": [
            {"op": "add", "type": "point", "at": [16, 5]},
            {"op": "add", "type": "fix", "a": "p5"}]}),
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn far_sides_sit_at_each_places_exit() {
        let (_dir, mut s) = stepped();
        put(
            &mut s,
            "h",
            json!({"format": 25, "points": ["p2", "p5"], "form": "simple",
            "diameter": 2, "depth": "through_all",
            "far": {"counterbore": {"diameter": 4, "depth": 1}}}),
        );
        let r = s.rebuilt(&id("p")).unwrap();
        let (_, out) = r.features.iter().find(|(f, _)| *f == id("h")).unwrap();
        let out = out.as_ref().unwrap();
        let b = out.solid(&id("b")).unwrap();
        // p2: 9 mm of bore and the counterbore's 1 mm; p5: 4 mm and 1 mm.
        near(
            volume(b),
            1500.0 - (PI * 9.0 + PI * 4.0) - (PI * 4.0 + PI * 4.0),
        );
        // Two tools: p5's differs from p2's.
        let applied = out.applied().unwrap();
        assert_eq!(applied.others.len(), 1);
        assert_eq!(applied.others[0].0, 1);
        // Both places' far faces carry their names.
        let far: BTreeSet<&str> = b
            .origins
            .values()
            .filter(|o| o["feature"] == "h" && o["segment"] == "far_counterbore_wall")
            .filter_map(|o| o["point"].as_str())
            .collect();
        assert_eq!(far, ["p2", "p5"].into());
    }

    #[test]
    fn through_next_stops_at_the_first_wall() {
        // b and `up` (z 5 to 10) make 10 mm; a slot (x 2 to 18, z 4 to 6)
        // leaves two walls along the axis at (10, 5).
        let (_dir, mut s) = drilled();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "up",
            "operation": "join", "targets": ["b"], "params": {"x": 20, "y": 10, "z": 5},
            "placement": {"origin": [0, 0, 5]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "slot",
            "operation": "cut", "targets": ["b"], "params": {"x": 16, "y": 12, "z": 2},
            "placement": {"origin": [2, -1, 4]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "t",
            "placement": {"origin": [0, 0, 10]}}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "t", "ops": [
            {"op": "add", "type": "point", "at": [10, 5]},
            {"op": "add", "type": "fix", "a": "p1"}]}),
        )
        .unwrap();
        let mut v = json!({"format": 25, "form": "simple", "diameter": 2,
                           "depth": "through_next"});
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("hole");
        v["sketch"] = json!("t");
        v["targets"] = json!(["b"]);
        let f = Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap();
        let mut list = crate::part::order(s.project().part(&id("p")).unwrap()).unwrap();
        list.push(id("n"));
        crate::save::save(&mut s, &id("p"), &[(&id("n"), &f)], list).unwrap();
        let before = 20.0 * 10.0 * 10.0 - 16.0 * 10.0 * 2.0;
        near(volume(&after(&mut s, "n").unwrap()), before - PI * 4.0);
        // Through all goes on through the lower wall.
        v["depth"] = json!("through_all");
        v["format"] = json!(3);
        let f = Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap();
        let list = crate::part::order(s.project().part(&id("p")).unwrap()).unwrap();
        crate::save::save(&mut s, &id("p"), &[(&id("n"), &f)], list).unwrap();
        near(volume(&after(&mut s, "n").unwrap()), before - PI * 8.0);
    }

    #[test]
    fn a_pattern_of_a_far_sided_hole_measures_each_instance() {
        let (_dir, mut s) = stepped();
        // The seed at p5 (16, 5) leaves at 5 mm; its copy 12 mm along -x,
        // at (4, 5), at 10 mm; its mirror in x = 10 mm, at (4, 5) too.
        put(
            &mut s,
            "h",
            json!({"format": 25, "points": ["p5"], "form": "simple",
            "diameter": 2, "depth": "through_all",
            "far": {"counterbore": {"diameter": 4, "depth": 1}}}),
        );
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "pattern", "id": "row",
            "features": ["h"], "direction": "x", "reverse": true, "spacing": 12, "count": 2}),
        )
        .unwrap();
        let r = s.rebuilt(&id("p")).unwrap();
        let (_, out) = r.features.iter().find(|(f, _)| *f == id("row")).unwrap();
        let b = out.as_ref().unwrap().solid(&id("b")).unwrap();
        near(
            volume(b),
            1500.0 - (PI * 4.0 + PI * 4.0) - (PI * 9.0 + PI * 4.0),
        );
    }

    #[test]
    fn a_tapered_pipe_wall_is_a_cone_from_the_gauge_plane() {
        let (dir, mut s) = drilled();
        // House values, made up for this test: never a standard's.
        std::fs::write(
            dir.path().join("hole_standards.json"),
            crate::doc::canonical::to_string(&json!({
                "format": 1, "kind": "ryucad.hole_standards",
                "threads": {"npt": {"1/8": {"tap_drill": "8.7 mm",
                    "gauge_diameter": "10.2 mm", "gauge_length": "4 mm"}}}})),
        )
        .unwrap();
        s.reload().unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "t",
            "params": {"x": 30, "y": 30, "z": 20}, "placement": {"origin": [40, 0, 0]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sketch", "id": "u",
            "placement": {"origin": [0, 0, 20]}}),
        )
        .unwrap();
        call(
            &mut s,
            "sketch.edit",
            json!({"part": "p", "sketch": "u", "ops": [
            {"op": "add", "type": "point", "at": [55, 15]},
            {"op": "add", "type": "fix", "a": "p1"}]}),
        )
        .unwrap();
        let v = json!({"format": 25, "kind": "ryucad.feature", "type": "hole", "sketch": "u",
            "form": "simple", "targets": ["t"], "depth": {"blind": "12 mm"},
            "thread": {"standard": "npt", "size": "1/8", "depth": "8 mm", "wall": "taper"}});
        let f = Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap();
        let mut list = crate::part::order(s.project().part(&id("p")).unwrap()).unwrap();
        list.push(id("n"));
        crate::save::save(&mut s, &id("p"), &[(&id("n"), &f)], list).unwrap();
        let r = s.rebuilt(&id("p")).unwrap();
        let (_, out) = r.features.iter().find(|(f, _)| *f == id("n")).unwrap();
        let b = out
            .as_ref()
            .unwrap_or_else(|e| panic!("{e}"))
            .solid(&id("t"))
            .unwrap();
        // 1:16 on the diameter: 10.45 mm at the mouth, 9.95 mm at 8 mm,
        // then the annulus in to the 8.7 mm drill, 4 mm more of it.
        let frustum = |a: f64, c: f64, h: f64| PI * h * (a * a + a * c + c * c) / 3.0;
        near(
            volume(b),
            18000.0 - frustum(5.225, 4.975, 8.0) - PI * 4.35 * 4.35 * 4.0,
        );
        let segs: BTreeSet<&str> = b
            .origins
            .values()
            .filter(|o| o["feature"] == "n")
            .filter_map(|o| o["segment"].as_str())
            .collect();
        assert!(
            segs.contains("taper_end") && segs.contains("drill_wall"),
            "{segs:?}"
        );
        // No gauge values: needs_project_table names them.
        std::fs::write(
            dir.path().join("hole_standards.json"),
            crate::doc::canonical::to_string(&json!({"format": 1, "kind": "ryucad.hole_standards",
                "threads": {"npt": {"1/8": {"tap_drill": "8.7 mm"}}}})),
        )
        .unwrap();
        s.reload().unwrap();
        // hole.derive takes a hole's fields, not the file's header.
        let mut fields = v.clone();
        for k in ["format", "kind", "type"] {
            fields.as_object_mut().unwrap().remove(k);
        }
        let e = call(&mut s, "hole.derive", json!({"fields": fields})).unwrap_err();
        assert_eq!(
            e.details["missing"],
            json!(["gauge_diameter", "gauge_length"])
        );
    }

    #[test]
    fn the_new_fields_through_the_protocol() {
        let (_dir, mut s) = drilled();
        let add = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "hole", "id": "h",
            "sketch": "s", "points": ["p1"], "diameter": "2 mm", "form": "counterdrill",
            "counterdrill": {"diameter": "4 mm", "depth": "1 mm"}, "depth": "through_all",
            "near": {"diameter": "5 mm"}, "targets": ["b"]}),
        )
        .unwrap();
        let f = &add["feature"];
        assert_eq!(f["counterdrill"]["diameter"]["value"], "4 mm");
        assert_eq!(f["near"]["diameter"]["value"], "5 mm");
        assert_eq!(f["spec"], "DIA 2 THRU, CDRILL 4 1 DEEP, NEAR 5");
        // Part of a mouth is laid over the rest; a new form drops the old.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "counterdrill": {"angle": "90 deg"}}),
        )
        .unwrap();
        assert_eq!(e["feature"]["counterdrill"]["depth"]["value"], "1 mm");
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "form": "spotface", "spotface": {"diameter": "6 mm"}}),
        )
        .unwrap();
        assert!(e["feature"].get("counterdrill").is_none());
        // "" takes near or far away.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h", "near": ""}),
        )
        .unwrap();
        assert!(e["feature"].get("near").is_none());
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "far": {"countersink": {"diameter": "3 mm", "angle": "90 deg"}}}),
        )
        .unwrap();
        assert_eq!(
            e["feature"]["far"]["countersink"]["angle"]["value"],
            "90 deg"
        );
        // A spec sets them; given twice is usage.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "spec": "DIA 2 THRU NEXT, NEAR 3", "near": {"diameter": "3 mm"}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Usage);
        // A spec naming no far side keeps it: through_next takes one.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "spec": "DIA 2 THRU NEXT"}),
        )
        .unwrap();
        assert_eq!(e["feature"]["depth"], "through_next");
        assert!(e["feature"].get("far").is_some());
        // A thread's wall, given flat, needs a thread.
        let e = call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "h",
            "thread_wall": "taper"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidParam, "{e}");
    }
}
