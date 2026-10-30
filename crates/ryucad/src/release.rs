//! ryupdm's releases through the protocol: proposing a part's next
//! revision and making one obsolete, listing records with where they
//! became official, and the release checks. Each write is one record
//! under `releases/`, a logged, undoable change, and nothing is committed:
//! a record becomes official when the release branch holds it.
//!
//! The release branch is read once per state of the refs it depends on
//! (the branch's tip and the `ryupdm/` tags) and kept in the session.
//!
//! `release.sync` is ryucad's one write to Git: it creates and moves
//! local `ryupdm/<part>/<label>` tags to match the records, and never
//! pushes, deletes a tag or touches any other ref. Records under approval
//! rules are left to the hosted service, whose tags carry attestations
//! (`left_to_service`).

use crate::assembly::{Pin, Pinned};
use crate::doc::assembly::Rev;
use crate::doc::id::Id;
use crate::doc::project::write_json;
use crate::error::{Code, Error, Result};
use crate::proto::VersionInfo;
use crate::server::{check_revision, part_id};
use crate::session::Session;
use ryupdm::approvals::Rule;
use ryupdm::attest::Verdict;
use ryupdm::config::Config;
use ryupdm::history::Releases;
use ryupdm::props::Pdm;
use ryupdm::record::{BuiltWith, Kind, Obsolete, Record, Records, Release, ReleasedBy};
use ryupdm::state::{self, Status};
use ryupdm::structure::At;
use serde_json::{Value, json};

/// What the release branch as read depends on: the branch's name and
/// tip, and every `ryupdm/` tag.
fn refs_key(repo: &crate::git::Repo, config: &Config) -> Result<String> {
    let branch = ryupdm::history::branch(repo.gix(), &config.release_branch)?;
    let tags = ryupdm::history::tags(repo.gix())?;
    Ok(format!(
        "{}\0{:?}\0{tags:?}",
        config.release_branch,
        branch.map(|b| b.tip)
    ))
}

/// The project's repository and its release branch as read, from the
/// session's copy while the refs haven't moved; `not_a_git_repo` outside
/// a repository.
pub(crate) fn history(session: &mut Session) -> Result<(crate::git::Repo, Releases)> {
    let repo = crate::git::repo(session.root())?;
    let config = Config::read(&*session)?;
    let key = refs_key(&repo, &config)?;
    if let Some((k, r)) = &session.releases
        && *k == key
    {
        return Ok((repo, r.clone()));
    }
    let releases = Releases::read(repo.gix(), repo.prefix(), &config)?;
    session.releases = Some((key, releases.clone()));
    Ok((repo, releases))
}

/// What the release branch's refs are now (its tip and the `ryupdm/`
/// tags), if the project is in a repository: what release states and
/// labels' pins move with besides the files.
pub(crate) fn refs(session: &Session) -> Option<String> {
    let repo = crate::git::repo(session.root()).ok()?;
    let config = Config::read(session).ok()?;
    refs_key(&repo, &config).ok()
}

/// `part` as the release `rev` names has it, for an instance: one of the
/// project's own labels ([`pinned`]), or another project's
/// ([`crate::deps::pinned`]).
pub(crate) fn pinned_rev(session: &mut Session, part: &Id, rev: &Rev) -> Result<Pinned> {
    match rev {
        Rev::Label(l) => pinned(session, part, l),
        Rev::Foreign { project, label } => crate::deps::pinned(session, project, part, label),
        Rev::Working => Err(Error::new(
            Code::Internal,
            format!("{part} at working has no release to pin"),
        )),
    }
}

