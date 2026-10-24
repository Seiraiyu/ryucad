//! The Releases tab: release records with where each became official and
//! its tag, the release checks' findings, and what a person does there,
//! each confirmed first: propose a part's next revision (a dry run shows
//! the label it takes and every reason it can't yet), make its latest
//! official revision obsolete, and sync the local tags. Each is the
//! request an agent sends: `release.list`, `release.check`,
//! `release.propose`, `release.obsolete` and `release.sync`.

use ryucad::error::Error;
use serde_json::Value;

/// A write the tab asks a person to confirm first.
#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    /// Make `part`'s latest official revision `label` obsolete.
    Obsolete { part: String, label: String },
    /// Create and move the local tags.
    Sync,
}

/// What the tab knows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReleaseTab {
    /// The write waiting for a person to confirm it (the Propose dialog is
    /// `proposal`).
    pub confirm: Option<Confirm>,
    /// The note typed into the open dialog.
    pub note: String,
    /// `release.list` for the whole project, once read.
    pub list: Option<Value>,
    /// The release checks' findings, once read.
    pub findings: Vec<Value>,
    /// The dry run of a proposal, while the Propose dialog is open: its
    /// part and `release.propose`'s answer.
    pub proposal: Option<(String, Value)>,
    /// A write is on its way.
    pub busy: bool,
    /// What the last write did, for people.
    pub done: Option<String>,
    /// Why the last request failed.
    pub error: Option<Error>,
}

/// A short commit for people.
fn short(c: &str) -> &str {
    &c[..c.len().min(10)]
}

impl ReleaseTab {
    /// `part`'s records, in label order as listed.
    pub fn records(&self, part: &str) -> Vec<&Value> {
        self.list
            .as_ref()
            .and_then(|l| l["records"].as_array())
            .into_iter()
            .flatten()
            .filter(|r| r["part"] == part)
            .collect()
    }

    /// The label `release.obsolete` would take for `part`: its latest
    /// official release, unless that is obsolete already or a record is
    /// still proposed.
    pub fn obsoletable(&self, part: &str) -> Option<String> {
        let records = self.records(part);
        if records.iter().any(|r| r["status"] != "official") {
            return None;
        }
        let latest = records
            .iter()
            .rev()
            .find(|r| r["kind"] == "release")?
            .get("label")?
            .as_str()?;
        let gone = records
            .iter()
            .any(|r| r["kind"] == "obsolete" && r["label"] == latest);
        (!gone).then(|| latest.to_owned())
    }

    /// The tags `release.sync` would write, over the whole project: each
    /// tag with `create` or `move`.
    pub fn to_sync(&self) -> Vec<(String, &'static str)> {
        self.list
            .as_ref()
            .and_then(|l| l["records"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let tag = &r["tag"];
                let what = match tag["status"].as_str()? {
                    "missing" => "create",
                    "moved" => "move",
                    _ => return None,
                };
                Some((tag["name"].as_str()?.to_owned(), what))
            })
            .collect()
    }

    /// The findings about `part`.
    pub fn findings_of(&self, part: &str) -> Vec<&Value> {
        self.findings.iter().filter(|f| f["part"] == part).collect()
    }

    /// Whether the project is in a Git repository, as the list says.
    pub fn in_repository(&self) -> bool {
        self.list.as_ref().is_some_and(|l| l["repository"] == true)
    }
}

/// A note as a request takes it: none when nothing was typed.
pub fn note(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// One record as the tab lists it: `A release, official at 0123456789,
/// tag ok: first (Dana)`, or `B release, proposed`.
pub fn record_line(r: &Value) -> String {
    let mut s = format!(
        "{} {}",
        r["label"].as_str().unwrap_or("?"),
        r["kind"].as_str().unwrap_or("record")
    );
    match r["status"].as_str() {
        Some("official") => {
            s.push_str(&format!(
                ", official at {}",
                short(r["commit"].as_str().unwrap_or_default())
            ));
            if let Some(t) = r["tag"]["status"].as_str() {
                s.push_str(&format!(", tag {t}"));
            }
        }
        Some(other) => s.push_str(&format!(", {other}")),
        None => {}
    }
    if let Some(e) = r["error"]["message"].as_str() {
        s.push_str(&format!(": can't be read ({e})"));
    }
    if let Some(n) = r["note"].as_str() {
        s.push_str(&format!(": {n}"));
    }
    let by = &r["released_by"];
    if let Some(who) = by["name"].as_str().or(by["participant"].as_str()) {
        s.push_str(&format!(" ({who})"));
    }
    s
}

/// What a `release.sync` answer did, for people.
pub fn synced(v: &Value) -> String {
    let n = |k: &str| v[k].as_array().map_or(0, Vec::len);
    let mut s = format!("tags: {} created, {} moved", n("created"), n("moved"));
    if n("orphaned") > 0 {
        s.push_str(&format!(", {} with no release (left alone)", n("orphaned")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list() -> Value {
        json!({"repository": true, "records": [
            {"part": "p", "label": "A", "kind": "release", "status": "official",
             "commit": "0123456789abcdef", "tag": {"name": "ryupdm/p/A", "status": "missing"},
             "note": "first", "released_by": {"name": "Dana", "participant": "Dana (window 1)"}},
            {"part": "p", "label": "B", "kind": "release", "status": "official",
             "commit": "fedcba9876543210",
             "tag": {"name": "ryupdm/p/B", "status": "moved", "points_at": "aaaa"},
             "released_by": {"participant": "agent"}},
            {"part": "q", "label": "A", "kind": "release", "status": "proposed",
             "released_by": {"participant": "agent"}},
        ]})
    }

    #[test]
    fn records_are_listed_per_part_with_their_status() {
        let t = ReleaseTab {
            list: Some(list()),
            ..ReleaseTab::default()
        };
        let lines: Vec<String> = t.records("p").into_iter().map(record_line).collect();
        assert_eq!(
            lines,
            [
                "A release, official at 0123456789, tag missing: first (Dana)",
                "B release, official at fedcba9876, tag moved (agent)"
            ]
        );
        assert_eq!(
            record_line(t.records("q")[0]),
            "A release, proposed (agent)"
        );
        assert!(t.in_repository());
    }

    #[test]
    fn only_the_latest_official_release_goes_obsolete() {
        let mut t = ReleaseTab {
            list: Some(list()),
            ..ReleaseTab::default()
        };
        assert_eq!(t.obsoletable("p").as_deref(), Some("B"));
        // A proposal pending: nothing to make obsolete yet.
        assert_eq!(t.obsoletable("q"), None);
        t.list.as_mut().unwrap()["records"]
            .as_array_mut()
            .unwrap()
            .push(json!({"part": "p", "label": "B", "kind": "obsolete", "status": "official"}));
        assert_eq!(t.obsoletable("p"), None);
    }

    #[test]
    fn sync_would_create_and_move_tags() {
        let t = ReleaseTab {
            list: Some(list()),
            ..ReleaseTab::default()
        };
        assert_eq!(
            t.to_sync(),
            [
                ("ryupdm/p/A".to_owned(), "create"),
                ("ryupdm/p/B".to_owned(), "move")
            ]
        );
        assert_eq!(
            synced(&json!({"created": [{}], "moved": [], "orphaned": ["ryupdm/x/A"]})),
            "tags: 1 created, 0 moved, 1 with no release (left alone)"
        );
    }
}
