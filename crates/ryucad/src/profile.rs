//! Sketches to ryukei profiles: the closed loops a sweep takes, with keys
//! made from the sketch's own IDs, and the kernel's faults mapped back to
//! them.
//!
//! A segment's key is SipHash of `ryucad.sketch\0<part>\0<sketch>\0<entity>`
//! and a loop's the same over its anchor: the first of the points where its
//! curves join, in [`natural`] order (a lone circle's anchor is the circle).
//! Editing a dimension, where a loop starts, or adding a segment or a
//! fillet renames nothing: generated point IDs only grow, so a new point
//! never sorts first.

use crate::doc::id::Id;
use crate::doc::sketch::Geometry;
use crate::error::{Code, Error, Result};
use crate::sketch::Solved;
use core::f64::consts::TAU;
use ryukei::ProfileError;
use ryukei::geom::{Circle2, Curve2, Frame, Frame2, Line2, Point2, Vector2};
use ryukei::math;
use ryukei::name::OpKey;
use ryukei::sweep::{Loop, LoopKey, Profile, Segment, SegmentKey, ValidProfile};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The 128-bit key for a sketch entity.
pub fn entity_key(part: &Id, sketch: &Id, entity: &Id) -> u128 {
    let mut bytes = b"ryucad.sketch\0".to_vec();
    for (i, s) in [part, sketch, entity].iter().enumerate() {
        if i > 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(s.as_str().as_bytes());
    }
    OpKey::from_bytes(&bytes).0
}

/// Which sketch entity each key of a profile came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keys {
    pub segments: BTreeMap<u128, Id>,
    /// A loop's key, and its anchor, which names it.
    pub loops: BTreeMap<u128, Id>,
}

impl Keys {
    fn segment(&self, k: SegmentKey) -> Id {
        self.segments[&k.0].clone()
    }

    fn loop_(&self, k: LoopKey) -> Id {
        self.loops[&k.0].clone()
    }

    /// The entities the kernel's profile fault names.
    pub fn of_fault(&self, e: &ProfileError) -> Vec<Id> {
        let mut out = match *e {
            ProfileError::EmptyLoop { loop_ } | ProfileError::Degenerate { loop_ } => {
                vec![self.loop_(loop_)]
            }
            ProfileError::BadRange { segment }
            | ProfileError::ShortSegment { segment }
            | ProfileError::ClosedSegmentNotAlone { segment }
            | ProfileError::SeamLoop { segment } => vec![self.segment(segment)],
            ProfileError::NotClosed { after, .. } => vec![self.segment(after)],
            ProfileError::Touching { a, b } => vec![self.segment(a), self.segment(b)],
            _ => Vec::new(),
        };
        out.dedup();
        out
    }
}

/// What a profile fault means, for people.
fn describe(e: &ProfileError) -> &'static str {
    match e {
        ProfileError::Touching { .. } => "curves cross, overlap or touch",
        ProfileError::ShortSegment { .. } => "a curve is shorter than the resolution",
        ProfileError::NotClosed { .. } => "a loop doesn't close",
        ProfileError::Degenerate { .. } => "a loop encloses too little area to orient",
        ProfileError::OutsideSizeBox => "the profile leaves the ±500 m size box",
        _ => "the loops don't make a profile",
    }
}

/// The order anchors are chosen in: by the text before an ID's trailing
/// digits, then by their number, so `p2` comes before `p10`.
pub fn natural(id: &Id) -> (&str, usize, &str, &str) {
    let s = id.as_str();
    let stem = s.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = s[stem.len()..].trim_start_matches('0');
    (stem, digits.len(), digits, s)
}

/// A loop's anchor: the first of its curves' end points in [`natural`]
/// order, or for a loop with none (a circle), its first curve.
pub fn anchor(solved: &Solved, l: &[(Id, bool)]) -> Id {
    let ends = l
        .iter()
        .flat_map(|(e, _)| match &solved.sketch.entities[e].geometry {
            Geometry::Line { start, end } | Geometry::Arc { start, end, .. } => vec![start, end],
            _ => vec![],
        });
    ends.min_by(|a, b| natural(a).cmp(&natural(b)))
        .or_else(|| {
            l.iter()
                .map(|(e, _)| e)
                .min_by(|a, b| natural(a).cmp(&natural(b)))
        })
        .expect("loops aren't empty")
        .clone()
}

