//! When release records became official, read from Git. A record is
//! official once the release branch holds it: `origin/<branch>` when that
//! remote-tracking branch exists, else the local `<branch>`. Nothing here
//! fetches, so official means as of the last fetch.
//!
//! The branch's first-parent chain is walked once, comparing each commit's
//! `releases` tree with its parent's and reading only those that differ.
//! A record's official commit is the oldest commit of the unbroken run of
//! commits, ending at the tip, that hold its path: with pull requests,
//! the merge or squash commit that brought it in. A record whose bytes
//! change after that, or that later leaves the branch, was modified.
//!
//! Tags `ryupdm/<part>/<label>` are read too: they are derived from the
//! records, so they are only compared, never trusted.
//!
//! Under approval rules (`approvals` in `ryupdm.json` at the official
//! commit's first parent) a record's tag is the hosted service's, an
//! annotated tag whose message attests who approved it: those are read
//! and verified ([`crate::attest`]), offline, once per tag object.

use crate::approvals::{Approvals, Rule};
use crate::attest::{self, Verdict};
use crate::config::Config;
use crate::record::{Kind, parse_name};
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::{PDM_CONFIG, RELEASES, ROOT_FILES};
use ryucad_doc::error::{Code, Error, Result};
use std::collections::BTreeMap;

/// The prefix of every tag ryupdm derives from a record.
pub const TAG_PREFIX: &str = "ryupdm/";

pub(crate) fn git_error(e: impl std::fmt::Display) -> Error {
    Error::new(Code::Io, format!("reading the Git repository: {e}"))
}

/// The branch whose history makes records official.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// As people name it: `origin/main`, or `main` without a remote.
    pub name: String,
    /// Its tip, in hex.
    pub tip: String,
}

/// Where a record became official.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Official {
    /// The commit, in hex.
    pub commit: String,
    /// The record's bytes there.
    pub blob: gix::ObjectId,
    /// The commit is the oldest a shallow clone holds, so the record may
    /// have been official earlier.
    pub shallow: bool,
}

/// An official record some rule applied to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    /// The rules that applied, read at `rules_at`.
    pub rules: Vec<Rule>,
    /// The official commit's first parent, in hex.
    pub rules_at: String,
    /// How its tag stands; `None` when the tag is at another commit
    /// (`release_tag_moved`), whose attestation isn't judged.
    pub verdict: Option<Verdict>,
}

/// An official record changed or deleted later on the release branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Modified {
    /// Where it became official.
    pub official: String,
    /// The first commit that changed or deleted it.
    pub at: String,
    pub deleted: bool,
}

/// What the release branch and the tags say about a project's records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Releases {
    /// `None` when neither `origin/<branch>` nor `<branch>` exists: then
    /// nothing is official.
    pub branch: Option<Branch>,
    /// The records the branch's tip holds, by path relative to the
    /// project, with where each became official.
    pub official: BTreeMap<String, Official>,
    /// Official records changed or deleted since, by path.
    pub modified: BTreeMap<String, Modified>,
    /// Every `ryupdm/<part>/<label>` tag, by `<part>/<label>`, with the
    /// commit it points at (`None` if it doesn't point at a commit).
    pub tags: BTreeMap<String, Option<String>>,
    /// Every `ryupdm/` tag's own object, by `<part>/<label>`: the tag
    /// object of an annotated tag, the commit of a lightweight one.
    pub objects: BTreeMap<String, gix::ObjectId>,
    /// Official records some rule applied to where they arrived, by path.
    pub approvals: BTreeMap<String, Approval>,
    /// `approvals` in `ryupdm.json` at the branch's tip: what a proposal
    /// will be judged by.
    pub tip_approvals: Option<Approvals>,
    /// The walk reached the end of a shallow clone's history.
    pub shallow: bool,
}

/// How a release's tag stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tag {
    /// It points at the record's official commit.
    Ok,
    Missing,
    /// It points elsewhere (`None`: at something that isn't a commit).
    Moved(Option<String>),
}

impl Tag {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tag::Ok => "ok",
            Tag::Missing => "missing",
            Tag::Moved(_) => "moved",
        }
    }
}

/// The tag a release of `part`'s `label` gets, without `refs/tags/`.
pub fn tag_name(part: &Id, label: &str) -> String {
    format!("{TAG_PREFIX}{part}/{label}")
}

/// The key of a record's tag below `ryupdm/`: `<part>/<label>`, or
/// `<part>/<label>.obsolete` for an obsolete record (tagged only under
/// rules).
pub fn tag_key(part: &Id, label: &str, kind: Kind) -> String {
    match kind {
        Kind::Release => format!("{part}/{label}"),
        Kind::Obsolete => format!("{part}/{label}.obsolete"),
    }
}