/// `part` as release `label` has it, for an instance: the files where its
/// record became official, else the working files when the proposed
/// record describes them. `label_not_found` without a record,
/// `label_not_official` otherwise; a record that can't be read gives its
/// own error.
pub(crate) fn pinned(session: &mut Session, part: &Id, label: &str) -> Result<Pinned> {
    let found = ryupdm::structure::lookup(&*session, part, label)?;
    let history = match history(session) {
        Ok(h) => Some(h),
        Err(e) if e.code == Code::NotAGitRepo => None,
        Err(e) => return Err(e),
    };
    let releases = history.as_ref().map(|(_, r)| r);
    let pin = ryupdm::structure::pin(&*session, &found, releases, part, label)?;
    let (part, commit) = match (pin.at, history) {
        (At::Commit(c), Some((repo, _))) => (session.pinned_part(&repo, part, &c)?, Some(c)),
        _ => (session.project().part(part)?.clone(), None),
    };
    Ok(Pinned {
        part,
        pin: Pin {
            commit,
            obsolete: pin.obsolete,
        },
    })
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
/// property fault, a missing number and missing required properties, and
/// for an assembly what it places that isn't released and item numbers
/// missing or shared.
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
            for name in ryupdm::props::missing(config, props) {
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
    // An assembly releases only releases: every instance, at any depth,
    // pinned to an official label that isn't obsolete.
    if p.file
        .as_ref()
        .is_ok_and(|f| f.kind == crate::doc::files::PartKind::Assembly)
    {
        problems.extend(unreleased(session, &p, &[])?.into_iter().map(|e| json!(e)));
    }
    // Its item numbers, which the release pins: complete and unique.
    problems.extend(
        ryupdm::items::faults(part, &p)
            .into_iter()
            .filter(|e| e.code != Code::BomItemUnused)
            .map(|e| json!(e)),
    );
    Ok(problems)
}

/// `instance_not_released` for each instance of assembly `p` (reached
/// through `path`) and below it that a release can't hold: at `working`,
/// at a label that doesn't resolve or isn't official, or at an obsolete
/// one.
fn unreleased(
    session: &mut Session,
    p: &crate::doc::project::Part,
    path: &[Id],
) -> Result<Vec<Error>> {
    use crate::doc::assembly::Rev;
    let mut out = Vec::new();
    for fid in p.order() {
        let Some(Ok(crate::doc::files::Feature::Instance(i))) = p.features.get(fid) else {
            continue;
        };
        let mut here = path.to_vec();
        here.push(fid.clone());
        let at = crate::assembly::path_text(&here);
        let refuse = |why: String| {
            Error::new(Code::InstanceNotReleased, format!("instance {at} {why}"))
                .with("instance", &at)
                .with("part", &i.part)
                .with("rev", i.rev.to_json())
        };
        let l = match &i.rev {
            Rev::Working => {
                out.push(refuse(format!(
                    "places {} as it is (working); pin it to an official release",
                    i.part
                )));
                continue;
            }
            Rev::Label(l) => l.clone(),
            Rev::Foreign { project, label } => format!("{label} of {project}"),
        };
        match pinned_rev(session, &i.part, &i.rev) {
            // Another project's label that can't be read says why itself:
            // locked, fetched, still official.
            Err(e) if i.rev.project().is_some() => out.push(
                e.with("instance", &at)
                    .with("part", &i.part)
                    .with("rev", i.rev.to_json()),
            ),
            Err(e) => out.push(refuse(format!("places {}@{l}: {}", i.part, e.message))),
            Ok(pin) if pin.pin.commit.is_none() => out.push(refuse(format!(
                "places {}@{l}, which isn't official yet",
                i.part
            ))),
            Ok(pin) if pin.pin.obsolete => {
                out.push(refuse(format!("places {}@{l}, which is obsolete", i.part)))
            }
            Ok(pin) => out.extend(unreleased(session, &pin.part, &here)?),
        }
    }
    Ok(out)
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
    let mut v = json!({"path": path, "record": record.to_json(), "state": s.to_json()});
    if let Some(a) = required(record.part(), &releases) {
        v["approval"] = a;
    }
    Ok(v)
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

/// What a proposal of a part would write, every check passed: its label,
/// what it supersedes, and the record's content.
struct Proposal {
    label: String,
    supersedes: Option<String>,
    content: String,
    properties: std::collections::BTreeMap<String, Value>,
    results: std::collections::BTreeMap<String, String>,
}

/// Every check `release.propose` makes of `part`, in order, and what its
/// record would hold. Writes nothing.
fn prepare(session: &mut Session, id: &Id, expected: Option<&str>) -> Result<Proposal> {
    let (_, releases) = history(session)?;
    check_revision(session.project().part(id)?, expected)?;
    let files = session.snapshot();
    let config = Config::read(&files)?;
    let records = Records::read(&files)?;
    let list = ordered(&config, &records, id)?;
    check_pending(&releases, &list, id)?;
    let problems = readiness(session, id, &config)?;
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
        .with("part", id)
        .with("problems", &problems));
    }
    let files = session.snapshot();
    let s = state::of(&files, &config, &records, id, &releases);
    if let Status::Released(label) = &s.status {
        return Err(Error::new(
            Code::NothingToRelease,
            format!("{id} is revision {label} as released: nothing to release"),
        )
        .with("part", id)
        .with("label", label));
    }
    let label = s.next.clone().expect("a known state");
    let supersedes = list
        .iter()
        .rev()
        .find(|(e, _)| e.kind == Kind::Release)
        .map(|(e, _)| e.label.clone());
    let content = ryupdm::content::hash(&files, id)?;
    let props = ryupdm::props::read(&files, id)?;
    let results = session
        .rebuilt(id)?
        .features
        .iter()
        .filter_map(|(fid, r)| {
            let b = r.as_ref().ok()?.body()?;
            Some((fid.to_string(), b.hash.clone()))
        })
        .collect();
    Ok(Proposal {
        label,
        supersedes,
        content,
        properties: ryupdm::record::revising(&config, &props),
        results,
    })
}