fn point(solved: &Solved, id: &Id) -> Point2 {
    match solved.sketch.entities[id].geometry {
        Geometry::Point { m } => Point2::new(m[0], m[1]),
        _ => unreachable!("validated: curves are built on points"),
    }
}

/// The trimmed curve a sketch curve is, walked backwards when `reversed`.
fn segment(solved: &Solved, id: &Id, reversed: bool, key: SegmentKey) -> Result<Segment> {
    let degenerate = |e: ryukei::Error| {
        Error::new(
            Code::ProfileInvalid,
            format!("{id} has no length or no radius"),
        )
        .with("entities", [id])
        .with("kernel", format!("{e:?}"))
    };
    let (curve, t0, t1) = match &solved.sketch.entities[id].geometry {
        Geometry::Line { start, end } => {
            let (a, b) = (point(solved, start), point(solved, end));
            let line = Line2::through(a, b).map_err(degenerate)?;
            (Curve2::Line2(line), 0.0, math::hypot(b.x - a.x, b.y - a.y))
        }
        Geometry::Arc { center, start, end } => {
            let (c, s, e) = (
                point(solved, center),
                point(solved, start),
                point(solved, end),
            );
            let x = Vector2::new(s.x - c.x, s.y - c.y);
            let r = math::hypot(x.x, x.y);
            let frame = Frame2::new(c, x, true).map_err(degenerate)?;
            let circle = Circle2::new(frame, r).map_err(degenerate)?;
            // The end's angle from the start, counterclockwise, in (0, 2π].
            let (dx, dy) = (e.x - c.x, e.y - c.y);
            let mut a = math::atan2(x.x * dy - x.y * dx, x.x * dx + x.y * dy);
            if a <= 0.0 {
                a += TAU;
            }
            (Curve2::Circle2(circle), 0.0, a)
        }
        Geometry::Circle { center, r_m } => {
            let frame = Frame2::new(point(solved, center), Vector2::new(1.0, 0.0), true)
                .map_err(degenerate)?;
            let circle = Circle2::new(frame, *r_m).map_err(degenerate)?;
            (Curve2::Circle2(circle), 0.0, TAU)
        }
        Geometry::Point { .. } => unreachable!("loops hold curves"),
    };
    let (t0, t1) = if reversed { (t1, t0) } else { (t0, t1) };
    Ok(Segment { key, curve, t0, t1 })
}

