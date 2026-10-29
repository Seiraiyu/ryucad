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
//! pixels. An assembly's occurrences take part the same way, solid by
//! solid: painted where their bodies differ, ghosted where they were when
//! they moved, changed or went (issue #109).

use crate::core::model::{BodyKey, Occurrence, Pose};
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

/// How a part's PDM differs in a compare: the records it adds, as the
/// tree marks them (`proposes B`, `obsoletes A`), and one line for each
/// property and the state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdmMark {
    pub marks: Vec<String>,
    pub lines: Vec<String>,
}

impl PdmMark {
    /// From a part's `pdm` block in a diff.
    pub fn of(v: &Value) -> PdmMark {
        let mut m = PdmMark::default();
        for r in v["records"]["added"].as_array().into_iter().flatten() {
            let label = r["label"].as_str().unwrap_or("?");
            m.marks.push(match r["kind"].as_str() {
                Some("obsolete") => format!("obsoletes {label}"),
                _ => format!("proposes {label}"),
            });
        }
        for (k, what) in [("removed", "record removed"), ("changed", "record changed")] {
            for r in v["records"][k].as_array().into_iter().flatten() {
                m.lines.push(format!(
                    "{what}: {} {}",
                    r["label"].as_str().unwrap_or("?"),
                    r["kind"].as_str().unwrap_or("")
                ));
            }
        }
        for (name, c) in v["properties"].as_object().into_iter().flatten() {
            let revising = if c["revising"] == true {
                " (revising)"
            } else {
                ""
            };
            m.lines.push(format!(
                "{name}: {} → {}{revising}",
                shown(&c["from"]),
                shown(&c["to"])
            ));
        }
        if let Some(s) = v.get("state") {
            m.lines.push(format!(
                "state: {} → {}",
                s["from"].as_str().unwrap_or("?"),
                s["to"].as_str().unwrap_or("?")
            ));
        }
        m
    }
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
    /// What differs in each part's PDM, by part (only parts that differ).
    pub pdm: BTreeMap<String, PdmMark>,
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
    /// Where an assembly's ghosts were: each occurrence's solid at the
    /// other side's pose. A part's ghosts are where their part is.
    pub ghost_poses: BTreeMap<BodyKey, Pose>,
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

/// A value in a change line: strings as they are, a reference to an
/// entity as its feature and the end of its name (`ex1/23ab0b16`, as the
/// edge checkboxes say it), the rest as JSON, and nothing as `(none)`.
fn shown(v: &Value) -> String {
    match v {
        Value::Null => "(none)".to_owned(),
        Value::String(s) => s.clone(),
        v => match reference(v) {
            Some((feature, name)) => {
                let end = name.len().saturating_sub(8);
                format!("{feature}/{}", name.get(end..).unwrap_or(name))
            }
            None => v.to_string(),
        },
    }
}

/// A reference (`{feature, name}`) as its two strings.
fn reference(v: &Value) -> Option<(&str, &str)> {
    let o = v.as_object()?;
    if o.len() != 2 {
        return None;
    }
    Some((o.get("feature")?.as_str()?, o.get("name")?.as_str()?))
}

/// One change as lines: a change inside objects or arrays (a fillet's
/// `groups`, which a diff gives whole) as each value that differs, by
/// its pointer below `at` (`/groups/0/radius: 3 mm -> 4 mm`); anything
/// else, and references, whole.
fn changed(at: &str, from: &Value, to: &Value, out: &mut Vec<String>) {
    if from == to {
        return;
    }
    let whole = |out: &mut Vec<String>| out.push(format!("{at}: {} → {}", shown(from), shown(to)));
    if reference(from).is_some() || reference(to).is_some() {
        return whole(out);
    }
    match (from, to) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for k in keys {
                let null = Value::Null;
                let key = k.replace('~', "~0").replace('/', "~1");
                changed(
                    &format!("{at}/{key}"),
                    a.get(k).unwrap_or(&null),
                    b.get(k).unwrap_or(&null),
                    out,
                );
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for i in 0..a.len().max(b.len()) {
                let null = Value::Null;
                changed(
                    &format!("{at}/{i}"),
                    a.get(i).unwrap_or(&null),
                    b.get(i).unwrap_or(&null),
                    out,
                );
            }
        }
        _ => whole(out),
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
        let at = c["at"].as_str().unwrap_or("");
        changed(at, &c["from"], &c["to"], &mut out);
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
            out.push(format!("result: {from} → {to}"));
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
                "sketch: {} → {}",
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
        self.pdm.clear();
        self.faces.clear();
        self.new_bodies.clear();
        self.wanted.clear();
        self.gone.clear();
        self.gone_bodies.clear();
        self.ghost_poses.clear();
        let parts = v["parts"].as_object().into_iter().flatten();
        for (part, p) in parts {
            if p["pdm"].is_object() {
                self.pdm.insert(part.clone(), PdmMark::of(&p["pdm"]));
            }
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
            // An assembly's occurrences, by path: each solid painted as
            // its body differs, and ghosted where it was when it moved,
            // changed or went.
            let occurrences = &p["occurrences"];
            let sides = occurrences["sides"].as_object().into_iter().flatten();
            for (path, s) in sides {
                let side = |v: &Value| (!v.is_null()).then(|| Occurrence::from_json(v));
                let (from, to) = (side(&s["from"]), side(&s["to"]));
                let key = |solid: &str| (part.clone(), format!("{path}/{solid}"));
                let hash_in = |o: &Option<Occurrence>, solid: &str| {
                    o.as_ref()?
                        .solids
                        .iter()
                        .find(|(s, _)| s == solid)
                        .map(|(_, h)| h.clone())
                };
                for (solid, hash) in to.iter().flat_map(|o| o.solids.iter()) {
                    let k = key(solid);
                    let b = &occurrences["bodies"][&k.1];
                    match hash_in(&from, solid) {
                        None => {
                            self.new_bodies.insert(k);
                        }
                        Some(was) if was != *hash && b.is_object() => {
                            let faces = self.faces.entry(k.clone()).or_default();
                            for n in strings(&b["faces"]["new"]) {
                                faces.insert(n, FaceMark::New);
                            }
                            for n in strings(&b["faces"]["changed"]) {
                                faces.insert(n, FaceMark::Changed);
                            }
                            let gone: BTreeSet<String> =
                                strings(&b["faces"]["gone"]).into_iter().collect();
                            if !gone.is_empty() {
                                self.gone.insert(k, gone);
                            }
                        }
                        Some(_) => {}
                    }
                }
                let Some(was) = &from else { continue };
                let moved = to.as_ref().is_none_or(|o| o.pose() != was.pose());
                for (solid, hash) in &was.solids {
                    let k = key(solid);
                    let now = hash_in(&to, solid);
                    if now.as_ref() == Some(hash) && !moved {
                        continue;
                    }
                    if now.is_none() {
                        self.gone_bodies.insert(k.clone());
                    }
                    self.wanted.insert(k.clone(), hash.clone());
                    self.ghost_poses.insert(k, was.pose());
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

    /// Where `key`'s ghost is drawn: an occurrence's where it was, else
    /// where its part is.
    pub fn ghost_pose(&self, key: &BodyKey) -> Pose {
        self.ghost_poses.get(key).copied().unwrap_or(Pose::IDENTITY)
    }

    /// What `mesh.get` is asked for to draw `key`'s ghost: an assembly's
    /// occurrence by its path (`<path>/<solid>` less its solid, whose
    /// body comes labelled `<path>/<solid>`), else the feature itself.
    pub fn ghost_request(&self, key: &BodyKey) -> String {
        match key.1.rsplit_once('/') {
            Some((path, _)) if self.ghost_poses.contains_key(key) => path.to_owned(),
            _ => key.1.clone(),
        }
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
    fn a_fillets_groups_change_by_the_values_that_differ() {
        // #103: a radius change showed both `groups` arrays whole.
        let edge = |n: char| json!({"feature": "ex1", "name": format!("{}23ab0b16", hex(n).get(..24).unwrap())});
        let from = json!([{"radius": "3 mm", "edges": [edge('a'), edge('b')]}]);
        let mut to = from.clone();
        to[0]["radius"] = json!("4 mm");
        let f = json!({"changes": [{"at": "/groups", "from": from, "to": to}]});
        assert_eq!(lines(&f), ["/groups/0/radius: 3 mm → 4 mm"]);
        // An edge more, a group fewer: the values added and removed.
        let mut more = from.clone();
        more[0]["edges"].as_array_mut().unwrap().push(edge('c'));
        let f = json!({"changes": [{"at": "/groups", "from": from, "to": more}]});
        assert_eq!(lines(&f), ["/groups/0/edges/2: (none) → ex1/23ab0b16"]);
        let f = json!({"changes": [{"at": "/groups", "from": from, "to": []}]});
        assert_eq!(
            lines(&f),
            [
                r#"/groups/0: {"edges":[{"feature":"ex1","name":"aaaaaaaaaaaaaaaaaaaaaaaa23ab0b16"},{"feature":"ex1","name":"bbbbbbbbbbbbbbbbbbbbbbbb23ab0b16"}],"radius":"3 mm"} → (none)"#
            ]
        );
        // A reference changes whole.
        let f = json!({"changes": [{"at": "/face", "from": edge('a'), "to": null}]});
        assert_eq!(lines(&f), ["/face: ex1/23ab0b16 → (none)"]);
    }

    #[test]
    fn an_assemblys_occurrences_are_painted_and_ghosted_where_they_were() {
        let occ = |path: &str, hash: &str, x: f64| {
            json!({"path": path, "part": "bolt", "rev": "working",
                   "solids": [{"solid": "shank", "hash": hash}],
                   "pose": {"origin_m": [x, 0, 0], "z": [0, 0, 1], "x": [1, 0, 0]}})
        };
        let body = json!({"faces": {"new": [], "changed": [hex('b')], "gone": [hex('c')]}});
        let mut c = Compare::new("HEAD");
        c.take_diff(&json!({"from": {"commit": "c0ffee"}, "parts": {"top": {
            "status": "same", "features": {}, "order": {"moved": []},
            "occurrences": {
                "added": ["s3/b1"], "removed": ["s2/b1"], "moved": ["s1/b1"],
                "changed": ["s1/b2"],
                "sides": {
                    "s1/b1": {"from": occ("s1/b1", "h", 0.0), "to": occ("s1/b1", "h", 0.005)},
                    "s1/b2": {"from": occ("s1/b2", "h", 0.08), "to": occ("s1/b2", "g", 0.08)},
                    "s2/b1": {"from": occ("s2/b1", "h", 0.2), "to": null},
                    "s3/b1": {"from": null, "to": occ("s3/b1", "h", 0.3)}},
                "bodies": {"s1/b2/shank": body}}}}}));
        let k = |p: &str| ("top".to_owned(), format!("{p}/shank"));
        // Painted: the changed bolt's faces, the added one whole.
        assert_eq!(
            c.faces[&k("s1/b2")].get(&hex('b')),
            Some(&FaceMark::Changed)
        );
        assert!(c.new_bodies.contains(&k("s3/b1")));
        assert!(!c.faces.contains_key(&k("s1/b1")), "only moved");
        // Ghosted where they were: moved, changed and gone; not the new.
        for (p, x) in [("s1/b1", 0.0), ("s1/b2", 0.08), ("s2/b1", 0.2)] {
            assert_eq!(c.wanted[&k(p)], "h");
            assert_eq!(c.ghost_pose(&k(p)).origin, [x, 0.0, 0.0]);
            assert_eq!(c.ghost_request(&k(p)), p);
        }
        assert!(!c.wanted.contains_key(&k("s3/b1")));
        assert!(c.is_gone(&k("s2/b1"), &hex('a')));
        assert!(c.is_gone(&k("s1/b2"), &hex('c')));
        assert!(!c.is_gone(&k("s1/b1"), &hex('c')));
        // A part's ghost is asked for by its solid, where it is.
        let mut p = Compare::new("HEAD");
        p.take_diff(&diff());
        assert_eq!(p.ghost_request(&key("b")), "b");
        assert_eq!(p.ghost_pose(&key("b")), Pose::IDENTITY);
    }

    #[test]
    fn a_parts_pdm_is_marked_and_said() {
        let mut c = Compare::new("HEAD");
        c.take_diff(&json!({"from": {"commit": "abc"}, "parts": {
            "p": {"status": "changed", "features": {}, "order": {"moved": []},
                  "pdm": {"properties": {"number": {"from": "100", "to": "101", "revising": true},
                                         "name": {"to": "Pin", "revising": false}},
                          "records": {"added": [{"label": "B", "kind": "release"},
                                                {"label": "A", "kind": "obsolete"}],
                                      "removed": [{"label": "C", "kind": "release"}],
                                      "changed": []},
                          "state": {"from": "released A", "to": "A modified, B proposed"}}},
            "q": {"status": "changed", "features": {}, "order": {"moved": []}},
        }}));
        let m = &c.pdm["p"];
        assert_eq!(m.marks, ["proposes B", "obsoletes A"]);
        assert_eq!(
            m.lines,
            [
                "record removed: C release",
                "name: (none) → Pin",
                "number: 100 → 101 (revising)",
                "state: released A → A modified, B proposed"
            ]
        );
        assert!(!c.pdm.contains_key("q"));
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
                "/params/x: 10 mm → 12 mm",
                "faces: 1 new, 1 changed, 1 gone",
                "edges: 0 new, 1 changed, 0 gone",
            ]
        );
        assert_eq!(c.features[&key("c")].change, Change::Removed);
        assert_eq!(c.features[&key("c")].lines, ["result: built → absent"]);
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
