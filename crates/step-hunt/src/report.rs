//! A run's records (`cases.jsonl`, one line per case as it finishes) and
//! the report made of them (`report.json`, `report.md`), judged against
//! the ledger.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ledger::{Judged, Ledger, State};
use crate::split::Case;
use crate::stages::Ran;

/// The report's schema; a field added keeps it, one changed or removed
/// moves it.
pub const SCHEMA: u32 = 1;

/// One case as it ran: a line of `cases.jsonl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub case: Case,
    #[serde(flatten)]
    pub ran: Ran,
    pub findings: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub key_dependent: bool,
}

/// Reads `cases.jsonl`, the last record of a case winning. A line cut
/// short (a run stopped while writing it) is skipped.
pub fn read_records(text: &str) -> BTreeMap<String, Record> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Record>(l).ok())
        .map(|r| (r.case.id.clone(), r))
        .collect()
}

fn judged(j: &Judged) -> String {
    match j {
        Judged::Issue(n) => format!("#{n}"),
        Judged::File(w) => format!("file: {w}"),
        Judged::Limit(w) => format!("limit: {w}"),
    }
}

/// The report: every case with its findings' states, sorted by case,
/// then the groups by signature.
pub fn report(
    ryucad: &Value,
    records: &BTreeMap<String, Record>,
    ledger: &Ledger,
    partial: bool,
) -> Value {
    let mut cases = Vec::new();
    let mut groups: BTreeMap<String, Vec<(String, State)>> = BTreeMap::new();
    let mut fixed = Vec::new();
    for (id, r) in records {
        let findings: Vec<Value> = r
            .findings
            .iter()
            .map(|sig| {
                let state = ledger.state(id, sig);
                groups
                    .entry(sig.clone())
                    .or_default()
                    .push((id.clone(), state.clone()));
                json!({"signature": sig, "state": state})
            })
            .collect();
        // A case that timed out never ran its later stages: what they
        // found before isn't fixed.
        let timed_out = r
            .ran
            .stages
            .iter()
            .any(|s| s.outcome == crate::stages::Outcome::Timeout);
        for (sig, j) in ledger.fixed(id, &r.findings) {
            if !timed_out {
                fixed.push(json!({"case": id, "signature": sig, "timed": crate::classify::is_timed(&sig), "was": j}));
            }
        }
        let mut v = serde_json::to_value(r).expect("a record serializes");
        v["findings"] = Value::Array(findings);
        cases.push(v);
    }
    let mut groups: Vec<(String, Vec<(String, State)>)> = groups.into_iter().collect();
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|(sig, members)| {
            let representative = members
                .iter()
                .map(|(id, _)| &records[id])
                .min_by(|a, b| a.case.faces.cmp(&b.case.faces).then_with(|| a.case.id.cmp(&b.case.id)))
                .expect("a group has members")
                .case
                .id
                .clone();
            let new = members.iter().filter(|(_, s)| *s == State::New).count();
            json!({"signature": sig, "timed": crate::classify::is_timed(&sig), "cases": members.len(), "new": new, "representative": representative,
                   "members": members.iter().map(|(id, s)| json!({"case": id, "state": s})).collect::<Vec<_>>()})
        })
        .collect();
    let failed = records
        .values()
        .filter(|r| {
            r.ran
                .stages
                .iter()
                .any(|s| s.outcome != crate::stages::Outcome::Ok)
        })
        .count();
    json!({
        "schema": SCHEMA,
        "ryucad": ryucad,
        "partial": partial,
        "cases": records.len(),
        "failed": failed,
        "groups": groups,
        "fixed": fixed,
        "records": cases,
    })
}