/// The profile of a solved sketch: every closed loop, or only those that
/// hold one of `chosen`'s entities. Faults are `profile_invalid`, naming
/// the sketch's entities.
pub fn profile(
    part: &Id,
    sketch: &Id,
    solved: &Solved,
    chosen: &[Id],
) -> Result<(ValidProfile, Keys)> {
    let all = &solved.report.loops;
    for c in chosen {
        if !all.iter().any(|l| l.iter().any(|(e, _)| e == c)) {
            return Err(Error::new(
                Code::ProfileInvalid,
                format!("{c} is not on a closed loop of sketch {sketch}"),
            )
            .with("entities", [c]));
        }
    }
    let loops: Vec<&Vec<(Id, bool)>> = all
        .iter()
        .filter(|l| chosen.is_empty() || l.iter().any(|(e, _)| chosen.contains(e)))
        .collect();
    if loops.is_empty() {
        return Err(Error::new(
            Code::ProfileInvalid,
            format!("sketch {sketch} has no closed loop to sweep"),
        )
        .with("entities", Vec::<Id>::new()));
    }
    let mut keys = Keys::default();
    let mut out = Vec::new();
    for l in loops {
        let anchor = anchor(solved, l);
        let lk = entity_key(part, sketch, &anchor);
        keys.loops.insert(lk, anchor);
        let mut segments = Vec::new();
        for (e, reversed) in l {
            let k = entity_key(part, sketch, e);
            keys.segments.insert(k, e.clone());
            segments.push(segment(solved, e, *reversed, SegmentKey(k))?);
        }
        out.push(Loop {
            key: LoopKey(lk),
            segments,
        });
    }
    let frame = Frame::from_rigid(&solved.frame).map_err(|e| {
        Error::new(Code::Internal, "a sketch frame that isn't one").with("kernel", format!("{e:?}"))
    })?;
    let valid = ValidProfile::new(Profile { frame, loops: out }).map_err(|e| match e {
        ryukei::Error::InvalidProfile(p) => {
            let entities = keys.of_fault(&p);
            Error::new(
                Code::ProfileInvalid,
                format!(
                    "sketch {sketch}: {} ({})",
                    describe(&p),
                    entities
                        .iter()
                        .map(Id::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with("entities", &entities)
            .with("kernel", format!("{p:?}"))
        }
        e => Error::new(Code::ProfileInvalid, format!("sketch {sketch}: {e}"))
            .with("kernel", format!("{e:?}")),
    })?;
    Ok((valid, keys))
}

/// Where each entity of a sweep came from, by the hex of its stable name:
/// a side from its sketch entity, a cap from its loop and end, an edge or
/// vertex from the entities either side of it. These are the names the
/// sweep derives from the profile's keys under `key`; entities a sweep
/// doesn't make simply have no origin.
pub fn origins(
    key: OpKey,
    sketch: &Id,
    valid: &ValidProfile,
    keys: &Keys,
) -> BTreeMap<String, Value> {
    use ryukei::name::{Name, Role, SweepRole as R};
    let derive = |role: R, parents: &[Name]| Name::derive(key, Role::Sweep(role), parents);
    let mut out = BTreeMap::new();
    let mut put = |n: Name, v: Value| {
        out.insert(n.to_string(), v);
    };
    for lump in valid.lumps() {
        let outer = &lump.outer;
        let lid = keys.loops[&outer.key.0].clone();
        let stand = derive(R::Loop, &[Name::from_u128(outer.key.0)]);
        let caps = [
            ("start", derive(R::StartCap, &[stand])),
            ("end", derive(R::EndCap, &[stand])),
        ];
        for (end, cap) in caps {
            put(cap, json!({"sketch": sketch, "cap": end, "loop": lid}));
        }
        for l in std::iter::once(outer).chain(&lump.holes) {
            let n = l.segments.len();
            let ids: Vec<Id> = l
                .segments
                .iter()
                .map(|g| keys.segments[&g.key.0].clone())
                .collect();
            let stands: Vec<Name> = l
                .segments
                .iter()
                .map(|g| derive(R::Segment, &[Name::from_u128(g.key.0)]))
                .collect();
            for (k, e) in ids.iter().enumerate() {
                let side = derive(R::Side, &[stands[k]]);
                put(side, json!({"sketch": sketch, "entity": e}));
                put(
                    derive(R::AxisEdge, &[stands[k]]),
                    json!({"sketch": sketch, "entity": e, "axis": true}),
                );
                for (end, cap) in caps {
                    put(
                        derive(R::CapEdge, &[cap, side]),
                        json!({"sketch": sketch, "entity": e, "cap": end}),
                    );
                }
                // The profile vertex between this curve and the one before.
                let pair = [stands[(k + n - 1) % n], stands[k]];
                let between = [&ids[(k + n - 1) % n], e];
                for (role, extra) in [
                    (R::Lateral, None),
                    (R::Vertex, None),
                    (R::StartVertex, Some(("cap", json!("start")))),
                    (R::EndVertex, Some(("cap", json!("end")))),
                    (R::Apex, Some(("apex", json!(true)))),
                ] {
                    let mut v = json!({"sketch": sketch, "entities": between});
                    if let Some((k, x)) = extra {
                        v[k] = x;
                    }
                    put(derive(role, &pair), v);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::sketch::Sketch;
    use crate::units::Unit;
    use serde_json::json;

    fn id(s: &str) -> Id {
        Id::parse(s, "entity").unwrap()
    }

    fn solved(v: serde_json::Value) -> Solved {
        let mut f = crate::doc::sketch::tests::rectangle();
        for (k, x) in v.as_object().unwrap() {
            f[k] = x.clone();
        }
        let p = [(
            "width".to_owned(),
            crate::expr::Expr::parse("20 mm").unwrap(),
        )]
        .into();
        let env = crate::expr::Env::new(&p);
        let s: Sketch = match crate::doc::files::Feature::from_json(&f, Unit::Mm, &env).unwrap() {
            crate::doc::files::Feature::Sketch(s) => s,
            _ => unreachable!(),
        };
        crate::sketch::solve(&id("s"), &s).unwrap()
    }

    #[test]
    fn keys_come_from_ids() {
        let p = Id::parse("p", "part").unwrap();
        let k = entity_key(&p, &id("s"), &id("l1"));
        assert_eq!(k, OpKey::from_bytes(b"ryucad.sketch\0p\0s\0l1").0);
        assert_ne!(k, entity_key(&p, &id("s"), &id("l2")));
    }

    #[test]
    fn a_rectangle_is_one_lump() {
        let s = solved(serde_json::json!({}));
        let p = Id::parse("p", "part").unwrap();
        let (v, keys) = profile(&p, &id("s"), &s, &[]).unwrap();
        assert_eq!(v.lumps().len(), 1);
        assert_eq!(v.lumps()[0].outer.segments.len(), 4);
        assert_eq!(keys.segments.len(), 4);
        assert_eq!(keys.loops.values().collect::<Vec<_>>(), [&id("p1")]);
        // Chosen by any of its entities; the construction diagonal isn't on one.
        assert!(profile(&p, &id("s"), &s, &[id("l3")]).is_ok());
        let e = profile(&p, &id("s"), &s, &[id("l5")]).unwrap_err();
        assert_eq!(
            (e.code, &e.details["entities"]),
            (Code::ProfileInvalid, &json!(["l5"]))
        );
    }

    #[test]
    fn anchors_sort_naturally() {
        let mut ids: Vec<Id> = ["p10", "p2", "a1", "p02", "corner", "p1"].map(id).into();
        ids.sort_by(|a, b| natural(a).cmp(&natural(b)));
        let ids: Vec<&str> = ids.iter().map(Id::as_str).collect();
        assert_eq!(ids, ["a1", "corner", "p1", "p02", "p2", "p10"]);
    }

    #[test]
    fn arcs_and_circles() {
        // A slot: two lines and two half circles, and a hole.
        let s = solved(serde_json::json!({
            "constraints": {},
            "entities": {
                "c1": {"m": [0, 0], "type": "point"},
                "c2": {"m": [0.02, 0], "type": "point"},
                "p1": {"m": [0, -0.005], "type": "point"},
                "p2": {"m": [0.02, -0.005], "type": "point"},
                "p3": {"m": [0.02, 0.005], "type": "point"},
                "p4": {"m": [0, 0.005], "type": "point"},
                "l1": {"end": "p2", "start": "p1", "type": "line"},
                "a1": {"center": "c2", "end": "p3", "start": "p2", "type": "arc"},
                "l2": {"end": "p4", "start": "p3", "type": "line"},
                "a2": {"center": "c1", "end": "p1", "start": "p4", "type": "arc"},
                "h": {"center": "c1", "r_m": 0.002, "type": "circle"}
            }
        }));
        let p = Id::parse("p", "part").unwrap();
        let (v, keys) = profile(&p, &id("s"), &s, &[]).unwrap();
        assert_eq!(v.lumps().len(), 1);
        assert_eq!(v.lumps()[0].holes.len(), 1);
        assert_eq!(keys.segments.len(), 5);
        // The slot is anchored on its first point, the hole on itself.
        let anchors: Vec<&Id> = keys.loops.values().collect();
        assert_eq!(anchors.len(), 2);
        assert!(anchors.contains(&&id("p1")) && anchors.contains(&&id("h")));
        let arc = v.lumps()[0]
            .outer
            .segments
            .iter()
            .find(|seg| keys.segments[&seg.key.0] == id("a1"))
            .unwrap();
        assert!(((arc.t1 - arc.t0).abs() - core::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn crossing_curves_are_named() {
        // A bow tie: l2 and l4 cross.
        let s = solved(serde_json::json!({
            "constraints": {},
            "entities": {
                "p1": {"m": [0, 0], "type": "point"},
                "p2": {"m": [0.02, 0], "type": "point"},
                "p3": {"m": [0, 0.01], "type": "point"},
                "p4": {"m": [0.02, 0.01], "type": "point"},
                "l1": {"end": "p2", "start": "p1", "type": "line"},
                "l2": {"end": "p3", "start": "p2", "type": "line"},
                "l3": {"end": "p4", "start": "p3", "type": "line"},
                "l4": {"end": "p1", "start": "p4", "type": "line"}
            }
        }));
        let p = Id::parse("p", "part").unwrap();
        let e = profile(&p, &id("s"), &s, &[]).unwrap_err();
        assert_eq!(e.code, Code::ProfileInvalid);
        assert_eq!(e.details["entities"], json!(["l2", "l4"]));
        assert!(e.message.contains("cross"), "{}", e.message);
    }
}
