//! A compare: the working files against a Git revision, as the window
//! shows it. It is read from `diff --model` from that revision to the
//! working files, and from the other side's meshes (`mesh.get` at that
//! revision), never from the files.
//!
//! Features are marked by how they differ. Only solids whose bodies, as
//! the model shows them, differ take part in the drawing: their faces are
//! coloured new or changed, and the other side's solid is ghosted with
//! the faces it loses in the removed colour. Unchanged solids are the same
//! on both sides, so a ghost of them would only fight the model for its
//! pixels.

use crate::core::model::BodyKey;
use ryucad::error::Error;
use ryucad::mesh::WireBody;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// How a feature differs from the revision compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    Changed,
    /// Only its result differs: something it reads changed.
    Rebuilt,
    Same,
}

impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Change::Added => "added",
            Change::Removed => "removed",
            Change::Changed => "changed",
            Change::Rebuilt => "rebuilt",
            Change::Same => "same",
        }
    }
}

/// One feature as the compare sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub change: Change,
    /// It moved in its part's list.
    pub moved: bool,
    /// Why it failed at the revision compared with: its error code.
    pub failed_there: Option<String>,
    /// What changed, one line each, for the property panel.
    pub lines: Vec<String>,
}

/// How a face of the working model differs from the other side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceMark {
    /// Its name is only in the working files.
    New,
    /// Its name is on both sides; its geometry differs.
    Changed,
}

