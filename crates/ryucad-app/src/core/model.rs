//! What the window shows of the model: parts, their features in order with
//! their rebuild status, their solids, and a display mesh per solid. It is
//! built from the daemon's reports and never from the files.

use crate::core::sketch::Drawing;
use ryucad::mesh::WireBody;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A feature as the last rebuild left it.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub id: String,
    /// `box`, `cylinder` and so on; `None` when the file can't be read.
    pub ty: Option<String>,
    /// Its result's hash, when it built.
    pub hash: Option<String>,
    /// What it does to solids: `join`, `cut` or `intersect`, or a
    /// boolean's `unite`, `subtract` or `intersect`; `None` for a feature
    /// that founds a solid, or makes none.
    pub operation: Option<String>,
    /// The solids it founds or acts on, by founder.
    pub solids: Vec<String>,
    /// The solids a boolean consumes.
    pub consumes: Vec<String>,
    /// A sketch's solve, when it solved.
    pub sketch: Option<SketchState>,
    /// The features it reads.
    pub uses: Vec<String>,
    /// Why it didn't build.
    pub error: Option<Value>,
}

/// How a sketch solved: `under`, `well` or `over`, the freedom left, and
/// whether its stored positions are stale.
#[derive(Clone, Debug, PartialEq)]
pub struct SketchState {
    pub state: String,
    pub dof: u64,
    pub stale: bool,
}

/// A solid as the model shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    /// Its founder.
    pub id: String,
    /// Its chain, founder first.
    pub features: Vec<String>,
    /// The hash of its body as shown, when anything of it built.
    pub hash: Option<String>,
    /// Why its last feature failed: it shows as the last that built.
    pub error: Option<Value>,
}

/// A part: its features in order, and its solids.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub id: String,
    pub revision: String,
    pub features: Vec<Feature>,
    /// The solids of the model, not consumed, in list order.
    pub solids: Vec<Solid>,
    /// Why the part's list can't be read.
    pub error: Option<Value>,
}

