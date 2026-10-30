//! A part's parameters, and the project's, through the protocol: list,
//! set, delete and rename them. Every write is checked first: the new
//! values must evaluate, no feature that rebuilt before may stop
//! evaluating because of them, and nothing may be left reading a parameter
//! that is gone. Sketches whose dimensions a new value moves are
//! re-solved and written in the same change, so they stay current, and
//! so are the assemblies over them.
//!
//! The project's parameters (the root `parameters.json`) are the scope
//! around every part's: a part reads one by its plain name, and a part
//! parameter may never take a project parameter's name
//! (`parameter_shadows`), so a name means one thing everywhere.

use crate::doc::canonical;
use crate::doc::files::{Feature, ParametersFile, expression, rename_in_feature};
use crate::doc::id::Id;
use crate::doc::project::{
    Part, Project, feature_path, parameters_path, project_parameters_path, write_json,
};
use crate::error::{Code, Error, Result};
use crate::expr::{self, Env};
use crate::server::{check_revision, part_id};
use crate::session::Session;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// Which parameters a request is about: a part's, else the project's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Part,
    Project,
}

/// A request's parameters: those of a part, or the project's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Part(String),
    Project,
}

impl Target {
    /// `part` names a part's parameters; `scope: project` the project's.
    pub fn of(part: Option<String>, scope: Option<Scope>) -> Result<Target> {
        match (part, scope.unwrap_or_default()) {
            (Some(p), Scope::Part) => Ok(Target::Part(p)),
            (None, Scope::Project) => Ok(Target::Project),
            (Some(_), Scope::Project) => Err(Error::new(
                Code::Usage,
                "the project's parameters belong to no part: give a part or scope project, not both",
            )),
            (None, Scope::Part) => Err(Error::new(
                Code::Usage,
                "name a part, or give scope project for the project's parameters",
            )),
        }
    }
}

/// The project's parameters file as it stands, or an empty one.
fn current_project(project: &Project) -> Result<ParametersFile> {
    match &project.parameters {
        None => Ok(ParametersFile::default()),
        Some(Ok(f)) => Ok(f.clone()),
        Some(Err(e)) => Err(e.clone()),
    }
}

/// Every name a part's files read: its listed features' (from their
/// text) and its own parameters'.
fn names_of(part: &Part) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = part
        .order()
        .iter()
        .flat_map(|f| part.uses.get(f).into_iter().flatten().cloned())
        .collect();
    if let Some(Ok(f)) = &part.parameters {
        out.extend(f.parameters.values().flat_map(|e| e.names()));
    }
    out
}

/// The part's own parameter names.
fn own(part: &Part) -> BTreeSet<String> {
    match &part.parameters {
        Some(Ok(f)) => f.parameters.keys().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

/// `parameter_shadows` for a project parameter `name` that `parts` have
/// of their own.
fn shadowed(name: &str, parts: &[&Id]) -> Error {
    let list: Vec<String> = parts.iter().map(|p| p.to_string()).collect();
    Error::new(
        Code::ParameterShadows,
        format!(
            "{name} is a parameter of {}: a project parameter can't share a part parameter's name; rename it there first",
            list.join(", ")
        ),
    )
    .with("parameter", name)
    .with("parts", list)
}

/// A value's report merged into `v`, or its error.
fn report_into(v: &mut Value, value: Result<crate::expr::Val>) {
    match value {
        Ok(val) => {
            let r = val.report();
            for (k, x) in r.as_object().expect("a report is an object") {
                v[k] = x.clone();
            }
        }
        Err(err) => v["error"] = json!(err),
    }
}

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
            report_into(&mut v, p.env.get(name));
            let (features, parameters) = readers(p, &file, name);
            v["used_by"] = json!({"features": features, "parameters": parameters});
            v
        })
        .collect();
    let mut v = json!({"part": p.id, "parameters": parameters, "revision": p.revision});
    if !p.project_reads.is_empty() {
        v["project_parameters"] = json!(p.project_reads);
    }
    Ok(v)
}

