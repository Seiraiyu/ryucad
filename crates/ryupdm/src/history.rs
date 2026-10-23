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

use crate::config::Config;
use crate::record::parse_name;
use ryucad_doc::doc::id::Id;
use ryucad_doc::doc::project::{RELEASES, ROOT_FILES};
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

/// The project folder's entry `path` (relative to the project, below
/// `prefix`) in the tree `tree`.
fn entry(
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
fn blobs(
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

/// Every `ryupdm/` tag, by `<part>/<label>`, with the commit it points at.
pub fn tags(repo: &gix::Repository) -> Result<BTreeMap<String, Option<String>>> {
    let mut out = BTreeMap::new();
    let refs = repo.references().map_err(git_error)?;
    let full_prefix = format!("refs/tags/{TAG_PREFIX}");
    for r in refs.prefixed(full_prefix.as_str()).map_err(git_error)? {
        let mut r = r.map_err(git_error)?;
        let name = r.name().as_bstr().to_string();
        let Some(key) = name.strip_prefix(&full_prefix) else {
            continue;
        };
        let commit = r.peel_to_commit().ok().map(|c| c.id.to_string());
        out.insert(key.to_owned(), commit);
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
        let tags = tags(repo)?;
        let Some(branch) = branch(repo, &config.release_branch)? else {
            return Ok(Releases {
                tags,
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
        Ok(Releases {
            branch: Some(branch),
            official,
            modified,
            tags,
            shallow,
        })
    }

    /// How the tag of the release of `part`'s `label`, official at
    /// `commit`, stands.
    pub fn tag(&self, part: &Id, label: &str, commit: &str) -> Tag {
        match self.tags.get(&format!("{part}/{label}")) {
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
}
