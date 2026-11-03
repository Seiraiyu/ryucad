//! The schematic lint's preferences, project data in `lint.json` (kind
//! `ryueda.lint`) beside the manifest: each rule's weight and severity, and
//! the styles the project prefers (`ryusch::lint::Prefs`). `lint.configure`
//! writes it and deletes it when it holds nothing; describe, check and the
//! layout commands lint under it, and each finding a preference drove says
//! which.

use crate::error::{Code, Error, Result};
use crate::fault::Fault;
use crate::project::Project;
use ryusch::lint::{LocalNets, Prefs, ResistorStyle, Severity};
use serde_json::{Map, Value, json};

pub const KIND: &str = "ryueda.lint";
pub const FORMAT: u64 = 1;

/// Whether `name` is a lint rule's code.
fn rule(name: &str) -> bool {
    ryusch::lint::RULES.iter().any(|r| r.code == name)
}

/// The rules, each with its default weight and its unit.
fn rules() -> Vec<Value> {
    ryusch::lint::RULES
        .iter()
        .map(|r| {
            json!({
                "code": r.code,
                "weight": r.weight,
                "per": r.unit,
                "what": r.what,
                "why": r.why,
                "fix": r.fix,
                "evidence": r.evidence,
            })
        })
        .collect()
}

/// Faults of `lint.json` as read: weights and severities name rules, and
/// weights aren't negative.
pub fn check(prefs: &Prefs) -> Vec<Fault> {
    let mut out = Vec::new();
    for (name, w) in &prefs.weights {
        if !rule(name) {
            out.push(
                Fault::error(Code::InvalidDocument, format!("{name} isn't a lint rule"))
                    .with("pointer", format!("/weights/{name}")),
            );
        } else if *w < 0 {
            out.push(
                Fault::error(
                    Code::InvalidDocument,
                    format!("{name}'s weight is {w}; a weight is 0 or more"),
                )
                .with("pointer", format!("/weights/{name}")),
            );
        }
    }
    for name in prefs.severity.keys() {
        if !rule(name) {
            out.push(
                Fault::error(Code::InvalidDocument, format!("{name} isn't a lint rule"))
                    .with("pointer", format!("/severity/{name}")),
            );
        }
    }
    out
}

/// The project's preferences; the defaults when `lint.json` doesn't read
/// (a fault of the project).
pub fn prefs(p: &Project) -> Prefs {
    p.lint.item.clone().unwrap_or_default()
}

/// Writes `prefs` as `lint.json`, or deletes it when it holds nothing.
fn save(session: &mut crate::session::Session, prefs: &Prefs) -> Result<()> {
    let mut w = crate::write::Writes::default();
    if prefs == &Prefs::default() {
        w.delete(crate::project::LINT.to_owned());
    } else {
        w.0.push((
            crate::project::LINT.to_owned(),
            Some(crate::doc::to_string(prefs, KIND, FORMAT)),
        ));
    }
    crate::write::commit(session, "lint.json", w)
}

/// `lint.get`: the project's preferences, the rules (default weights,
/// what each finds, why, how to fix it, its evidence), and the project's
/// revision; with `explain`, one rule and the project's weight and
/// severity for it.
pub fn get(p: &Project, explain: Option<&str>) -> Result<Value> {
    p.lint.item.as_ref().map_err(|e| e.clone())?;
    if let Some(code) = explain {
        let Some(rule) = ryusch::lint::RULES.iter().find(|r| r.code == code) else {
            return Err(
                Error::new(Code::NotFound, format!("{code} isn't a lint rule"))
                    .with("param", "explain")
                    .with(
                        "codes",
                        ryusch::lint::RULES
                            .iter()
                            .map(|r| r.code)
                            .collect::<Vec<_>>(),
                    ),
            );
        };
        let prefs = prefs(p);
        let (weight, _) = prefs.weight(code);
        let (severity, _) = prefs.severity(code);
        return Ok(json!({
            "rule": rule,
            "weight_now": weight,
            "severity_now": severity.word(),
            "revision": p.revision,
        }));
    }
    Ok(json!({
        "prefs": prefs(p),
        "rules": rules(),
        "revision": p.revision,
    }))
}

