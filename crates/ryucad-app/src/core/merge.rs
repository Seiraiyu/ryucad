//! A merge in progress, as the window knows it: the `merge.status` the
//! daemon answers when the window opens and announces (`merge.changed`)
//! whenever it changes, and the `merge.resolve` requests the conflict
//! panel sends. ryucad never stages or commits: once everything is
//! settled, `git add` and the commit are the person's.

use crate::core::model::Part;
use ryucad::error::Error;
use ryucad::proto::{Request, Resolution};
use serde_json::Value;
use std::collections::BTreeMap;

/// The merge as the window knows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merge {
    /// `merge.status`'s answer; `null` when it couldn't be made.
    pub status: Value,
    /// Why there is no status.
    pub error: Option<Error>,
    /// A resolution is on its way.
    pub resolving: bool,
    /// Why the last resolution failed.
    pub failed: Option<Error>,
}

impl Merge {
    /// A merge with its status.
    pub fn of(status: Value) -> Merge {
        Merge {
            status,
            ..Merge::default()
        }
    }

    /// A merge whose status couldn't be made.
    pub fn broken(e: Error) -> Merge {
        Merge {
            error: Some(e),
            ..Merge::default()
        }
    }

    /// What Git is in the middle of: `merge`, `cherry-pick`, `revert` or
    /// `rebase`.
    pub fn step(&self) -> &str {
        self.status["step"].as_str().unwrap_or("merge")
    }

