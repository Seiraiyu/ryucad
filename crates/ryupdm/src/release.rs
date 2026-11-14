//! The steps of a release that need no session, for every product's
//! commands: a part's records in order, a proposal still pending, the
//! next label, the revision an obsolete record may name, who writes a
//! record, a record and its approval as `release.list` gives them, and
//! the tags `release.sync` makes and moves. ryucad's `release.rs` holds
//! its own copy of these (design 629's "ryucad doesn't notice"); ryueda
//! calls them.

use crate::approvals::Rule;
use crate::attest::Verdict;
use crate::config::Config;
use crate::history::{Approval, Releases, Tag};
use crate::product::Product;
use crate::record::{Entry, Kind, Record, Records, ReleasedBy};
use crate::state::{self, Status};
use ryu_doc::error::{Code, Error, Result};
use ryu_doc::id::Id;
use serde_json::{Value, json};

/// What the release branch as read depends on: the branch's name and
/// tip, and every `ryupdm/` tag with its own object (a tag replaced at
/// the same commit says something else).
pub fn refs_key(repo: &gix::Repository, config: &Config) -> Result<String> {
    let branch = crate::history::branch(repo, &config.release_branch)?;
    let tags = crate::history::tag_refs(repo)?;
    Ok(format!(
        "{}\0{:?}\0{tags:?}",
        config.release_branch,
        branch.map(|b| b.tip)
    ))
}

/// Who is writing a record: Git's author identity, and the participant.
pub fn released_by(repo: &gix::Repository, participant: &str) -> ReleasedBy {
    let author = repo.author().and_then(|a| a.ok());
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

/// The part's records, in order, refusing those this build can't read.
pub fn ordered<'a>(
    config: &Config,
    records: &'a Records,
    part: &'a Id,
) -> Result<Vec<(&'a Entry, &'a Record)>> {
    for e in records.of(part) {
        if let Err(err) = &e.record {
            return Err(err.clone());
        }
    }
    state::ordered(config.revisions, records, part)
}

/// Refuses while any record of `part` is still proposed.
pub fn check_pending(releases: &Releases, list: &[(&Entry, &Record)], part: &Id) -> Result<()> {
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

/// The label a proposal of a part takes, and the release it supersedes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Next {
    pub label: String,
    pub supersedes: Option<String>,
}

/// The next revision of `part` in the project of `P` that `src` holds:
/// refused while a record is pending (`release_pending`) and when the
/// part is its latest release as it is (`nothing_to_release`).
pub fn next_for<P: Product>(
    src: &dyn ryu_doc::source::Source,
    config: &Config,
    records: &Records,
    releases: &Releases,
    part: &Id,
) -> Result<Next> {
    let list = ordered(config, records, part)?;
    check_pending(releases, &list, part)?;
    let s = state::of_for::<P>(src, config, records, part, releases);
    match &s.status {
        Status::Released(label) => {
            return Err(Error::new(
                Code::NothingToRelease,
                format!("{part} is revision {label} as released: nothing to release"),
            )
            .with("part", part)
            .with("label", label));
        }
        Status::Unknown(e) => return Err(e.clone()),
        _ => {}
    }
    let label = s.next.clone().ok_or_else(|| {
        Error::new(Code::Internal, format!("{part} has no next label")).with("part", part)
    })?;
    let supersedes = list
        .iter()
        .rev()
        .find(|(e, _)| e.kind == Kind::Release)
        .map(|(e, _)| e.label.clone());
    Ok(Next { label, supersedes })
}

/// Refuses an obsolete record of `part` at `label` unless `label` is its
/// latest official release and isn't obsolete already.
pub fn obsolete_target(
    config: &Config,
    records: &Records,
    releases: &Releases,
    part: &Id,
    label: &str,
) -> Result<()> {
    let list = ordered(config, records, part)?;
    check_pending(releases, &list, part)?;
    let latest = list
        .iter()
        .rev()
        .find(|(e, _)| e.kind == Kind::Release && releases.official.contains_key(&e.path));
    let Some((latest, _)) = latest else {
        return Err(Error::new(
            Code::InvalidParam,
            format!("{part} has no official release to make obsolete"),
        )
        .with("part", part)
        .with("param", "label"));
    };
    if latest.label != label {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{part}'s latest official revision is {}: only it can be made obsolete",
                latest.label
            ),
        )
        .with("part", part)
        .with("param", "label")
        .with("latest", &latest.label));
    }
    if list
        .iter()
        .any(|(e, _)| e.kind == Kind::Obsolete && e.label == label)
    {
        return Err(Error::new(
            Code::NothingToRelease,
            format!("{part} {label} is already obsolete"),
        )
        .with("part", part)
        .with("label", label));
    }
    Ok(())
}