/// `param.list` of the project's parameters: each as written, its value
/// or error, the project parameters that read it and the parts that read
/// it (directly, or through another project parameter).
pub fn list_project(session: &Session) -> Result<Value> {
    let project = session.project();
    let file = current_project(project)?;
    let parameters: Vec<Value> = file
        .parameters
        .iter()
        .map(|(name, e)| {
            let mut v = json!({"name": name, "value": e.text()});
            report_into(&mut v, project.env.get(name));
            let parts: Vec<&Id> = project
                .parts
                .values()
                .filter(|p| p.project_reads.contains(name))
                .map(|p| &p.id)
                .collect();
            let parameters: Vec<&String> = file
                .parameters
                .iter()
                .filter(|(_, e)| e.names().contains(name))
                .map(|(k, _)| k)
                .collect();
            v["used_by"] = json!({"parts": parts, "parameters": parameters});
            v
        })
        .collect();
    Ok(json!({
        "scope": "project",
        "parameters": parameters,
        "project_revision": crate::config::project_revision(session.root())?,
    }))
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
    check(
        session,
        p,
        &changed,
        &Env::within(&file.parameters, &session.project().env),
    )?;
    let before = crate::sketch::current(session, &pid)?;
    let assemblies = crate::mate::current(session, &pid)?;
    save(session, &pid, &file)?;
    let sketches = crate::sketch::keep_current(session, &pid, &before)?;
    let assemblies = crate::mate::keep_current(session, &assemblies)?;
    let mut v = list(session, part)?;
    if !sketches.is_empty() {
        v["sketches"] = json!(sketches);
    }
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
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
    if p.env.outer_has(&to) {
        return Err(expr::shadows(&to));
    }
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

/// The parts whose results `changed` project parameters may move: those
/// reading any of them, or naming any.
fn reached(project: &Project, changed: &BTreeSet<String>) -> Vec<Id> {
    project
        .parts
        .values()
        .filter(|p| {
            let names = names_of(p);
            changed
                .iter()
                .any(|n| p.project_reads.contains(n) || names.contains(n))
        })
        .map(|p| p.id.clone())
        .collect()
}

/// Writes the project's parameters file (or deletes it, empty) and every
/// file in `writes`, then reloads, keeping the sketches and assemblies of
/// `reached` parts current; the answer is `param.list`'s with what was
/// kept current.
fn save_project(
    session: &mut Session,
    file: &ParametersFile,
    writes: &[(std::path::PathBuf, Value)],
    reached: &[Id],
) -> Result<Value> {
    let mut sketches_before = Vec::new();
    let mut assemblies_before: Vec<Id> = Vec::new();
    for part in reached {
        sketches_before.push((part.clone(), crate::sketch::current(session, part)?));
        for a in crate::mate::current(session, part)? {
            if !assemblies_before.contains(&a) {
                assemblies_before.push(a);
            }
        }
    }
    for (path, v) in writes {
        write_json(path, v)?;
    }
    let path = project_parameters_path(session.root());
    if file.parameters.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| Error::io(&path, &e))?;
        }
    } else {
        write_json(&path, &file.to_json())?;
    }
    session.reload()?;
    let mut sketches = Vec::new();
    for (part, before) in &sketches_before {
        for mut kept in crate::sketch::keep_current(session, part, before)? {
            kept["part"] = json!(part);
            sketches.push(kept);
        }
    }
    let assemblies = crate::mate::keep_current(session, &assemblies_before)?;
    let mut v = list_project(session)?;
    if !sketches.is_empty() {
        v["sketches"] = json!(sketches);
    }
    if !assemblies.is_empty() {
        v["assemblies"] = json!(assemblies);
    }
    Ok(v)
}

/// `param.set` of the project's parameters: each changed one must
/// evaluate, no part may have one of that name of its own, and every
/// feature of every part that loads now must still load.
pub fn set_project(
    session: &mut Session,
    values: Map<String, Value>,
    expected: Option<&str>,
) -> Result<Value> {
    crate::config::check_project_revision(session.root(), expected)?;
    if values.is_empty() {
        return Err(Error::new(
            Code::Usage,
            "give at least one parameter: NAME=EXPRESSION",
        ));
    }
    let project = session.project();
    let mut file = current_project(project)?;
    let mut changed = BTreeSet::new();
    for (k, v) in &values {
        let at = format!("/parameters/{k}");
        let name = expr::parameter_name(k).map_err(|e| e.with("pointer", &at))?;
        let e = expression(v).map_err(|e| e.with("pointer", &at))?;
        let parts: Vec<&Id> = project
            .parts
            .values()
            .filter(|p| own(p).contains(&name))
            .map(|p| &p.id)
            .collect();
        if !parts.is_empty() {
            return Err(shadowed(&name, &parts).with("pointer", &at));
        }
        file.parameters.insert(name.clone(), e);
        changed.insert(name);
    }
    let env = file.env();
    for name in &changed {
        env.get(name)
            .map_err(|e| e.with("pointer", format!("/parameters/{name}")))?;
    }
    let reached = reached(project, &changed);
    for pid in &reached {
        let p = project.part(pid)?;
        let part_env = match &p.parameters {
            None => env.clone(),
            Some(Ok(f)) => Env::within(&f.parameters, &env),
            Some(Err(_)) => continue,
        };
        check(session, p, &[], &part_env).map_err(|e| e.with("part", pid))?;
    }
    save_project(session, &file, &[], &reached)
}

