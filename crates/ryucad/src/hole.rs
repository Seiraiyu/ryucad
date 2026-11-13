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
use std::collections::BTreeSet;

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

/// The solved sketch a hole drills at, and the points it drills: those
/// it lists, else every point no curve uses, in natural order (`p2` before `p10`). A listed ID
/// that isn't a point of the sketch is `not_found` or
/// `reference_wrong_kind`; a sketch with no point to drill is
/// `invalid_param`.
pub fn points(id: &Id, h: &Hole, solved: &Solved) -> Result<Vec<(Id, Point3)>> {
    let s = &solved.sketch;
    let at = |m: [f64; 2]| solved.frame.apply_point(Point3::new(m[0], m[1], 0.0));
    if !h.points.is_empty() {
        return h
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| match s.entities.get(p).map(|e| &e.geometry) {
                Some(Geometry::Point { m }) => Ok((p.clone(), at(*m))),
                Some(g) => Err(Error::new(
                    Code::ReferenceWrongKind,
                    format!("hole {id}: {p} is a {}, not a point", g.type_name()),
                )
                .with("feature", id)
                .with("entity", p)
                .with("pointer", format!("/points/{i}"))),
                None => Err(Error::new(
                    Code::NotFound,
                    format!("hole {id}: sketch {} has no {p}", h.sketch),
                )
                .with("feature", id)
                .with("entity", p)
                .with("pointer", format!("/points/{i}"))),
            })
            .collect();
    }
    let used: BTreeSet<&Id> = s
        .entities
        .values()
        .flat_map(|e| match &e.geometry {
            Geometry::Point { .. } => Vec::new(),
            Geometry::Line { start, end } => vec![start, end],
            Geometry::Arc { center, start, end } => vec![center, start, end],
            Geometry::Circle { center, .. } => vec![center],
        })
        .collect();
    let mut free: Vec<(Id, Point3)> = s
        .entities
        .iter()
        .filter(|(p, _)| !used.contains(p))
        .filter_map(|(p, e)| match e.geometry {
            Geometry::Point { m } => Some((p.clone(), at(m))),
            _ => None,
        })
        .collect();
    free.sort_by(|(a, _), (b, _)| crate::profile::natural(a).cmp(&crate::profile::natural(b)));
    if free.is_empty() {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "hole {id}: sketch {} has no point that no curve uses; list the points to drill",
                h.sketch
            ),
        )
        .with("feature", id)
        .with("pointer", "/points"));
    }
    Ok(free)
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
fn corners(h: &Hole, s: &Sizes, depth: f64, up: f64) -> Vec<Corner> {
    let r = s.drill.si / 2.0;
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
        out.push(line((r, cdepth.si), "wall"));
    } else if let Some((d, _)) = &s.countersink {
        let z = s.mouth_depth().expect("a countersink has a depth");
        out.push(line((d.si / 2.0, 0.0), "countersink"));
        out.push(line((r, z), "wall"));
    } else if let Some((d, cdepth, a)) = &s.counterdrill {
        let rc = d.si / 2.0;
        let z = near(&mut out, 0.0, rc);
        out.push(line((rc, z), "counterdrill_wall"));
        out.push(line((rc, cdepth.si), "counterdrill_floor"));
        out.push(line((r, cdepth.si + (rc - r) / tan(a)), "wall"));
    } else if let Some((d, sdepth)) = &s.spotface {
        let rs = d.si / 2.0;
        out.push(line((rs, -up), "spotface_wall"));
        out.push(line((rs, sdepth.si), "spotface_floor"));
        let z = near(&mut out, sdepth.si, r);
        out.push(line((r, z), "wall"));
    } else {
        let z = near(&mut out, 0.0, r);
        out.push(line((r, z), "wall"));
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
            out.push(line((r, depth), "bottom"));
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
    out
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
    let sketch = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let (origin, down) = (sketch.origin(), -sketch.z().get());
    let d = depth(part, id, h, s, origin, down, targets, before)?;
    // The profile's plane holds the axis: x across, y down the hole.
    let frame = Frame::new(origin, sketch.y().get(), sketch.x().get()).map_err(|e| {
        Error::new(Code::Internal, "a hole's frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let up = if s.spotface.is_some() {
        reach_back(origin, down, targets)
    } else {
        0.0
    };
    let corners = corners(h, s, d, up);
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
    let origins: crate::rebuild::Origins = crate::profile::origins(key, &h.sketch, &valid, &keys)
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
                json!({"format": 24, "points": ["p1"], "form": "counterdrill", "diameter": 4,
                       "counterdrill": {"diameter": 8, "depth": 1}, "depth": "through_all"}),
                {
                    let floor = 2.0 / t59;
                    PI * 16.0 * 1.0 + frustum(4.0, 2.0, floor) + PI * 4.0 * (5.0 - 1.0 - floor)
                },
            ),
            // A spotface 0.5 mm deep: the top 0.5 mm within 8 mm goes.
            (
                json!({"format": 24, "points": ["p1"], "form": "spotface", "diameter": 4,
                       "spotface": {"diameter": 8, "depth": 0.5}, "depth": "through_all"}),
                PI * 16.0 * 0.5 + PI * 4.0 * 4.5,
            ),
            // A near chamfer, 6 mm at 90 deg: 1 mm down to the bore.
            (
                json!({"format": 24, "points": ["p1"], "form": "simple", "diameter": 4,
                       "near": {"diameter": 6}, "depth": "through_all"}),
                frustum(3.0, 2.0, 1.0) + PI * 4.0 * 4.0,
            ),
            // To the tip: 3 mm to the tip, the shoulder above it.
            (
                json!({"format": 24, "points": ["p1"], "form": "simple", "diameter": 4,
                       "depth": {"tip": 3}, "bottom": {"drill": "118 deg"}}),
                PI * 4.0 * (3.0 - 2.0 / t59) + cone(2.0, 2.0 / t59),
            ),
            // A ball bottom: 3 mm to the shoulder, then a hemisphere.
            (
                json!({"format": 24, "points": ["p1"], "form": "simple", "diameter": 3,
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
            json!({"format": 24, "points": ["p1"], "form": "counterdrill",
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
            json!({"format": 24, "points": ["p1"], "form": "simple", "diameter": 4,
            "depth": {"tip": 1}, "bottom": {"drill": "118 deg"}}),
        );
        assert_eq!(
            after(&mut s, "h").unwrap_err().details["pointer"],
            "/depth/tip"
        );
    }
}