/// A part's release state as `pdm.changed` gives it.
pub fn state_value(part: &Id, s: &state::State, releases: &Releases) -> Value {
    let tags: Vec<Value> = releases
        .official
        .iter()
        .filter_map(|(path, o)| {
            let name = path.strip_prefix(&format!("releases/{part}/"))?;
            let (label, kind) = crate::record::parse_name(name)?;
            (kind == Kind::Release).then(|| {
                json!({"tag": releases.name_of(part, &label, Kind::Release),
                       "status": releases.tag(part, &label, &o.commit).as_str()})
            })
        })
        .collect();
    json!({"part": part, "state": s.to_json(), "tags": tags})
}

/// The rules a proposal of `part` will be judged by: those at the release
/// branch's tip as this clone has it. `None` when none apply.
pub fn required(part: &Id, releases: &Releases) -> Option<Value> {
    let rules = releases.tip_approvals.as_ref()?.applicable(part);
    (!rules.is_empty()).then(
        || json!({"required": true, "rules": rules.iter().map(Rule::to_json).collect::<Vec<_>>()}),
    )
}

/// An official record's `approval` under rules: how its tag stands and,
/// where its attestation verifies, who proposed and approved it.
pub fn approval(a: &Approval) -> Value {
    let mut v = json!({
        "rules": a.rules.iter().map(Rule::to_json).collect::<Vec<_>>(),
        "rules_at": a.rules_at,
        "status": a.verdict.as_ref().map_or("moved", Verdict::status),
    });
    match &a.verdict {
        Some(Verdict::Invalid(c)) => v["clause"] = json!(c.name()),
        Some(Verdict::Revoked { key, revoked }) => {
            v["key"] = json!(key);
            v["revoked"] = json!(revoked);
        }
        Some(Verdict::Unknown { key, format }) => {
            v["key"] = json!(key);
            v["format"] = json!(format);
        }
        _ => {}
    }
    if let Some((at, key)) = a.verdict.as_ref().and_then(Verdict::attestation) {
        v["outcome"] = json!(at.outcome.as_str());
        v["approvers"] = json!(
            at.approvals
                .iter()
                .map(|x| {
                    let mut a = json!({"login": x.github.login, "id": x.github.id, "at": x.at});
                    if !x.via.is_empty() {
                        a["via"] = json!(x.via);
                    }
                    a
                })
                .collect::<Vec<_>>()
        );
        v["proposed_by"] = json!({"login": at.proposed_by.login, "id": at.proposed_by.id});
        v["key"] = json!(key);
        if !at.missing.is_empty() {
            v["missing"] = json!(
                at.missing
                    .iter()
                    .map(crate::approvals::Unmet::to_json)
                    .collect::<Vec<_>>()
            );
        }
    }
    v
}

