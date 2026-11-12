//! Requests on the project and its parts: info, create, delete, list, rebuild, the format check.

use crate::doc::assembly::Rev;
use crate::doc::files::{Feature, PartFile, PartKind};
use crate::doc::id::Id;
use crate::doc::project::{MANIFEST, feature_path, parameters_path, part_dir, part_path};
use crate::error::{Code, Error, Result};
use crate::part::{check_revision, order, part_id, part_kind, part_report};
use crate::proto::{Answer, Raw, VersionInfo};
use crate::session::Session;
use ryu_session::job::Phase;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub(crate) fn project_info(session: &Session) -> Value {
    let p = session.project();
    json!({
        "root": p.root,
        "default_unit": p.manifest.default_unit,
        "parts": p.parts.keys().collect::<Vec<_>>(),
        "version": VersionInfo::current(),
    })
}

pub(crate) fn part_create(session: &mut Session, part: &str, kind: Option<&str>) -> Result<Value> {
    let id = part_id(part)?;
    let kind = kind
        .map(PartKind::parse)
        .transpose()?
        .unwrap_or(PartKind::Part);
    if session.project().parts.contains_key(&id) {
        return Err(
            Error::new(Code::DuplicateId, format!("part {id} already exists")).with("part", &id),
        );
    }
    crate::pdm::check_not_retired(session.root(), &id)?;
    session.write_json(
        &part_path(session.root(), &id),
        &PartFile {
            kind,
            ..PartFile::default()
        }
        .to_json(),
    )?;
    session.reload()?;
    let revision = &session.project().part(&id)?.revision;
    Ok(json!({"part": id, "kind": kind.as_str(), "revision": revision}))
}

pub(crate) fn part_delete(
    session: &mut Session,
    part: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let id = part_id(part)?;
    check_revision(session.project().part(&id)?, expected)?;
    // An assembly placing the part as it is would have nothing to place;
    // a release it places is read from history.
    let placed = &id;
    let users: Vec<String> = session
        .project()
        .parts
        .iter()
        .flat_map(|(a, p)| {
            p.order()
                .iter()
                .filter_map(move |fid| match p.features.get(fid) {
                    Some(Ok(Feature::Instance(i)))
                        if i.part == *placed && i.rev == Rev::Working =>
                    {
                        Some(format!("{a}/{fid}"))
                    }
                    _ => None,
                })
        })
        .collect();
    if !users.is_empty() {
        return Err(Error::new(
            Code::HasDependents,
            format!("assemblies place {id}: {}", users.join(", ")),
        )
        .with("part", &id)
        .with("instances", users));
    }
    let dir = part_dir(session.root(), &id);
    let mine = format!("parts/{id}/");
    let files: Vec<String> = session
        .snapshot()
        .keys()
        .filter(|p| p.starts_with(&mine))
        .cloned()
        .collect();
    for rel in files {
        let at = rel
            .split('/')
            .fold(session.root().to_path_buf(), |p, s| p.join(s));
        session.remove_file(&at)?;
    }
    if dir.exists() {
        // no session: what the files leave on disk (empty folders, stray files) goes too
        fs::remove_dir_all(&dir).map_err(|e| Error::io(&dir, &e))?;
    }
    // A daemon sees its own removal whatever file events say (#263).
    crate::doc::source::note_written(&dir);
    session.reload()?;
    Ok(json!({"part": id, "deleted": true}))
}

pub(crate) fn part_list(session: &Session) -> Value {
    let parts: Vec<Value> = session
        .project()
        .parts
        .values()
        .map(|p| {
            let kind = part_kind(p);
            let mut v = json!({"part": p.id, "kind": kind.as_str(),
                               "features": p.order().len(), "revision": p.revision});
            if let Err(e) = &p.file {
                v["error"] = json!(e);
            }
            // What its rebuild was, where this session built it at its
            // revision: a window lists 10,000 parts without asking each.
            if let Some((hash, failed)) = session.summary(&p.id) {
                v["hash"] = json!(hash);
                v["failed"] = json!(failed);
            }
            v
        })
        .collect();
    // What a window shows when it isn't told: the project's one root
    // assembly, else every part that isn't an assembly.
    let model = crate::assembly::shown_parts(session.project());
    json!({ "parts": parts, "model": model })
}

/// An object's JSON text with `extra`, a member already written as text,
/// in its sorted place: the bytes the object would give holding it.
fn object_text(v: &Value, extra: Option<(&str, &str)>) -> String {
    let Some((key, raw)) = extra else {
        return v.to_string();
    };
    let mut text = String::from("{");
    let mut extra = Some(raw);
    let mut first = true;
    let mut member = |text: &mut String, k: &str, raw: &str| {
        if !std::mem::take(&mut first) {
            text.push(',');
        }
        text.push_str(&json!(k).to_string());
        text.push(':');
        text.push_str(raw);
    };
    for (k, value) in v.as_object().into_iter().flatten() {
        if k.as_str() > key
            && let Some(raw) = extra.take()
        {
            member(&mut text, key, raw);
        }
        if k != key {
            member(&mut text, k, &value.to_string());
        }
    }
    if let Some(raw) = extra {
        member(&mut text, key, raw);
    }
    text.push('}');
    text
}

