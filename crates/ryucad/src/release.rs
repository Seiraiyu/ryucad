//! ryupdm's releases through the protocol: proposing a part's next
//! revision and making one obsolete. Each writes one record under
//! `releases/` as a logged, undoable change, and never commits: a record
//! becomes official when the release branch holds it.

use crate::doc::id::Id;
use crate::doc::project::write_json;
use crate::error::{Code, Error, Result};
use crate::proto::VersionInfo;
use crate::server::{check_revision, part_id};
use crate::session::Session;
use ryupdm::config::Config;
use ryupdm::history::Releases;
use ryupdm::props::Pdm;
use ryupdm::record::{BuiltWith, Kind, Obsolete, Record, Records, Release, ReleasedBy};
use ryupdm::state::{self, Status};
use serde_json::{Value, json};

/// The project's repository and its release branch as read, or
/// `not_a_git_repo`.
pub(crate) fn history(session: &Session) -> Result<(crate::git::Repo, Releases)> {
    let repo = crate::git::repo(session.root())?;
    let config = Config::read(&session.snapshot())?;
    let releases = Releases::read(repo.gix(), repo.prefix(), &config)?;
    Ok((repo, releases))
}

/// Who is writing a record: Git's author identity, and the participant.
fn released_by(repo: &crate::git::Repo, participant: &str) -> ReleasedBy {
    let author = repo.gix().author().and_then(|a| a.ok());
    let text = |s: &[u8]| {
        let s = String::from_utf8_lossy(s).trim().to_owned();
        (!s.is_empty()).then_some(s)
    };
    ReleasedBy {
        name: author.as_ref().and_then(|a| text(a.name)),
        email: author.as_ref().and_then(|a| text(a.email)),
        participant: participant.to_owned(),
    }
}

/// Why `part` can't be released as it is: every load, rebuild and
/// property fault, a missing number and missing required properties.
fn readiness(session: &mut Session, part: &Id, config: &Config) -> Result<Vec<Value>> {
    let mut problems = Vec::new();
    let p = session.project().part(part)?.clone();
    if let Err(e) = &p.file {
        problems.push(json!(e));
    }
    if let Some(Err(e)) = &p.parameters {
        problems.push(json!(e));
    }
    for (id, f) in &p.features {
        if let Err(e) = f
            && p.order().contains(id)
        {
            problems.push(json!(e));
        }
    }
    if problems.is_empty() {
        for (fid, r) in &session.rebuilt(part)?.features {
            if let Err(e) = r {
                let mut v = json!(e);
                v["feature"] = json!(fid);
                problems.push(v);
            }
        }
    }
    let pdm = Pdm::read(&session.snapshot(), session.project());
    let pp = &pdm.parts[part];
    match &pp.file {
        Err(e) => problems.push(json!(e)),
        Ok(props) => {
            problems.extend(pp.faults.iter().map(|e| json!(e)));
            for (name, decl) in config.decls() {
                if decl.required && !props.values.contains_key(&name) {
                    problems.push(json!(
                        Error::new(
                            Code::PropertyInvalid,
                            format!("part {part}: {name} is required to release"),
                        )
                        .with("part", part)
                        .with("property", &name)
                        .with("missing", true)
                    ));
                }
            }
        }
    }
    Ok(problems)
}

/// Writes `record` (giving the project its ID first if it has none), and
/// answers with it and the part's state.
fn write(session: &mut Session, record: &Record) -> Result<Value> {
    let path = record.path();
    let at = path
        .split('/')
        .fold(session.root().to_path_buf(), |p, s| p.join(s));
    write_json(&at, &record.to_json())?;
    session.reload()?;
    let (_, releases) = history(session)?;
    let s = state::derive(&session.snapshot(), record.part(), &releases);
    Ok(json!({"path": path, "record": record.to_json(), "state": s.to_json()}))
}

/// The part's records, in order, refusing those this build can't read.
fn ordered<'a>(
    config: &Config,
    records: &'a Records,
    part: &'a Id,
) -> Result<Vec<(&'a ryupdm::record::Entry, &'a Record)>> {
    for e in records.of(part) {
        if let Err(err) = &e.record {
            return Err(err.clone());
        }
    }
    state::ordered(config.revisions, records, part)
}

/// Refuses while any record of `part` is still proposed.
fn check_pending(
    releases: &Releases,
    list: &[(&ryupdm::record::Entry, &Record)],
    part: &Id,
) -> Result<()> {
    if let Some((e, _)) = list
        .iter()
        .find(|(e, _)| !releases.official.contains_key(&e.path))
    {
        return Err(Error::new(
            Code::ReleasePending,
            format!(
                "{part} already has a proposed record, {}, that isn't official yet: merge it to the release branch, or delete it to withdraw it",
                e.path
            ),
        )
        .with("part", part)
        .with("path", &e.path)
        .with("label", &e.label));
    }
    Ok(())
}

