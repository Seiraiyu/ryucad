//! Where a part's features put things, for the instance patterns that
//! follow them (design 521, issue #547): each `pattern`'s instances as
//! the motions that move its seeds, each `hole`'s points, and each
//! sketch as solved, all in the part's frame. An occurrence carries its
//! part's, as it carries its datums. Their hash is apart from the
//! model's: only an assembly with a following pattern keys its rebuild
//! on it, so nothing else rebuilds when a sketch point moves.

use crate::connector::{Scope, pose};
use crate::doc::files::Feature;
use crate::doc::id::{CopyKey, Id};
use crate::doc::mate::{InstancePattern, PatternForm};
use crate::doc::project::Part;
use crate::error::{Code, Error, Result};
use crate::rebuild::{Output, PartResult, Results};
use crate::sketch::Solved;
use ryukei::geom::{Point3, Rigid};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What one feature of a part gives a following pattern.
#[derive(Clone, Debug)]
pub enum Driver {
    /// A pattern's instances in its order, each with the motion that
    /// moves its seeds: `None` for a mirror's reflection.
    Pattern(Vec<([u32; 2], Option<Rigid>)>),
    /// A hole's points in the order it drills them.
    Hole(Vec<(Id, Point3)>),
    /// A sketch as solved: its points are read when a pattern names them.
    Sketch(Arc<Solved>),
}

/// A part's drivers by feature, those that built, and their hash (empty
/// with none).
#[derive(Clone, Debug, Default)]
pub struct Places {
    pub by_feature: BTreeMap<Id, Driver>,
    pub hash: String,
}

impl Places {
    /// The places of part `part` as `result` built it.
    pub fn of(part: &Part, result: &PartResult) -> Places {
        let mut by_feature = BTreeMap::new();
        for (fid, r) in &result.features {
            let (Some(Ok(f)), Ok(out)) = (part.features.get(fid), r) else {
                continue;
            };
            let d = match (f, out) {
                (Feature::Pattern(p), Output::Solids(_)) => {
                    match crate::pattern::motions(part, fid, p, result) {
                        Ok(m) => Driver::Pattern(
                            m.into_iter()
                                .map(|(i, m)| match m {
                                    crate::pattern::Motion::Rigid(r) => (i, Some(r)),
                                    crate::pattern::Motion::Mirror(_) => (i, None),
                                })
                                .collect(),
                        ),
                        Err(_) => continue,
                    }
                }
                (Feature::Hole(h), Output::Solids(_)) => {
                    let Some(Ok(Output::Sketch(s))) = result.result(&h.sketch) else {
                        continue;
                    };
                    match crate::hole::points(fid, h, s) {
                        Ok(p) => Driver::Hole(p),
                        Err(_) => continue,
                    }
                }
                (Feature::Sketch(_), Output::Sketch(s)) => Driver::Sketch(Arc::clone(s)),
                _ => continue,
            };
            by_feature.insert(fid.clone(), d);
        }
        let hash = hash_of(&by_feature);
        Places { by_feature, hash }
    }
}

fn hash_of(by: &BTreeMap<Id, Driver>) -> String {
    if by.is_empty() {
        return String::new();
    }
    let mut h = blake3::Hasher::new();
    for (id, d) in by {
        h.update(format!("{id}\n").as_bytes());
        match d {
            Driver::Pattern(list) => {
                for (i, m) in list {
                    h.update(format!(" {i:?}").as_bytes());
                    if let Some(r) = m {
                        h.update(&crate::assembly::pose_bits(r));
                    }
                }
            }
            Driver::Hole(points) => {
                for (p, at) in points {
                    h.update(format!(" {p}").as_bytes());
                    for x in [at.x, at.y, at.z] {
                        h.update(&x.to_bits().to_le_bytes());
                    }
                }
            }
            Driver::Sketch(s) => {
                h.update(s.hash.as_bytes());
            }
        }
    }
    h.finalize().to_hex()[..32].to_owned()
}

/// A following pattern's places as resolved: the instance that carries
/// them (ryumate's body), and each copy's key with its motion in that
/// instance's part frame, the seed's own place left out, in the driver's
/// order.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub carrier: Id,
    pub places: Vec<(CopyKey, ryumate::Pose)>,
}

