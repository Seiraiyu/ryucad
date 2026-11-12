//! What changed between two layouts of a block, by key (plan 4.B: ryueda's
//! `diff --layout` and its compare renders): each key of
//! [`crate::merge::Key`]'s space added, removed or changed, a part whose
//! place alone changed on its sheet `moved`; and the instructions either
//! side has that the other hasn't, by what they say.

use crate::layout::{Instruction, Layout};
use crate::merge::{Key, values};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Added,
    Removed,
    Changed,
    /// A part's place on the same sheet (its position, turn or mirror).
    Moved,
}

/// One key that differs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub key: Key,
    pub change: Change,
    /// The sheet it draws on: the newer side's, else the older one's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Value>,
}

/// The sheet key `k` draws on, from its value `v` or its key.
fn sheet_of(k: &Key, v: Option<&Value>, to: &Layout, from: &Layout) -> Option<String> {
    match k.section {
        "sheets" => Some(k.key.clone()),
        "routes" | "buses" => k.key.split_once('|').map(|(_, s)| s.to_owned()),
        "text" => to
            .placed
            .get(&k.key)
            .or_else(|| from.placed.get(&k.key))
            .map(|p| p.sheet.clone()),
        _ => v?.get("sheet")?.as_str().map(str::to_owned),
    }
}

/// Every key of `from` and `to` that differs, in key order.
pub fn layouts(from: &Layout, to: &Layout) -> Vec<Entry> {
    let (a, b) = (values(from), values(to));
    let keys: BTreeSet<&Key> = a.keys().chain(b.keys()).collect();
    let mut out = Vec::new();
    for k in keys {
        let (x, y) = (a.get(k), b.get(k));
        let change = match (x, y) {
            (None, Some(_)) => Change::Added,
            (Some(_), None) => Change::Removed,
            (Some(x), Some(y)) if x == y => continue,
            (Some(x), Some(y)) if k.section == "placed" && x.get("sheet") == y.get("sheet") => {
                Change::Moved
            }
            _ => Change::Changed,
        };
        out.push(Entry {
            sheet: sheet_of(k, y.or(x), to, from),
            key: k.clone(),
            change,
            from: x.cloned(),
            to: y.cloned(),
        });
    }
    out
}

/// An instruction with its ID.
pub type Numbered = (String, Instruction);

/// The instructions `to` has that `from` hasn't (added) and those `from`
/// has that `to` hasn't (dropped), by what they say: a renumbered one is
/// neither.
pub fn instructions(from: &Layout, to: &Layout) -> (Vec<Numbered>, Vec<Numbered>) {
    let has = |l: &Layout, i: &Instruction| l.instructions.values().any(|x| x == i);
    let only = |a: &Layout, b: &Layout| -> Vec<Numbered> {
        a.instructions
            .iter()
            .filter(|(_, i)| !has(b, i))
            .map(|(k, i)| (k.clone(), i.clone()))
            .collect()
    };
    (only(to, from), only(from, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_diffs_by_key_and_a_place_alone_is_a_move() {
        let block = crate::testing::seeded(3, 6);
        let a = crate::engine::lay_out(&block).unwrap().layout;
        assert!(layouts(&a, &a).is_empty());
        let mut b = a.clone();
        let (key, p) = b.placed.iter_mut().next().unwrap();
        let key = key.clone();
        p.at[0] += crate::geom::GRID;
        let gone = b.labels.keys().next().cloned();
        if let Some(net) = &gone {
            b.labels.remove(net);
        }
        let d = layouts(&a, &b);
        let moved = d.iter().find(|e| e.key.section == "placed").unwrap();
        assert_eq!(
            (moved.key.key.as_str(), moved.change),
            (key.as_str(), Change::Moved)
        );
        assert_eq!(moved.sheet.as_deref(), Some(a.placed[&key].sheet.as_str()));
        if gone.is_some() {
            assert!(
                d.iter()
                    .any(|e| e.key.section == "labels" && e.change == Change::Removed)
            );
        }
        let back = layouts(&b, &a);
        assert!(
            back.iter()
                .any(|e| e.key.section == "labels" && e.change == Change::Added)
        );
    }

    #[test]
    fn instructions_differ_by_what_they_say() {
        use crate::layout::Instruction;
        let block = crate::testing::seeded(3, 6);
        let a = crate::engine::lay_out(&block).unwrap().layout;
        let mut b = a.clone();
        let keys: Vec<String> = b.placed.keys().take(2).cloned().collect();
        b.instructions.insert(
            "i9".into(),
            Instruction::Group {
                parts: keys.clone(),
            },
        );
        let (added, dropped) = instructions(&a, &b);
        assert_eq!(added.len(), 1);
        assert!(dropped.is_empty());
        // Renumbered but saying the same: no change.
        let mut c = b.clone();
        let i = c.instructions.remove("i9").unwrap();
        c.instructions.insert("i10".into(), i);
        assert_eq!(instructions(&b, &c), (Vec::new(), Vec::new()));
    }
}