/// `lint.configure`: weights by rule (a number, or `null` for the
/// default), severities by rule (`error`, `warning`, `off`, or `null`), and
/// preferred styles (`resistor`: `us` or `iec`; `local_nets`: `wires` or
/// `labels`; `null` for the default).
pub fn configure(
    session: &mut crate::session::Session,
    weights: &Map<String, Value>,
    severity: &Map<String, Value>,
    prefer: &Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let p = session.project();
    crate::server::check_revision("the project", &p.revision, expected)?;
    p.lint.item.as_ref().map_err(|e| e.clone())?;
    let mut prefs = prefs(p);
    let unknown = |name: &str, what: &str| {
        Error::new(Code::InvalidParam, format!("{name} isn't a lint rule"))
            .with("param", format!("{what}/{name}"))
            .with(
                "rules",
                ryusch::lint::RULES
                    .iter()
                    .map(|r| r.code)
                    .collect::<Vec<_>>(),
            )
    };
    for (name, v) in weights {
        if !rule(name) {
            return Err(unknown(name, "weights"));
        }
        match v {
            Value::Null => {
                prefs.weights.remove(name);
            }
            v => {
                let w = v.as_i64().filter(|w| *w >= 0).ok_or_else(|| {
                    Error::new(
                        Code::InvalidParam,
                        format!("{name}'s weight is a whole number, 0 or more, not {v}"),
                    )
                    .with("param", format!("weights/{name}"))
                })?;
                prefs.weights.insert(name.clone(), w);
            }
        }
    }
    for (name, v) in severity {
        if !rule(name) {
            return Err(unknown(name, "severity"));
        }
        match v {
            Value::Null => {
                prefs.severity.remove(name);
            }
            v => {
                let s: Severity = serde_json::from_value(v.clone()).map_err(|_| {
                    Error::new(
                        Code::InvalidParam,
                        format!("{name}'s severity is error, warning or off, not {v}"),
                    )
                    .with("param", format!("severity/{name}"))
                })?;
                prefs.severity.insert(name.clone(), s);
            }
        }
    }
    for (name, v) in prefer {
        let bad = || {
            Error::new(
                Code::InvalidParam,
                format!(
                    "prefer {name} = {v}: resistor is us or iec, local_nets is wires or labels"
                ),
            )
            .with("param", format!("prefer/{name}"))
        };
        match name.as_str() {
            "resistor" => {
                prefs.prefer.resistor = match v {
                    Value::Null => ResistorStyle::default(),
                    v => serde_json::from_value(v.clone()).map_err(|_| bad())?,
                }
            }
            "local_nets" => {
                prefs.prefer.local_nets = match v {
                    Value::Null => LocalNets::default(),
                    v => serde_json::from_value(v.clone()).map_err(|_| bad())?,
                }
            }
            _ => return Err(bad()),
        }
    }
    save(session, &prefs)?;
    get(session.project(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weight_or_severity_names_a_rule() {
        let mut p = Prefs::default();
        p.weights.insert("sch_jog".into(), 5);
        p.severity.insert("sch_floating".into(), Severity::Off);
        assert!(check(&p).is_empty());
        p.weights.insert("sch_nonsense".into(), 1);
        p.weights.insert("sch_bends".into(), -1);
        p.severity.insert("erc_thing".into(), Severity::Error);
        let pointers: Vec<String> = check(&p)
            .iter()
            .map(|f| f.error.details["pointer"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            pointers,
            [
                "/weights/sch_bends",
                "/weights/sch_nonsense",
                "/severity/erc_thing"
            ]
        );
    }
}