/// `release.propose`: writes the record of `part`'s next revision.
pub fn propose(
    session: &mut Session,
    participant: &str,
    part: &str,
    note: Option<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let id = part_id(part)?;
    let (repo, releases) = history(session)?;
    check_revision(session.project().part(&id)?, expected)?;
    let files = session.snapshot();
    let config = Config::read(&files)?;
    let records = Records::read(&files)?;
    let list = ordered(&config, &records, &id)?;
    check_pending(&releases, &list, &id)?;
    let problems = readiness(session, &id, &config)?;
    if !problems.is_empty() {
        return Err(Error::new(
            Code::ReleaseNotReady,
            format!(
                "{id} can't be released as it is: {}",
                problems
                    .iter()
                    .filter_map(|p| p["message"].as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        )
        .with("part", &id)
        .with("problems", &problems));
    }
    let files = session.snapshot();
    let s = state::of(&files, &config, &records, &id, &releases);
    if let Status::Released(label) = &s.status {
        return Err(Error::new(
            Code::NothingToRelease,
            format!("{id} is revision {label} as released: nothing to release"),
        )
        .with("part", &id)
        .with("label", label));
    }
    let label = s.next.clone().expect("a known state");
    let supersedes = list
        .iter()
        .rev()
        .find(|(e, _)| e.kind == Kind::Release)
        .map(|(e, _)| e.label.clone());
    let content = ryupdm::content::hash(&files, &id)?;
    let props = ryupdm::props::read(&files, &id)?;
    let results = session
        .rebuilt(&id)?
        .features
        .iter()
        .filter_map(|(fid, r)| {
            let b = r.as_ref().ok()?.body()?;
            Some((fid.to_string(), b.hash.clone()))
        })
        .collect();
    let project = crate::pdm::ensure_id(session)?;
    let v = VersionInfo::current();
    let record = Record::Release(Release {
        project,
        part: id.clone(),
        label,
        content,
        properties: ryupdm::record::revising(&config, &props),
        results,
        built_with: BuiltWith {
            ryucad: v.ryucad,
            ryukei: v.ryukei,
            commit: v.commit,
        },
        released_by: released_by(&repo, participant),
        supersedes,
        note,
    });
    write(session, &record)
}

/// `release.obsolete`: writes an obsolete record for `part`'s latest
/// official revision, which must be `label`. The part may be deleted.
pub fn obsolete(
    session: &mut Session,
    participant: &str,
    part: &str,
    label: &str,
    note: Option<String>,
    expected: Option<&str>,
) -> Result<Value> {
    let id = part_id(part)?;
    let (repo, releases) = history(session)?;
    if let Ok(p) = session.project().part(&id) {
        check_revision(p, expected)?;
    }
    let files = session.snapshot();
    let config = Config::read(&files)?;
    let records = Records::read(&files)?;
    let list = ordered(&config, &records, &id)?;
    check_pending(&releases, &list, &id)?;
    let latest = list
        .iter()
        .rev()
        .find(|(e, _)| e.kind == Kind::Release && releases.official.contains_key(&e.path));
    let Some((latest, _)) = latest else {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{id} has no official release to make obsolete"),
        )
        .with("part", &id)
        .with("param", "label"));
    };
    if latest.label != label {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{id}'s latest official revision is {}: only it can be made obsolete",
                latest.label
            ),
        )
        .with("part", &id)
        .with("param", "label")
        .with("latest", &latest.label));
    }
    if list
        .iter()
        .any(|(e, _)| e.kind == Kind::Obsolete && e.label == label)
    {
        return Err(Error::new(
            Code::NothingToRelease,
            format!("{id} {label} is already obsolete"),
        )
        .with("part", &id)
        .with("label", label));
    }
    let project = crate::pdm::ensure_id(session)?;
    let record = Record::Obsolete(Obsolete {
        project,
        part: id,
        label: label.to_owned(),
        released_by: released_by(&repo, participant),
        note,
    });
    write(session, &record)
}

