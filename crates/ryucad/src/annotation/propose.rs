//! Proposed changes (design 374): a request a form, label, drag or handle
//! would have sent, run against the files as its proposer sees them (the
//! working files with the proposer's own open changes laid over them, its
//! draft) in a capture session, so nothing reaches the folder. What it
//! would have written is kept in the annotation; `annotation:<id>` names
//! the files with it laid over; `annotation.apply` writes it.

use super::{Annotation, AuthorKind, By, Change, chain, hash, is_in, lay, read_all, readable};
use crate::activity::Files;
use crate::doc::id::Id;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The proposer's open annotations whose changes aren't in `files` yet:
/// each after every one its `after` names, else by when it was made, then
/// by ID.
pub fn mine<'a>(all: &'a BTreeMap<Id, Annotation>, by: &By, files: &Files) -> Vec<&'a Annotation> {
    let mut left: Vec<&Annotation> = all
        .values()
        .filter(|a| a.by == *by && a.status.is_open())
        .filter(|a| a.change.as_ref().is_some_and(|c| !is_in(files, c)))
        .collect();
    left.sort_by(|a, b| (&a.at, &a.id).cmp(&(&b.at, &b.id)));
    let mut out: Vec<&Annotation> = Vec::new();
    while !left.is_empty() {
        let placed: BTreeSet<&Id> = out.iter().map(|a| &a.id).collect();
        let waiting: BTreeSet<&Id> = left.iter().map(|a| &a.id).collect();
        // The first whose changes it stands on are laid already; a cycle
        // (files edited by hand) is broken by taking the first left.
        let i = left
            .iter()
            .position(|a| {
                a.change
                    .as_ref()
                    .expect("filtered")
                    .after
                    .iter()
                    .all(|x| placed.contains(x) || !waiting.contains(x))
            })
            .unwrap_or(0);
        out.push(left.remove(i));
    }
    out
}

/// `by`'s draft: the working files with `by`'s open changes laid over
/// them in order.
pub fn draft(session: &Session, by: &By) -> Files {
    let working = session.snapshot();
    let all = readable(&read_all(&working));
    let mut files = working.clone();
    for a in mine(&all, by, &working) {
        lay(&mut files, a.change.as_ref().expect("a change"));
    }
    files
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::annotation::tests::{comment, plate, put};
    use serde_json::json;

    /// `a` carrying a change that writes `value` to `rel`, over `base`.
    pub(crate) fn changing(
        id: &str,
        by: &By,
        at: &str,
        rel: &str,
        base: Option<&[u8]>,
        value: Value,
        after: Vec<&str>,
    ) -> Annotation {
        let mut a = comment(id, vec![]);
        a.by = by.clone();
        a.at = at.into();
        a.change = Some(Change {
            request: json!({"method": "feature.edit", "params": {}}),
            rough: false,
            base: [(rel.to_owned(), base.map(crate::annotation::hash))].into(),
            writes: [(rel.to_owned(), Some(value))].into(),
            after: after
                .into_iter()
                .map(|s| crate::doc::annotation::check_id(s).unwrap())
                .collect(),
        });
        a
    }

    #[test]
    fn a_draft_lays_the_proposers_open_changes_in_order() {
        let (_dir, mut s, _) = plate();
        let dave = By {
            participant: "dave (window 1)".into(),
            kind: AuthorKind::Human,
        };
        let rel = "parts/plate/features/slab.json";
        let now = s.snapshot()[rel].clone();
        let mut v: Value = serde_json::from_slice(&now).unwrap();
        v["params"]["x"] = json!("50 mm");
        let first = changing(
            "aaaaaaaaaa",
            &dave,
            "2026-11-05T14:00:00Z",
            rel,
            Some(&now),
            v.clone(),
            vec![],
        );
        let one = crate::annotation::bytes_of(&Some(v.clone())).unwrap();
        v["params"]["y"] = json!("30 mm");
        // Made in the same second, but laid after the change it stands on.
        let second = changing(
            "bbbbbbbbbb",
            &dave,
            "2026-11-05T14:00:00Z",
            rel,
            Some(&one),
            v.clone(),
            vec!["aaaaaaaaaa"],
        );
        let theirs = changing(
            "cccccccccc",
            &By::agent("agent"),
            "2026-11-05T13:00:00Z",
            rel,
            Some(&now),
            json!({}),
            vec![],
        );
        put(&mut s, &second);
        put(&mut s, &first);
        put(&mut s, &theirs);
        let all = crate::annotation::readable(&crate::annotation::read_all(&s.snapshot()));
        let order: Vec<&str> = mine(&all, &dave, &s.snapshot())
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(order, ["aaaaaaaaaa", "bbbbbbbbbb"]);
        let d = draft(&s, &dave);
        assert_eq!(
            d[rel].to_vec(),
            crate::annotation::bytes_of(&Some(v)).unwrap()
        );
        // Another participant's draft is the working files.
        assert_eq!(draft(&s, &By::agent("other"))[rel], now);
    }
}
