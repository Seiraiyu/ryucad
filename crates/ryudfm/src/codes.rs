//! The reader's codes as `ryu_doc`'s: a job that can't be read becomes an
//! [`Error`] with its stable [`Code`], and each fault or warning on a board
//! a finding with its own.

use crate::error::{Code, Error};
use crate::odb;
use ryu_board::model::{Location, Notice};
use serde_json::{Map, Value, json};

/// The code of a job that can't be read.
pub fn code(c: odb::Code) -> Code {
    match c {
        odb::Code::NotABoard => Code::NotABoard,
        odb::Code::Unreadable => Code::OdbUnreadable,
        odb::Code::VersionUnsupported => Code::OdbVersionUnsupported,
        odb::Code::StepAmbiguous => Code::OdbStepAmbiguous,
        odb::Code::StepNotFound => Code::OdbStepNotFound,
        odb::Code::ArchiveUnsafe => Code::ArchiveUnsafe,
    }
}

/// A job that can't be read, as an error: where in the job (`file`,
/// `line`, `column`) and, for the step codes, the steps it has.
pub fn error(e: &odb::Error) -> Error {
    let mut err = Error::new(code(e.code), e.message.clone());
    if let Some(l) = &e.location {
        err = located(err, l);
    }
    if !e.steps.is_empty() {
        err = err.with("steps", &e.steps);
    }
    err
}

fn located(mut e: Error, l: &Location) -> Error {
    e = e.with("file", &l.path);
    if let Some(line) = l.line {
        e = e.with("line", line);
    }
    if let Some(col) = l.column {
        e = e.with("column", col);
    }
    e
}

/// A notice's code as a [`Code`]; every code the reader writes is one.
pub fn notice_code(code: &str) -> Option<Code> {
    serde_json::from_value(json!(code)).ok()
}

/// Whether a notice's code is a fault (something failed) rather than a
/// warning.
pub fn is_fault(code: &str) -> bool {
    odb::fault::ALL.contains(&code)
}

/// A notice as JSON: `code`, `message`, and `about`, `file`, `line` and
/// `column` where it has them.
pub fn notice(n: &Notice) -> Value {
    let mut o = Map::new();
    o.insert("code".into(), json!(n.code));
    o.insert("message".into(), json!(n.message));
    if let Some(a) = &n.about {
        o.insert("about".into(), json!(a));
    }
    if let Some(l) = &n.location {
        o.insert("file".into(), json!(l.path));
        if let Some(line) = l.line {
            o.insert("line".into(), json!(line));
        }
        if let Some(col) = l.column {
            o.insert("column".into(), json!(col));
        }
    }
    Value::Object(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reader_code_is_a_stable_code() {
        for c in odb::Code::ALL {
            assert_eq!(json!(code(c)), json!(c.as_str()));
        }
        for c in odb::fault::ALL.iter().chain(&odb::warning::ALL) {
            let code = notice_code(c).unwrap_or_else(|| panic!("{c} isn't a Code"));
            assert_eq!(json!(code), json!(c));
        }
        assert!(is_fault("odb_layer_unreadable") && !is_fault("odb_rounded"));
        assert_eq!(notice_code("nope"), None);
    }

    #[test]
    fn errors_say_where_and_which_steps() {
        let mut e = odb::Error::new(odb::Code::StepAmbiguous, "two steps could be the board")
            .at(odb::location("matrix/matrix", Some(3), Some(1)));
        e.steps = vec!["pcb".into(), "panel".into()];
        let err = error(&e);
        assert_eq!(err.code, Code::OdbStepAmbiguous);
        assert_eq!(
            err.details,
            json!({"file": "matrix/matrix", "line": 3, "column": 1, "steps": ["pcb", "panel"]})
        );
    }
}
