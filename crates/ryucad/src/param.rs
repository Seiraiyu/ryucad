//! A part's parameters through the protocol: list, set, delete and rename
//! them. Every write is checked first: the new values must evaluate, no
//! feature that rebuilt before may stop evaluating because of them, and
//! nothing may be left reading a parameter that is gone.

use crate::doc::canonical;
use crate::doc::files::{Feature, ParametersFile, expression, rename_in_feature};
use crate::doc::id::Id;
use crate::doc::project::{Part, feature_path, parameters_path, write_json};
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

/// The parameter `name`, or `not_found`.
fn existing(part: &Part, file: &ParametersFile, name: &str) -> Result<()> {
    if file.parameters.contains_key(name) {
        return Ok(());
    }
    Err(Error::new(
        Code::NotFound,
        format!("no parameter {name} in part {}", part.id),
    )
    .with("part", &part.id)
    .with("parameter", name))
}

/// `param.delete`: removes a parameter that nothing reads. The file goes
/// with its last parameter.
pub fn delete(
    session: &mut Session,
    part: &str,
    name: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let mut file = current(p)?;
    existing(p, &file, name)?;
    let (features, parameters) = readers(p, &file, name);
    if !features.is_empty() || !parameters.is_empty() {
        let who: Vec<String> = features
            .iter()
            .map(|f| format!("feature {f}"))
            .chain(parameters.iter().map(|q| format!("parameter {q}")))
            .collect();
        return Err(Error::new(
            Code::HasDependents,
            format!("{name} is read by {}; change those first", who.join(", ")),
        )
        .with("parameter", name)
        .with("features", features)
        .with("parameters", parameters));
    }
    file.parameters.remove(name);
    if file.parameters.is_empty() {
        let path = parameters_path(session.root(), &pid);
        std::fs::remove_file(&path).map_err(|e| Error::io(&path, &e))?;
        session.reload()?;
    } else {
        save(session, &pid, &file)?;
    }
    list(session, part)
}

/// `param.rename`: renames a parameter, and every expression that reads
/// it, in the parameters and the feature files, as one change. Names of
/// faces and edges never depend on parameters, so nothing else changes.
pub fn rename(
    session: &mut Session,
    part: &str,
    from: &str,
    to: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let pid = part_id(part)?;
    let p = session.project().part(&pid)?;
    check_revision(p, expected)?;
    let file = current(p)?;
    existing(p, &file, from)?;
    let to = expr::parameter_name(to)?;
    if file.parameters.contains_key(&to) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("part {pid} already has a parameter {to}"),
        )
        .with("parameter", &to));
    }
    let renamed = ParametersFile {
        parameters: file
            .parameters
            .iter()
            .map(|(k, e)| {
                let k = if k == from { to.clone() } else { k.clone() };
                (k, e.rename(from, &to))
            })
            .collect(),
    };
    // Every file that names it, listed or not, is rewritten from its text.
    let mut writes = Vec::new();
    for (fid, names) in &p.uses {
        if !names.contains(from) {
            continue;
        }
        let path = feature_path(session.root(), &pid, fid);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
        let mut v = canonical::parse(&text)?;
        if rename_in_feature(&mut v, from, &to) {
            writes.push((path, v));
        }
    }
    for (path, v) in &writes {
        write_json(path, v)?;
    }
    save(session, &pid, &renamed)?;
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
    fn delete_only_what_nothing_reads() {
        let (dir, mut s) = with_part();
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "2 mm", "d": "r * 2", "spare": 1}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": "d"}}),
        )
        .unwrap();
        let e = call(&mut s, "param.delete", json!({"part": "p", "name": "r"})).unwrap_err();
        assert_eq!(e.code, Code::HasDependents);
        assert_eq!(e.details["parameters"], json!(["d"]));
        let e = call(&mut s, "param.delete", json!({"part": "p", "name": "d"})).unwrap_err();
        assert_eq!(e.details["features"], json!(["ball"]));
        let e = call(&mut s, "param.delete", json!({"part": "p", "name": "x"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let r = call(
            &mut s,
            "param.delete",
            json!({"part": "p", "name": "spare"}),
        )
        .unwrap();
        assert_eq!(r["parameters"].as_array().unwrap().len(), 2);
        // With the last parameter, the file goes.
        call(&mut s, "feature.delete", json!({"part": "p", "id": "ball"})).unwrap();
        call(&mut s, "param.delete", json!({"part": "p", "name": "d"})).unwrap();
        call(&mut s, "param.delete", json!({"part": "p", "name": "r"})).unwrap();
        assert!(!dir.path().join("parts/p/parameters.json").exists());
        let r = call(&mut s, "param.list", json!({"part": "p"})).unwrap();
        assert_eq!(r["parameters"], json!([]));
    }

    #[test]
    fn rename_rewrites_every_reader() {
        let (dir, mut s) = with_part();
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "2 mm", "d": "r*2"}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": "r+d"},
                   "placement": {"origin": ["r", 0, 0]}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "plain", "params": {"radius": 1}}),
        )
        .unwrap();
        let plain = fs::read_to_string(dir.path().join("parts/p/features/plain.json")).unwrap();
        let r = call(
            &mut s,
            "param.rename",
            json!({"part": "p", "from": "r", "to": "radius"}),
        )
        .unwrap();
        assert_eq!(r["parameters"][0]["value"], "radius * 2");
        assert_eq!(r["parameters"][1]["name"], "radius");
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "ball"})).unwrap();
        assert_eq!(show["params"]["radius"]["value"], "radius + d");
        assert_eq!(show["params"]["radius"]["m"], 0.006);
        assert_eq!(show["placement"]["origin"][0]["value"], "radius");
        let after = fs::read_to_string(dir.path().join("parts/p/features/plain.json")).unwrap();
        assert_eq!(plain, after);
        for (to, code) in [("d", Code::DuplicateId), ("Bad", Code::InvalidId)] {
            let e = call(
                &mut s,
                "param.rename",
                json!({"part": "p", "from": "radius", "to": to}),
            )
            .unwrap_err();
            assert_eq!(e.code, code, "{to}");
        }
        let e = call(
            &mut s,
            "param.rename",
            json!({"part": "p", "from": "r", "to": "q"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
    }

    #[test]
    fn a_rename_is_one_change_and_undoes_as_one() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (_dir, mut s) = with_part();
        let mut live = Live::new();
        let mut go = |s: &mut crate::session::Session, method: &str, params| {
            call_live(s, &mut live, None, method, params).unwrap()
        };
        go(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "2 mm"}}),
        );
        go(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "sphere", "id": "ball", "params": {"radius": "r"}}),
        );
        go(
            &mut s,
            "param.rename",
            json!({"part": "p", "from": "r", "to": "q"}),
        );
        let log = go(&mut s, "activity.log", json!({}));
        let last = log["entries"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["method"], "param.rename");
        let files: Vec<&str> = last["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            ["parts/p/features/ball.json", "parts/p/parameters.json"]
        );
        go(&mut s, "undo", json!({}));
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "ball"})).unwrap();
        assert_eq!(show["params"]["radius"]["value"], "r");
        let list = call(&mut s, "param.list", json!({"part": "p"})).unwrap();
        assert_eq!(list["parameters"][0]["name"], "r");
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
