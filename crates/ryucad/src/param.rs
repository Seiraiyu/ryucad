//! A part's parameters through the protocol: list them, set them. Every
//! write is checked first: the new values must evaluate, and no feature
//! that rebuilt before may stop evaluating because of them.

use crate::doc::files::{Feature, ParametersFile, expression};
use crate::doc::id::Id;
use crate::doc::project::{Part, parameters_path, write_json};
use crate::error::{Code, Error, Result};
use crate::expr::{self, Env};
use crate::server::{check_revision, part_id};
use crate::session::Session;
use serde_json::{Map, Value, json};

/// The part's parameters file as it stands, or an empty one.
fn current(part: &Part) -> Result<ParametersFile> {
    match &part.parameters {
        None => Ok(ParametersFile::default()),
        Some(Ok(f)) => Ok(f.clone()),
        Some(Err(e)) => Err(e.clone()),
    }
}

/// What reads `name`: features (from their text) and other parameters.
fn readers(part: &Part, file: &ParametersFile, name: &str) -> (Vec<Id>, Vec<String>) {
    let features = part
        .order()
        .iter()
        .filter(|f| part.uses.get(*f).is_some_and(|u| u.contains(name)))
        .cloned()
        .collect();
    let parameters = file
        .parameters
        .iter()
        .filter(|(_, e)| e.names().contains(name))
        .map(|(k, _)| k.clone())
        .collect();
    (features, parameters)
}

/// `param.list`: every parameter as written, its value or error, and what
/// reads it.
pub fn list(session: &Session, part: &str) -> Result<Value> {
    let p = session.project().part(&part_id(part)?)?;
    let file = current(p)?;
    let parameters: Vec<Value> = file
        .parameters
        .iter()
        .map(|(name, e)| {
            let mut v = json!({"name": name, "value": e.text()});
            match p.env.get(name) {
                Ok(val) => {
                    let r = val.report();
                    for (k, x) in r.as_object().expect("a report is an object") {
                        v[k] = x.clone();
                    }
                }
                Err(err) => v["error"] = json!(err),
            }
            let (features, parameters) = readers(p, &file, name);
            v["used_by"] = json!({"features": features, "parameters": parameters});
            v
        })
        .collect();
    Ok(json!({"part": p.id, "parameters": parameters, "revision": p.revision}))
}

/// Refuses parameters that would break what works now: each changed
/// parameter must evaluate, and every feature that loads now must still
/// load with `env`.
fn check(session: &Session, part: &Part, changed: &[String], env: &Env) -> Result<()> {
    for name in changed {
        env.get(name)
            .map_err(|e| e.with("pointer", format!("/parameters/{name}")))?;
    }
    let unit = session.project().manifest.default_unit;
    for fid in part.order() {
        if let Some(Ok(f)) = part.features.get(fid) {
            Feature::from_json(&f.to_json(), unit, env).map_err(|e| e.with("feature", fid))?;
        }
    }
    Ok(())
}

/// Writes the parameters file and reloads.
fn save(session: &mut Session, part: &Id, file: &ParametersFile) -> Result<()> {
    write_json(&parameters_path(session.root(), part), &file.to_json())?;
    session.reload()
}