/// `release.propose`: writes the record of `part`'s next revision. With
/// `dry_run`, writes nothing and answers the label it would take and
/// every reason it would be refused (`problems`, each an error as JSON:
/// `release_not_ready`'s list, or the one refusal), for a person to see
/// before proposing.
pub fn propose(
    session: &mut Session,
    participant: &str,
    part: &str,
    note: Option<String>,
    expected: Option<&str>,
    dry_run: bool,
) -> Result<Value> {
    let id = part_id(part)?;
    if dry_run {
        return match prepare(session, &id, expected) {
            Ok(p) => Ok(
                json!({"part": id, "label": p.label, "supersedes": p.supersedes,
                               "ready": true, "problems": []}),
            ),
            // No such part is no proposal to judge.
            Err(e) if e.code == Code::NotFound => Err(e),
            Err(e) => {
                let problems = match e.code {
                    Code::ReleaseNotReady => e.details["problems"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                    _ => vec![json!(e)],
                };
                let next = state_of(session, &id)["next"].clone();
                Ok(
                    json!({"part": id, "label": next, "supersedes": null, "ready": false,
                          "problems": problems}),
                )
            }
        };
    }
    let p = prepare(session, &id, expected)?;
    let (repo, _) = history(session)?;
    let project = crate::pdm::ensure_id(session)?;
    let v = VersionInfo::current();
    let record = Record::Release(Release {
        project,
        part: id.clone(),
        label: p.label,
        content: p.content,
        properties: p.properties,
        results: p.results,
        built_with: BuiltWith {
            ryucad: v.ryucad,
            ryukei: v.ryukei,
            commit: v.commit,
        },
        released_by: released_by(&repo, participant),
        supersedes: p.supersedes,
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
pub(crate) fn state_of(session: &mut Session, part: &Id) -> Value {
    let releases = history(session).map(|(_, r)| r).unwrap_or_default();
    state::derive(&session.snapshot(), part, &releases).to_json()
}

/// Each part's release state and its releases' tags, for `pdm.changed`:
/// `{part, state, tags: [{tag, status}]}` by part.
pub(crate) fn summary(session: &mut Session) -> std::collections::BTreeMap<Id, Value> {
    let releases = history(session).map(|(_, r)| r).unwrap_or_default();
    state::derive_all(&*session, &releases)
        .into_iter()
        .map(|(part, s)| {
            let tags: Vec<Value> = releases
                .official
                .iter()
                .filter_map(|(path, o)| {
                    let name = path.strip_prefix(&format!("releases/{part}/"))?;
                    let (label, kind) = ryupdm::record::parse_name(name)?;
                    (kind == Kind::Release).then(|| {
                        json!({"tag": ryupdm::history::tag_name(&part, &label),
                               "status": releases.tag(&part, &label, &o.commit).as_str()})
                    })
                })
                .collect();
            let v = json!({"part": part, "state": s.to_json(), "tags": tags});
            (part, v)
        })
        .collect()
}

/// The rules a proposal of `part` will be judged by: those at the release
/// branch's tip as this clone has it. `None` when none apply.
fn required(part: &Id, releases: &Releases) -> Option<Value> {
    let rules = releases.tip_approvals.as_ref()?.applicable(part);
    (!rules.is_empty()).then(
        || json!({"required": true, "rules": rules.iter().map(Rule::to_json).collect::<Vec<_>>()}),
    )
}

/// An official record's `approval` under rules: how its tag stands and,
/// where its attestation verifies, who proposed and approved it.
fn approval(a: &ryupdm::history::Approval) -> Value {
    let mut v = json!({
        "rules": a.rules.iter().map(Rule::to_json).collect::<Vec<_>>(),
        "rules_at": a.rules_at,
        "status": a.verdict.as_ref().map_or("moved", Verdict::status),
    });
    if let Some(Verdict::Invalid(c)) = &a.verdict {
        v["clause"] = json!(c.name());
    }
    if let Some((at, key)) = a.verdict.as_ref().and_then(Verdict::attestation) {
        v["outcome"] = json!(at.outcome.as_str());
        v["approvers"] = json!(
            at.approvals
                .iter()
                .map(|x| json!({"login": x.github.login, "id": x.github.id, "at": x.at}))
                .collect::<Vec<_>>()
        );
        v["proposed_by"] = json!({"login": at.proposed_by.login, "id": at.proposed_by.id});
        v["key"] = json!(key);
        if !at.missing.is_empty() {
            v["missing"] = json!(
                at.missing
                    .iter()
                    .map(ryupdm::approvals::Unmet::to_json)
                    .collect::<Vec<_>>()
            );
        }
    }
    v
}

/// One record as `release.list` gives it.
fn listed(entry: &ryupdm::record::Entry, releases: &Releases, has_repo: bool) -> Value {
    let mut v = json!({"part": entry.part, "label": entry.label, "kind": entry.kind.as_str(),
                       "path": entry.path});
    match &entry.record {
        Err(e) => v["error"] = json!(e),
        Ok(r) => {
            if let Some(rel) = r.release() {
                v["supersedes"] = json!(rel.supersedes);
            }
            if let Some(n) = r.note() {
                v["note"] = json!(n);
            }
            let by = r.released_by();
            v["released_by"] = json!({"name": by.name, "email": by.email,
                                      "participant": by.participant});
        }
    }
    match releases.official.get(&entry.path) {
        Some(o) => {
            v["status"] = json!("official");
            v["commit"] = json!(o.commit);
            if o.shallow {
                v["shallow"] = json!(true);
            }
            let ruled = releases.approvals.get(&entry.path);
            if entry.kind == Kind::Release || ruled.is_some() {
                let t = releases.tag_of(&entry.part, &entry.label, entry.kind, &o.commit);
                let mut tag = json!({
                    "name": ryupdm::history::tag_name_of(&entry.part, &entry.label, entry.kind),
                    "status": t.as_str()});
                if let ryupdm::history::Tag::Moved(to) = t {
                    tag["points_at"] = json!(to);
                }
                // Under rules only the service tags it.
                if ruled.is_some() {
                    tag["service"] = json!(true);
                }
                v["tag"] = tag;
            }
            if let Some(a) = ruled {
                v["approval"] = approval(a);
            }
        }
        None => {
            v["status"] = json!(if has_repo { "proposed" } else { "unofficial" });
            if let Some(a) = required(&entry.part, releases) {
                v["approval"] = a;
            }
        }
    }
    v
}

/// `release.list`: every record (or one part's) with where it became
/// official and its tag, and each part's state.
pub fn list(session: &mut Session, part: Option<&str>) -> Result<Value> {
    let part = part.map(part_id).transpose()?;
    let (repo, releases) = match history(session) {
        Ok((repo, r)) => (Some(repo), r),
        Err(e) if e.code == Code::NotAGitRepo => (None, Releases::default()),
        Err(e) => return Err(e),
    };
    let files = session.snapshot();
    let config = Config::read(&files)?;
    let records = Records::read(&files)?;
    let mine = |p: &Id| part.as_ref().is_none_or(|x| x == p);
    if let Some(p) = &part
        && session.project().part(p).is_err()
        && records.of(p).next().is_none()
    {
        return Err(Error::new(Code::NotFound, format!("no part {p}")).with("part", p));
    }
    let list: Vec<Value> = records
        .entries
        .iter()
        .filter(|e| mine(&e.part))
        .map(|e| listed(e, &releases, repo.is_some()))
        .collect();
    let parts: Vec<Value> = state::derive_all(&files, &releases)
        .into_iter()
        .filter(|(p, _)| mine(p))
        .map(|(p, s)| {
            let mut v = s.to_json();
            v["part"] = json!(p);
            v
        })
        .collect();
    Ok(json!({
        "repository": repo.is_some(),
        "release_branch": config.release_branch,
        "branch": releases.branch.as_ref().map(|b| json!({"name": b.name, "tip": b.tip})),
        "shallow": releases.shallow,
        "records": list,
        "parts": parts,
    }))
}

/// `release.sync`: the tag of every official release, created where
/// missing and moved where it points elsewhere. A tag no official release
/// accounts for is reported, never deleted. Fails as a whole on the first
/// ref that can't be written, saying which were.
pub fn sync(session: &mut Session) -> Result<Value> {
    use gix::refs::transaction::{PreviousValue, RefEdit};
    let (repo, releases) = history(session)?;
    let mut created = Vec::new();
    let mut left = Vec::new();
    let mut moved = Vec::new();
    let mut unchanged = 0;
    for (path, o) in &releases.official {
        let mut seg = path.split('/').skip(1);
        let (Some(part), Some(file)) = (seg.next(), seg.next()) else {
            continue;
        };
        let (Ok(part), Some((label, Kind::Release))) =
            (Id::parse(part, "part"), ryupdm::record::parse_name(file))
        else {
            continue;
        };
        let name = ryupdm::history::tag_name(&part, &label);
        // Under rules the tag is the service's, annotated and signed.
        if releases.approvals.contains_key(path) {
            left.push(json!({"tag": name, "path": path}));
            continue;
        }
        let expected = match releases.tag(&part, &label, &o.commit) {
            ryupdm::history::Tag::Ok => {
                unchanged += 1;
                continue;
            }
            ryupdm::history::Tag::Missing => PreviousValue::MustNotExist,
            ryupdm::history::Tag::Moved(_) => PreviousValue::MustExist,
        };
        let failed = |e: String| {
            Error::new(
                Code::Internal,
                format!("writing tag {name}: {e}; tags written before it are kept"),
            )
            .with("tag", &name)
            .with("created", &created)
            .with("moved", &moved)
        };
        let id = gix::ObjectId::from_hex(o.commit.as_bytes()).map_err(|e| failed(e.to_string()))?;
        let full = gix::refs::FullName::try_from(format!("refs/tags/{name}"))
            .map_err(|e| failed(e.to_string()))?;
        let move_from = match &expected {
            PreviousValue::MustExist => Some(releases.tags[&format!("{part}/{label}")].clone()),
            _ => None,
        };
        repo.gix()
            .edit_reference(RefEdit::update(full, id, expected, "ryucad release sync"))
            .map_err(|e| failed(e.to_string()))?;
        match move_from {
            Some(from) => moved.push(json!({"tag": name, "from": from, "to": o.commit})),
            None => created.push(json!({"tag": name, "at": o.commit})),
        }
    }
    let orphaned: Vec<String> = releases
        .orphaned_tags()
        .into_iter()
        .map(|k| format!("{}{k}", ryupdm::history::TAG_PREFIX))
        .collect();
    Ok(
        json!({"created": created, "moved": moved, "unchanged": unchanged,
              "orphaned": orphaned, "left_to_service": left}),
    )
}

/// Rebuilds released content for `release_result_changed`, once per
/// content hash in a session.
struct Rebuilds<'a> {
    root: std::path::PathBuf,
    cache: &'a mut std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

impl ryupdm::check::Results for Rebuilds<'_> {
    fn bodies(
        &mut self,
        src: &dyn crate::doc::source::Source,
        part: &Id,
        content: &str,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        if let Some(b) = self.cache.get(content) {
            return Ok(b.clone());
        }
        let project = crate::doc::project::Project::load_from(&self.root, src)?;
        let built = crate::rebuild::rebuild_part(project.part(part)?);
        let bodies: std::collections::BTreeMap<String, String> = built
            .features
            .iter()
            .filter_map(|(fid, r)| Some((fid.to_string(), r.as_ref().ok()?.body()?.hash.clone())))
            .collect();
        self.cache.insert(content.to_owned(), bodies.clone());
        Ok(bodies)
    }
}

/// The release findings for the project (a `schema: 1` finding each):
/// with `results`, official releases are rebuilt too.
pub(crate) fn findings(session: &mut Session, results: bool) -> Result<Vec<Value>> {
    let files = session.snapshot();
    let found = match history(session) {
        Ok((repo, releases)) => {
            let h = ryupdm::check::History {
                repo: repo.gix(),
                prefix: repo.prefix(),
                releases: &releases,
            };
            let root = session.root().to_path_buf();
            let mut rebuilds = Rebuilds {
                root,
                cache: &mut session.release_results,
            };
            let r: Option<&mut dyn ryupdm::check::Results> =
                if results { Some(&mut rebuilds) } else { None };
            ryupdm::check::run(&files, Some(h), r)?
        }
        Err(e) if e.code == Code::NotAGitRepo => ryupdm::check::run(&files, None, None)?,
        Err(e) => return Err(e),
    };
    Ok(found.iter().map(ryupdm::check::Finding::to_json).collect())
}

/// `release.check`: the release checks alone, reported as `check` reports;
/// fails with `check_failed` when any finding is an error.
pub fn check(session: &mut Session, results: bool) -> Result<Value> {
    let findings = findings(session, results)?;
    let errors = findings.iter().filter(|f| f["severity"] == "error").count();
    let report = json!({
        "schema": ryupdm::check::SCHEMA,
        "passed": errors == 0,
        "errors": errors,
        "warnings": findings.len() - errors,
        "findings": findings,
    });
    if errors > 0 {
        return Err(Error {
            code: Code::CheckFailed,
            message: format!("{errors} release check(s) failed"),
            details: report,
        });
    }
    Ok(report)
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
    fn a_dry_run_says_the_label_and_every_reason_and_writes_nothing() {
        let (dir, mut s) = released_project();
        let dry = |s: &mut Session| {
            call(s, "release.propose", json!({"part": "p", "dry_run": true})).unwrap()
        };
        let v = dry(&mut s);
        assert_eq!(
            v,
            json!({"part": "p", "label": "A", "supersedes": null, "ready": true,
                   "problems": []})
        );
        assert!(!dir.path().join("releases").exists());
        // Not ready: every reason, as a proposal would list them.
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": null}}),
        )
        .unwrap();
        let v = dry(&mut s);
        assert_eq!((&v["ready"], &v["label"]), (&json!(false), &json!("A")));
        assert_eq!(v["problems"][0]["code"], "property_invalid");
        assert_eq!(v["problems"][0]["details"]["missing"], true);
        // Any other refusal is the one problem.
        call(
            &mut s,
            "props.set",
            json!({"part": "p", "properties": {"number": "100"}}),
        )
        .unwrap();
        propose(&mut s).unwrap();
        let v = dry(&mut s);
        assert_eq!(v["problems"][0]["code"], "release_pending", "{v}");
        assert_eq!(v["label"], "B");
        let e = call(
            &mut s,
            "release.propose",
            json!({"part": "q", "dry_run": true}),
        )
        .unwrap_err();
        assert_eq!(e.code, Code::NotFound);
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
    fn records_are_listed_with_their_commits_and_tags() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        let v = call(&mut s, "release.list", json!({})).unwrap();
        assert_eq!(
            (&v["branch"]["name"], &v["records"], &v["parts"][0]["text"]),
            (&json!("main"), &json!([]), &json!("in work"))
        );
        propose(&mut s).unwrap();
        let v = call(&mut s, "release.list", json!({"part": "p"})).unwrap();
        let r = &v["records"][0];
        assert_eq!(
            (&r["label"], &r["status"], &r["note"]),
            (&json!("A"), &json!("proposed"), &json!("first"))
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        let v = call(&mut s, "release.list", json!({})).unwrap();
        let r = &v["records"][0];
        assert_eq!(
            (&r["status"], &r["commit"]),
            (&json!("official"), &json!(head))
        );
        assert_eq!(r["tag"], json!({"name": "ryupdm/p/A", "status": "missing"}));
        assert_eq!(v["parts"][0]["text"], "released A");
        git(root, &["tag", "ryupdm/p/A", &head]);
        let v = call(&mut s, "release.list", json!({})).unwrap();
        assert_eq!(v["records"][0]["tag"]["status"], "ok");
        let e = call(&mut s, "release.list", json!({"part": "nope"})).unwrap_err();
        assert_eq!(e.code, Code::NotFound);
        // Outside a repository, records are listed, nothing official.
        let (_d, mut t) = fresh();
        let v = call(&mut t, "release.list", json!({})).unwrap();
        assert_eq!(
            (&v["repository"], &v["branch"]),
            (&json!(false), &Value::Null)
        );
    }

    #[test]
    fn the_checks_report_and_fail_as_check_does() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        propose(&mut s).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        let v = call(&mut s, "release.check", json!({})).unwrap();
        assert_eq!(
            (&v["schema"], &v["passed"], &v["warnings"]),
            (&json!(1), &json!(true), &json!(1))
        );
        assert_eq!(v["findings"][0]["code"], "release_tag_missing");
        assert_eq!(v["findings"][0]["source"], "release");
        // Rebuilt, A's bodies are what the record says, once per content.
        let v = call(&mut s, "release.check", json!({"results": true})).unwrap();
        assert_eq!(v["warnings"], 1);
        assert_eq!(s.release_results.len(), 1);
        // A proposal the part has moved on from fails.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        propose(&mut s).unwrap();
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 13}}),
        )
        .unwrap();
        let e = call(&mut s, "release.check", json!({})).unwrap_err();
        assert_eq!(e.code, Code::CheckFailed);
        let codes: Vec<&str> = e.details["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["code"].as_str().unwrap())
            .collect();
        assert_eq!(codes, ["release_stale", "release_tag_missing"]);
        assert_eq!(e.details["errors"], 1);
    }

    #[test]
    fn the_branch_is_read_again_only_when_refs_move() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        call(&mut s, "release.list", json!({})).unwrap();
        let key = s.releases.as_ref().unwrap().0.clone();
        call(&mut s, "release.list", json!({})).unwrap();
        assert_eq!(s.releases.as_ref().unwrap().0, key);
        git(root, &["commit", "-q", "--allow-empty", "-m", "moves main"]);
        call(&mut s, "release.list", json!({})).unwrap();
        assert_ne!(s.releases.as_ref().unwrap().0, key);
    }

    fn rev(root: &std::path::Path, what: &str) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-parse", what])
            .current_dir(root)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn sync_makes_and_moves_tags_and_leaves_others() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        propose(&mut s).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        let a = rev(root, "HEAD");
        git(root, &["commit", "-q", "--allow-empty", "-m", "later"]);
        git(root, &["tag", "ryupdm/q/Z", "HEAD"]);
        git(root, &["tag", "other", "HEAD"]);
        let v = call(&mut s, "release.sync", json!({})).unwrap();
        assert_eq!(
            v,
            json!({"created": [{"tag": "ryupdm/p/A", "at": a}], "moved": [], "unchanged": 0,
                   "orphaned": ["ryupdm/q/Z"], "left_to_service": []})
        );
        assert_eq!(rev(root, "ryupdm/p/A"), a);
        // Again: nothing to do.
        let v = call(&mut s, "release.sync", json!({})).unwrap();
        assert_eq!((&v["created"], &v["unchanged"]), (&json!([]), &json!(1)));
        // Moved by hand, moved back.
        git(root, &["tag", "-f", "ryupdm/p/A", "HEAD"]);
        let later = rev(root, "HEAD");
        let v = call(&mut s, "release.sync", json!({})).unwrap();
        assert_eq!(
            v["moved"],
            json!([{"tag": "ryupdm/p/A", "from": later, "to": a}])
        );
        assert_eq!(rev(root, "ryupdm/p/A"), a);
        // Others are left alone; nothing official, nothing written.
        assert_eq!(rev(root, "other"), later);
        assert_eq!(rev(root, "ryupdm/q/Z"), later);
        let v = call(&mut s, "release.check", json!({})).unwrap();
        assert_eq!(v["findings"][0]["code"], "release_tag_orphaned");
        let (_d, mut t) = fresh();
        assert_eq!(
            call(&mut t, "release.sync", json!({})).unwrap_err().code,
            Code::NotAGitRepo
        );
    }

    #[test]
    fn writes_are_logged_and_only_files_are_undone() {
        use crate::live::Live;
        use crate::server::tests::call_live;
        let (dir, mut s) = released_project();
        let root = dir.path();
        let mut live = Live::new();
        // As a human names itself.
        let me = call_live(
            &mut s,
            &mut live,
            None,
            "session.hello",
            json!({"kind": "human", "name": "Dana"}),
        )
        .unwrap()["participant"]
            .as_str()
            .unwrap()
            .to_owned();
        let me = Some(me.as_str());
        let v = call_live(
            &mut s,
            &mut live,
            me,
            "release.propose",
            json!({"part": "p"}),
        )
        .unwrap();
        assert_eq!(v["record"]["released_by"]["participant"], "Dana");
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        call_live(&mut s, &mut live, me, "release.sync", json!({})).unwrap();
        let log = call_live(&mut s, &mut live, None, "activity.log", json!({})).unwrap();
        let kinds: Vec<(&str, &str)> = log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["kind"].as_str().unwrap(),
                    e["method"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [("change", "release.propose"), ("refs", "release.sync")]
        );
        // Undo passes over the tags to the record.
        let r = call_live(&mut s, &mut live, me, "undo", json!({})).unwrap();
        assert_eq!(r["entry"]["kind"], "undo");
        assert!(!root.join("releases/p/A.json").exists());
        assert_eq!(rev(root, "ryupdm/p/A").len(), 40);
    }

    #[test]
    fn subscribers_hear_when_release_states_and_tags_change() {
        use crate::notify::Topic;
        let (dir, mut s) = released_project();
        let root = dir.path();
        let mut live = crate::live::Live::new();
        let sub = live.hub.subscribe("p-x", &[Topic::Pdm]);
        let mut told = |s: &mut Session| -> Vec<String> {
            s.reload().unwrap();
            crate::server::publish(&mut live, s);
            sub.drain()
                .iter()
                .flat_map(|l| {
                    let v: Value = serde_json::from_str(l).unwrap();
                    assert_eq!(v["method"], "pdm.changed");
                    v["params"]["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| {
                            let tags: Vec<&str> = p["tags"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|t| t["status"].as_str().unwrap())
                                .collect();
                            format!(
                                "{} {} {tags:?}",
                                p["part"].as_str().unwrap(),
                                p["state"]["text"].as_str().unwrap()
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        // The first reading is where it starts.
        assert!(told(&mut s).is_empty());
        assert!(told(&mut s).is_empty());
        propose(&mut s).unwrap();
        assert_eq!(told(&mut s), [r#"p in work, A proposed []"#]);
        // The release branch moving makes it official, behind the
        // session's back.
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        assert_eq!(told(&mut s), [r#"p released A ["missing"]"#]);
        call(&mut s, "release.sync", json!({})).unwrap();
        assert_eq!(told(&mut s), [r#"p released A ["ok"]"#]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        assert_eq!(told(&mut s), [r#"p A modified ["ok"]"#]);
        // A part without release changes says nothing.
        call(&mut s, "part.create", json!({"part": "q"})).unwrap();
        assert_eq!(told(&mut s), [r#"q in work []"#]);
        call(
            &mut s,
            "feature.add",
            json!({"part": "q", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        assert!(told(&mut s).is_empty());
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

    #[test]
    fn diff_shows_proposals_and_what_they_do_to_states() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        propose(&mut s).unwrap();
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert_eq!(
            v["parts"]["p"]["pdm"],
            json!({"properties": {},
                   "records": {"added": [{"label": "A", "kind": "release"}],
                               "removed": [], "changed": []},
                   "state": {"from": "in work", "to": "in work, A proposed"}})
        );
        // Official on main, then worked on.
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert_eq!(
            v["parts"]["p"]["pdm"]["state"],
            json!({"from": "released A", "to": "A modified"})
        );
        assert_eq!(v["parts"]["p"]["pdm"]["properties"], json!({}));
        assert!(v.get("pdm").is_none());
        call(
            &mut s,
            "pdm.configure",
            json!({"properties": {"finish": {"type": "string"}}}),
        )
        .unwrap();
        let v = call(&mut s, "diff", json!({})).unwrap();
        assert_eq!(
            v["pdm"]["changes"],
            json!([{"at": "/properties",
                    "to": {"finish": {"required": false, "revising": true, "type": "string"}}}])
        );
    }

    /// Rules over every part (1 of alice, bob) trusting the test key,
    /// configured and committed.
    pub(crate) fn ruled(s: &mut Session, root: &std::path::Path) {
        use ryupdm::attest::{TEST_SEED, hex, public_key, test_key_id};
        call(
            s,
            "pdm.configure",
            json!({"approvals": {"keys": {test_key_id(): hex(&public_key(&TEST_SEED))},
                   "rules": [{"count": 1, "from": ["alice", "bob"], "parts": ["*"]}]}}),
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "rules"]);
    }

    /// The service's tag on the record at `rel`, official at `commit`.
    pub(crate) fn attest(
        root: &std::path::Path,
        rel: &str,
        kind: Kind,
        commit: &str,
        approved: bool,
    ) {
        use ryupdm::attest::*;
        let bytes = std::fs::read(root.join(rel)).unwrap();
        let v = crate::doc::canonical::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
        let record = Record::from_json(&v, kind).unwrap();
        let rules = vec![ryupdm::approvals::Rule {
            parts: vec!["*".into()],
            count: 1,
            from: vec!["alice".into(), "bob".into()],
        }];
        let a = Attestation {
            approvals: if approved {
                vec![Approval {
                    at: "2026-11-02T09:14:03Z".into(),
                    commit: commit.into(),
                    github: User {
                        id: 1,
                        login: "alice".into(),
                    },
                    review: 1,
                }]
            } else {
                vec![]
            },
            commit: commit.into(),
            label: record.label().into(),
            missing: if approved {
                vec![]
            } else {
                vec![ryupdm::approvals::Unmet {
                    rule: rules[0].clone(),
                    needed: 1,
                }]
            },
            outcome: if approved {
                Outcome::Approved
            } else {
                Outcome::Unapproved
            },
            part: record.part().clone(),
            project: record.project().clone(),
            proposed_by: User {
                id: 2,
                login: "dana".into(),
            },
            pull: Some(Pull {
                number: 1,
                repo: "acme/widgets".into(),
            }),
            record: blake3::hash(&bytes).to_hex().to_string(),
            record_kind: kind,
            rules,
            rules_at: rev(root, &format!("{commit}^")),
            signed_at: "2026-11-02T09:20:11Z".into(),
        };
        let name = ryupdm::history::tag_name_of(record.part(), record.label(), kind);
        let file = root.join(".git/ATTEST_MSG");
        std::fs::write(&file, sign(&a, &TEST_SEED)).unwrap();
        git(
            root,
            &[
                "tag",
                "-a",
                "--cleanup=verbatim",
                "-F",
                file.to_str().unwrap(),
                &name,
                commit,
            ],
        );
    }

    #[test]
    fn records_under_rules_say_who_approved_them() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        ruled(&mut s, root);
        let v = propose(&mut s).unwrap();
        // The proposal names whom to ask.
        assert_eq!(v["approval"]["required"], true);
        assert_eq!(v["approval"]["rules"][0]["from"], json!(["alice", "bob"]));
        let listed = call(&mut s, "release.list", json!({})).unwrap();
        assert_eq!(listed["records"][0]["approval"]["required"], true);
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        let official = rev(root, "HEAD");
        let listed = call(&mut s, "release.list", json!({})).unwrap();
        let r = &listed["records"][0];
        assert_eq!(
            (&r["approval"]["status"], &r["tag"]["service"]),
            (&json!("missing"), &json!(true))
        );
        attest(root, "releases/p/A.json", Kind::Release, &official, true);
        let listed = call(&mut s, "release.list", json!({})).unwrap();
        let a = &listed["records"][0]["approval"];
        assert_eq!(
            (
                &a["status"],
                &a["outcome"],
                &a["approvers"][0]["login"],
                &a["proposed_by"]["login"]
            ),
            (
                &json!("approved"),
                &json!("approved"),
                &json!("alice"),
                &json!("dana")
            )
        );
        assert_eq!(a["key"], ryupdm::attest::test_key_id());
        assert_eq!(a["rules_at"], rev(root, &format!("{official}^")));
        // A record no rule covers has no `approval`.
        let (_dir2, mut plain) = released_project();
        propose(&mut plain).unwrap();
        let listed = call(&mut plain, "release.list", json!({})).unwrap();
        assert!(listed["records"][0].get("approval").is_none());
    }

    #[test]
    fn sync_leaves_records_under_rules_to_the_service() {
        let (dir, mut s) = released_project();
        let root = dir.path();
        ruled(&mut s, root);
        propose(&mut s).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "release A"]);
        let v = call(&mut s, "release.sync", json!({})).unwrap();
        assert_eq!(v["created"], json!([]));
        assert_eq!(
            v["left_to_service"],
            json!([{"tag": "ryupdm/p/A", "path": "releases/p/A.json"}])
        );
        let tags = std::process::Command::new("git")
            .args(["tag", "-l", "ryupdm/*"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(tags.stdout.is_empty());
        // A lightweight tag an older sync made is left as it is, and the
        // check says how to replace it.
        let official = rev(root, "HEAD");
        git(root, &["tag", "ryupdm/p/A", &official]);
        call(&mut s, "release.sync", json!({})).unwrap();
        assert_eq!(rev(root, "refs/tags/ryupdm/p/A"), official);
        let e = call(&mut s, "release.check", json!({})).unwrap_err();
        let f = &e.details["findings"][0];
        assert_eq!(
            (&f["code"], &f["details"]["clause"]),
            (&json!("release_attestation_invalid"), &json!("lightweight"))
        );
    }
}