/// Resolves the places of following pattern `id` (`p`) in `scope`:
/// through `on`'s instances to its part's occurrence, its feature's
/// driver there, and each place but the seed's.
pub fn resolve(id: &Id, p: &InstancePattern, scope: &dyn Scope) -> Result<Resolved> {
    let (on, from, listed) = match &p.form {
        PatternForm::Feature { on, from } => (on, from.as_ref(), None),
        PatternForm::Points { on, from, points } => (on, from.as_ref(), Some(&points[..])),
        _ => unreachable!("only a following pattern has places"),
    };
    let text = on.text();
    let fail = |code, msg: String| {
        Error::new(code, format!("{id}: its places {text} {msg}"))
            .with("feature", id)
            .with("param", "on")
    };
    let carrier = on.instance[0].clone();
    if p.instances.contains(&carrier) {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{id}: {carrier} carries its places and is one of its seeds; a copy can't move itself"),
        )
        .with("feature", id)
        .with("pointer", "/instances"));
    }
    let placed = match scope.instance(&carrier) {
        None => {
            return Err(fail(
                Code::NotFound,
                format!("name {carrier}, which isn't an instance the assembly lists"),
            ));
        }
        Some(Err(e)) => {
            return Err(fail(
                Code::DependencyFailed,
                format!("name {carrier}, which failed to rebuild"),
            )
            .with("dependency", &carrier)
            .with("cause", e.code));
        }
        Some(Ok(p)) => p,
    };
    let inside = &on.instance[1..];
    let Some(o) = placed.model.occurrences.iter().find(|o| o.path == inside) else {
        return Err(fail(
            Code::NotFound,
            format!(
                "name {}, where {} places no part",
                crate::assembly::path_text(&on.instance),
                placed.part
            ),
        ));
    };
    let there = |e: Error| e.with("part", &o.part).with("rev", o.rev.to_json());
    let Some(d) = o.places.by_feature.get(&on.feature) else {
        return Err(there(fail(
            Code::NotFound,
            format!(
                "name {}, which isn't a pattern, hole or sketch of {} that built",
                on.feature, o.part
            ),
        )));
    };
    let motions: Vec<(CopyKey, Rigid)> = match (d, listed) {
        (Driver::Pattern(list), None) => {
            if from.is_some() {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{id}: a pattern's seed is at its own place, [0, 0]; leave from out"),
                )
                .with("feature", id)
                .with("pointer", "/from"));
            }
            list.iter()
                .map(|(i, m)| match m {
                    Some(r) => Ok((CopyKey::Index(*i), *r)),
                    None => Err(there(fail(
                        Code::InvalidParam,
                        format!(
                            "name mirror pattern {}: an instance can't be mirrored",
                            on.feature
                        ),
                    ))),
                })
                .collect::<Result<_>>()?
        }
        (Driver::Hole(points), None) => translations(id, points, from)?,
        (Driver::Sketch(s), Some(listed)) => {
            let points = crate::hole::sketch_points(id.as_str(), id, &on.feature, listed, s)?;
            if points.is_empty() {
                return Err(there(fail(
                    Code::OperationNoEffect,
                    format!(
                        "name sketch {}, which has no point that no curve uses; list them",
                        on.feature
                    ),
                )));
            }
            translations(id, &points, from)?
        }
        (Driver::Sketch(_), None) => {
            return Err(there(fail(
                Code::ReferenceWrongKind,
                format!(
                    "name sketch {}: a sketch's points are form points",
                    on.feature
                ),
            )));
        }
        (_, Some(_)) => {
            return Err(there(fail(
                Code::ReferenceWrongKind,
                format!(
                    "name {}, which isn't a sketch: form points takes a sketch",
                    on.feature
                ),
            )));
        }
    };
    if motions.is_empty() {
        return Err(fail(
            Code::OperationNoEffect,
            "give no place but the seed's".to_owned(),
        ));
    }
    // A motion in the part's frame, seen from the carrying instance's.
    let w = pose(&o.pose);
    let wi = w.inverse();
    Ok(Resolved {
        carrier,
        places: motions
            .into_iter()
            .map(|(k, m)| (k, w.then_after(&pose(&m)).then_after(&wi)))
            .collect(),
    })
}