/// `param.delete` of a project parameter nothing reads: no project
/// parameter and no part. The file goes with its last parameter.
pub fn delete_project(session: &mut Session, name: &str, expected: Option<&str>) -> Result<Value> {
    crate::config::check_project_revision(session.root(), expected)?;
    let project = session.project();
    let mut file = current_project(project)?;
    if !file.parameters.contains_key(name) {
        return Err(Error::new(
            Code::NotFound,
            format!("the project has no parameter {name}"),
        )
        .with("parameter", name));
    }
    let parts: Vec<Id> = project
        .parts
        .values()
        .filter(|p| p.project_reads.contains(name))
        .map(|p| p.id.clone())
        .collect();
    let parameters: Vec<String> = file
        .parameters
        .iter()
        .filter(|(_, e)| e.names().contains(name))
        .map(|(k, _)| k.clone())
        .collect();
    if !parts.is_empty() || !parameters.is_empty() {
        let who: Vec<String> = parts
            .iter()
            .map(|p| format!("part {p}"))
            .chain(parameters.iter().map(|q| format!("parameter {q}")))
            .collect();
        return Err(Error::new(
            Code::HasDependents,
            format!("{name} is read by {}; change those first", who.join(", ")),
        )
        .with("parameter", name)
        .with("parts", parts)
        .with("parameters", parameters));
    }
    file.parameters.remove(name);
    save_project(session, &file, &[], &[])
}

