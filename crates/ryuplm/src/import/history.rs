//! Legacy history (design 935): a legacy system's revisions of an item,
//! read one row per (item, revision) through a mapping's `history`
//! block, kept in the item's provenance as a record of what that system
//! said. They aren't releases: nothing derives a state or a content hash
//! from them.

use crate::error::Result;
use crate::id::Id;
use crate::source::Source;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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

/// The items with release records: their folders under `releases/`
/// (design 935: an import never moves their carried label).
pub fn released(src: &dyn Source) -> Result<BTreeSet<Id>> {
    Ok(src
        .list(ryupdm::RELEASES)?
        .into_iter()
        .filter(|(_, k)| *k == crate::source::Kind::Dir)
        .filter_map(|(d, _)| Id::parse(&d, "item").ok())
        .collect())
}

/// The source's revisions in its order, each stored one it no longer has
/// kept after the stored revision before it: a revision gone from the
/// source stays (design 935).
pub fn merged(stored: &[Value], new: &[Value]) -> Vec<Value> {
    let label = |v: &Value| v["label"].as_str().unwrap_or_default().to_owned();
    let mut out: Vec<Value> = new.to_vec();
    let mut before: Option<String> = None;
    for s in stored {
        let l = label(s);
        if !out.iter().any(|x| label(x) == l) {
            let at = before
                .as_ref()
                .and_then(|b| out.iter().position(|x| &label(x) == b))
                .map_or(0, |i| i + 1);
            out.insert(at, s.clone());
        }
        before = Some(l);
    }
    out
}