/// A rebuild's report, `{commit?, parts, placed?, warnings?}`, written as
/// JSON text part by part: a whole project's (89 MB for the 10,000-part
/// benchmark) held as a `Value` tree took about 18 times that in the
/// daemon (#533). The text is the bytes the `Value` would give. A report
/// with failures is `rebuild_failed`, the report its details.
pub(crate) fn rebuild(
    session: &mut Session,
    part: Option<&str>,
    commit: Option<&str>,
) -> Result<Answer> {
    let ids: Vec<Id> = match part {
        Some(p) => vec![part_id(p)?],
        None => session.project().parts.keys().cloned().collect(),
    };
    // Members in their sorted order, as a `Value`'s map writes them.
    let mut text = String::from("{");
    if let Some(c) = commit {
        text.push_str(r#""commit":"#);
        text.push_str(&json!(c).to_string());
        text.push(',');
    }
    text.push_str(r#""parts":["#);
    let mut warnings = Vec::new();
    let mut failed = 0;
    let job = session.job.clone();
    let stage = job.begin(Phase::Rebuild, "parts", ids.len());
    for (i, id) in ids.into_iter().enumerate() {
        stage.step(|| id.to_string())?;
        // A part whose list can't be read can't be rebuilt.
        order(session.project().part(&id)?)?;
        let (mut report, occurrences) = part_report(session, &id, true)?;
        failed += report["failed"].as_u64().unwrap_or(0) as usize;
        if let Some(Value::Array(w)) = report.as_object_mut().and_then(|o| o.remove("warnings")) {
            warnings.extend(w);
        }
        if i > 0 {
            text.push(',');
        }
        let occurrences = occurrences.as_deref().map(|o| ("occurrences", o));
        text.push_str(&object_text(&report, occurrences));
    }
    text.push(']');
    // One assembly's rebuild takes in what fails in every assembly it
    // places, at any depth (issue #158), as `placed`: each such assembly
    // with its failing features. The whole project's lists them already.
    if let Some(top) = part {
        let top = part_id(top)?;
        let mut placed = Vec::new();
        for a in crate::assembly::placed_assemblies(session.project(), &top) {
            let p = session.project().part(&a)?.clone();
            let r = match session.rebuilt(&a) {
                Ok(r) => r,
                Err(e) if e.code == Code::Cancelled => return Err(e),
                Err(_) => continue,
            };
            let features: Vec<Value> = r
                .features
                .iter()
                .filter_map(|(fid, res)| {
                    let e = res.as_ref().err()?;
                    let ty = p.features.get(fid)?.as_ref().ok().map(Feature::type_name);
                    Some(json!({"id": fid, "type": ty, "ok": false, "error": e}))
                })
                .collect();
            if !features.is_empty() {
                failed += features.len();
                placed.push(json!({"part": a, "revision": p.revision,
                                   "failed": features.len(), "features": features}));
            }
        }
        if !placed.is_empty() {
            text.push_str(r#","placed":"#);
            text.push_str(&json!(placed).to_string());
        }
    }
    if !warnings.is_empty() {
        text.push_str(r#","warnings":"#);
        text.push_str(&json!(warnings).to_string());
    }
    text.push('}');
    if failed > 0 {
        // Every feature's status still reaches the agent, in the details.
        return Ok(Answer::raw(Raw::Error {
            code: Code::RebuildFailed,
            message: format!("{failed} feature(s) failed to rebuild"),
            details: text,
        }));
    }
    Ok(Answer::raw(Raw::Result(text)))
}

pub(crate) fn check_format(session: &Session) -> Result<Value> {
    let p = session.project();
    let root = &p.root;
    let rel = |path: &Path| crate::doc::project::relative(root, path);
    let mut non_canonical = Vec::new();
    let mut errors = Vec::new();
    let mut orphans = Vec::new();
    let mut check = |path: &Path| {
        if let Ok(text) = fs::read_to_string(path)
            && crate::doc::canonical::parse(&text).is_ok()
            && !crate::doc::canonical::is_canonical(&text)
        {
            non_canonical.push(rel(path));
        }
    };
    check(&root.join(MANIFEST));
    check(&root.join(crate::doc::project::PDM_CONFIG));
    for part in p.parts.values() {
        check(&part_path(root, &part.id));
        check(&crate::doc::project::properties_path(root, &part.id));
        if let Err(e) = &part.file {
            errors.push(json!(e));
        }
        match &part.parameters {
            Some(Ok(_)) => check(&parameters_path(root, &part.id)),
            Some(Err(e)) => errors.push(json!(e)),
            None => {}
        }
        for (fid, f) in &part.features {
            check(&feature_path(root, &part.id, fid));
            if let Err(e) = f {
                errors.push(json!(e));
            }
        }
        orphans.extend(
            part.orphans()
                .into_iter()
                .map(|f| rel(&feature_path(root, &part.id, f))),
        );
    }
    let clean =
        non_canonical.is_empty() && errors.is_empty() && orphans.is_empty() && p.unknown.is_empty();
    Ok(
        json!({"clean": clean, "non_canonical": non_canonical, "errors": errors,
              "orphans": orphans, "unknown": p.unknown}),
    )
}
