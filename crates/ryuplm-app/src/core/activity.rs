//! The activity log as the window lists it, and what undo and redo apply
//! to: `activity.log`'s entries, newest first.

use serde_json::Value;

/// Whether `undo` applies to an entry: a change or a redo not undone.
pub fn can_undo(e: &Value) -> bool {
    e["undone_by"].is_null() && matches!(e["kind"].as_str(), Some("change" | "redo"))
}

/// Whether `redo` applies: an undo not redone yet.
pub fn can_redo(e: &Value) -> bool {
    e["undone_by"].is_null() && e["kind"] == "undo"
}
