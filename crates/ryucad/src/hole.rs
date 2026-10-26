//! Holes to ryukei tools. A hole's profile lies in the half-plane through
//! its axis: the bore, a counterbore or countersink at its mouth, and a
//! flat or drilled bottom, as lines from the axis out and back. It is
//! turned all round once, at the sketch's origin against its normal, and
//! placed at each point, so every point's tool is the same body under a
//! key of its own.
//!
//! A segment's key is SipHash of `ryucad.hole\0<part>\0<feature>\0<segment>`
//! (`top`, `counterbore_wall`, `counterbore_floor`, `countersink`, `wall`,
//! `bottom`, `tip`, `axis`), never of a size, so resizing a hole renames
//! nothing and switching its form renames only what its new segments
//! touch. A point's tool key is `ryucad.tool\0<part>\0<feature>\0<point>`.

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
use ryukei::geom::{Axis, Curve2, Frame, Line2, Point2, Point3, Vector3};
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
fn depth(
    part: &Part,
    id: &Id,
    h: &Hole,
    from: Point3,
    dir: Vector3,
    targets: &[&Built],
    before: &dyn Results,
) -> Result<f64> {
    let d = match &h.depth {
        Depth::Blind(q) => return Ok(q.meters()),
        Depth::ThroughAll => crate::rebuild::through_all("hole", id, from, dir, false, targets)?.1,
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
    if let Some((mouth, at)) = h.mouth_depth()
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

/// The profile's corners, `(radius, depth)` from the mouth on the axis,
/// each with the segment that leaves it: out along the mouth, down the
/// counterbore or countersink and the bore, across the bottom or down the
/// drill point to the axis, and up the axis.
fn corners(h: &Hole, depth: f64) -> Vec<((f64, f64), &'static str)> {
    let r = h.diameter.meters() / 2.0;
    let mut out = vec![((0.0, 0.0), "top")];
    match &h.form {
        HoleForm::Simple => out.push(((r, 0.0), "wall")),
        HoleForm::Counterbore { diameter, depth } => {
            let rc = diameter.meters() / 2.0;
            out.push(((rc, 0.0), "counterbore_wall"));
            out.push(((rc, depth.meters()), "counterbore_floor"));
            out.push(((r, depth.meters()), "wall"));
        }
        HoleForm::Countersink { diameter, .. } => {
            let (d, _) = h.mouth_depth().expect("a countersink has a depth");
            out.push(((diameter.meters() / 2.0, 0.0), "countersink"));
            out.push(((r, d), "wall"));
        }
    }
    match &h.bottom {
        Bottom::Flat => {
            out.push(((r, depth), "bottom"));
            out.push(((0.0, depth), "axis"));
        }
        Bottom::Drill(angle) => {
            out.push(((r, depth), "tip"));
            let point = r / ryukei::math::tan(angle.radians() / 2.0);
            out.push(((0.0, depth + point), "axis"));
        }
    }
    out
}

/// The hole's tool: its profile turned all round its axis, at the
/// sketch's origin, drilling against the sketch's normal; and its frame's
/// origin, from which each point's tool is placed.
pub fn tool(
    part: &Part,
    id: &Id,
    h: &Hole,
    solved: &Solved,
    targets: &[&Built],
    before: &dyn Results,
) -> Result<(Built, Point3)> {
    let sketch = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let (origin, down) = (sketch.origin(), -sketch.z().get());
    let d = depth(part, id, h, origin, down, targets, before)?;
    // The profile's plane holds the axis: x across, y down the hole.
    let frame = Frame::new(origin, sketch.y().get(), sketch.x().get()).map_err(|e| {
        Error::new(Code::Internal, "a hole's frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let corners = corners(h, d);
    let mut keys = Keys::default();
    let segments: Vec<Segment> = corners
        .iter()
        .enumerate()
        .map(|(i, ((r0, d0), name))| {
            let ((r1, d1), _) = corners[(i + 1) % corners.len()];
            let (a, b) = (Point2::new(*r0, *d0), Point2::new(r1, d1));
            let k = segment_key(&part.id, id, name);
            keys.segments.insert(
                k,
                Id::parse(name, "entity").expect("a segment's name is an ID"),
            );
            let line = Line2::through(a, b).expect("a hole's corners are apart");
            Segment {
                key: SegmentKey(k),
                curve: Curve2::Line2(line),
                t0: 0.0,
                t1: a.distance(b),
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
    let origins = crate::profile::origins(key, &h.sketch, &valid, &keys)
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

/// Whether a feature is a hole whose depth reaches past its targets.
pub fn through(f: &Feature) -> bool {
    matches!(f, Feature::Hole(h) if h.depth == Depth::ThroughAll)
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::doc::files::Feature;
    use crate::doc::id::Id;
    use crate::error::Code;
    use crate::rebuild::Built;
    use crate::server::tests::call;
    use crate::session::Session;
    use crate::units::Unit;
    use core::f64::consts::PI;
    use serde_json::{Value, json};

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
        v["format"] = json!(3);
        v["kind"] = json!("ryucad.feature");
        v["type"] = json!("hole");
        v["sketch"] = json!("s");
        if v.get("targets").is_none() {
            v["targets"] = json!(["b"]);
        }
        let f = Feature::from_json(&v, Unit::Mm, &crate::expr::Env::empty()).unwrap();
        let pid = id("p");
        let mut list = crate::server::order(s.project().part(&pid).unwrap()).unwrap();
        if !list.contains(&id(h)) {
            list.push(id(h));
        }
        crate::server::save(s, &pid, &[(&id(h), &f)], list).unwrap();
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
}