/// `param.set`: sets or adds parameters, each an expression.
pub fn set(
    session: &mut Session,
    part: &str,
    values: Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    if values.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "give at least one parameter: NAME=EXPRESSION",
        ));
    }
    let mut file = current(p)?;
    let mut changed = Vec::new();
    for (k, v) in &values {
        let at = format!("/parameters/{k}");
        let name = expr::parameter_name(k).map_err(|e| e.with("pointer", &at))?;
        let e = expression(v).map_err(|e| e.with("pointer", &at))?;
        file.parameters.insert(name.clone(), e);
        changed.push(name);
    }
    check(session, p, &changed, &file.env())?;
    save(session, &pid, &file)?;
    list(session, part)
}

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::server::tests::{call, fresh};
    use serde_json::json;
    use std::fs;

    fn with_part() -> (tempfile::TempDir, crate::session::Session) {
        let (dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        (dir, s)
    }

    #[test]
    fn set_then_list() {
        let (dir, mut s) = with_part();
        let r = call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"wall": "3mm", "hole": "wall*2", "n": 4}}),
        )
        .unwrap();
        assert_eq!(
            r["parameters"],
            json!([
                {"name": "hole", "value": "wall * 2", "kind": "length", "m": 0.006,
                 "used_by": {"features": [], "parameters": []}},
                {"name": "n", "value": "4", "kind": "number", "n": 4.0,
                 "used_by": {"features": [], "parameters": []}},
                {"name": "wall", "value": "3 mm", "kind": "length", "m": 0.003,
                 "used_by": {"features": [], "parameters": ["hole"]}},
            ])
        );
        let text = fs::read_to_string(dir.path().join("parts/p/parameters.json")).unwrap();
        assert!(crate::doc::canonical::is_canonical(&text));
        assert!(text.contains("\"wall * 2\""));
        let list = call(&mut s, "param.list", json!({"part": "p"})).unwrap();
        assert_eq!(list["parameters"], r["parameters"]);
        assert_eq!(list["revision"], r["revision"]);
    }

    #[test]
    fn features_read_parameters_and_follow_them() {
        let (_dir, mut s) = with_part();
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "2 mm"}}),
        )
        .unwrap();
        let r = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": "r*2"}}),
        )
        .unwrap();
        assert_eq!(
            r["feature"]["params"]["radius"],
            json!({"value": "r * 2", "m": 0.004, "expr": true})
        );
        let list = call(&mut s, "param.list", json!({"part": "p"})).unwrap();
        assert_eq!(
            list["parameters"][0]["used_by"]["features"],
            json!(["ball"])
        );
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "3 mm"}}),
        )
        .unwrap();
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "ball"})).unwrap();
        assert_eq!(show["params"]["radius"]["m"], 0.006);
        // An unknown parameter is refused before anything is written.
        let e = call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "params": {"radius": "gap"}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::UnknownParameter);
    }

    #[test]
    fn set_refuses_what_would_break() {
        let (dir, mut s) = with_part();
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "2 mm"}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": "r"}}),
        )
        .unwrap();
        let before = fs::read_to_string(dir.path().join("parts/p/parameters.json")).unwrap();
        for (params, code, detail) in [
            (
                json!({"r": "-1 mm"}),
                Code::InvalidParam,
                json!({"feature": "ball"}),
            ),
            (
                json!({"r": "1 deg"}),
                Code::UnitMismatch,
                json!({"feature": "ball"}),
            ),
            (
                json!({"r": "q", "q": "r"}),
                Code::ParameterCycle,
                json!({"pointer": "/parameters/q"}),
            ),
            (
                json!({"x": "1 mm + 1"}),
                Code::UnitMismatch,
                json!({"pointer": "/parameters/x"}),
            ),
            (
                json!({"Wall": "1 mm"}),
                Code::InvalidId,
                json!({"pointer": "/parameters/Wall"}),
            ),
            (
                json!({"x": "1 +"}),
                Code::InvalidQuantity,
                json!({"pointer": "/parameters/x"}),
            ),
            (json!({}), Code::Usage, json!({})),
        ] {
            let e = call(
                &mut s,
                "param.set",
                json!({"part": "p", "parameters": params}),
            )
            .unwrap_err();
            assert_eq!(e.code, code, "{params}: {e}");
            for (k, v) in detail.as_object().unwrap() {
                assert_eq!(&e.details[k], v, "{params}");
            }
        }
        let after = fs::read_to_string(dir.path().join("parts/p/parameters.json")).unwrap();
        assert_eq!(before, after);
        let e = call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "3 mm"}, "expected_revision": "old"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
    }

    #[test]
    fn a_broken_parameters_file_is_reported() {
        let (dir, mut s) = with_part();
        fs::write(dir.path().join("parts/p/parameters.json"), "{").unwrap();
        let e = call(&mut s, "param.list", json!({"part": "p"})).unwrap_err();
        assert_eq!(e.code, Code::InvalidJson);
        let e = call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": 1}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::InvalidJson);
    }
}
