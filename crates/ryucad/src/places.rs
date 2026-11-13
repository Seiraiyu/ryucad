//! Where a part's features put things, for the instance patterns that
//! follow them (design 521, issue #547): each `pattern`'s instances as
//! the motions that move its seeds, each `hole`'s points, and each
//! sketch as solved, all in the part's frame. An occurrence carries its
//! part's, as it carries its datums. Their hash is apart from the
//! model's: only an assembly with a following pattern keys its rebuild
//! on it, so nothing else rebuilds when a sketch point moves.

use crate::doc::files::Feature;
use crate::doc::id::Id;
use crate::doc::project::Part;
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
}