/// The tag of a record of either kind, without `refs/tags/`.
pub fn tag_name_of(part: &Id, label: &str, kind: Kind) -> String {
    format!("{TAG_PREFIX}{}", tag_key(part, label, kind))
}

/// The part, label and kind of the record at `path`
/// (`releases/<part>/<file>`).
pub fn record_at(path: &str) -> Option<(Id, String, Kind)> {
    let mut seg = path.split('/').skip(1);
    let (part, file) = (seg.next()?, seg.next()?);
    let (label, kind) = parse_name(file)?;
    Some((Id::parse(part, "part").ok()?, label, kind))
}

/// The project folder's entry `path` (relative to the project, below
/// `prefix`) in the tree `tree`.
pub(crate) fn entry(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    prefix: &str,
    path: &str,
) -> Result<Option<gix::ObjectId>> {
    let full = format!("{prefix}{path}");
    let full = full.trim_end_matches('/');
    if full.is_empty() {
        return Ok(Some(tree));
    }
    let tree = repo
        .find_object(tree)
        .map_err(git_error)?
        .try_into_tree()
        .map_err(git_error)?;
    Ok(tree
        .lookup_entry(full.split('/'))
        .map_err(git_error)?
        .map(|e| e.object_id()))
}

/// Every blob under the tree `id`, by path below it starting with `at`.
pub(crate) fn blobs(
    repo: &gix::Repository,
    id: gix::ObjectId,
    at: &str,
    out: &mut BTreeMap<String, gix::ObjectId>,
) -> Result<()> {
    let obj = repo.find_object(id).map_err(git_error)?;
    let Ok(tree) = obj.try_into_tree() else {
        return Ok(());
    };
    let decoded = tree.decode().map_err(git_error)?;
    for e in &decoded.entries {
        let path = format!("{at}{}", e.filename);
        if e.mode.is_tree() {
            blobs(repo, e.oid.to_owned(), &format!("{path}/"), out)?;
        } else if e.mode.is_blob() {
            out.insert(path, e.oid.to_owned());
        }
    }
    Ok(())
}

/// The records under the `releases` tree `id`, by path relative to the
/// project: files named as records, in folders named as parts.
fn records(repo: &gix::Repository, id: gix::ObjectId) -> Result<BTreeMap<String, gix::ObjectId>> {
    let mut all = BTreeMap::new();
    blobs(repo, id, &format!("{RELEASES}/"), &mut all)?;
    all.retain(|path, _| {
        let mut seg = path.split('/').skip(1);
        match (seg.next(), seg.next(), seg.next()) {
            (Some(part), Some(file), None) => {
                Id::parse(part, "part").is_ok() && parse_name(file).is_some()
            }
            _ => false,
        }
    });
    Ok(all)
}

/// The records the commit `id` holds for the project at `prefix`, by path
/// relative to the project.
pub fn records_at(
    repo: &gix::Repository,
    id: &str,
    prefix: &str,
) -> Result<BTreeMap<String, gix::ObjectId>> {
    let commit = repo.find_commit(commit_of(repo, id)?).map_err(git_error)?;
    let tree = commit.tree_id().map_err(git_error)?.detach();
    match entry(repo, tree, prefix, RELEASES)? {
        Some(t) => records(repo, t),
        None => Ok(BTreeMap::new()),
    }
}

/// The commit `rev` names, in hex.
fn commit_of(repo: &gix::Repository, rev: &str) -> Result<gix::ObjectId> {
    let id = repo.rev_parse_single(rev).map_err(|e| {
        Error::new(Code::RevNotFound, format!("no revision {rev}: {e}")).with("rev", rev)
    })?;
    Ok(id
        .object()
        .map_err(git_error)?
        .peel_to_commit()
        .map_err(git_error)?
        .id)
}

/// The project's files in the commit `rev` names, by path relative to
/// the project (`prefix` is where it sits in the repository, ending in
/// `/` unless it is the root): the manifest and the other files at its
/// root, and everything under `parts/` and `releases/`. Works on a bare
/// repository.
pub fn files(repo: &gix::Repository, rev: &str, prefix: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    let commit = repo.find_commit(commit_of(repo, rev)?).map_err(git_error)?;
    let tree = commit.tree_id().map_err(git_error)?.detach();
    let mut ids = BTreeMap::new();
    for name in ROOT_FILES {
        if let Some(id) = entry(repo, tree, prefix, name)? {
            ids.insert(name.to_owned(), id);
        }
    }
    for dir in ["parts", RELEASES] {
        if let Some(id) = entry(repo, tree, prefix, dir)? {
            blobs(repo, id, &format!("{dir}/"), &mut ids)?;
        }
    }
    ids.into_iter()
        .map(|(path, id)| {
            let data = repo.find_object(id).map_err(git_error)?.data.clone();
            Ok((path, data))
        })
        .collect()
}

