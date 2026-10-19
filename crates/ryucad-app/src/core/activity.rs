//! The session's activity log as the window knows it: every change, who
//! made it and the files it touched, kept up to date from
//! `activity.appended`. Entries stay as the daemon reports them.

use serde_json::Value;

/// How many entries the window asks for when it opens.
pub const LOADED: usize = 200;

#[derive(Debug, Default)]
pub struct Activity {
    /// Oldest first.
    pub entries: Vec<Value>,
    /// Bumped whenever an entry arrives or changes.
    pub generation: u64,
}

/// Whether `undo` applies to an entry: a change or a redo not undone yet.
pub fn can_undo(e: &Value) -> bool {
    e["undone_by"].is_null() && matches!(e["kind"].as_str(), Some("change" | "redo"))
}

/// Whether `redo` applies to an entry: an undo not redone yet.
pub fn can_redo(e: &Value) -> bool {
    e["undone_by"].is_null() && e["kind"] == "undo"
}

impl Activity {
    /// Replaces the entries with an `activity.log` answer.
    pub fn reset(&mut self, entries: &Value) {
        self.entries = entries.as_array().cloned().unwrap_or_default();
        self.generation += 1;
    }

    /// Takes an `activity.appended` entry. An undo or redo marks the entry
    /// it reverted, as the daemon does.
    pub fn appended(&mut self, entry: &Value) {
        if let Some(target) = entry["target"].as_u64()
            && let Some(t) = self.entries.iter_mut().find(|e| e["seq"] == target)
        {
            t["undone_by"] = entry["seq"].clone();
        }
        self.entries.push(entry.clone());
        // The daemon keeps 1000; so does the window.
        let over = self
            .entries
            .len()
            .saturating_sub(ryucad::activity::MAX_ENTRIES);
        self.entries.drain(..over);
        self.generation += 1;
    }

    pub fn get(&self, seq: u64) -> Option<&Value> {
        self.entries.iter().find(|e| e["seq"] == seq)
    }

    /// What an entry did, for its row: the method and the parts and
    /// features it touched, and for an undo or redo, what it reverted.
    pub fn describe(&self, e: &Value) -> String {
        let reverted = |verb: &str| match e["target"].as_u64().and_then(|t| self.get(t)) {
            Some(t) => format!("{verb} of {}", self.describe(t)),
            None => format!("{verb} of an earlier change"),
        };
        match e["kind"].as_str() {
            Some("undo") => reverted("undo"),
            Some("redo") => reverted("redo"),
            Some("external") => format!("files changed: {}", touched(e)),
            _ => {
                let method = e["method"].as_str().unwrap_or_default();
                match touched(e) {
                    t if t.is_empty() => method.to_owned(),
                    t => format!("{method} {t}"),
                }
            }
        }
    }
}

/// The parts and features (`part/feature`) whose files an entry touched,
/// and any other file by its path.
fn touched(e: &Value) -> String {
    let mut names: Vec<String> = Vec::new();
    for f in e["files"].as_array().into_iter().flatten() {
        let Some(path) = f["path"].as_str() else {
            continue;
        };
        let parts: Vec<&str> = path.split('/').collect();
        let name = match parts.as_slice() {
            ["parts", part, "features", file] => {
                format!("{part}/{}", file.strip_suffix(".json").unwrap_or(file))
            }
            ["parts", part, ..] => (*part).to_owned(),
            _ => path.to_owned(),
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    const SHOWN: usize = 3;
    let more = names.len().saturating_sub(SHOWN);
    names.truncate(SHOWN);
    let mut out = names.join(", ");
    if more > 0 {
        out.push_str(&format!(" and {more} more"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn undos_mark_what_they_undid() {
        let mut a = Activity::default();
        a.reset(&json!([{"seq": 1, "kind": "change", "undone_by": null}]));
        assert!(can_undo(&a.entries[0]));
        a.appended(&json!({"seq": 2, "kind": "undo", "target": 1, "undone_by": null}));
        assert!(!can_undo(a.get(1).unwrap()));
        assert!(can_redo(a.get(2).unwrap()));
        a.appended(&json!({"seq": 3, "kind": "redo", "target": 2, "undone_by": null}));
        assert!(!can_redo(a.get(2).unwrap()));
        assert!(can_undo(a.get(3).unwrap()));
        assert!(!can_undo(
            &json!({"seq": 4, "kind": "external", "undone_by": null})
        ));
        assert_eq!(a.generation, 3);
    }

    #[test]
    fn rows_say_what_they_touched() {
        let mut a = Activity::default();
        a.reset(&json!([
            {"seq": 7, "kind": "change", "method": "feature.edit",
             "params": {"part": "p", "feature": "boss"},
             "files": [{"path": "parts/p/features/boss.json", "change": "modified"}]},
            {"seq": 8, "kind": "change", "method": "doctor.repair", "params": {},
             "files": [{"path": "parts/q/part.json", "change": "created"},
                       {"path": "parts/q/features/a.json", "change": "modified"}]},
            {"seq": 9, "kind": "external", "method": null,
             "files": [{"path": "ryucad.json", "change": "modified"}]},
            {"seq": 10, "kind": "undo", "target": 7, "files": []},
            {"seq": 11, "kind": "undo", "target": 2, "files": []},
        ]));
        let d: Vec<String> = a.entries.iter().map(|e| a.describe(e)).collect();
        assert_eq!(
            d,
            [
                "feature.edit p/boss",
                "doctor.repair q, q/a",
                "files changed: ryucad.json",
                "undo of feature.edit p/boss",
                "undo of an earlier change",
            ]
        );
    }
}
