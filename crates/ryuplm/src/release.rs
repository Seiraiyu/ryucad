//! Releases of items (design 734): ryupdm's records, states, tags and
//! checks, as ryueda's `pdm::release` drives them, for a product whose
//! parts have no results.

use crate::Plm;
use crate::catalogue::NAME;
use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::session::Session;
use crate::write::{Writes, commit};
use ryupdm::config::Config;
use ryupdm::history::Releases;
use ryupdm::product::Product;
use ryupdm::record::{BuiltWith, Record, Records, Release};
use serde_json::{Value, json};

type Repo = ryu_git::Repo<Plm>;

/// The repository and the release branch as read, kept in the session
/// while the refs don't move; `not_a_git_repo` outside one.
pub fn history(session: &mut Session) -> Result<(Repo, Releases)> {
    let repo: Repo = ryu_git::repo::repo(session.root())?;
    let config = Config::read_in(&session.snapshot(), NAME)?;
    let key = ryupdm::release::refs_key(repo.gix(), &config)?;
    if let Some((k, r)) = &session.pdm
        && *k == key
    {
        return Ok((repo, r.clone()));
    }
    let releases = Releases::read(repo.gix(), repo.prefix(), &config)?;
    session.pdm = Some((key, releases.clone()));
    Ok((repo, releases))
}

/// The release branch, or nothing official outside a repository.
pub fn releases_or_none(session: &mut Session) -> Releases {
    history(session).map(|(_, r)| r).unwrap_or_default()
}

fn check_expected(session: &Session, expected: Option<&str>) -> Result<()> {
    match expected {
        Some(e) if e != session.revision() => Err(Error::new(
            Code::Conflict,
            "the catalogue changed; read it again and retry",
        )
        .with("expected", e)
        .with("actual", session.revision())),
        _ => Ok(()),
    }
}

/// Every reason `id` can't be released: its error faults, then each
/// required property it lacks.
pub fn problems(session: &Session, config: &Config, id: &Id) -> Vec<Value> {
    let mut out: Vec<Value> = crate::judge::all(
        &session.snapshot(),
        session.catalogue(),
        &Default::default(),
    )
    .into_iter()
    .filter(|f| f.is_error() && f.error.details["item"] == json!(id))
    .map(|f| f.to_json())
    .collect();
    // As ryueda writes a missing property: `property_invalid`, `missing`.
    for name in crate::judge::missing(session.catalogue(), config, id) {
        let mut v = json!(
            Error::new(
                Code::PropertyInvalid,
                format!("item {id}: {name} is required to release")
            )
            .with("item", id)
            .with("property", &name)
            .with("missing", true)
        );
        v["severity"] = json!("error");
        out.push(v);
    }
    out
}

fn not_ready(id: &Id, problems: Vec<Value>) -> Error {
    let text: Vec<&str> = problems
        .iter()
        .filter_map(|p| p["message"].as_str())
        .collect();
    Error::new(
        Code::ReleaseNotReady,
        format!("{id} can't be released as it is: {}", text.join("; ")),
    )
    .with("item", id)
    .with("problems", &problems)
}