    pub fn conflicts(&self) -> &[Value] {
        self.status["conflicts"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
    }

    /// Nothing is left to settle: the files are ready for `git add`.
    pub fn clean(&self) -> bool {
        self.error.is_none() && self.status["clean"] == true
    }

    /// Files settled in the working tree that Git still lists unmerged,
    /// or whose working copy the index doesn't hold: everything the
    /// commit hook refuses, new files included.
    pub fn unstaged(&self) -> Vec<&str> {
        self.status["unstaged"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    }

    /// New project files the index doesn't hold at all (`untracked`):
    /// the merge didn't settle them, so they aren't waiting for `git add`
    /// as its files are, but the commit still refuses them.
    pub fn untracked(&self) -> Vec<&str> {
        self.status["untracked"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    }

    /// What the merge settled that waits for `git add`: the unstaged
    /// files but the untracked ones.
    pub fn settled(&self) -> Vec<&str> {
        let untracked = self.untracked();
        self.unstaged()
            .into_iter()
            .filter(|f| !untracked.contains(f))
            .collect()
    }

    pub fn conflict(&self, id: &str) -> Option<&Value> {
        self.conflicts().iter().find(|c| c["id"] == id)
    }

    /// One line for the window's status line and title.
    pub fn summary(&self) -> String {
        let step = self.step();
        if self.error.is_some() {
            return format!("{step}: can't tell what is left");
        }
        match self.conflicts().len() {
            0 => format!("{step}: settled, ready to git add and commit"),
            1 => format!("{step}: 1 conflict"),
            n => format!("{step}: {n} conflicts"),
        }
    }
}

/// A conflict's heading in the panel: its kind and what it concerns, in
/// the model's terms: `value in base`, `pdm_broke in q number`,
/// `pdm_broke in p A`.
pub fn title(c: &Value) -> String {
    let concerns = &c["concerns"];
    let kind = c["kind"].as_str().unwrap_or("conflict");
    let part = concerns["part"].as_str();
    let what = match (
        part,
        concerns["property"].as_str().or(concerns["label"].as_str()),
    ) {
        (Some(p), Some(x)) => format!("{p} {x}"),
        _ => concerns["feature"]
            .as_str()
            .or(concerns["parameter"].as_str())
            .or(c["files"][0].as_str())
            .or(c["id"].as_str())
            .unwrap_or_default()
            .to_owned(),
    };
    format!("{kind} in {what}")
}

/// What a person does about a conflict that has no choice to take, if
/// anything needs saying: a broken feature is fixed in the model, a PDM
/// property set in the Part section.
pub fn hint(c: &Value) -> Option<&'static str> {
    match (c["kind"].as_str(), c["concerns"]["property"].is_string()) {
        (Some("merge_broke"), _) => {
            Some("Fix it in the model; the status is read again as it changes.")
        }
        (Some("pdm_broke"), true) => {
            Some("Set the property in the Part section (or with ryucad merge resolve --value).")
        }
        (Some("pdm_broke"), false) => {
            Some("Take its repair, or change the part so the proposal describes it.")
        }
        _ => None,
    }
}

/// The `merge.resolve` request settling conflict `c` by `choice` (`ours`,
/// `theirs` or `repair` with `repair`), guarded by the revisions of the
/// part it concerns as the window last saw them.
pub fn resolution(
    c: &Value,
    choice: &str,
    repair: Option<&str>,
    parts: &BTreeMap<String, Part>,
) -> Request {
    let mut expected_revisions = BTreeMap::new();
    if let Some(p) = c["concerns"]["part"].as_str()
        && let Some(part) = parts.get(p)
    {
        expected_revisions.insert(p.to_owned(), part.revision.clone());
    }
    Request::MergeResolve {
        resolutions: vec![Resolution {
            conflict: c["id"].as_str().unwrap_or_default().to_owned(),
            choice: choice.to_owned(),
            value: None,
            repair: repair.map(str::to_owned),
        }],
        expected_revisions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status() -> Value {
        json!({
            "step": "merge",
            "clean": false,
            "conflicts": [
                {"id": "parts/p/features/b.json#/params/x", "kind": "value",
                 "concerns": {"part": "p", "feature": "b"}, "choices": ["ours", "theirs", "value"]},
                {"id": "parts/p/features/s.json#merge_broke", "kind": "merge_broke",
                 "concerns": {"part": "p", "feature": "s"}, "choices": []}
            ],
            "unstaged": ["parts/p/features/c.json"]
        })
    }

    #[test]
    fn a_status_is_read() {
        let m = Merge::of(status());
        assert_eq!(m.step(), "merge");
        assert_eq!(m.conflicts().len(), 2);
        assert!(!m.clean());
        assert_eq!(m.unstaged(), ["parts/p/features/c.json"]);
        assert_eq!(m.settled(), ["parts/p/features/c.json"]);
        assert!(m.untracked().is_empty());
        assert_eq!(m.summary(), "merge: 2 conflicts");
        assert_eq!(
            m.conflict("parts/p/features/s.json#merge_broke").unwrap()["kind"],
            "merge_broke"
        );
        let clean = Merge::of(json!({"step": "cherry-pick", "clean": true, "conflicts": []}));
        assert_eq!(
            clean.summary(),
            "cherry-pick: settled, ready to git add and commit"
        );
        assert!(clean.clean());
        let e = Error::new(ryucad::error::Code::MergeUnsupported, "octopus");
        assert!(!Merge::broken(e).clean());
    }

    #[test]
    fn conflicts_are_titled_in_the_models_terms() {
        let s = status();
        let titles: Vec<String> = s["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .map(title)
            .collect();
        assert_eq!(titles, ["value in b", "merge_broke in s"]);
        let number = json!({"kind": "pdm_broke", "id": "parts/q/properties.json#pdm_broke/x",
                            "concerns": {"part": "q", "property": "number", "parts": ["p"]}});
        assert_eq!(title(&number), "pdm_broke in q number");
        assert!(hint(&number).unwrap().contains("Part section"));
        let stale = json!({"kind": "pdm_broke", "concerns": {"part": "p", "label": "A"}});
        assert_eq!(title(&stale), "pdm_broke in p A");
        assert!(hint(&stale).unwrap().contains("repair"));
        assert_eq!(hint(&s["conflicts"][0]), None);
        let file = json!({"kind": "modify_delete", "concerns": {}, "files": ["ryucad.json"]});
        assert_eq!(title(&file), "modify_delete in ryucad.json");
    }

    #[test]
    fn a_resolution_is_guarded_by_its_part() {
        let part = Part {
            id: "p".into(),
            revision: "r1".into(),
            features: Vec::new(),
            solids: Vec::new(),
            by_id: Default::default(),
            by_path: Default::default(),
            error: None,
            assembly: false,
            occurrences: Vec::new(),
            solve: None,
        };
        let parts = [("p".to_owned(), part)].into_iter().collect();
        let m = Merge::of(status());
        let r = resolution(&m.conflicts()[0], "theirs", None, &parts);
        assert_eq!(
            r,
            Request::MergeResolve {
                resolutions: vec![Resolution {
                    conflict: "parts/p/features/b.json#/params/x".into(),
                    choice: "theirs".into(),
                    value: None,
                    repair: None,
                }],
                expected_revisions: [("p".to_owned(), "r1".to_owned())].into_iter().collect(),
            }
        );
        // A part the window doesn't know isn't guarded.
        let r = resolution(&m.conflicts()[0], "ours", None, &BTreeMap::new());
        let Request::MergeResolve {
            expected_revisions, ..
        } = r
        else {
            unreachable!()
        };
        assert!(expected_revisions.is_empty());
    }
}
