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
}