impl Part {
    /// A part from a rebuild report (`rebuild` or `model.changed`).
    pub fn from_report(v: &Value) -> Part {
        let features = v["features"]
            .as_array()
            .map(|fs| {
                fs.iter()
                    .map(|f| Feature {
                        id: text(&f["id"]),
                        ty: f["type"].as_str().map(str::to_owned),
                        hash: f["hash"].as_str().map(str::to_owned),
                        sketch: f.get("sketch").map(|s| SketchState {
                            state: text(&s["state"]),
                            dof: s["dof"].as_u64().unwrap_or_default(),
                            stale: s["stale"] == true,
                        }),
                        uses: texts(&f["uses"]),
                        operation: f["operation"].as_str().map(str::to_owned),
                        solids: texts(&f["solids"]),
                        consumes: texts(&f["consumes"]),
                        error: (f["ok"] == false).then(|| f["error"].clone()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let solids = v["solids"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| Solid {
                id: text(&s["solid"]),
                features: texts(&s["features"]),
                hash: s["hash"].as_str().map(str::to_owned),
                error: (s["ok"] == false).then(|| s["error"].clone()),
            })
            .collect();
        Part {
            id: text(&v["part"]),
            revision: text(&v["revision"]),
            features,
            solids,
            error: v.get("error").cloned(),
        }
    }

    pub fn feature(&self, id: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    /// The solid founded by `id`, if the model shows it.
    pub fn solid(&self, id: &str) -> Option<&Solid> {
        self.solids.iter().find(|s| s.id == id)
    }

    /// The solids the model shows that `feature` founds or acts on.
    pub fn solids_of(&self, feature: &str) -> Vec<&Solid> {
        self.solids
            .iter()
            .filter(|s| s.features.iter().any(|f| f == feature))
            .collect()
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().map(text).collect()
}

/// A body's place: its part and the feature that names it: a solid's
/// founder, or a sketch.
pub type BodyKey = (String, String);

/// The model as the window knows it.
#[derive(Debug, Default)]
pub struct Model {
    pub default_unit: String,
    pub parts: BTreeMap<String, Part>,
    /// Display meshes by part and solid, each for the body hash it says.
    pub meshes: BTreeMap<BodyKey, Arc<WireBody>>,
    /// Meshes asked for and not yet answered, by the body hash they were
    /// asked for at: a burst of changes asks for each body once.
    pub asked: BTreeMap<BodyKey, String>,
    /// How sketches are drawn, by part and feature, each for the part
    /// revision it says.
    pub sketches: BTreeMap<BodyKey, Arc<Drawing>>,
    /// Sketches asked for and not yet answered, by the part revision they
    /// were asked for at.
    pub sketches_asked: BTreeMap<BodyKey, String>,
    /// Bumped whenever anything here changes, so views know to redraw.
    pub generation: u64,
}

impl Model {
    /// Takes a part's report. Returns the bodies whose meshes are now
    /// missing or stale: they need fetching ([`Model::sketches_wanted`]
    /// says the same of sketches).
    pub fn apply_part(&mut self, report: &Value) -> Vec<BodyKey> {
        let part = Part::from_report(report);
        let id = part.id.clone();
        // Meshes of solids that are gone, or changed, go.
        self.meshes.retain(|(p, s), m| {
            *p != id
                || part
                    .solid(s)
                    .is_some_and(|s| s.hash.as_deref() == Some(&m.hash))
        });
        // Drawings of sketches that are gone, or failed, go; the others
        // stay drawn until their new drawing comes.
        self.sketches
            .retain(|(p, f), _| *p != id || part.feature(f).is_some_and(|f| f.sketch.is_some()));
        let wanted = part
            .solids
            .iter()
            .filter(|s| s.hash.is_some())
            .map(|s| (id.clone(), s.id.clone()))
            .filter(|k| !self.meshes.contains_key(k))
            .collect();
        self.parts.insert(id, part);
        self.generation += 1;
        wanted
    }

    /// The bodies of `keys` not already asked for at their current hash,
    /// with that hash, now noted as asked for.
    pub fn ask(&mut self, keys: Vec<BodyKey>) -> Vec<(BodyKey, String)> {
        let mut out = Vec::new();
        for key in keys {
            let Some(hash) = self
                .parts
                .get(&key.0)
                .and_then(|p| p.solid(&key.1))
                .and_then(|s| s.hash.clone())
            else {
                continue;
            };
            if self.asked.get(&key) != Some(&hash) {
                self.asked.insert(key.clone(), hash.clone());
                out.push((key, hash));
            }
        }
        out
    }

    /// An answer to asking for `key` at `hash` came, or will never come.
    /// Whatever it brought, later reports ask again if they need to.
    pub fn answered(&mut self, key: &BodyKey, hash: &str) {
        if self.asked.get(key).is_some_and(|h| h == hash) {
            self.asked.remove(key);
        }
    }

    /// The sketches of `part` whose drawings are missing or older than
    /// the part, not already asked for at its revision, with that
    /// revision, now noted as asked for.
    pub fn ask_sketches(&mut self, part: &str) -> Vec<(BodyKey, String)> {
        let Some(p) = self.parts.get(part) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for f in p.features.iter().filter(|f| f.sketch.is_some()) {
            let key = (p.id.clone(), f.id.clone());
            let drawn = self
                .sketches
                .get(&key)
                .is_some_and(|d| d.revision == p.revision);
            if !drawn && self.sketches_asked.get(&key) != Some(&p.revision) {
                self.sketches_asked.insert(key.clone(), p.revision.clone());
                out.push((key, p.revision.clone()));
            }
        }
        out
    }

    /// An answer to asking for sketch `key` at `revision` came, or never
    /// will.
    pub fn sketch_answered(&mut self, key: &BodyKey, revision: &str) {
        if self.sketches_asked.get(key).is_some_and(|r| r == revision) {
            self.sketches_asked.remove(key);
        }
    }

    /// Keeps a sketch's drawing if it is still a sketch that solved, and
    /// the drawing is of the part as it is now, or the only one there is.
    pub fn take_sketch(&mut self, key: &BodyKey, d: Drawing) -> bool {
        let Some(p) = self.parts.get(&key.0) else {
            return false;
        };
        let solved = p.feature(&key.1).is_some_and(|f| f.sketch.is_some());
        let current = d.revision == p.revision || !self.sketches.contains_key(key);
        if !solved || !current {
            return false;
        }
        self.sketches.insert(key.clone(), Arc::new(d));
        self.generation += 1;
        true
    }

    /// Drops every drawing: the display quality or the default unit
    /// changed. Returns the parts to ask again.
    pub fn clear_sketches(&mut self) -> Vec<String> {
        self.sketches.clear();
        self.sketches_asked.clear();
        self.generation += 1;
        self.parts.keys().cloned().collect()
    }

    /// Forgets a part, its meshes and its sketches.
    pub fn remove_part(&mut self, id: &str) {
        self.parts.remove(id);
        self.meshes.retain(|(p, _), _| p != id);
        self.asked.retain(|(p, _), _| p != id);
        self.sketches.retain(|(p, _), _| p != id);
        self.sketches_asked.retain(|(p, _), _| p != id);
        self.generation += 1;
    }

    /// Keeps a fetched mesh if its body is still the current one.
    pub fn take_mesh(&mut self, part: &str, body: WireBody) -> bool {
        let current = self
            .parts
            .get(part)
            .and_then(|p| p.solid(&body.feature))
            .and_then(|s| s.hash.as_deref());
        if current != Some(&body.hash) {
            return false;
        }
        let key = (part.to_owned(), body.feature.clone());
        self.meshes.insert(key, Arc::new(body));
        self.generation += 1;
        true
    }

    /// Drops every mesh: the display quality changed.
    pub fn clear_meshes(&mut self) -> Vec<BodyKey> {
        self.meshes.clear();
        self.asked.clear();
        self.generation += 1;
        self.parts
            .values()
            .flat_map(|p| {
                p.solids
                    .iter()
                    .filter(|s| s.hash.is_some())
                    .map(|s| (p.id.clone(), s.id.clone()))
            })
            .collect()
    }

    /// The solid a feature shows in: its own, or the first the model
    /// shows of those it acts on.
    pub fn solid_of(&self, key: &BodyKey) -> Option<BodyKey> {
        let p = self.parts.get(&key.0)?;
        let s = p.solids_of(&key.1).into_iter().next()?;
        Some((key.0.clone(), s.id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A part of solids, each founded by one feature with its hash, or
    /// failed.
    fn report(hashes: &[(&str, Option<&str>)]) -> Value {
        let features: Vec<Value> = hashes
            .iter()
            .map(|(id, h)| match h {
                Some(h) => json!({"id": id, "type": "box", "ok": true, "hash": h, "solids": [id]}),
                None => json!({"id": id, "type": "torus", "ok": false, "solids": [id],
                               "error": {"code": "rebuild_failed"}}),
            })
            .collect();
        let solids: Vec<Value> = hashes
            .iter()
            .map(|(id, h)| match h {
                Some(h) => json!({"solid": id, "ok": true, "features": [id], "hash": h}),
                None => json!({"solid": id, "ok": false, "features": [id],
                               "error": {"code": "rebuild_failed"}}),
            })
            .collect();
        json!({"part": "p", "revision": "r", "features": features, "solids": solids})
    }

    fn body(feature: &str, hash: &str) -> WireBody {
        WireBody {
            feature: feature.into(),
            hash: hash.into(),
            mesh: Err(json!(null)),
        }
    }

    #[test]
    fn meshes_follow_the_bodies() {
        let mut m = Model::default();
        let want = m.apply_part(&report(&[("a", Some("1")), ("b", Some("2"))]));
        assert_eq!(want.len(), 2);
        assert!(m.take_mesh("p", body("a", "1")));
        assert!(m.take_mesh("p", body("b", "2")));
        // A stale mesh is refused.
        assert!(!m.take_mesh("p", body("a", "0")));
        // Only the changed body is wanted again; the failed one loses its mesh.
        let want = m.apply_part(&report(&[("a", Some("1")), ("b", None), ("c", Some("3"))]));
        assert_eq!(want, [("p".to_owned(), "c".to_owned())]);
        assert_eq!(m.meshes.len(), 1);
        assert_eq!(
            m.parts["p"].features[1].error,
            Some(json!({"code": "rebuild_failed"}))
        );
        assert_eq!(m.parts["p"].features[0].ty.as_deref(), Some("box"));
        assert_eq!(m.parts["p"].features[1].ty.as_deref(), Some("torus"));
        let g = m.generation;
        m.remove_part("p");
        assert!(m.meshes.is_empty() && m.parts.is_empty() && m.generation > g);
    }

    fn sketched(revision: &str, ok: bool) -> Value {
        let s = if ok {
            json!({"id": "s", "type": "sketch", "ok": true,
                   "sketch": {"state": "under", "dof": 3, "stale": false}})
        } else {
            json!({"id": "s", "type": "sketch", "ok": false, "error": {"code": "sketch_unsolved"}})
        };
        json!({"part": "p", "revision": revision, "features": [s,
               {"id": "e", "type": "extrude", "ok": true, "hash": "1", "uses": ["s"]}]})
    }

    fn drawing(revision: &str) -> Drawing {
        Drawing {
            revision: revision.into(),
            ..Drawing::default()
        }
    }

    #[test]
    fn sketches_follow_the_part_revision() {
        let mut m = Model::default();
        m.apply_part(&sketched("r1", true));
        let p = &m.parts["p"];
        assert_eq!(p.features[1].uses, ["s"]);
        assert_eq!(p.features[0].sketch.as_ref().unwrap().dof, 3);
        let key = ("p".to_owned(), "s".to_owned());
        assert_eq!(m.ask_sketches("p"), [(key.clone(), "r1".to_owned())]);
        // Asked once per revision.
        assert!(m.ask_sketches("p").is_empty());
        assert!(m.take_sketch(&key, drawing("r1")));
        m.sketch_answered(&key, "r1");
        assert!(m.ask_sketches("p").is_empty());
        // A new revision asks again, and the old drawing stays until the
        // new one comes; an answer from before the drawing kept is refused.
        m.apply_part(&sketched("r2", true));
        assert_eq!(m.ask_sketches("p").len(), 1);
        assert!(m.sketches.contains_key(&key));
        assert!(m.take_sketch(&key, drawing("r2")));
        assert!(!m.take_sketch(&key, drawing("r1")));
        // A sketch that fails is no longer drawn.
        m.apply_part(&sketched("r3", false));
        assert!(m.sketches.is_empty());
        assert!(m.ask_sketches("p").is_empty());
        assert!(!m.take_sketch(&key, drawing("r3")));
        m.apply_part(&sketched("r4", true));
        assert_eq!(m.clear_sketches(), ["p"]);
        assert_eq!(m.ask_sketches("p").len(), 1);
    }

    #[test]
    fn a_body_is_asked_for_once_per_hash() {
        let mut m = Model::default();
        let want = m.apply_part(&report(&[("a", Some("1"))]));
        assert_eq!(m.ask(want).len(), 1);
        // More reports before the answer: nothing new is asked for.
        let want = m.apply_part(&report(&[("a", Some("1")), ("b", Some("2"))]));
        let asked = m.ask(want);
        assert_eq!(asked, [(("p".to_owned(), "b".to_owned()), "2".to_owned())]);
        // A body that changed while asked for is asked for again.
        let want = m.apply_part(&report(&[("a", Some("3")), ("b", Some("2"))]));
        let asked = m.ask(want);
        assert_eq!(asked, [(("p".to_owned(), "a".to_owned()), "3".to_owned())]);
        // The stale answer leaves the newer question open.
        let a = ("p".to_owned(), "a".to_owned());
        assert!(!m.take_mesh("p", body("a", "1")));
        m.answered(&a, "1");
        assert!(m.ask(vec![a.clone()]).is_empty());
        // An answer that brought nothing lets the next report ask again.
        m.answered(&a, "3");
        assert_eq!(m.ask(vec![a]).len(), 1);
    }
}