/// The files of the project's part `part` in the commit `rev` names,
/// with the files at the project's root (the manifest and ryupdm's
/// settings): what reading that one part needs, without the rest.
pub fn part_files(
    repo: &gix::Repository,
    rev: &str,
    prefix: &str,
    part: &Id,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let commit = repo.find_commit(commit_of(repo, rev)?).map_err(git_error)?;
    let tree = commit.tree_id().map_err(git_error)?.detach();
    let mut ids = BTreeMap::new();
    for name in ROOT_FILES {
        if let Some(id) = entry(repo, tree, prefix, name)? {
            ids.insert(name.to_owned(), id);
        }
    }
    let dir = format!("parts/{part}");
    if let Some(id) = entry(repo, tree, prefix, &dir)? {
        blobs(repo, id, &format!("{dir}/"), &mut ids)?;
    }
    ids.into_iter()
        .map(|(path, id)| {
            let data = repo.find_object(id).map_err(git_error)?.data.clone();
            Ok((path, data))
        })
        .collect()
}

/// The release branch: `origin/<name>` if the repository has it, else the
/// local branch, else `None`.
pub fn branch(repo: &gix::Repository, name: &str) -> Result<Option<Branch>> {
    for (full, shown) in [
        (
            format!("refs/remotes/origin/{name}"),
            format!("origin/{name}"),
        ),
        (format!("refs/heads/{name}"), name.to_owned()),
    ] {
        let Some(mut r) = repo.try_find_reference(full.as_str()).map_err(git_error)? else {
            continue;
        };
        let tip = r.peel_to_commit().map_err(git_error)?.id;
        return Ok(Some(Branch {
            name: shown,
            tip: tip.to_string(),
        }));
    }
    Ok(None)
}

/// Each `ryupdm/` tag's commit and own object, by `<part>/<label>`.
pub type TagRefs = BTreeMap<String, (Option<String>, Option<gix::ObjectId>)>;

/// Every `ryupdm/` tag, by `<part>/<label>`: the commit it points at,
/// and its own object.
pub fn tag_refs(repo: &gix::Repository) -> Result<TagRefs> {
    let mut out = BTreeMap::new();
    let refs = repo.references().map_err(git_error)?;
    let full_prefix = format!("refs/tags/{TAG_PREFIX}");
    for r in refs.prefixed(full_prefix.as_str()).map_err(git_error)? {
        let mut r = r.map_err(git_error)?;
        let name = r.name().as_bstr().to_string();
        let Some(key) = name.strip_prefix(&full_prefix) else {
            continue;
        };
        let object = r.target().try_id().map(|id| id.to_owned());
        let commit = r.peel_to_commit().ok().map(|c| c.id.to_string());
        out.insert(key.to_owned(), (commit, object));
    }
    Ok(out)
}

/// Every `ryupdm/` tag, by `<part>/<label>`, with the commit it points at.
pub fn tags(repo: &gix::Repository) -> Result<BTreeMap<String, Option<String>>> {
    Ok(tag_refs(repo)?
        .into_iter()
        .map(|(k, (c, _))| (k, c))
        .collect())
}

/// `ryupdm.json` at the commit `commit` (hex), for the project at
/// `prefix`: the defaults when it has none.
pub fn config_at(repo: &gix::Repository, commit: &str, prefix: &str) -> Result<Config> {
    let c = repo
        .find_commit(commit_of(repo, commit)?)
        .map_err(git_error)?;
    let tree = c.tree_id().map_err(git_error)?.detach();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if let Some(id) = entry(repo, tree, prefix, PDM_CONFIG)? {
        files.insert(
            PDM_CONFIG.to_owned(),
            repo.find_object(id).map_err(git_error)?.data.clone(),
        );
    }
    Config::read(&files)
}

/// The verdict on the tag object `object` for what `expect` says.
fn verdict(repo: &gix::Repository, object: gix::ObjectId, expect: &attest::Expect) -> Verdict {
    let Ok(obj) = repo.find_object(object) else {
        return Verdict::Invalid(attest::Clause::Unreadable(
            "the tag's object is missing".into(),
        ));
    };
    if obj.kind != gix::object::Kind::Tag {
        return Verdict::Invalid(attest::Clause::Lightweight);
    }
    // A tag object is its headers, a blank line, then the message.
    let data = &obj.data;
    match data.windows(2).position(|w| w == b"\n\n") {
        None => Verdict::Invalid(attest::Clause::NoAttestation),
        Some(at) => attest::verify_message(&data[at + 2..], expect),
    }
}