#[allow(clippy::too_many_arguments)]
pub fn propose(
    session: &mut Session,
    items: &[String],
    all: bool,
    dry_run: bool,
    note: Option<String>,
    me: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    check_expected(session, expected)?;
    let (repo, releases) = history(session)?;
    ryupdm::history::check_folder(repo.prefix())?;
    let mut ids: Vec<Id> = items
        .iter()
        .map(|s| session.catalogue().find(s))
        .collect::<Result<_>>()?;
    let files = session.snapshot();
    if all {
        for (id, s) in ryupdm::state::derive_all_for::<Plm>(&files, &releases) {
            if matches!(s.status.name(), "in_work" | "modified")
                && session.catalogue().items.contains_key(&id)
            {
                ids.push(id);
            }
        }
    }
    ids.sort();
    ids.dedup();
    if ids.is_empty() && !all {
        return Err(Error::new(
            Code::Usage,
            "name an item to release, or pass --all",
        ));
    }
    let alone = ids.len() == 1 && !all;
    let config = Config::read_in(&files, NAME)?;
    let records = Records::read_in(&files, NAME)?;
    let (mut ready, mut refused) = (Vec::new(), Vec::new());
    for id in &ids {
        let next = ryupdm::release::next_for::<Plm>(&files, &config, &records, &releases, id);
        let p = problems(session, &config, id);
        let outcome = match next {
            Err(e) => Err(e),
            Ok(_) if !p.is_empty() => Err(not_ready(id, p)),
            Ok(n) => Ok(n),
        };
        match outcome {
            Ok(n) => ready.push((id.clone(), n)),
            Err(e) if alone && !dry_run => return Err(e),
            Err(e) => refused.push((id.clone(), e)),
        }
    }
    if dry_run {
        let mut out: Vec<Value> = ready
            .iter()
            .map(|(id, n)| json!({"item": id, "label": n.label, "supersedes": n.supersedes, "ready": true, "problems": []}))
            .collect();
        for (id, e) in &refused {
            let problems = match e.code {
                Code::ReleaseNotReady => e.details["problems"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
                _ => vec![json!(e)],
            };
            out.push(json!({"item": id, "label": null, "supersedes": null, "ready": false, "problems": problems}));
        }
        out.sort_by(|a, b| a["item"].as_str().cmp(&b["item"].as_str()));
        return Ok(json!({"items": out}));
    }
    if ready.is_empty() && !refused.is_empty() && !all {
        let first = refused.remove(0).1;
        return Err(first.with(
            "refused",
            refused
                .iter()
                .map(|(p, e)| json!({"item": p, "error": e}))
                .collect::<Vec<_>>(),
        ));
    }
    let project = session.catalogue().manifest.project.clone();
    let v = crate::proto::VersionInfo::current();
    let by = ryupdm::release::released_by(repo.gix(), me.unwrap_or("agent"));
    let made: Vec<Record> = ready
        .into_iter()
        .map(|(id, n)| -> Result<Record> {
            let props = ryupdm::props::read_for::<Plm>(&files, &id)?;
            Ok(Record::Release(Release {
                project: project.clone(),
                content: <Plm as Product>::content(&files, &id)?,
                properties: ryupdm::record::revising(&config, &props),
                part: id,
                label: n.label,
                results: Default::default(),
                built_with: BuiltWith::of_product(NAME, &v.ryuplm, &v.commit),
                released_by: by.clone(),
                supersedes: n.supersedes,
                note: note.clone(),
            }))
        })
        .collect::<Result<_>>()?;
    if !made.is_empty() {
        commit(
            session,
            "the release",
            Writes(
                made.iter()
                    .map(|r| (r.path(), Some(crate::canonical::to_string(&r.to_json()))))
                    .collect(),
            ),
        )?;
    }
    let (_, releases) = history(session)?;
    let files = session.snapshot();
    let proposed: Vec<Value> = made
        .iter()
        .map(|r| {
            let s = ryupdm::state::derive_for::<Plm>(&files, r.part(), &releases);
            let mut v = json!({"item": r.part(), "label": r.label(), "path": r.path(), "record": r.to_json(), "state": s.to_json()});
            if let Some(a) = ryupdm::release::required(r.part(), &releases) {
                v["approval"] = a;
            }
            v
        })
        .collect();
    Ok(json!({
        "proposed": proposed,
        "refused": refused.iter().map(|(p, e)| json!({"item": p, "error": e})).collect::<Vec<_>>(),
        "revision": session.revision(),
    }))
}

pub fn obsolete(
    session: &mut Session,
    item: &str,
    label: &str,
    note: Option<String>,
    me: Option<&str>,
    expected: Option<&str>,
) -> Result<Value> {
    check_expected(session, expected)?;
    // The item may be gone: an ID names it then.
    let id = session
        .catalogue()
        .find(item)
        .or_else(|e| Id::parse(item, "item").map_err(|_| e))?;
    let (repo, releases) = history(session)?;
    ryupdm::history::check_folder(repo.prefix())?;
    let files = session.snapshot();
    let config = Config::read_in(&files, NAME)?;
    let records = Records::read_in(&files, NAME)?;
    ryupdm::release::obsolete_target(&config, &records, &releases, &id, label)?;
    let record = Record::Obsolete(ryupdm::record::Obsolete {
        project: session.catalogue().manifest.project.clone(),
        part: id.clone(),
        label: label.to_owned(),
        released_by: ryupdm::release::released_by(repo.gix(), me.unwrap_or("agent")),
        note,
    });
    commit(
        session,
        "the obsolete record",
        Writes(vec![(
            record.path(),
            Some(crate::canonical::to_string(&record.to_json())),
        )]),
    )?;
    let (_, releases) = history(session)?;
    let s = ryupdm::state::derive_for::<Plm>(&session.snapshot(), &id, &releases);
    Ok(
        json!({"path": record.path(), "record": record.to_json(), "state": s.to_json(), "revision": session.revision()}),
    )
}

pub fn list(session: &mut Session, item: Option<&str>) -> Result<Value> {
    let id = item
        .map(|s| {
            session
                .catalogue()
                .find(s)
                .or_else(|e| Id::parse(s, "item").map_err(|_| e))
        })
        .transpose()?;
    let (has_repo, releases) = match history(session) {
        Ok((_, r)) => (true, r),
        Err(e) if e.code == Code::NotAGitRepo => (false, Releases::default()),
        Err(e) => return Err(e),
    };
    let files = session.snapshot();
    let config = Config::read_in(&files, NAME)?;
    let records = Records::read_in(&files, NAME)?;
    let mine = |p: &Id| id.as_ref().is_none_or(|x| x == p);
    let shown: Vec<Value> = records
        .entries
        .iter()
        .filter(|e| mine(&e.part))
        .map(|e| ryupdm::release::listed(e, &releases, has_repo))
        .collect();
    let items: Vec<Value> = ryupdm::state::derive_all_for::<Plm>(&files, &releases)
        .into_iter()
        .filter(|(p, _)| mine(p))
        .map(|(p, s)| {
            let mut v = s.to_json();
            v["item"] = json!(p);
            v
        })
        .collect();
    Ok(json!({
        "repository": has_repo,
        "release_branch": config.release_branch,
        "records": shown,
        "items": items,
        "revision": session.revision(),
    }))
}

/// ryupdm's release findings (`source: release`).
pub fn findings(session: &mut Session) -> Result<Vec<Value>> {
    let files = session.snapshot();
    let found = match history(session) {
        Ok((repo, releases)) => {
            let h = ryupdm::check::History {
                repo: repo.gix(),
                prefix: repo.prefix(),
                releases: &releases,
            };
            ryupdm::check::run_for::<Plm>(&files, Some(h), None)?
        }
        Err(e) if e.code == Code::NotAGitRepo => ryupdm::check::run_for::<Plm>(&files, None, None)?,
        Err(e) => return Err(e),
    };
    Ok(found
        .iter()
        .map(|f| {
            let mut v = f.to_json();
            v["source"] = json!("release");
            v
        })
        .collect())
}

pub fn check(session: &mut Session) -> Result<Value> {
    let f = findings(session)?;
    let errors = f.iter().filter(|x| x["severity"] == "error").count();
    let report = json!({"schema": ryupdm::check::SCHEMA, "passed": errors == 0, "errors": errors,
        "warnings": f.len() - errors, "findings": f});
    if errors > 0 {
        return Err(Error {
            code: Code::CheckFailed,
            message: format!("{errors} release check(s) failed"),
            details: report,
        });
    }
    Ok(report)
}

pub fn sync(session: &mut Session) -> Result<Value> {
    let (repo, releases) = history(session)?;
    ryupdm::release::sync(repo.gix(), &releases, NAME)
}