/// One record as `release.list` gives it; `has_repo` says whether the
/// project is in a repository (else a record not official is
/// `unofficial`, not `proposed`). Packages are ryucad's (question 850).
pub fn listed(entry: &Entry, releases: &Releases, has_repo: bool) -> Value {
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
                    "name": releases.name_of(&entry.part, &entry.label, entry.kind),
                    "status": t.as_str()});
                if let Tag::Moved(to) = t {
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

/// `release.sync`: the tag of every official release, created where
/// missing and moved where it points elsewhere. A tag no official release
/// accounts for is reported, never deleted. Fails as a whole on the first
/// ref that can't be written, saying which were. `product` names the
/// reflog's message (`<product> release sync`).
pub fn sync(repo: &gix::Repository, releases: &Releases, product: &str) -> Result<Value> {
    use gix::refs::transaction::{PreviousValue, RefEdit};
    crate::history::check_folder(&releases.prefix)?;
    let mut created = Vec::new();
    let mut left = Vec::new();
    let mut moved = Vec::new();
    let mut unchanged = 0;
    let message = format!("{product} release sync");
    for (path, o) in &releases.official {
        let mut seg = path.split('/').skip(1);
        let (Some(part), Some(file)) = (seg.next(), seg.next()) else {
            continue;
        };
        let (Ok(part), Some((label, Kind::Release))) =
            (Id::parse(part, "part"), crate::record::parse_name(file))
        else {
            continue;
        };
        let name = releases.name_of(&part, &label, Kind::Release);
        // Under rules the tag is the service's, annotated and signed.
        if releases.approvals.contains_key(path) {
            left.push(json!({"tag": name, "path": path}));
            continue;
        }
        let expected = match releases.tag(&part, &label, &o.commit) {
            Tag::Ok => {
                unchanged += 1;
                continue;
            }
            Tag::Missing => PreviousValue::MustNotExist,
            Tag::Moved(_) => PreviousValue::MustExist,
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
        repo.edit_reference(RefEdit::update(full, id, expected, message.as_str()))
            .map_err(|e| failed(e.to_string()))?;
        match move_from {
            Some(from) => moved.push(json!({"tag": name, "from": from, "to": o.commit})),
            None => created.push(json!({"tag": name, "at": o.commit})),
        }
    }
    let orphaned: Vec<String> = releases
        .orphaned_tags()
        .into_iter()
        .map(|k| releases.name(k))
        .collect();
    Ok(
        json!({"created": created, "moved": moved, "unchanged": unchanged,
              "orphaned": orphaned, "left_to_service": left}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::{commit, git, put};
    use crate::testing::{Toy, id, project, record};

    fn toy(dir: &std::path::Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        for (p, b) in project(&[("a", "{}")]) {
            put(dir, &p, std::str::from_utf8(&b).unwrap());
        }
        commit(dir, "start");
    }

    fn read(dir: &std::path::Path) -> (gix::Repository, Records, Releases, Config) {
        let repo = gix::open(dir).unwrap();
        let src = crate::testing::folder(dir);
        let config = Config::read_in(&src, "toy").unwrap();
        let records = Records::read_in(&src, "toy").unwrap();
        let releases = Releases::read(&repo, "", &config).unwrap();
        (repo, records, releases, config)
    }

    #[test]
    fn the_first_release_is_a_then_pending_then_official_and_tagged() {
        let d = tempfile::tempdir().unwrap();
        toy(d.path());
        let src = crate::testing::folder(d.path());
        let (_, records, releases, config) = read(d.path());
        let n = next_for::<Toy>(&src, &config, &records, &releases, &id("a")).unwrap();
        assert_eq!((n.label.as_str(), n.supersedes), ("A", None));
        let content = <Toy as Product>::content(&src, &id("a")).unwrap();
        put(
            d.path(),
            "releases/a/A.json",
            std::str::from_utf8(&record("a", "A", &content)).unwrap(),
        );
        let (_, records, _, config) = read(d.path());
        // Not on the release branch yet: pending.
        let e =
            next_for::<Toy>(&src, &config, &records, &Releases::default(), &id("a")).unwrap_err();
        assert_eq!(e.code, Code::ReleasePending);
        commit(d.path(), "A");
        let (repo, records, releases, config) = read(d.path());
        assert_eq!(
            next_for::<Toy>(&src, &config, &records, &releases, &id("a"))
                .unwrap_err()
                .code,
            Code::NothingToRelease
        );
        let v = sync(&repo, &releases, "toy").unwrap();
        assert_eq!(v["created"][0]["tag"], "ryupdm/a/A");
        let (_, _, releases, _) = read(d.path());
        assert_eq!(
            sync(&gix::open(d.path()).unwrap(), &releases, "toy").unwrap()["unchanged"],
            1
        );
        let listed = listed(&records.entries[0], &releases, true);
        assert_eq!(
            (listed["status"].as_str(), listed["tag"]["status"].as_str()),
            (Some("official"), Some("ok"))
        );
        obsolete_target(&config, &records, &releases, &id("a"), "A").unwrap();
        assert_eq!(
            obsolete_target(&config, &records, &releases, &id("a"), "B")
                .unwrap_err()
                .code,
            Code::InvalidParam
        );
    }

    #[test]
    fn github_urls_name_their_owner() {
        use crate::approvals::github_repo;
        assert_eq!(
            github_repo("git@github.com:o/r.git"),
            Some(("o".into(), "r".into()))
        );
        assert_eq!(github_repo("https://example.com/o/r"), None);
    }
}