/// What a verdict depends on: the tag's object, and the record's path,
/// bytes and official commit (which fix the rules and keys).
type MemoKey = (gix::ObjectId, String, gix::ObjectId, String);

/// Verdicts already reached, so a tip that moves verifies nothing again.
fn memo() -> &'static std::sync::Mutex<BTreeMap<MemoKey, Verdict>> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<BTreeMap<MemoKey, Verdict>>> =
        std::sync::OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// The official records some rule applied to where they arrived, with
/// their tags' verdicts.
fn judged(
    repo: &gix::Repository,
    prefix: &str,
    official: &BTreeMap<String, Official>,
    tags: &TagRefs,
) -> Result<BTreeMap<String, Approval>> {
    let mut configs: BTreeMap<String, Option<Config>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (path, o) in official {
        let Some((part, label, kind)) = record_at(path) else {
            continue;
        };
        let id = gix::ObjectId::from_hex(o.commit.as_bytes()).map_err(git_error)?;
        let commit = repo.find_commit(id).map_err(git_error)?;
        let Some(parent) = commit.parent_ids().next().map(|p| p.detach()) else {
            continue;
        };
        // A shallow clone may lack the parent: its rules can't be read.
        if !repo.has_object(parent) {
            continue;
        }
        let parent = parent.to_string();
        // Settings that don't read there judge nothing (doctor's, then).
        let config = configs
            .entry(parent.clone())
            .or_insert_with(|| config_at(repo, &parent, prefix).ok());
        let Some(approvals) = config.as_ref().and_then(|c| c.approvals.as_ref()) else {
            continue;
        };
        let rules = approvals.applicable(&part);
        if rules.is_empty() {
            continue;
        }
        let bytes = repo.find_object(o.blob).map_err(git_error)?.data.clone();
        // A record that doesn't read is doctor's; it names no project here.
        let Some(project) = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|t| ryucad_doc::doc::canonical::parse(t).ok())
            .and_then(|v| crate::record::Record::from_json(&v, kind).ok())
            .map(|r| r.project().clone())
        else {
            continue;
        };
        let key = tag_key(&part, &label, kind);
        let verdict = match tags.get(&key) {
            None => Some(Verdict::Missing),
            Some((Some(c), Some(object))) if *c == o.commit => {
                let memo_key = (*object, path.clone(), o.blob, o.commit.clone());
                let known = memo().lock().expect("the memo").get(&memo_key).cloned();
                Some(match known {
                    Some(v) => v,
                    None => {
                        let keys = attest::trusted(&approvals.keys);
                        let record = blake3::hash(&bytes).to_hex().to_string();
                        let v = verdict(
                            repo,
                            *object,
                            &attest::Expect {
                                project: &project,
                                part: &part,
                                label: &label,
                                kind,
                                commit: &o.commit,
                                record: &record,
                                rules: &rules,
                                rules_at: &parent,
                                keys: &keys,
                            },
                        );
                        let mut m = memo().lock().expect("the memo");
                        if m.len() >= 4096 {
                            m.clear();
                        }
                        m.insert(memo_key, v.clone());
                        v
                    }
                })
            }
            // Elsewhere, or at something that isn't a commit.
            Some(_) => None,
        };
        out.insert(
            path.clone(),
            Approval {
                rules,
                rules_at: parent,
                verdict,
            },
        );
    }
    Ok(out)
}

/// One record's unbroken run of commits on the chain, oldest first.
struct Run {
    /// Where it started, an index into the chain.
    start: usize,
    /// Its bytes there.
    first: gix::ObjectId,
    /// Its bytes at the last commit read.
    last: gix::ObjectId,
    /// The first commit after `start` that changed it.
    changed: Option<usize>,
}