/// The part's state, for `props.get`: in work outside a repository.
pub(crate) fn state_of(session: &Session, part: &Id) -> Value {
    let releases = history(session).map(|(_, r)| r).unwrap_or_default();
    state::derive(&session.snapshot(), part, &releases).to_json()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::tests::git;
    use crate::server::tests::{call, fresh};

    /// A project in a repository on `main` with a part `p` holding a box,
    /// numbered, committed.
    pub(crate) fn released_project() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": {"x": 10, "y": 20, "z": 5}}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": "100"}}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "start"]);
        (dir, s)
    }

    fn propose(s: &mut Session) -> Result<Value> {
        call(s, "release.propose", json!({"part": "p", "note": "first"}))
    }

    #[test]
    fn a_release_is_proposed_then_official() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        let v = propose(&mut s).unwrap();
        assert_eq!(v["path"], "releases/p/A.json");
        let r = &v["record"];
        assert_eq!(
            (&r["label"], &r["supersedes"], &r["note"]),
            (&json!("A"), &Value::Null, &json!("first"))
        );
        assert_eq!(r["properties"], json!({"number": "100"}));
        assert_eq!(r["results"]["b"].as_str().unwrap().len(), 32);
        assert_eq!(r["released_by"]["participant"], "agent");
        assert_eq!(
            r["project"],
            s.project()
                .manifest
                .project
                .as_ref()
                .map(|p| json!(p))
                .unwrap()
        );
        assert_eq!(v["state"]["text"], "in work, A proposed");
        let on_disk = std::fs::read(root.join("releases/p/A.json")).unwrap();
        assert!(crate::doc::canonical::is_canonical(
            std::str::from_utf8(&on_disk).unwrap()
        ));
        // Pending until main holds it.
        let e = propose(&mut s).unwrap_err();
        assert_eq!(
            (e.code, &e.details["label"]),
            (Code::ReleasePending, &json!("A"))
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        let e = propose(&mut s).unwrap_err();
        assert_eq!(e.code, Code::NothingToRelease);
        let got = call(&mut s, "props.get", json!({"part": "p"})).unwrap();
        assert_eq!(got["state"]["text"], "released A");
        // An edit is work towards B, which supersedes A.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        let got = call(&mut s, "props.get", json!({"part": "p"})).unwrap();
        assert_eq!(got["state"]["text"], "A modified");
        let v = propose(&mut s).unwrap();
        assert_eq!(
            (&v["record"]["label"], &v["record"]["supersedes"]),
            (&json!("B"), &json!("A"))
        );
    }

    #[test]
    fn a_part_that_isnt_ready_is_refused_with_every_reason() {
        let (_dir, mut s) = released_project();
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"finish": {"type": "string", "required": true}}}),
        )
        .unwrap();
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": null}}),
        )
        .unwrap();
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "torus", "id": "t",
                   "params": {"major_radius": 1, "minor_radius": 1}}),
        )
        .unwrap();
        let e = propose(&mut s).unwrap_err();
        assert_eq!(e.code, Code::ReleaseNotReady);
        let problems: Vec<(&str, &str)> = e.details["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["code"].as_str().unwrap(),
                    p["details"]["property"]
                        .as_str()
                        .or(p["feature"].as_str())
                        .unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            problems,
            [
                ("rebuild_failed", "t"),
                ("property_invalid", "number"),
                ("property_invalid", "finish")
            ]
        );
    }

    #[test]
    fn writes_need_a_repository_and_a_known_part() {
        let (_dir, mut s) = fresh();
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        assert_eq!(propose(&mut s).unwrap_err().code, Code::NotAGitRepo);
        let (_dir, mut s) = released_project();
        let e = call(&mut s, "release.propose", json!({"part": "q"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        let e = call(
            &mut s,
            "release.propose",
            json!({"part": "p", "expected_revision": "0000"}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::Conflict);
    }

    #[test]
    fn a_record_this_build_cant_read_blocks_releases() {
        let (dir, mut s) = released_project();
        propose(&mut s).unwrap();
        let path = dir.path().join("releases/p/A.json");
        let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["format"] = json!(2);
        v["approvals"] = json!([]);
        write_json(&path, &v).unwrap();
        let e = propose(&mut s).unwrap_err();
        assert_eq!(e.code, Code::FormatTooNew);
        let got = call(&mut s, "props.get", json!({"part": "p"})).unwrap();
        assert_eq!(got["state"]["state"], "unknown");
    }

    #[test]
    fn only_the_latest_official_revision_goes_obsolete() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        let obsolete = |s: &mut Session, label: &str| {
            call(s, "release.obsolete", json!({"part": "p", "label": label}))
        };
        assert_eq!(obsolete(&mut s, "A").unwrap_err().code, Code::InvalidParam);
        propose(&mut s).unwrap();
        // A proposal isn't official yet.
        assert_eq!(
            obsolete(&mut s, "A").unwrap_err().code,
            Code::ReleasePending
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        let e = obsolete(&mut s, "B").unwrap_err();
        assert_eq!(
            (e.code, &e.details["latest"]),
            (Code::InvalidParam, &json!("A"))
        );
        let v = obsolete(&mut s, "A").unwrap();
        assert_eq!(v["path"], "releases/p/A.obsolete.json");
        assert_eq!(v["state"]["text"], "released A, A obsolete proposed");
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "obsolete A"]);
        assert_eq!(
            obsolete(&mut s, "A").unwrap_err().code,
            Code::NothingToRelease
        );
        let got = call(&mut s, "props.get", json!({"part": "p"})).unwrap();
        assert_eq!(got["state"]["text"], "A obsolete");
        // Released again, as B.
        let v = propose(&mut s).unwrap();
        assert_eq!(
            (&v["record"]["label"], &v["record"]["supersedes"]),
            (&json!("B"), &json!("A"))
        );
        // A deleted part's latest revision can still be made obsolete.
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release B"]);
        call(&mut s, "part.delete", json!({"part": "p"})).unwrap();
        let v = obsolete(&mut s, "B").unwrap();
        assert_eq!(v["path"], "releases/p/B.obsolete.json");
    }
}
