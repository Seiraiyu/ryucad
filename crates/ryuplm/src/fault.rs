//! What a check finds in a file that reads: an error, which a write
//! refuses to add, or a warning. A fault fails only what reads its object.

use crate::error::{Code, Error};
use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// One finding: its code, severity, message and details (the file's
/// `path` and a JSON `pointer` into it).
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub severity: Severity,
    pub error: Error,
}

impl Fault {
    pub fn of(severity: Severity, error: Error) -> Fault {
        Fault { severity, error }
    }

    pub fn error(code: Code, message: impl Into<String>) -> Fault {
        Fault {
            severity: Severity::Error,
            error: Error::new(code, message),
        }
    }

    pub fn warning(code: Code, message: impl Into<String>) -> Fault {
        Fault {
            severity: Severity::Warning,
            error: Error::new(code, message),
        }
    }

    /// Adds `key: value` to the details.
    pub fn with(mut self, key: &str, value: impl Serialize) -> Fault {
        self.error = self.error.with(key, value);
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    pub fn to_json(&self) -> Value {
        let mut v = Map::new();
        v.insert("code".into(), json!(self.error.code));
        v.insert("severity".into(), json!(self.severity));
        v.insert("message".into(), json!(self.error.message));
        if !self.error.details.is_null() {
            v.insert("details".into(), self.error.details.clone());
        }
        Value::Object(v)
    }

    /// [`Fault::to_json`], its message and details moved rather than
    /// copied: a check's answer holds every fault.
    pub fn into_json(self) -> Value {
        let mut v = Map::new();
        v.insert("code".into(), json!(self.error.code));
        v.insert("severity".into(), json!(self.severity));
        v.insert("message".into(), Value::String(self.error.message));
        if !self.error.details.is_null() {
            v.insert("details".into(), self.error.details);
        }
        Value::Object(v)
    }
}

/// The JSON of each fault.
pub fn list(faults: &[Fault]) -> Value {
    Value::Array(faults.iter().map(Fault::to_json).collect())
}

/// [`list`], the faults moved into it.
pub fn into_list(faults: Vec<Fault>) -> Value {
    Value::Array(faults.into_iter().map(Fault::into_json).collect())
}