/// Each point but the seed's (`from`, else the first) as the translation
/// from the seed's, keyed by its ID.
fn translations(
    id: &Id,
    points: &[(Id, Point3)],
    from: Option<&Id>,
) -> Result<Vec<(CopyKey, Rigid)>> {
    let seed = match from {
        None => &points[0],
        Some(f) => points.iter().find(|(p, _)| p == f).ok_or_else(|| {
            Error::new(
                Code::InvalidParam,
                format!("{id}: from {f} isn't one of its places"),
            )
            .with("feature", id)
            .with("pointer", "/from")
        })?,
    };
    points
        .iter()
        .filter(|(p, _)| *p != seed.0)
        .map(|(p, at)| {
            let key = CopyKey::point(p);
            if key == CopyKey::Index([0, 0]) {
                return Err(Error::new(
                    Code::InvalidParam,
                    format!("{id}: point {p} can't name a copy (copy 0 is the seed); rename it"),
                )
                .with("feature", id));
            }
            Ok((key, Rigid::translation(at.to_vector() - seed.1.to_vector())))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::doc::assembly::Rev;
    use crate::session::Session;
    use serde_json::{Value, json};

    pub(crate) fn must(s: &mut Session, m: &str, p: Value) -> Value {
        crate::server::handle(s, crate::proto::Request::from_rpc(m, Some(p)).unwrap())
            .unwrap_or_else(|e| panic!("{m}: {e}"))
    }

    pub(crate) fn id(s: &str) -> Id {
        Id::parse(s, "id").unwrap()
    }

    /// Part `tray`: a plate 60 × 40 × 5 mm below xy; sketch `seat` on xy
    /// with one point p1 (10, 10), hole `h` through the plate there, and
    /// pattern `grid` of `h`, 3 along x 20 mm apart by 2 along y 20 mm
    /// apart, skipping [2, 1]; sketch `layout` on xy with free points p1
    /// (5, 35), p2 (25, 35) and p5 (45, 35) and a line from p3 (2, 2) to p4
    /// (8, 2), and hole `row` through the plate at its free points. Part
    /// `screw`: a cylinder. Assembly `pack`: `lid` of tray at (100, 0, 0)
    /// mm; `screw-0` of screw at (110, 10, 0), over `grid`'s seed;
    /// `bolt-0` and `pin-0` of screw at (105, 35, 0), over `layout`'s p1.
    pub(crate) fn tray() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        crate::server::init(dir.path(), None).unwrap();
        let mut s = Session::open(dir.path()).unwrap();
        let add = |s: &mut Session, f: Value| must(s, "feature.add", f);
        must(&mut s, "part.create", json!({"part": "tray"}));
        add(
            &mut s,
            json!({"part": "tray", "type": "box", "id": "plate",
                           "params": {"x": 60, "y": 40, "z": 5},
                           "placement": {"origin": [0, 0, -5]}}),
        );
        add(
            &mut s,
            json!({"part": "tray", "type": "sketch", "id": "seat", "plane": "xy"}),
        );
        must(
            &mut s,
            "sketch.edit",
            json!({"part": "tray", "sketch": "seat",
             "ops": [{"op": "add", "type": "point", "at": [10, 10]}]}),
        );
        add(
            &mut s,
            json!({"part": "tray", "type": "hole", "id": "h", "sketch": "seat",
                           "diameter": 3, "depth": "through_all"}),
        );
        add(
            &mut s,
            json!({"part": "tray", "type": "pattern", "id": "grid", "features": ["h"],
                           "direction": "x", "spacing": 20, "count": 3,
                           "direction2": "y", "spacing2": 20, "count2": 2, "skip": [[2, 1]]}),
        );
        add(
            &mut s,
            json!({"part": "tray", "type": "sketch", "id": "layout", "plane": "xy"}),
        );
        must(
            &mut s,
            "sketch.edit",
            json!({"part": "tray", "sketch": "layout", "ops": [
            {"op": "add", "id": "p1", "type": "point", "at": [5, 35]},
            {"op": "add", "id": "p2", "type": "point", "at": [25, 35]},
            {"op": "add", "id": "p3", "type": "point", "at": [2, 2]},
            {"op": "add", "id": "p4", "type": "point", "at": [8, 2]},
            {"op": "add", "id": "p5", "type": "point", "at": [45, 35]},
            {"op": "add", "type": "line", "start": "p3", "end": "p4"}]}),
        );
        add(
            &mut s,
            json!({"part": "tray", "type": "hole", "id": "row", "sketch": "layout",
                           "diameter": 3, "depth": "through_all"}),
        );
        must(&mut s, "part.create", json!({"part": "screw"}));
        add(
            &mut s,
            json!({"part": "screw", "type": "cylinder", "id": "shank",
                           "params": {"radius": 1.4, "height": 8}}),
        );
        must(
            &mut s,
            "part.create",
            json!({"part": "pack", "kind": "assembly"}),
        );
        for (i, of, at) in [
            ("lid", "tray", [100, 0, 0]),
            ("screw-0", "screw", [110, 10, 0]),
            ("bolt-0", "screw", [105, 35, 0]),
            ("pin-0", "screw", [105, 35, 0]),
        ] {
            add(
                &mut s,
                json!({"part": "pack", "type": "instance", "id": i, "of": of,
                               "placement": {"origin": at}}),
            );
        }
        (dir, s)
    }

    #[test]
    fn a_parts_model_carries_its_patterns_holes_and_sketches() {
        let (_dir, mut s) = tray();
        let m = s.model(&id("tray"), &Rev::Working).unwrap();
        let places = &m.occurrences[0].places;
        let keys: Vec<&str> = places.by_feature.keys().map(Id::as_str).collect();
        assert_eq!(keys, ["grid", "h", "layout", "row", "seat"]);
        let Driver::Pattern(grid) = &places.by_feature[&id("grid")] else {
            panic!()
        };
        let at: Vec<([u32; 2], [i64; 3])> = grid
            .iter()
            .map(|(i, r)| {
                let t = r.unwrap().translation_part();
                (*i, [t.x, t.y, t.z].map(|c| (c * 1e3).round() as i64))
            })
            .collect();
        assert_eq!(
            at,
            [
                ([1, 0], [20, 0, 0]),
                ([2, 0], [40, 0, 0]),
                ([0, 1], [0, 20, 0]),
                ([1, 1], [20, 20, 0])
            ]
        );
        let Driver::Hole(row) = &places.by_feature[&id("row")] else {
            panic!()
        };
        let row: Vec<(&str, [i64; 3])> = row
            .iter()
            .map(|(p, a)| {
                (
                    p.as_str(),
                    [a.x, a.y, a.z].map(|c| (c * 1e3).round() as i64),
                )
            })
            .collect();
        assert_eq!(
            row,
            [("p1", [5, 35, 0]), ("p2", [25, 35, 0]), ("p5", [45, 35, 0])]
        );
        assert!(matches!(
            places.by_feature[&id("layout")],
            Driver::Sketch(_)
        ));
        assert!(!places.hash.is_empty() && !m.places_hash.is_empty());
        // Moving a point no feature drills changes the places, not the model.
        let before = (m.hash.clone(), m.places_hash.clone());
        must(
            &mut s,
            "feature.add",
            json!({"part": "tray", "type": "sketch", "id": "spare",
                                           "plane": "xy"}),
        );
        must(
            &mut s,
            "sketch.edit",
            json!({"part": "tray", "sketch": "spare",
             "ops": [{"op": "add", "type": "point", "at": [1, 1]}]}),
        );
        let m = s.model(&id("tray"), &Rev::Working).unwrap();
        assert_eq!(m.hash, before.0);
        assert_ne!(m.places_hash, before.1);
    }

    /// The pack's rebuild as connectors are resolved in it.
    struct In<'a>(&'a crate::rebuild::PartResult);

    impl crate::connector::Scope for In<'_> {
        fn instance(
            &self,
            i: &Id,
        ) -> Option<core::result::Result<&crate::assembly::Placed, &Error>> {
            match self.0.result(i)? {
                Ok(o) => o.instance().map(Ok),
                Err(e) => Some(Err(e)),
            }
        }

        fn places(&self, _: &Id, _: &Id, _: &CopyKey) -> Option<Result<bool>> {
            None
        }
    }

    fn pattern(form: &str, feature: &str, from: Option<&str>, points: &[&str]) -> InstancePattern {
        let on = crate::doc::mate::Driver {
            instance: vec![id("lid")],
            feature: id(feature),
        };
        let from = from.map(id);
        InstancePattern {
            form: match form {
                "feature" => PatternForm::Feature { on, from },
                _ => PatternForm::Points {
                    on,
                    from,
                    points: points.iter().map(|p| id(p)).collect(),
                },
            },
            instances: vec![id("screw-0")],
            skip: Vec::new(),
        }
    }

    fn mm(p: &ryumate::Pose) -> [i64; 3] {
        p.origin.map(|c| (c * 1e3).round() as i64)
    }

    #[test]
    fn places_resolve_by_their_drivers_keys() {
        let (_dir, mut s) = tray();
        let r = s.rebuilt(&id("pack")).unwrap().clone();
        let got = |p: &InstancePattern| -> Result<Vec<(String, [i64; 3])>> {
            let r = resolve(&id("screws"), p, &In(&r))?;
            assert_eq!(r.carrier, id("lid"));
            Ok(r.places.iter().map(|(k, m)| (k.text(), mm(m))).collect())
        };
        let s = |v: &[(&str, [i64; 3])]| -> Vec<(String, [i64; 3])> {
            v.iter().map(|(k, a)| (k.to_string(), *a)).collect()
        };
        assert_eq!(
            got(&pattern("feature", "grid", None, &[])).unwrap(),
            s(&[
                ("1", [20, 0, 0]),
                ("2", [40, 0, 0]),
                ("0:1", [0, 20, 0]),
                ("1:1", [20, 20, 0])
            ])
        );
        assert_eq!(
            got(&pattern("feature", "row", None, &[])).unwrap(),
            s(&[("p2", [20, 0, 0]), ("p5", [40, 0, 0])])
        );
        assert_eq!(
            got(&pattern("feature", "row", Some("p2"), &[])).unwrap(),
            s(&[("p1", [-20, 0, 0]), ("p5", [20, 0, 0])])
        );
        assert_eq!(
            got(&pattern("points", "layout", None, &["p1", "p5"])).unwrap(),
            s(&[("p5", [40, 0, 0])])
        );
        assert_eq!(
            got(&pattern("points", "layout", Some("p5"), &[])).unwrap(),
            s(&[("p1", [-40, 0, 0]), ("p2", [-20, 0, 0])])
        );
        // Refusals.
        let code = |p: InstancePattern| got(&p).unwrap_err();
        assert_eq!(
            code(pattern("feature", "layout", None, &[])).code,
            Code::ReferenceWrongKind
        );
        assert_eq!(
            code(pattern("points", "row", None, &[])).code,
            Code::ReferenceWrongKind
        );
        assert_eq!(
            code(pattern("feature", "h", None, &[])).code,
            Code::OperationNoEffect
        );
        assert_eq!(
            code(pattern("feature", "nothing", None, &[])).code,
            Code::NotFound
        );
        let e = code(pattern("feature", "row", Some("p3"), &[]));
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidParam, &json!("/from"))
        );
        let e = code(pattern("feature", "grid", Some("p1"), &[]));
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidParam, &json!("/from"))
        );
        let e = code(pattern("points", "layout", None, &["p3", "l1"]));
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::ReferenceWrongKind, &json!("/points/1"))
        );
        let mut p = pattern("feature", "grid", None, &[]);
        p.instances = vec![id("lid")];
        assert_eq!(code(p).details["pointer"], "/instances");
        let mut p = pattern("feature", "grid", None, &[]);
        if let PatternForm::Feature { on, .. } = &mut p.form {
            on.instance = vec![id("nobody")];
        }
        assert_eq!(code(p).code, Code::NotFound);
    }

    /// Every copy's path in the pack's model with its origin, mm.
    fn copies(s: &mut Session) -> BTreeMap<String, [i64; 3]> {
        let m = s.model(&id("pack"), &Rev::Working).unwrap();
        m.occurrences
            .iter()
            .filter(|o| o.path[0].copy_of().is_some())
            .map(|o| {
                let t = o.pose.translation_part();
                (
                    crate::assembly::path_text(&o.path),
                    [t.x, t.y, t.z].map(|c| (c * 1e3).round() as i64),
                )
            })
            .collect()
    }

    #[test]
    fn copies_follow_a_parts_pattern_hole_and_sketch_and_keep_their_keys() {
        let (_dir, mut s) = tray();
        for (pid, seed, form, on, points) in [
            ("screws", "screw-0", "feature", "pack/lid/grid", json!(null)),
            ("bolts", "bolt-0", "feature", "pack/lid/row", json!(null)),
            (
                "pins",
                "pin-0",
                "points",
                "pack/lid/layout",
                json!(["p1", "p5"]),
            ),
        ] {
            let mut f = json!({"part": "pack", "type": "instance_pattern", "id": pid,
                               "instances": [seed], "form": form, "on": on});
            if !points.is_null() {
                f["points"] = points;
            }
            must(&mut s, "feature.add", f);
        }
        let want = |v: &[(&str, [i64; 3])]| -> BTreeMap<String, [i64; 3]> {
            v.iter().map(|(k, a)| (k.to_string(), *a)).collect()
        };
        assert_eq!(
            copies(&mut s),
            want(&[
                ("screws:1/screw-0", [130, 10, 0]),
                ("screws:2/screw-0", [150, 10, 0]),
                ("screws:0:1/screw-0", [110, 30, 0]),
                ("screws:1:1/screw-0", [130, 30, 0]),
                ("bolts:p2/bolt-0", [125, 35, 0]),
                ("bolts:p5/bolt-0", [145, 35, 0]),
                ("pins:p5/pin-0", [145, 35, 0]),
            ])
        );
        // A mate on a copy, by its key, to a fixed nut where the copy is.
        must(
            &mut s,
            "feature.add",
            json!({"part": "pack", "type": "instance", "id": "nut",
             "of": "screw", "fixed": true, "placement": {"origin": [145, 35, 0]}}),
        );
        must(
            &mut s,
            "feature.add",
            json!({"part": "pack", "type": "mate", "id": "m",
             "form": "fastened", "a": "pack/bolts:p5/bolt-0", "b": "pack/nut"}),
        );
        // p2 removed: p5's copy keeps its key, and its mate holds.
        must(
            &mut s,
            "sketch.edit",
            json!({"part": "tray", "sketch": "layout",
             "ops": [{"op": "remove", "id": "p2"}]}),
        );
        let c = copies(&mut s);
        assert!(!c.contains_key("bolts:p2/bolt-0"));
        assert_eq!(c["bolts:p5/bolt-0"], [145, 35, 0]);
        let r = s.rebuilt(&id("pack")).unwrap().clone();
        assert_eq!(r.failed(), 0, "{:?}", r.features);
        // A point added is a copy added; nothing else is renumbered.
        must(
            &mut s,
            "sketch.edit",
            json!({"part": "tray", "sketch": "layout",
             "ops": [{"op": "add", "id": "p7", "type": "point", "at": [35, 35]}]}),
        );
        assert_eq!(copies(&mut s)["bolts:p7/bolt-0"], [135, 35, 0]);
        // The lid turned a quarter about z: the copies turn with it.
        must(
            &mut s,
            "assembly.move",
            json!({"part": "pack", "instance": "lid",
             "axis": [0, 0, 1], "angle": "90 deg"}),
        );
        assert_eq!(copies(&mut s)["screws:1/screw-0"], [110, 30, 0]);
        // Refused at write: a seed that carries the places.
        let e = crate::server::handle(&mut s, crate::proto::Request::from_rpc("feature.add",
            Some(json!({"part": "pack", "type": "instance_pattern", "id": "x", "instances": ["lid"],
                        "form": "feature", "on": "pack/lid/grid"}))).unwrap()).unwrap_err();
        assert_eq!(e.details["pointer"], "/instances");
    }
}
