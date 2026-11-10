//! The drawing lint's preferences (design 417, plan 9.C):
//! `drawing_lint.json` beside the manifest (kind `ryucad.drawing_lint`,
//! format 1), each rule's weight and severity by code. Written only when
//! it holds something; a rule it doesn't name keeps its default.
//!
//! ```json
//! {"format": 1, "kind": "ryucad.drawing_lint",
//!  "severity": {"dwg_untoleranced": "off", "dwg_view_overlap": "error"},
//!  "weights": {"dwg_text_overlap": 5}}
//! ```

use super::canonical;
use super::files::{header, invalid};
use super::project::conflicted;
use super::source::Source;
use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const DRAWING_LINT: &str = "drawing_lint.json";
pub const KIND: &str = "ryucad.drawing_lint";
pub const FORMAT: u64 = 1;

/// How a rule's findings count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Off,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Off => "off",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Severity> {
        match s {
            "off" => Some(Severity::Off),
            "warning" => Some(Severity::Warning),
            "error" => Some(Severity::Error),
            _ => None,
        }
    }
}

/// The project's lint preferences.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    pub weights: BTreeMap<String, i64>,
    pub severity: BTreeMap<String, Severity>,
}

/// Whether `s` is a rule code as the file names one: `dwg_` and a word.
pub fn code_ok(s: &str) -> bool {
    s.strip_prefix("dwg_")
        .is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
}

impl Prefs {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"format": FORMAT, "kind": KIND});
        if !self.weights.is_empty() {
            v["weights"] = json!(self.weights);
        }
        if !self.severity.is_empty() {
            v["severity"] = json!(
                self.severity
                    .iter()
                    .map(|(k, s)| (k.clone(), json!(s.as_str())))
                    .collect::<serde_json::Map<_, _>>()
            );
        }
        v
    }

    /// Reads the file; `known` says which codes are rules.
    pub fn from_json(v: &Value, known: &dyn Fn(&str) -> bool) -> Result<Prefs> {
        let mut o = header(v, KIND, FORMAT)?;
        let mut p = Prefs::default();
        if let Some(w) = o.take_opt("weights") {
            let m = w
                .as_object()
                .ok_or_else(|| invalid("/weights", "weights by rule"))?;
            for (k, x) in m {
                let at = format!("/weights/{k}");
                if !code_ok(k) || !known(k) {
                    return Err(invalid(&at, &format!("{k} is no drawing lint rule")));
                }
                let n = x
                    .as_i64()
                    .filter(|n| (0..=100).contains(n))
                    .ok_or_else(|| invalid(&at, "a weight is a whole number from 0 to 100"))?;
                p.weights.insert(k.clone(), n);
            }
        }
        if let Some(s) = o.take_opt("severity") {
            let m = s
                .as_object()
                .ok_or_else(|| invalid("/severity", "severities by rule"))?;
            for (k, x) in m {
                let at = format!("/severity/{k}");
                if !code_ok(k) || !known(k) {
                    return Err(invalid(&at, &format!("{k} is no drawing lint rule")));
                }
                let sv = x
                    .as_str()
                    .and_then(Severity::parse)
                    .ok_or_else(|| invalid(&at, "off, warning or error"))?;
                p.severity.insert(k.clone(), sv);
            }
        }
        o.finish()?;
        Ok(p)
    }
}

/// The project's lint preferences, or `None` when it has no file.
pub fn read(src: &dyn Source, known: &dyn Fn(&str) -> bool) -> Result<Option<Prefs>> {
    let Some(bytes) = src
        .read(DRAWING_LINT)
        .map_err(|e| e.with("path", DRAWING_LINT))?
    else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        Error::new(Code::InvalidJson, format!("{DRAWING_LINT} is not UTF-8"))
            .with("path", DRAWING_LINT)
    })?;
    let v = canonical::parse(text).map_err(|e| e.with("path", DRAWING_LINT))?;
    if let Some(e) = conflicted(&v, DRAWING_LINT) {
        return Err(e);
    }
    Prefs::from_json(&v, known)
        .map(Some)
        .map_err(|e| e.with("path", DRAWING_LINT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_read_and_write_themselves_and_refuse_strangers() {
        let known = |c: &str| c == "dwg_untoleranced" || c == "dwg_view_overlap";
        let v = json!({"format": 1, "kind": KIND,
            "severity": {"dwg_untoleranced": "off"}, "weights": {"dwg_view_overlap": 12}});
        let p = Prefs::from_json(&v, &known).unwrap();
        assert_eq!(p.to_json(), v);
        for (bad, pointer) in [
            (
                json!({"format": 1, "kind": KIND, "weights": {"dwg_nope": 1}}),
                "/weights/dwg_nope",
            ),
            (
                json!({"format": 1, "kind": KIND, "weights": {"dwg_untoleranced": -1}}),
                "/weights/dwg_untoleranced",
            ),
            (
                json!({"format": 1, "kind": KIND, "severity": {"dwg_untoleranced": "loud"}}),
                "/severity/dwg_untoleranced",
            ),
        ] {
            let e = Prefs::from_json(&bad, &known).unwrap_err();
            assert_eq!(e.details["pointer"], pointer);
        }
    }
}