/// `param.rename` of a project parameter: the project's file, and every
/// expression that reads it in every part (its parameters, its feature
/// files and its explodes), as one change.
pub fn rename_project(
    session: &mut Session,
    from: &str,
    to: &str,
    expected: Option<&str>,
) -> Result<Value> {
    crate::config::check_project_revision(session.root(), expected)?;
    let project = session.project();
    let file = current_project(project)?;
    if !file.parameters.contains_key(from) {
        return Err(Error::new(
            Code::NotFound,
            format!("the project has no parameter {from}"),
        )
        .with("parameter", from));
    }
    let to = expr::parameter_name(to)?;
    if file.parameters.contains_key(&to) {
        return Err(Error::new(
            Code::DuplicateId,
            format!("the project already has a parameter {to}"),
        )
        .with("parameter", &to));
    }
    let parts: Vec<&Id> = project
        .parts
        .values()
        .filter(|p| own(p).contains(&to))
        .map(|p| &p.id)
        .collect();
    if !parts.is_empty() {
        return Err(shadowed(&to, &parts));
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
    let root = session.root().to_path_buf();
    let mut writes = Vec::new();
    let mut reached = Vec::new();
    for p in project.parts.values() {
        if own(p).contains(from) {
            // Its own parameter of that name, not the project's.
            continue;
        }
        let before = writes.len();
        if let Some(Ok(f)) = &p.parameters
            && f.parameters.values().any(|e| e.names().contains(from))
        {
            let f = ParametersFile {
                parameters: f
                    .parameters
                    .iter()
                    .map(|(k, e)| (k.clone(), e.rename(from, &to)))
                    .collect(),
            };
            writes.push((parameters_path(&root, &p.id), f.to_json()));
        }
        // Every file that names it, listed or not, from its text.
        for (fid, names) in &p.uses {
            if !names.contains(from) {
                continue;
            }
            let path = feature_path(&root, &p.id, fid);
            let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, &e))?;
            let mut v = canonical::parse(&text)?;
            if rename_in_feature(&mut v, from, &to) {
                writes.push((path, v));
            }
        }
        for eid in p.explodes.keys() {
            let path = crate::doc::project::part_dir(&root, &p.id)
                .join(crate::doc::explode::explode_file(eid));
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut v) = canonical::parse(&text) else {
                continue;
            };
            let mut changed = false;
            for s in crate::doc::explode::expression_strings(&mut v) {
                if let Ok(e) = expr::Expr::parse(s)
                    && e.names().contains(from)
                {
                    *s = e.rename(from, &to).text().to_owned();
                    changed = true;
                }
            }
            if changed {
                writes.push((path, v));
            }
        }
        if writes.len() > before {
            reached.push(p.id.clone());
        }
    }
    save_project(session, &renamed, &writes, &reached)
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

    #[test]
    fn sketches_follow_their_parameters_in_the_same_change() {
        use crate::live::Live;
        use crate::server::tests::{call_live, with_sketch};
        let (dir, mut s) = with_sketch();
        let mut live = Live::new();
        let r = call_live(
            &mut s,
            &mut live,
            None,
            "param.set",
            json!({"part": "p", "parameters": {"width": "15 mm"}}),
        )
        .unwrap();
        assert_eq!(r["sketches"], json!([{"feature": "s", "ok": true}]));
        // Written back: current, so it rebuilds as stored.
        let rb = call(&mut s, "rebuild", json!({})).unwrap();
        assert_eq!(rb["parts"][0]["features"][0]["sketch"]["stale"], false);
        assert!(rb.get("warnings").is_none());
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let last = log["entries"].as_array().unwrap().last().unwrap().clone();
        let files: Vec<&str> = last["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            ["parts/p/features/s.json", "parts/p/parameters.json"]
        );
        // A parameter nothing reads leaves the sketch alone.
        let r = call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"spare": 1}}),
        )
        .unwrap();
        assert!(r.get("sketches").is_none());
        // One that can't be solved is reported and left as it was; the
        // parameter is still set.
        let path = dir.path().join("parts/p/features/s.json");
        let before = fs::read_to_string(&path).unwrap();
        let r = call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"width": "50 mm"}}),
        )
        .unwrap();
        assert_eq!(r["sketches"][0]["ok"], false);
        assert_eq!(r["sketches"][0]["error"]["code"], "sketch_unsolved");
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert_eq!(r["parameters"][1]["value"], "50 mm");
    }

    /// Parts `p` and `q`, a sphere each reading the project's `wall`.
    fn with_project_parameter() -> (tempfile::TempDir, crate::session::Session) {
        let (dir, mut s) = fresh();
        call(
            &mut s,
            "param.set",
            json!({"scope": "project", "parameters": {"wall": "3 mm", "gap": "wall / 2"}}),
        )
        .unwrap();
        for (part, radius) in [("p", "wall"), ("q", "gap * 2")] {
            call(&mut s, "part.create", json!({"part": part})).unwrap();
            call(
                &mut s,
                "feature.add",
                json!({"part": part, "type": "sphere", "id": "ball", "params": {"radius": radius}}),
            )
            .unwrap();
        }
        (dir, s)
    }

    fn radius(s: &mut crate::session::Session, part: &str) -> serde_json::Value {
        call(s, "feature.show", json!({"part": part, "id": "ball"})).unwrap()["params"]
            ["radius"]["m"]
            .clone()
    }

    #[test]
    fn project_parameters_are_read_by_every_part() {
        let (dir, mut s) = with_project_parameter();
        let text = fs::read_to_string(dir.path().join("parameters.json")).unwrap();
        assert!(crate::doc::canonical::is_canonical(&text));
        let list = call(&mut s, "param.list", json!({"scope": "project"})).unwrap();
        assert_eq!(list["scope"], "project");
        assert_eq!(
            list["parameters"],
            json!([
                {"name": "gap", "value": "wall / 2", "kind": "length", "m": 0.0015,
                 "used_by": {"parts": ["q"], "parameters": []}},
                {"name": "wall", "value": "3 mm", "kind": "length", "m": 0.003,
                 "used_by": {"parts": ["p", "q"], "parameters": ["gap"]}},
            ])
        );
        let rev = list["project_revision"].as_str().unwrap().to_owned();
        let q = call(&mut s, "param.list", json!({"part": "q"})).unwrap();
        assert_eq!(q["project_parameters"], json!(["gap", "wall"]));
        // A new value moves every part that reads it.
        let r = call(
            &mut s,
            "param.set",
            json!({"scope": "project", "parameters": {"wall": "4 mm"},
                   "expected_revision": rev}),
        )
        .unwrap();
        assert_eq!(r["parameters"][1]["m"], 0.004);
        assert_eq!(radius(&mut s, "p"), 0.004);
        assert_eq!(radius(&mut s, "q"), 0.004);
        // Guarded by the project's revision.
        let e = call(
            &mut s,
            "param.set",
            json!({"scope": "project", "parameters": {"wall": "5 mm"},
                   "expected_revision": rev}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        // Nothing that loads now may stop loading.
        let e = call(
            &mut s,
            "param.set",
            json!({"scope": "project", "parameters": {"wall": "-1 mm"}}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["part"], &e.details["feature"]),
            (Code::InvalidParam, &json!("p"), &json!("ball"))
        );
        assert_eq!(radius(&mut s, "p"), 0.004);
        // A name means one thing everywhere: a part can't take a project
        // parameter's name, nor the project a part's.
        let e = call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"wall": "1 mm"}}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ParameterShadows);
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"r": "1 mm"}}),
        )
        .unwrap();
        let e = call(
            &mut s,
            "param.set",
            json!({"scope": "project", "parameters": {"r": "1 mm"}}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["parts"]),
            (Code::ParameterShadows, &json!(["p"]))
        );
        let e = call(
            &mut s,
            "param.rename",
            json!({"part": "p", "from": "r", "to": "gap"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::ParameterShadows);
        // What is read can't go; what isn't can.
        let e = call(
            &mut s,
            "param.delete",
            json!({"scope": "project", "name": "gap"}),
        )
        .unwrap_err();
        assert_eq!(
            (e.code, &e.details["parts"]),
            (Code::HasDependents, &json!(["q"]))
        );
        call(
            &mut s,
            "param.set",
            json!({"scope": "project", "parameters": {"spare": 2}}),
        )
        .unwrap();
        call(
            &mut s,
            "param.delete",
            json!({"scope": "project", "name": "spare"}),
        )
        .unwrap();
        // A part or the project, never both, never neither.
        for v in [json!({"part": "p", "scope": "project"}), json!({})] {
            let e = call(&mut s, "param.list", v).unwrap_err();
            assert_eq!(e.code, Code::Usage);
        }
    }

    #[test]
    fn a_project_rename_rewrites_every_part_as_one_change() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (dir, mut s) = with_project_parameter();
        // `p` reads it through a parameter of its own too.
        call(
            &mut s,
            "param.set",
            json!({"part": "p", "parameters": {"twice": "wall * 2"}}),
        )
        .unwrap();
        let mut live = Live::new();
        let r = call_live(
            &mut s,
            &mut live,
            None,
            "param.rename",
            json!({"scope": "project", "from": "wall", "to": "skin"}),
        )
        .unwrap();
        assert_eq!(r["parameters"][0]["value"], "skin / 2");
        assert_eq!(r["parameters"][1]["name"], "skin");
        let show = call(&mut s, "feature.show", json!({"part": "p", "id": "ball"})).unwrap();
        assert_eq!(show["params"]["radius"]["value"], "skin");
        let p = call(&mut s, "param.list", json!({"part": "p"})).unwrap();
        assert_eq!(p["parameters"][0]["value"], "skin * 2");
        // `q` reads it only through `gap`: its files don't change.
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let last = log["entries"].as_array().unwrap().last().unwrap().clone();
        let files: Vec<&str> = last["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            [
                "parameters.json",
                "parts/p/features/ball.json",
                "parts/p/parameters.json"
            ]
        );
        call_live(&mut s, &mut live, None, "undo", json!({})).unwrap();
        let text = fs::read_to_string(dir.path().join("parameters.json")).unwrap();
        assert!(text.contains("\"wall\""));
        assert_eq!(radius(&mut s, "p"), 0.003);
        let e = call(
            &mut s,
            "param.rename",
            json!({"scope": "project", "from": "wall", "to": "gap"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::DuplicateId);
    }

    #[test]
    fn project_parameters_keep_every_sketch_reading_them_current() {
        use crate::server::tests::with_sketch;
        let (dir, mut s) = with_sketch();
        // The sketch's `width` becomes the project's.
        fs::remove_file(dir.path().join("parts/p/parameters.json")).unwrap();
        crate::doc::project::write_json(
            &dir.path().join("parameters.json"),
            &json!({"format": 1, "kind": "ryucad.parameters", "parameters": {"width": "20 mm"}}),
        )
        .unwrap();
        s.refresh().unwrap();
        let r = call(
            &mut s,
            "param.set",
            json!({"scope": "project", "parameters": {"width": "15 mm"}}),
        )
        .unwrap();
        assert_eq!(
            r["sketches"],
            json!([{"part": "p", "feature": "s", "ok": true}])
        );
        let rb = call(&mut s, "rebuild", json!({})).unwrap();
        assert_eq!(rb["parts"][0]["features"][0]["sketch"]["stale"], false);
    }
}
