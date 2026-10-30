//! The activity feed (design 199): the session's activity log newest
//! first, grouped into runs of one participant's entries, each entry its
//! method in words and the features it touched. Pure: `ui::tiles::
//! activity` draws it.

use crate::core::toast::{features_of, verb};
use serde_json::Value;

/// Who made a run of entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    Agent {
        id: String,
        name: String,
    },
    Human {
        id: String,
        name: String,
    },
    /// Changes made outside ryucad.
    External,
}

impl Who {
    fn of(e: &Value) -> Who {
        if e["kind"] == "external" {
            return Who::External;
        }
        let p = &e["participant"];
        let id = p["id"].as_str().unwrap_or_default().to_owned();
        let name = p["name"].as_str().unwrap_or("someone").to_owned();
        match p["kind"].as_str() {
            Some("human") => Who::Human { id, name },
            _ => Who::Agent { id, name },
        }
    }

    /// The participant's ID, for a filter and a colour.
    pub fn id(&self) -> Option<&str> {
        match self {
            Who::Agent { id, .. } | Who::Human { id, .. } => Some(id),
            Who::External => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Who::Agent { name, .. } | Who::Human { name, .. } => name,
            Who::External => "external",
        }
    }
}

/// One row of the feed.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub seq: u64,
    pub time_ms: u64,
    /// The method in words: `edited`, `changed on disk`.
    pub what: String,
    /// The features it touched, `<part>/<feature>`; else the files.
    pub chips: Vec<String>,
    /// Whether the chips are features (with Show) or files.
    pub features: bool,
    pub undo: bool,
    pub redo: bool,
}

/// One participant's entries in a row, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub who: Who,
    pub rows: Vec<Row>,
}

/// The feed of `entries` (oldest first, as the log keeps them), newest
/// first; `only` keeps one participant's (by ID), `external` for changes
/// from outside.
pub fn runs(entries: &[Value], only: Option<&str>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for e in entries.iter().rev() {
        let who = Who::of(e);
        let keep = match only {
            None => true,
            Some("external") => who == Who::External,
            Some(id) => who.id() == Some(id),
        };
        if !keep {
            continue;
        }
        let row = row(e);
        match out.last_mut() {
            Some(r) if r.who == who => r.rows.push(row),
            _ => out.push(Run {
                who,
                rows: vec![row],
            }),
        }
    }
    out
}

fn row(e: &Value) -> Row {
    let features: Vec<String> = features_of(e)
        .into_iter()
        .map(|(p, f)| format!("{p}/{f}"))
        .collect();
    let what = match e["kind"].as_str() {
        Some("external") => "changed on disk".to_owned(),
        Some("refs") => "moved release tags".to_owned(),
        _ => verb(e["method"].as_str().unwrap_or_default(), &e["kind"]),
    };
    let (chips, are_features) = if features.is_empty() {
        let files = e["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f["path"].as_str().map(str::to_owned))
            .collect();
        (files, false)
    } else {
        (features, true)
    };
    Row {
        seq: e["seq"].as_u64().unwrap_or_default(),
        time_ms: e["time_ms"].as_u64().unwrap_or_default(),
        what,
        chips,
        features: are_features,
        undo: crate::core::activity::can_undo(e),
        redo: crate::core::activity::can_redo(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn e(seq: u64, who: (&str, &str, &str), method: &str, kind: &str, files: &[&str]) -> Value {
        let files: Vec<Value> = files.iter().map(|p| json!({"path": p})).collect();
        json!({"seq": seq, "time_ms": seq * 1000, "kind": kind, "method": method,
               "undone_by": null, "files": files,
               "participant": {"id": who.0, "kind": who.1, "name": who.2}})
    }

    #[test]
    fn entries_group_into_runs_newest_first() {
        let agent = ("p-1", "agent", "agent");
        let reviewer = ("p-2", "human", "reviewer");
        let log = [
            e(
                1,
                reviewer,
                "assembly.move",
                "change",
                &["parts/frame/features/pin.json"],
            ),
            e(
                2,
                agent,
                "param.set",
                "change",
                &["parts/bracket/parameters.json"],
            ),
            e(
                3,
                agent,
                "feature.add",
                "change",
                &["parts/bracket/features/c1.json"],
            ),
            e(
                4,
                agent,
                "feature.edit",
                "change",
                &["parts/bracket/features/f1.json"],
            ),
            json!({"seq": 5, "time_ms": 5000, "kind": "external",
                   "files": [{"path": "parts/bracket/features/e1.json"}]}),
        ];
        let runs = runs(&log, None);
        let shape: Vec<(&str, Vec<&str>)> = runs
            .iter()
            .map(|r| {
                (
                    r.who.name(),
                    r.rows.iter().map(|x| x.what.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("external", vec!["changed on disk"]),
                ("agent", vec!["edited", "added", "set parameters"]),
                ("reviewer", vec!["moved"]),
            ]
        );
        let edited = &runs[1].rows[0];
        assert_eq!(
            (edited.chips.clone(), edited.features),
            (vec!["bracket/f1".to_owned()], true)
        );
        assert!(edited.undo && !edited.redo);
        let params = &runs[1].rows[2];
        assert_eq!(params.chips, ["parts/bracket/parameters.json"]);
        assert!(!params.features);
    }

    #[test]
    fn a_filter_keeps_one_participant() {
        let log = [
            e(1, ("p-1", "agent", "agent"), "feature.edit", "change", &[]),
            e(
                2,
                ("p-2", "human", "reviewer"),
                "feature.edit",
                "change",
                &[],
            ),
            e(3, ("p-1", "agent", "agent"), "undo", "undo", &[]),
        ];
        let only = runs(&log, Some("p-1"));
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].rows.len(), 2);
        assert_eq!(only[0].rows[0].what, "undid");
        assert!(only[0].rows[0].redo);
        assert!(runs(&log, Some("external")).is_empty());
    }
}
