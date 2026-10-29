//! `check`: the project (or one board, or a job at a path) loaded from
//! nothing, every load fault reported as a `schema: 1` finding
//! `{code, severity, source, board, message, details}`: a board that can't
//! be read is an error (`source: project` for its file and input, `odb`
//! for the reader's refusal), each fault the reader recorded an error and
//! each warning a warning. Any error fails it with `check_failed`, the
//! report in its details. Stage 2's rules join these findings.

use crate::codes;
use crate::error::{Code, Error, Result};
use crate::session::Session;
use ryu_board::model::{Board, Notice};
use serde_json::{Map, Value, json};

/// The findings' schema: adding a field or a code isn't a new one.
pub const SCHEMA: u64 = 1;

fn of_notice(n: &Notice, board: Option<&str>) -> Value {
    let mut details = Map::new();
    let v = codes::notice(n);
    for k in ["about", "file", "line", "column"] {
        if let Some(x) = v.get(k) {
            details.insert(k.into(), x.clone());
        }
    }
    json!({
        "code": n.code,
        "severity": if codes::is_fault(&n.code) { "error" } else { "warning" },
        "source": "odb",
        "board": board,
        "message": n.message,
        "details": details,
    })
}

/// A board's faults and warnings as findings.
pub fn board(b: &Board, id: Option<&str>) -> Vec<Value> {
    b.faults
        .iter()
        .chain(&b.warnings)
        .map(|n| of_notice(n, id))
        .collect()
}

/// A board that couldn't be read, as a finding.
fn refused(e: &Error, id: Option<&str>) -> Value {
    let project = matches!(
        e.code,
        Code::BoardInputMissing
            | Code::BoardInputChanged
            | Code::InvalidJson
            | Code::InvalidDocument
            | Code::FormatTooNew
            | Code::Io
    );
    json!({
        "code": e.code,
        "severity": "error",
        "source": if project { "project" } else { "odb" },
        "board": id,
        "message": e.message,
        "details": e.details,
    })
}

/// The report, or `check_failed` holding it when any finding is an error.
fn report(mut v: Value, findings: Vec<Value>) -> Result<Value> {
    let errors = findings.iter().filter(|f| f["severity"] == "error").count();
    v["schema"] = json!(SCHEMA);
    v["errors"] = json!(errors);
    v["warnings"] = json!(findings.len() - errors);
    v["findings"] = json!(findings);
    if errors > 0 {
        return Err(Error::new(
            Code::CheckFailed,
            format!("{errors} error(s); see details.report"),
        )
        .with("report", v));
    }
    Ok(v)
}

/// The project at `session`'s root (or its board `only`), loaded from
/// nothing: not from the session's boards, which it leaves as they are.
pub fn project(session: &Session, only: Option<&str>) -> Result<Value> {
    let mut fresh = Session::open(session.root())?;
    let ids: Vec<String> = match only {
        Some(b) => vec![fresh.project().board(b)?.id.to_string()],
        None => fresh
            .project()
            .boards
            .keys()
            .map(|k| k.to_string())
            .collect(),
    };
    let mut findings = Vec::new();
    for id in &ids {
        match fresh.board(id) {
            Ok(b) => findings.extend(board(&b, Some(id))),
            Err(e) => findings.push(refused(&e, Some(id))),
        }
    }
    report(json!({"boards": ids}), findings)
}

/// The job at `path`, read in this process.
pub fn path(path: &std::path::Path, step: Option<&str>) -> Result<Value> {
    let options = crate::odb::ReadOptions {
        step: step.map(str::to_owned),
        ..Default::default()
    };
    let findings = match crate::odb::read_path(path, &options) {
        Ok(b) => board(&b, None),
        Err(e) => vec![refused(&codes::error(&e), None)],
    };
    report(json!({"path": path}), findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_job_passes_and_a_broken_one_fails() {
        let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
        let v = path(&corpus.join("minimal-clean.tgz"), None).unwrap();
        assert_eq!(
            (v["schema"].clone(), v["errors"].clone()),
            (json!(1), json!(0))
        );
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.tgz");
        std::fs::write(&bad, b"not a job").unwrap();
        let e = path(&bad, None).unwrap_err();
        assert_eq!(e.code, Code::CheckFailed);
        let f = &e.details["report"]["findings"][0];
        assert_eq!(
            (
                f["code"].clone(),
                f["severity"].clone(),
                f["source"].clone()
            ),
            (json!("not_a_board"), json!("error"), json!("odb"))
        );
    }
}
