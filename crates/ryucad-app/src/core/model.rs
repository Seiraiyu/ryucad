//! What the window shows of the model: parts, their features in order with
//! their rebuild status, and a display mesh per body. It is built from the
//! daemon's reports and never from the files.

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
    /// The body's hash, when it built.
    pub hash: Option<String>,
    /// Why it didn't build.
    pub error: Option<Value>,
}

/// A part: its features in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub id: String,
    pub revision: String,
    pub features: Vec<Feature>,
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
                        error: (f["ok"] == false).then(|| f["error"].clone()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Part {
            id: text(&v["part"]),
            revision: text(&v["revision"]),
            features,
            error: v.get("error").cloned(),
        }
    }

    pub fn feature(&self, id: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

/// A body's place: its part and feature.
pub type BodyKey = (String, String);

/// The model as the window knows it.
#[derive(Debug, Default)]
pub struct Model {
    pub default_unit: String,
    pub parts: BTreeMap<String, Part>,
    /// Display meshes by part and feature, each for the body hash it says.
    pub meshes: BTreeMap<BodyKey, Arc<WireBody>>,
    /// Meshes asked for and not yet answered, by the body hash they were
    /// asked for at: a burst of changes asks for each body once.
    pub asked: BTreeMap<BodyKey, String>,
    /// Bumped whenever anything here changes, so views know to redraw.
    pub generation: u64,
}

impl Model {
    /// Takes a part's report. Returns the bodies whose meshes are now
    /// missing or stale: they need fetching.
    pub fn apply_part(&mut self, report: &Value) -> Vec<BodyKey> {
        let part = Part::from_report(report);
        let id = part.id.clone();
        // Meshes of features that are gone, or failed, go.
        self.meshes.retain(|(p, f), m| {
            *p != id
                || part
                    .feature(f)
                    .is_some_and(|f| f.hash.as_deref() == Some(&m.hash))
        });
        let wanted = part
            .features
            .iter()
            .filter(|f| f.hash.is_some())
            .map(|f| (id.clone(), f.id.clone()))
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
                .and_then(|p| p.feature(&key.1))
                .and_then(|f| f.hash.clone())
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

    /// Forgets a part and its meshes.
    pub fn remove_part(&mut self, id: &str) {
        self.parts.remove(id);
        self.meshes.retain(|(p, _), _| p != id);
        self.asked.retain(|(p, _), _| p != id);
        self.generation += 1;
    }

    /// Keeps a fetched mesh if its body is still the current one.
    pub fn take_mesh(&mut self, part: &str, body: WireBody) -> bool {
        let current = self
            .parts
            .get(part)
            .and_then(|p| p.feature(&body.feature))
            .and_then(|f| f.hash.as_deref());
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
                p.features
                    .iter()
                    .filter(|f| f.hash.is_some())
                    .map(|f| (p.id.clone(), f.id.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn report(hashes: &[(&str, Option<&str>)]) -> Value {
        let features: Vec<Value> = hashes
            .iter()
            .map(|(id, h)| match h {
                Some(h) => json!({"id": id, "type": "box", "ok": true, "hash": h}),
                None => json!({"id": id, "type": "torus", "ok": false, "error": {"code": "rebuild_failed"}}),
            })
            .collect();
        json!({"part": "p", "revision": "r", "features": features})
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
