//! Legacy history (design 935): a legacy system's revisions of an item,
//! read one row per (item, revision) through a mapping's `history`
//! block, kept in the item's provenance as a record of what that system
//! said. They aren't releases: nothing derives a state or a content hash
//! from them.

use serde_json::{Value, json};
use std::collections::BTreeMap;

/// One legacy revision as a history mapping read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    /// As written, trimmed.
    pub label: String,
    pub date: Option<String>,
    pub state: Option<String>,
    pub change: Option<String>,
    pub author: Option<String>,
    /// The mapping's fields read on this revision's row, by target.
    pub values: BTreeMap<String, Value>,
    /// The row's cells nothing maps.
    pub extra: BTreeMap<String, String>,
    /// The row it was read from.
    pub at: u64,
    /// Whether the mapping's `released` rule matched its row.
    pub released: bool,
}

impl Revision {
    /// `{label, date?, state?, change?, author?, values, extra?}`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"label": self.label, "values": self.values});
        for (k, x) in [
            ("date", &self.date),
            ("state", &self.state),
            ("change", &self.change),
            ("author", &self.author),
        ] {
            if let Some(x) = x {
                v[k] = json!(x);
            }
        }
        if !self.extra.is_empty() {
            v["extra"] = json!(self.extra);
        }
        v
    }
}

/// A key's revisions in order, and the carried one's label: the last
/// released in order, or none when the mapping's rule released none.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Legacy {
    pub revisions: Vec<Revision>,
    pub carried: Option<String>,
}

impl Legacy {
    pub fn json(&self) -> Vec<Value> {
        self.revisions.iter().map(Revision::to_json).collect()
    }
}
