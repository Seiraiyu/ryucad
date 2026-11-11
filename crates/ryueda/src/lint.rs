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

/// A rule's code as files and requests may name it: the old
/// `sch_resistor_style` is `sym_shape`.
fn canonical(name: &str) -> &str {
    if name == crate::symlint::ALIAS.0 {
        crate::symlint::ALIAS.1
    } else {
        name
    }
}

/// Whether `name` is a lint rule's code: the sheet lint's or the symbol
/// lint's, or the old code `sym_shape` replaced.
fn rule(name: &str) -> bool {
    let name = canonical(name);
    ryusch::lint::RULES.iter().any(|r| r.code == name) || crate::symlint::rule(name).is_some()
}

/// Every rule: the sheet lint's, then the symbol lint's.
fn all_rules() -> impl Iterator<Item = &'static ryusch::lint::Rule> {
    ryusch::lint::RULES
        .iter()
        .chain(crate::symlint::RULES.iter())
}

/// `lint.json` as plan 2.D reads it: the old resistor code's weight and
/// severity are `sym_shape`'s (unless it has its own).
pub fn upgrade(prefs: &mut Prefs) {
    let (old, new) = crate::symlint::ALIAS;
    if let Some(w) = prefs.weights.remove(old) {
        prefs.weights.entry(new.to_owned()).or_insert(w);
    }
    if let Some(s) = prefs.severity.remove(old) {
        prefs.severity.entry(new.to_owned()).or_insert(s);
    }
}

/// The rules, each with its default weight and its unit.
fn rules() -> Vec<Value> {
    all_rules()
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
    merged(p, p.lint.item.clone().unwrap_or_default())
}