impl Releases {
    /// Reads the release branch `config` names and the tags, for the
    /// project at `prefix` in the repository (ending in `/` unless it is
    /// the root).
    pub fn read(repo: &gix::Repository, prefix: &str, config: &Config) -> Result<Releases> {
        let refs = tag_refs(repo)?;
        let tags: BTreeMap<String, Option<String>> = refs
            .iter()
            .map(|(k, (c, _))| (k.clone(), c.clone()))
            .collect();
        let objects: BTreeMap<String, gix::ObjectId> = refs
            .iter()
            .filter_map(|(k, (_, o))| Some((k.clone(), (*o)?)))
            .collect();
        let Some(branch) = branch(repo, &config.release_branch)? else {
            return Ok(Releases {
                tags,
                objects,
                ..Releases::default()
            });
        };
        // The first-parent chain, newest first, each commit with its
        // `releases` tree.
        let mut chain: Vec<(gix::ObjectId, Option<gix::ObjectId>)> = Vec::new();
        let mut next = Some(gix::ObjectId::from_hex(branch.tip.as_bytes()).map_err(git_error)?);
        let mut shallow = false;
        while let Some(id) = next {
            let commit = repo.find_commit(id).map_err(git_error)?;
            let tree = commit.tree_id().map_err(git_error)?.detach();
            chain.push((id, entry(repo, tree, prefix, RELEASES)?));
            next = commit.parent_ids().next().map(|p| p.detach());
            // A shallow clone's oldest commits name parents it lacks.
            if next.is_some_and(|p| !repo.has_object(p)) {
                shallow = true;
                next = None;
            }
        }
        chain.reverse();
        let mut runs: BTreeMap<String, Run> = BTreeMap::new();
        let mut ended: BTreeMap<String, (Run, usize)> = BTreeMap::new();
        let mut seen: Option<Option<gix::ObjectId>> = None;
        for (i, (_, tree)) in chain.iter().enumerate() {
            if seen == Some(*tree) {
                continue;
            }
            seen = Some(*tree);
            let now = match tree {
                Some(t) => records(repo, *t)?,
                None => BTreeMap::new(),
            };
            let gone: Vec<String> = runs
                .keys()
                .filter(|p| !now.contains_key(*p))
                .cloned()
                .collect();
            for path in gone {
                let run = runs.remove(&path).expect("a run");
                ended.insert(path, (run, i));
            }
            for (path, blob) in now {
                match runs.get_mut(&path) {
                    Some(run) => {
                        if run.last != blob && run.changed.is_none() {
                            run.changed = Some(i);
                        }
                        run.last = blob;
                    }
                    None => {
                        ended.remove(&path);
                        runs.insert(
                            path,
                            Run {
                                start: i,
                                first: blob,
                                last: blob,
                                changed: None,
                            },
                        );
                    }
                }
            }
        }
        let hex = |i: usize| chain[i].0.to_string();
        let mut official = BTreeMap::new();
        let mut modified = BTreeMap::new();
        for (path, run) in &runs {
            official.insert(
                path.clone(),
                Official {
                    commit: hex(run.start),
                    blob: run.first,
                    shallow: shallow && run.start == 0,
                },
            );
            if let Some(at) = run.changed {
                modified.insert(
                    path.clone(),
                    Modified {
                        official: hex(run.start),
                        at: hex(at),
                        deleted: false,
                    },
                );
            }
        }
        for (path, (run, end)) in ended {
            modified.insert(
                path,
                Modified {
                    official: hex(run.start),
                    at: hex(end),
                    deleted: true,
                },
            );
        }
        let approvals = judged(repo, prefix, &official, &refs)?;
        let tip_approvals = config_at(repo, &branch.tip, prefix)
            .ok()
            .and_then(|c| c.approvals);
        Ok(Releases {
            branch: Some(branch),
            official,
            modified,
            tags,
            objects,
            approvals,
            tip_approvals,
            shallow,
        })
    }

    /// How the tag of the release of `part`'s `label`, official at
    /// `commit`, stands.
    pub fn tag(&self, part: &Id, label: &str, commit: &str) -> Tag {
        self.tag_of(part, label, Kind::Release, commit)
    }

    /// How the tag of a record of either kind, official at `commit`,
    /// stands.
    pub fn tag_of(&self, part: &Id, label: &str, kind: Kind, commit: &str) -> Tag {
        match self.tags.get(&tag_key(part, label, kind)) {
            None => Tag::Missing,
            Some(Some(c)) if c == commit => Tag::Ok,
            Some(other) => Tag::Moved(other.clone()),
        }
    }