/// A compare as the window knows it.
#[derive(Debug, Default)]
pub struct Compare {
    /// The revision as it was given (`HEAD`, a branch, a commit).
    pub rev: String,
    /// The commit it named when the diff was made.
    pub commit: Option<String>,
    /// The diff has come.
    pub loaded: bool,
    /// Why the diff failed.
    pub error: Option<Error>,
    /// Features that differ, by part and feature.
    pub features: BTreeMap<BodyKey, Mark>,
    /// Features only the other side has, by part, in the diff's order.
    pub removed: BTreeMap<String, Vec<String>>,
    /// Faces of the working bodies that differ, by body and name (hex).
    pub faces: BTreeMap<BodyKey, BTreeMap<String, FaceMark>>,
    /// Working bodies new as a whole: added, or failing on the other side.
    pub new_bodies: BTreeSet<BodyKey>,
    /// The other side's bodies to ghost, with their hashes there.
    pub wanted: BTreeMap<BodyKey, String>,
    /// Faces of the ghosts the working files no longer have, by name
    /// (hex); a body gone as a whole is in `gone_bodies` instead.
    pub gone: BTreeMap<BodyKey, BTreeSet<String>>,
    pub gone_bodies: BTreeSet<BodyKey>,
    /// The ghosts' meshes, each for the hash it says.
    pub ghosts: BTreeMap<BodyKey, Arc<WireBody>>,
    /// Ghosts asked for and not yet answered, by hash.
    pub asked: BTreeMap<BodyKey, String>,
    /// Bumped whenever anything here changes, so views know to redraw.
    pub generation: u64,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

/// A value in a change line: strings as they are, the rest as JSON, and
/// nothing as `(none)`.
fn shown(v: &Value) -> String {
    match v {
        Value::Null => "(none)".to_owned(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// `added`, `removed` and `changed` lists as lines: `entities added: l5`.
fn listed(what: &str, v: &Value, out: &mut Vec<String>) {
    for k in ["added", "removed", "changed"] {
        let ids = strings(&v[k]);
        if !ids.is_empty() {
            out.push(format!("{what} {k}: {}", ids.join(", ")));
        }
    }
}

/// A side's result as people read it: `built`, `failed (code)`, `absent`.
fn result(v: &Value) -> String {
    match v["ok"].as_bool() {
        Some(true) => "built".to_owned(),
        Some(false) => format!("failed ({})", v["error"].as_str().unwrap_or("?")),
        None => "absent".to_owned(),
    }
}

/// What changed in one feature, one line each.
fn lines(f: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for c in f["changes"].as_array().into_iter().flatten() {
        out.push(format!(
            "{}: {} -> {}",
            c["at"].as_str().unwrap_or(""),
            shown(&c["from"]),
            shown(&c["to"])
        ));
    }
    for side in strings(&f["invalid"]) {
        let at = if side == "from" { "there" } else { "here" };
        out.push(format!("can't be read {at}"));
    }
    if f.get("entities").is_some() {
        listed("entities", &f["entities"], &mut out);
        listed("constraints", &f["constraints"], &mut out);
        if f["positions"] == "changed" {
            out.push("positions changed".to_owned());
        }
    }
    let m = &f["model"];
    if m.is_object() {
        let (from, to) = (result(&m["from"]), result(&m["to"]));
        if from != to {
            out.push(format!("result: {from} -> {to}"));
        }
        let b = &m["body"];
        if b.is_object() {
            for kind in ["faces", "edges", "vertices"] {
                let n = |k: &str| b[kind][k].as_array().map_or(0, Vec::len);
                let (new, changed, gone) = (n("new"), n("changed"), n("gone"));
                if new + changed + gone > 0 {
                    out.push(format!("{kind}: {new} new, {changed} changed, {gone} gone"));
                }
            }
        }
        let s = &m["sketch"];
        if s.is_object() && s["from"] != s["to"] {
            let state = |v: &Value| {
                format!(
                    "{}, {} dof",
                    v["state"].as_str().unwrap_or("?"),
                    v["dof"].as_u64().unwrap_or(0)
                )
            };
            out.push(format!(
                "sketch: {} -> {}",
                state(&s["from"]),
                state(&s["to"])
            ));
        }
    }
    out
}

impl Compare {
    /// A compare with `rev` whose diff hasn't come yet.
    pub fn new(rev: &str) -> Compare {
        Compare {
            rev: rev.to_owned(),
            ..Compare::default()
        }
    }

    /// Takes a `diff --model` answer from the revision to the working
    /// files. Ghosts of bodies that still differ at the same hash are
    /// kept; the rest go.
    pub fn take_diff(&mut self, v: &Value) {
        self.commit = v["from"]["commit"].as_str().map(str::to_owned);
        self.loaded = true;
        self.error = None;
        self.features.clear();
        self.removed.clear();
        self.faces.clear();
        self.new_bodies.clear();
        self.wanted.clear();
        self.gone.clear();
        self.gone_bodies.clear();
        let parts = v["parts"].as_object().into_iter().flatten();
        for (part, p) in parts {
            let moved = strings(&p["order"]["moved"]);
            let features = p["features"].as_object().into_iter().flatten();
            for (id, f) in features {
                let key = (part.clone(), id.clone());
                let m = &f["model"];
                let change = match f["status"].as_str() {
                    Some("added") => Change::Added,
                    Some("removed") => Change::Removed,
                    Some("changed") => Change::Changed,
                    _ if m.is_object() => Change::Rebuilt,
                    _ => Change::Same,
                };
                let is_moved = moved.contains(id);
                let mut lines = lines(f);
                if is_moved {
                    lines.insert(0, "moved in the part's list".to_owned());
                }
                let failed_there = (m["from"]["ok"] == false)
                    .then(|| m["from"]["error"].as_str().unwrap_or("?").to_owned());
                if change == Change::Removed {
                    self.removed
                        .entry(part.clone())
                        .or_default()
                        .push(id.clone());
                }
                if change != Change::Same || is_moved {
                    self.features.insert(
                        key.clone(),
                        Mark {
                            change,
                            moved: is_moved,
                            failed_there,
                            lines,
                        },
                    );
                }
            }
            // The model's solids, by founder: their faces new or changed,
            // ghosts of what they were, and what is gone of them.
            let solids = p["solids"].as_object().into_iter().flatten();
            for (id, m) in solids {
                let key = (part.clone(), id.clone());
                let (from_ok, to_ok) = (m["from"]["ok"] == true, m["to"]["ok"] == true);
                let b = &m["body"];
                if b.is_object() {
                    let faces = self.faces.entry(key.clone()).or_default();
                    for n in strings(&b["faces"]["new"]) {
                        faces.insert(n, FaceMark::New);
                    }
                    for n in strings(&b["faces"]["changed"]) {
                        faces.insert(n, FaceMark::Changed);
                    }
                    let gone: BTreeSet<String> = strings(&b["faces"]["gone"]).into_iter().collect();
                    if !gone.is_empty() {
                        self.gone.insert(key.clone(), gone);
                    }
                } else if to_ok && !from_ok {
                    // New, or failing there: all of it is new.
                    self.new_bodies.insert(key.clone());
                }
                if from_ok {
                    if let Some(h) = m["from"]["hash"].as_str() {
                        self.wanted.insert(key.clone(), h.to_owned());
                    }
                    if !to_ok {
                        self.gone_bodies.insert(key);
                    }
                }
            }
        }
        let wanted = &self.wanted;
        self.ghosts
            .retain(|k, g| wanted.get(k).is_some_and(|h| *h == g.hash));
        self.generation += 1;
    }

    /// The diff failed: nothing is marked.
    pub fn failed(&mut self, e: Error) {
        let rev = std::mem::take(&mut self.rev);
        *self = Compare::new(&rev);
        self.loaded = true;
        self.error = Some(e);
        self.generation += 1;
    }

    /// The ghosts wanted and not held or asked for at their hash, with it,
    /// now noted as asked for.
    pub fn ask(&mut self) -> Vec<(BodyKey, String)> {
        let mut out = Vec::new();
        for (key, hash) in &self.wanted {
            let held = self.ghosts.get(key).is_some_and(|g| g.hash == *hash);
            if !held && self.asked.get(key) != Some(hash) {
                self.asked.insert(key.clone(), hash.clone());
                out.push((key.clone(), hash.clone()));
            }
        }
        out
    }

    /// An answer to asking for ghost `key` at `hash` came, or never will.
    pub fn answered(&mut self, key: &BodyKey, hash: &str) {
        if self.asked.get(key).is_some_and(|h| h == hash) {
            self.asked.remove(key);
        }
    }

    /// The other side has no body for `key` at `hash` (a sketch has
    /// none): there is no ghost to wait for.
    pub fn no_ghost(&mut self, key: &BodyKey, hash: &str) {
        if self.wanted.get(key).is_some_and(|h| h == hash) {
            self.wanted.remove(key);
            self.generation += 1;
        }
    }

    /// Keeps a ghost if the compare still wants it at its hash.
    pub fn take_ghost(&mut self, part: &str, body: WireBody) -> bool {
        let key = (part.to_owned(), body.feature.clone());
        if self.wanted.get(&key) != Some(&body.hash) {
            return false;
        }
        self.ghosts.insert(key, Arc::new(body));
        self.generation += 1;
        true
    }

    /// Drops every ghost: the display quality changed.
    pub fn clear_ghosts(&mut self) {
        self.ghosts.clear();
        self.asked.clear();
        self.generation += 1;
    }

    /// Whether a ghost's face (by name, hex) is gone from the working
    /// files.
    pub fn is_gone(&self, key: &BodyKey, name: &str) -> bool {
        self.gone_bodies.contains(key) || self.gone.get(key).is_some_and(|g| g.contains(name))
    }

    /// How many features were added, removed and otherwise changed.
    pub fn counts(&self) -> (usize, usize, usize) {
        let n = |c: Change| self.features.values().filter(|m| m.change == c).count();
        let changed = self.features.len() - n(Change::Added) - n(Change::Removed);
        (n(Change::Added), n(Change::Removed), changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hex(c: char) -> String {
        c.to_string().repeat(32)
    }

    /// A diff from `HEAD`: box `b` grew (one face changed, one new, one
    /// gone), sphere `c` was removed, cylinder `n` added, sketch `s` gained
    /// a line, and `t` failed at `HEAD` but builds now.
    fn diff() -> Value {
        let body = json!({"faces": {"new": [hex('a')], "changed": [hex('b')], "gone": [hex('c')]},
                          "edges": {"new": [], "changed": [hex('d')], "gone": []},
                          "vertices": {"new": [], "changed": [], "gone": []}});
        json!({
            "from": {"rev": "HEAD", "commit": "c0ffee"},
            "to": {"rev": "working"},
            "parts": {"p": {
                "status": "changed",
                "order": {"added": ["n"], "removed": ["c"], "moved": ["b"]},
                "features": {
                    "b": {"status": "changed",
                          "changes": [{"at": "/params/x", "from": "10 mm", "to": "12 mm"}],
                          "model": {"from": {"ok": true, "hash": "h1"},
                                    "to": {"ok": true, "hash": "h2"}, "body": body}},
                    "c": {"status": "removed",
                          "model": {"from": {"ok": true, "hash": "h3"}, "to": null}},
                    "n": {"status": "added",
                          "model": {"from": null, "to": {"ok": true, "hash": "h4"}}},
                    "s": {"status": "changed",
                          "entities": {"added": ["l5"], "removed": [], "changed": []},
                          "constraints": {"added": [], "removed": [], "changed": []},
                          "positions": "changed"},
                    "t": {"status": "same",
                          "model": {"from": {"ok": false, "error": "rebuild_failed"},
                                    "to": {"ok": true, "hash": "h5"}}}
                },
                "solids": {
                    "b": {"from": {"ok": true, "hash": "h1"}, "to": {"ok": true, "hash": "h2"},
                          "body": body},
                    "c": {"from": {"ok": true, "hash": "h3"}, "to": null},
                    "n": {"from": null, "to": {"ok": true, "hash": "h4"}},
                    "t": {"from": {"ok": false}, "to": {"ok": true, "hash": "h5"}}
                }
            }}
        })
    }

    fn key(f: &str) -> BodyKey {
        ("p".to_owned(), f.to_owned())
    }

    #[test]
    fn a_diff_marks_features_faces_and_ghosts() {
        let mut c = Compare::new("HEAD");
        c.take_diff(&diff());
        assert_eq!(c.commit.as_deref(), Some("c0ffee"));
        let b = &c.features[&key("b")];
        assert_eq!((b.change, b.moved), (Change::Changed, true));
        assert_eq!(
            b.lines,
            [
                "moved in the part's list",
                "/params/x: 10 mm -> 12 mm",
                "faces: 1 new, 1 changed, 1 gone",
                "edges: 0 new, 1 changed, 0 gone",
            ]
        );
        assert_eq!(c.features[&key("c")].change, Change::Removed);
        assert_eq!(c.features[&key("c")].lines, ["result: built -> absent"]);
        assert_eq!(c.removed["p"], ["c"]);
        assert_eq!(c.features[&key("n")].change, Change::Added);
        assert_eq!(
            c.features[&key("s")].lines,
            ["entities added: l5", "positions changed"]
        );
        let t = &c.features[&key("t")];
        assert_eq!(t.change, Change::Rebuilt);
        assert_eq!(t.failed_there.as_deref(), Some("rebuild_failed"));
        assert_eq!(c.counts(), (1, 1, 3));
        // Faces: b's by name; n and t new as a whole.
        assert_eq!(c.faces[&key("b")][&hex('a')], FaceMark::New);
        assert_eq!(c.faces[&key("b")][&hex('b')], FaceMark::Changed);
        assert_eq!(
            c.new_bodies,
            [key("n"), key("t")].into_iter().collect::<BTreeSet<_>>()
        );
        // Ghosts: b and c as they were; c is gone whole, b loses a face.
        assert_eq!(c.wanted.len(), 2);
        assert!(c.is_gone(&key("b"), &hex('c')) && !c.is_gone(&key("b"), &hex('b')));
        assert!(c.is_gone(&key("c"), &hex('e')));
    }

    fn ghost(feature: &str, hash: &str) -> WireBody {
        WireBody {
            feature: feature.into(),
            hash: hash.into(),
            mesh: Err(json!(null)),
        }
    }

    #[test]
    fn ghosts_are_asked_for_once_and_kept_while_wanted() {
        let mut c = Compare::new("HEAD");
        c.take_diff(&diff());
        let asked = c.ask();
        assert_eq!(asked.len(), 2);
        assert!(c.ask().is_empty());
        assert!(c.take_ghost("p", ghost("b", "h1")));
        assert!(!c.take_ghost("p", ghost("b", "h0")));
        assert!(!c.take_ghost("p", ghost("n", "h4")));
        c.answered(&key("b"), "h1");
        // A feature with no body there wants no ghost.
        assert!(c.wanted.contains_key(&key("c")));
        c.no_ghost(&key("c"), "h0");
        assert!(c.wanted.contains_key(&key("c")));
        c.no_ghost(&key("c"), "h3");
        assert!(!c.wanted.contains_key(&key("c")));
        // The same diff again keeps the ghost; one where b no longer
        // differs drops it.
        c.take_diff(&diff());
        assert!(c.ghosts.contains_key(&key("b")));
        let mut d = diff();
        for list in ["features", "solids"] {
            d["parts"]["p"][list].as_object_mut().unwrap().remove("b");
        }
        c.take_diff(&d);
        assert!(c.ghosts.is_empty());
        c.failed(Error::new(ryucad::error::Code::RevNotFound, "gone"));
        assert!(c.features.is_empty() && c.error.is_some() && c.rev == "HEAD");
    }
}