/// `own` (a project's `lint.json`) over the organisation's style's lint,
/// a locked weight or severity always the organisation's (plan 2.E).
fn merged(p: &Project, own: Prefs) -> Prefs {
    let Some(org) = p
        .org_style
        .as_ref()
        .and_then(|o| o.style.item.as_ref().ok())
    else {
        return own;
    };
    let mut out = own.clone();
    for (code, w) in &org.lint.weights {
        let locked = org
            .locks
            .iter()
            .any(|l| *l == format!("lint.weights.{code}"));
        if locked || !own.weights.contains_key(code) {
            out.weights.insert(code.clone(), *w);
        }
    }
    for (code, s) in &org.lint.severity {
        let locked = org
            .locks
            .iter()
            .any(|l| *l == format!("lint.severity.{code}"));
        if locked || !own.severity.contains_key(code) {
            out.severity.insert(code.clone(), *s);
        }
    }
    out
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
        let Some(rule) = all_rules().find(|r| r.code == canonical(code)) else {
            return Err(
                Error::new(Code::NotFound, format!("{code} isn't a lint rule"))
                    .with("param", "explain")
                    .with("codes", all_rules().map(|r| r.code).collect::<Vec<_>>()),
            );
        };
        let prefs = prefs(p);
        let weight = if crate::symlint::rule(rule.code).is_some() {
            crate::symlint::weight(&prefs, rule.code).0
        } else {
            prefs.weight(rule.code).0
        };
        let (severity, _) = prefs.severity(rule.code);
        let mut v = json!({
            "rule": rule,
            "weight_now": weight,
            "severity_now": severity.word(),
            "revision": p.revision,
        });
        if rule.code != code {
            v["alias_of"] = json!(rule.code);
        }
        return Ok(v);
    }
    Ok(json!({
        "prefs": prefs(p),
        "own": p.lint.item.clone().unwrap_or_default(),
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
    dry_run: bool,
) -> Result<Value> {
    let p = session.project();
    crate::server::check_revision("the project", &p.revision, expected)?;
    p.lint.item.as_ref().map_err(|e| e.clone())?;
    // The project's own file: the organisation's layer is applied when
    // judging, never written into it.
    let mut prefs = p.lint.item.clone().unwrap_or_default();
    let layers = crate::cascade::Layers::of(p);
    let locked = |lock: String| -> Result<()> {
        if layers.locked(&lock) {
            return Err(Error::new(
                Code::StyleLocked,
                format!("the organisation's style locks {lock}"),
            )
            .with("lock", &lock)
            .with("path", layers.org_path().unwrap_or_default()));
        }
        Ok(())
    };
    for name in weights.keys() {
        locked(format!("lint.weights.{}", canonical(name)))?;
    }
    for name in severity.keys() {
        locked(format!("lint.severity.{}", canonical(name)))?;
    }
    let unknown = |name: &str, what: &str| {
        Error::new(Code::InvalidParam, format!("{name} isn't a lint rule"))
            .with("param", format!("{what}/{name}"))
            .with("rules", all_rules().map(|r| r.code).collect::<Vec<_>>())
    };
    for (name, v) in weights {
        let name = &canonical(name).to_owned();
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
        let name = &canonical(name).to_owned();
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
    if dry_run {
        let p = session.project();
        return Ok(tried(p, &merged(p, prefs)));
    }
    save(session, &prefs)?;
    get(session.project(), None)
}

/// What `prefs` would change, written nowhere: every laid-out block's
/// sheets linted under the project's preferences and under `prefs`, each
/// sheet's score and penalty before and after, and the findings that come
/// and go (matched by code, sheet and what they name), in the agent's
/// names.
fn tried(p: &Project, prefs: &Prefs) -> Value {
    let now = self::prefs(p);
    let mut blocks = Vec::new();
    for b in p.conn.blocks.values() {
        if b.layout.is_none() {
            continue;
        }
        let Ok(laid) = crate::schem::lay(p, b, None, &[], ryusch::input::Relayout::None) else {
            continue;
        };
        let names = crate::vocab::Names {
            p,
            b,
            built: &laid.built,
        };
        let (f0, s0) = ryusch::lint::lint_with(&laid.built.input, &laid.outcome, &now);
        let (f1, s1) = ryusch::lint::lint_with(&laid.built.input, &laid.outcome, prefs);
        let same = |a: &ryusch::lint::Finding, b: &ryusch::lint::Finding| {
            a.code == b.code && a.sheet == b.sheet && a.refs == b.refs
        };
        let shown = |f: &ryusch::lint::Finding| {
            let mut v = json!(f);
            names.fixes_in(&mut v);
            v
        };
        let mut sheets = Vec::new();
        for (before, after) in s0.iter().zip(&s1) {
            let came: Vec<Value> = f1
                .iter()
                .filter(|f| f.sheet == after.sheet && !f0.iter().any(|g| same(f, g)))
                .map(shown)
                .collect();
            let went: Vec<Value> = f0
                .iter()
                .filter(|f| f.sheet == before.sheet && !f1.iter().any(|g| same(f, g)))
                .map(shown)
                .collect();
            let name = laid
                .outcome
                .layout
                .sheets
                .get(&after.sheet)
                .map_or_else(|| after.sheet.clone(), |s| s.name.clone());
            sheets.push(json!({
                "sheet": after.sheet,
                "name": name,
                "before": {"score": before.score, "penalty": before.penalty},
                "after": {"score": after.score, "penalty": after.penalty},
                "came": came,
                "went": went,
            }));
        }
        blocks.push(json!({"block": b.id, "sheets": sheets}));
    }
    let key = |f: &Fault| {
        (
            f.error.code,
            f.error.details.get("symbol").cloned(),
            f.error.details.get("pointer").cloned(),
        )
    };
    let (s0, s1) = (
        crate::symlint::faults(p, &now),
        crate::symlint::faults(p, prefs),
    );
    let came: Vec<Fault> = s1
        .iter()
        .filter(|f| !s0.iter().any(|g| key(g) == key(f)))
        .cloned()
        .collect();
    let went: Vec<Fault> = s0
        .iter()
        .filter(|f| !s1.iter().any(|g| key(g) == key(f)))
        .cloned()
        .collect();
    json!({
        "dry_run": true,
        "prefs": prefs,
        "blocks": blocks,
        "symbols": {"came": crate::fault::list(&came), "went": crate::fault::list(&went)},
        "revision": p.revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_resistor_code_reads_as_sym_shape() {
        let mut p = Prefs::default();
        p.weights.insert("sch_resistor_style".into(), 4);
        p.severity
            .insert("sch_resistor_style".into(), Severity::Error);
        upgrade(&mut p);
        assert_eq!(p.weights.get("sym_shape"), Some(&4));
        assert_eq!(p.severity.get("sym_shape"), Some(&Severity::Error));
        assert!(!p.weights.contains_key("sch_resistor_style"));
        assert!(check(&p).is_empty());
        // Symbol rules are rules.
        p.weights.insert("sym_pin_side".into(), 7);
        assert!(check(&p).is_empty());
    }

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