/// The report as Markdown: counts, then each group (most cases first)
/// with its representative, then what was fixed.
pub fn markdown(report: &Value, records: &BTreeMap<String, Record>) -> String {
    let mut out = String::new();
    let r = &report["ryucad"];
    let _ = writeln!(out, "# step-hunt report\n");
    let _ = writeln!(
        out,
        "ryucad {} ({}){}: {} cases, {} failed.\n",
        r["ryucad"].as_str().unwrap_or("?"),
        r["commit"].as_str().unwrap_or("?"),
        if report["partial"] == true {
            ", **partial**"
        } else {
            ""
        },
        report["cases"],
        report["failed"],
    );
    for g in report["groups"].as_array().into_iter().flatten() {
        let rep = &records[g["representative"].as_str().unwrap_or_default()];
        let _ = writeln!(
            out,
            "## `{}`\n",
            g["signature"].as_str().unwrap_or_default()
        );
        let _ = writeln!(
            out,
            "{} cases, {} new. Representative `{}` ({} faces):\n",
            g["cases"], g["new"], rep.case.id, rep.case.faces
        );
        for s in &rep.ran.stages {
            let _ = writeln!(
                out,
                "- {}: {:.1} s, {}",
                s.stage,
                s.seconds,
                serde_json::to_value(&s.outcome).expect("serializes")["outcome"]
                    .as_str()
                    .unwrap_or("?")
            );
        }
        let _ = writeln!(out);
        for m in g["members"].as_array().into_iter().flatten() {
            let id = m["case"].as_str().unwrap_or_default();
            let state = match serde_json::from_value::<State>(m["state"].clone()) {
                Ok(State::New) => "new".to_owned(),
                Ok(State::Known { judged: j }) => judged(&j),
                Ok(State::Fixed { judged: j }) => format!("fixed ({})", judged(&j)),
                Err(_) => "?".to_owned(),
            };
            let o = &records[id].case.origins;
            let first = o
                .first()
                .map(|o| {
                    format!(
                        "{} {} #{} {}",
                        o.source,
                        o.file,
                        o.representation,
                        o.product.as_deref().unwrap_or("")
                    )
                })
                .unwrap_or_default();
            let more = if o.len() > 1 {
                format!(" and {} more", o.len() - 1)
            } else {
                String::new()
            };
            let _ = writeln!(out, "- `{id}` {state}: {}{more}", first.trim_end());
        }
        let _ = writeln!(out);
    }
    let fixed = report["fixed"].as_array().cloned().unwrap_or_default();
    if !fixed.is_empty() {
        let _ = writeln!(out, "## Fixed\n");
        for f in fixed {
            let was = serde_json::from_value::<Judged>(f["was"].clone())
                .map(|j| judged(&j))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "- `{}` `{}`: {was}; close or verify it",
                f["case"].as_str().unwrap_or_default(),
                f["signature"].as_str().unwrap_or_default()
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::{Kind, Origin};
    use crate::stages::{Outcome, Stage};

    fn record(id: &str, faces: usize, findings: &[&str]) -> Record {
        Record {
            case: Case {
                id: id.into(),
                kind: Kind::Body,
                path: "p".into(),
                faces,
                surfaces: vec![],
                unsplit: false,
                origins: vec![Origin {
                    source: "s".into(),
                    file: "f.stp".into(),
                    representation: 7,
                    product: Some("Pin".into()),
                }],
            },
            ran: Ran {
                stages: vec![Stage {
                    stage: "import".into(),
                    seconds: 1.5,
                    outcome: if findings.is_empty() {
                        Outcome::Ok
                    } else {
                        Outcome::Timeout
                    },
                }],
                key: None,
            },
            findings: findings.iter().map(|s| (*s).to_owned()).collect(),
            key_dependent: false,
        }
    }

    #[test]
    fn records_read_back_and_skip_a_cut_line() {
        let r = record("aaaa", 3, &["import/timeout/faces<50/-"]);
        let text = format!(
            "{}\n{}",
            serde_json::to_string(&r).unwrap(),
            "{\"case\": {\"id\""
        );
        let back = read_records(&text);
        assert_eq!(back.len(), 1);
        assert_eq!(back["aaaa"], r);
    }

    #[test]
    fn groups_sort_by_size_and_judge_against_the_ledger() {
        let records: BTreeMap<String, Record> = [
            record("aaaa", 9, &["x/one"]),
            record("bbbb", 3, &["x/one"]),
            record("cccc", 1, &["x/two"]),
            record("dddd", 1, &[]),
        ]
        .into_iter()
        .map(|r| (r.case.id.clone(), r))
        .collect();
        let ledger = Ledger::parse("case aaaa #5 x/one\ncase dddd #6 x/three\n").unwrap();
        let v = report(
            &json!({"ryucad": "0.7.2", "commit": "abc"}),
            &records,
            &ledger,
            false,
        );
        assert_eq!(v["failed"], 3);
        assert_eq!(v["groups"][0]["signature"], "x/one");
        assert_eq!(v["groups"][0]["representative"], "bbbb");
        assert_eq!(v["groups"][0]["new"], 1);
        assert_eq!(v["fixed"][0]["case"], "dddd");
        let md = markdown(&v, &records);
        assert!(md.contains("## `x/one`"), "{md}");
        assert!(md.contains("- `aaaa` #5: s f.stp #7 Pin"), "{md}");
        assert!(md.contains("- `bbbb` new"), "{md}");
        assert!(md.contains("## Fixed"), "{md}");
    }
}