    /// Tags no official release record accounts for, by `<part>/<label>`.
    pub fn orphaned_tags(&self) -> Vec<&str> {
        self.tags
            .keys()
            .filter(|key| {
                let path = format!("{RELEASES}/{key}.json");
                !self.official.contains_key(&path)
            })
            .map(String::as_str)
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    /// Runs git in `dir` with a fixed identity, and returns what it printed.
    pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Writes `text` at `rel` under `dir`.
    pub(crate) fn put(dir: &Path, rel: &str, text: &str) {
        let path = rel.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Commits everything in `dir` with `message`; returns the commit.
    pub(crate) fn commit(dir: &Path, message: &str) -> String {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
        git(dir, &["rev-parse", "HEAD"])
    }

    /// A repository with one commit on `main`.
    pub(crate) fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        put(dir.path(), "ryucad.json", "{}\n");
        commit(dir.path(), "start");
        dir
    }

    fn read(dir: &Path) -> Releases {
        let repo = gix::open(dir).unwrap();
        Releases::read(&repo, "", &Config::default()).unwrap()
    }

    const A: &str = "releases/p/A.json";

    #[test]
    fn no_branch_no_releases() {
        let dir = repo();
        put(dir.path(), A, "a");
        commit(dir.path(), "propose A");
        let repo = gix::open(dir.path()).unwrap();
        let config = Config {
            release_branch: "release".into(),
            ..Config::default()
        };
        let r = Releases::read(&repo, "", &config).unwrap();
        assert_eq!(r.branch, None);
        assert!(r.official.is_empty());
    }

    #[test]
    fn a_record_is_official_from_the_commit_that_brings_it() {
        let dir = repo();
        let d = dir.path();
        let first = commit(d, "nothing yet");
        let c = {
            put(d, A, "a");
            commit(d, "release A")
        };
        commit(d, "more work");
        let r = read(d);
        assert_eq!(r.branch.as_ref().unwrap().name, "main");
        assert_eq!(r.official[A].commit, c);
        assert_ne!(c, first);
        assert!(!r.official[A].shallow && !r.shallow);
        assert!(r.modified.is_empty());
        // Only records count.
        put(d, "releases/p/notes.txt", "x");
        put(d, "releases/p/deeper/B.json", "x");
        commit(d, "strays");
        assert_eq!(read(d).official.keys().collect::<Vec<_>>(), [A]);
    }

    #[test]
    fn a_record_changed_or_deleted_later_was_modified() {
        let dir = repo();
        let d = dir.path();
        put(d, A, "a");
        let official = commit(d, "release A");
        put(d, A, "a, edited");
        let edit = commit(d, "edit A");
        put(d, A, "a, edited twice");
        commit(d, "edit A again");
        let r = read(d);
        assert_eq!(r.official[A].commit, official);
        assert_eq!(
            r.modified[A],
            Modified {
                official: official.clone(),
                at: edit,
                deleted: false
            }
        );
        std::fs::remove_file(d.join(A)).unwrap();
        let gone = commit(d, "delete A");
        let r = read(d);
        assert!(!r.official.contains_key(A));
        assert_eq!(
            r.modified[A],
            Modified {
                official,
                at: gone,
                deleted: true
            }
        );
    }

    #[test]
    fn merged_squashed_or_fast_forwarded_it_is_official_where_main_gets_it() {
        for how in ["merge", "squash", "ff"] {
            let dir = repo();
            let d = dir.path();
            git(d, &["switch", "-q", "-c", "propose"]);
            put(d, A, "a");
            let proposed = commit(d, "propose A");
            git(d, &["switch", "-q", "main"]);
            // Before it reaches main, it is only proposed.
            assert!(read(d).official.is_empty(), "{how}");
            let at = match how {
                "merge" => {
                    commit(d, "main moves on");
                    git(d, &["merge", "-q", "--no-ff", "-m", "merge", "propose"]);
                    git(d, &["rev-parse", "HEAD"])
                }
                "squash" => {
                    git(d, &["merge", "-q", "--squash", "propose"]);
                    commit(d, "squashed")
                }
                _ => {
                    git(d, &["merge", "-q", "--ff-only", "propose"]);
                    proposed.clone()
                }
            };
            let r = read(d);
            assert_eq!(r.official[A].commit, at, "{how}");
            assert_eq!(how == "ff", at == proposed);
        }
    }

    #[test]
    fn origin_decides_when_there_is_one() {
        let start = repo();
        let up = tempfile::tempdir().unwrap();
        git(
            up.path(),
            &["clone", "-q", "--bare", start.path().to_str().unwrap(), "."],
        );
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git(d, &["clone", "-q", up.path().to_str().unwrap(), "."]);
        put(d, A, "a");
        commit(d, "release A, not pushed");
        // The local main holds it; origin/main doesn't, so it isn't
        // official.
        let r = read(d);
        assert_eq!(r.branch.as_ref().unwrap().name, "origin/main");
        assert!(r.official.is_empty());
        git(d, &["push", "-q", "origin", "main"]);
        git(d, &["fetch", "-q"]);
        let r = read(d);
        assert_eq!(r.official[A].commit, git(d, &["rev-parse", "HEAD"]));
    }

    #[test]
    fn rewritten_history_moves_the_official_commit() {
        let dir = repo();
        let d = dir.path();
        put(d, A, "a");
        let before = commit(d, "release A");
        git(d, &["commit", "-q", "--amend", "-m", "release A, reworded"]);
        let after = git(d, &["rev-parse", "HEAD"]);
        assert_ne!(before, after);
        assert_eq!(read(d).official[A].commit, after);
    }

    #[test]
    fn tags_are_compared_with_official_commits() {
        let dir = repo();
        let d = dir.path();
        put(d, A, "a");
        let a = commit(d, "release A");
        let later = commit(d, "later");
        let r = read(d);
        let p = Id::parse("p", "part").unwrap();
        assert_eq!(tag_name(&p, "A"), "ryupdm/p/A");
        assert_eq!(r.tag(&p, "A", &a), Tag::Missing);
        git(d, &["tag", "ryupdm/p/A", &a]);
        git(d, &["tag", "ryupdm/p/Z", &a]);
        git(d, &["tag", "-a", "-m", "annotated", "ryupdm/q/A", &later]);
        git(d, &["tag", "other", &a]);
        let r = read(d);
        assert_eq!(r.tag(&p, "A", &a), Tag::Ok);
        assert_eq!(r.tag(&p, "A", &later), Tag::Moved(Some(a.clone())));
        // An annotated tag counts by the commit it names.
        assert_eq!(r.tags["q/A"], Some(later));
        assert_eq!(r.orphaned_tags(), ["p/Z", "q/A"]);
    }

    #[test]
    fn a_shallow_clone_says_how_far_it_sees() {
        let up = repo();
        let u = up.path();
        put(u, A, "a");
        commit(u, "release A");
        commit(u, "later");
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let url = format!("file://{}", u.display());
        git(d, &["clone", "-q", "--depth", "1", &url, "."]);
        let r = read(d);
        assert!(r.shallow);
        let tip = git(d, &["rev-parse", "HEAD"]);
        assert_eq!(
            r.official[A],
            Official {
                commit: tip,
                blob: r.official[A].blob,
                shallow: true
            }
        );
    }

    #[test]
    fn a_project_below_the_root_and_a_bare_repository() {
        let dir = repo();
        let d = dir.path();
        put(d, "cad/ryucad.json", "{}\n");
        put(d, "cad/parts/p/part.json", "{}\n");
        put(d, &format!("cad/{A}"), "a");
        put(d, "cad/elsewhere.txt", "x");
        put(d, A, "not the project's");
        let c = commit(d, "release A below");
        let bare = tempfile::tempdir().unwrap();
        git(
            bare.path(),
            &["clone", "-q", "--bare", d.to_str().unwrap(), "."],
        );
        let repo = gix::open(bare.path()).unwrap();
        let r = Releases::read(&repo, "cad/", &Config::default()).unwrap();
        assert_eq!(r.official[A].commit, c);
        let f = files(&repo, "main", "cad/").unwrap();
        assert_eq!(
            f.keys().collect::<Vec<_>>(),
            ["parts/p/part.json", A, "ryucad.json"]
        );
        assert_eq!(f[A], b"a");
        let e = files(&repo, "nowhere", "cad/").unwrap_err();
        assert_eq!(e.code, Code::RevNotFound);
    }

    use crate::attest::{self, Attestation, Outcome, Pull, TEST_SEED, User};
    use crate::record::Kind;

    const PROJECT: &str = "0123456789abcdef0123456789abcdef";

    /// `ryupdm.json` with one rule over every part (1 of alice, bob)
    /// trusting the test key.
    fn ruled(d: &Path) {
        let public = attest::public_key(&TEST_SEED);
        put(
            d,
            "ryupdm.json",
            &ryucad_doc::doc::canonical::to_string(&serde_json::json!({
                "approvals": {
                    "keys": {attest::test_key_id(): attest::hex(&public)},
                    "rules": [{"count": 1, "from": ["alice", "bob"], "parts": ["*"]}]
                },
                "format": 2, "kind": "ryupdm.config"
            })),
        );
    }

    /// The attestation the service would sign for `path` (record bytes
    /// `bytes`) made official at `commit`.
    pub(crate) fn attestation(
        d: &Path,
        part: &str,
        label: &str,
        kind: Kind,
        bytes: &[u8],
        commit: &str,
    ) -> Attestation {
        Attestation {
            approvals: vec![attest::Approval {
                at: "2026-11-02T09:14:03Z".into(),
                commit: commit.into(),
                github: User {
                    id: 1,
                    login: "alice".into(),
                },
                review: 9,
            }],
            commit: commit.into(),
            label: label.into(),
            missing: vec![],
            outcome: Outcome::Approved,
            part: Id::parse(part, "part").unwrap(),
            project: ryucad_doc::doc::id::ProjectId::parse(PROJECT).unwrap(),
            proposed_by: User {
                id: 2,
                login: "dana".into(),
            },
            pull: Some(Pull {
                number: 1,
                repo: "acme/widgets".into(),
            }),
            record: blake3::hash(bytes).to_hex().to_string(),
            record_kind: kind,
            rules: vec![crate::approvals::Rule {
                parts: vec!["*".into()],
                count: 1,
                from: vec!["alice".into(), "bob".into()],
            }],
            rules_at: git(d, &["rev-parse", &format!("{commit}^")]),
            signed_at: "2026-11-02T09:20:11Z".into(),
        }
    }

    /// An annotated tag `name` at `commit` whose message is `message`
    /// exactly.
    pub(crate) fn tag(d: &Path, name: &str, commit: &str, message: &str) {
        let file = d.join(".git").join("TAG_MESSAGE_TEST");
        std::fs::write(&file, message).unwrap();
        git(
            d,
            &[
                "tag",
                "-a",
                "--cleanup=verbatim",
                "-F",
                file.to_str().unwrap(),
                name,
                commit,
            ],
        );
    }

    /// A record of `part`'s `label` that reads, as canonical text.
    pub(crate) fn record_text(part: &str, label: &str, kind: Kind) -> String {
        use crate::record::{BuiltWith, Obsolete, Record, Release, ReleasedBy};
        let project = ryucad_doc::doc::id::ProjectId::parse(PROJECT).unwrap();
        let part = Id::parse(part, "part").unwrap();
        let by = ReleasedBy {
            name: None,
            email: None,
            participant: "agent".into(),
        };
        let r = match kind {
            Kind::Release => Record::Release(Release {
                project,
                part,
                label: label.into(),
                content: "0".repeat(64),
                properties: BTreeMap::new(),
                results: BTreeMap::new(),
                built_with: BuiltWith {
                    ryucad: "0".into(),
                    ryukei: "0".into(),
                    commit: "c".into(),
                },
                released_by: by,
                supersedes: None,
                note: None,
            }),
            Kind::Obsolete => Record::Obsolete(Obsolete {
                project,
                part,
                label: label.into(),
                released_by: by,
                note: None,
            }),
        };
        ryucad_doc::doc::canonical::to_string(&r.to_json())
    }

    fn record(part: &str, label: &str) -> String {
        record_text(part, label, Kind::Release)
    }

    #[test]
    fn records_under_rules_are_judged_by_their_tags() {
        let dir = repo();
        let d = dir.path();
        ruled(d);
        commit(d, "rules");
        put(d, A, &record("p", "A"));
        let official = commit(d, "release A");
        let r = read(d);
        let a = &r.approvals[A];
        assert_eq!(a.rules_at, git(d, &["rev-parse", "HEAD^"]));
        assert_eq!(a.rules.len(), 1);
        assert_eq!(a.verdict, Some(attest::Verdict::Missing));
        // The service's tag: approved.
        let at = attestation(
            d,
            "p",
            "A",
            Kind::Release,
            record("p", "A").as_bytes(),
            &official,
        );
        tag(d, "ryupdm/p/A", &official, &attest::sign(&at, &TEST_SEED));
        let r = read(d);
        assert_eq!(
            r.approvals[A].verdict.as_ref().unwrap().status(),
            "approved"
        );
        assert!(r.objects.contains_key("p/A"));
        // A lightweight tag instead: invalid.
        git(d, &["tag", "-d", "ryupdm/p/A"]);
        git(d, &["tag", "ryupdm/p/A", &official]);
        assert_eq!(
            read(d).approvals[A].verdict,
            Some(attest::Verdict::Invalid(attest::Clause::Lightweight))
        );
        // A tag at another commit: its attestation isn't judged.
        let later = commit(d, "later");
        git(d, &["tag", "-f", "ryupdm/p/A", &later]);
        assert_eq!(read(d).approvals[A].verdict, None);
        // The tip's rules are kept for proposals.
        assert_eq!(read(d).tip_approvals.unwrap().rules.len(), 1);
    }

    #[test]
    fn rules_are_read_where_the_record_arrived() {
        let dir = repo();
        let d = dir.path();
        // Rules added in the same commit as the record don't apply to it.
        ruled(d);
        put(d, A, &record("p", "A"));
        commit(d, "rules and release A at once");
        assert!(read(d).approvals.is_empty());
        // A record with no rule matching it is judged as before.
        let mut v: serde_json::Value = ryucad_doc::doc::canonical::parse(
            &std::fs::read_to_string(d.join("ryupdm.json")).unwrap(),
        )
        .unwrap();
        v["approvals"]["rules"][0]["parts"] = serde_json::json!(["motor-*"]);
        put(d, "ryupdm.json", &ryucad_doc::doc::canonical::to_string(&v));
        commit(d, "rules for motors");
        put(d, "releases/p/B.json", &record("p", "B"));
        commit(d, "release B");
        assert!(!read(d).approvals.contains_key("releases/p/B.json"));
    }
}
