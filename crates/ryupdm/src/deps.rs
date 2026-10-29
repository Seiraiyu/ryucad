//! Other projects' releases, fetched: the machine's dependency cache. Each
//! dependency's repository is a bare repository in the cache's folder,
//! one per URL, shared by every project on the machine that names it. A
//! fetch takes the remote's `HEAD`, the release branch of every project
//! asked for (named by its `ryupdm.json` at that `HEAD`, `main` by
//! default) and the `ryupdm/` tags, and nothing else; nothing is ever
//! pushed. Everything else here reads what was fetched, offline.

use crate::config::Config;
use crate::history::git_error;
use ryucad_doc::doc::project::PDM_CONFIG;
use ryucad_doc::error::{Code, Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Where the cache keeps what a fetch found at the remote's `HEAD`.
pub const HEAD_REF: &str = "refs/ryucad/head";

/// The machine's dependency cache: a folder of bare repositories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cache {
    pub dir: PathBuf,
}

/// What a fetch of one URL found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub url: String,
    /// The remote's `HEAD`, in hex.
    pub head: String,
    /// Each project folder asked for, with its release branch and that
    /// branch's tip (`None` when the remote has no such branch).
    pub branches: BTreeMap<String, (String, Option<String>)>,
    /// The branches' tips before the fetch, by branch, for those the cache
    /// already had.
    pub before: BTreeMap<String, String>,
}

/// The URL as the cache keys it: without a trailing `/` or `.git`, and
/// with an `http(s)://` URL's scheme and host in lower case, so the
/// spellings of one repository share its folder.
pub fn normalize(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/').to_owned();
    if let Some(s) = u.strip_suffix(".git") {
        u = s.to_owned();
    }
    for scheme in ["https://", "http://"] {
        if u.len() >= scheme.len() && u[..scheme.len()].eq_ignore_ascii_case(scheme) {
            let rest = &u[scheme.len()..];
            let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            u = format!("{scheme}{}{path}", host.to_ascii_lowercase());
        }
    }
    u
}

/// The folder name of `url`'s repository in the cache.
pub fn folder(url: &str) -> String {
    let h = blake3::hash(normalize(url).as_bytes());
    format!("{}.git", &h.to_hex()[..32])
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Cache {
        Cache { dir: dir.into() }
    }

    /// The bare repository `url`'s fetches go to.
    pub fn path(&self, url: &str) -> PathBuf {
        self.dir.join(folder(url))
    }

    /// `url`'s repository, `None` when nothing was fetched from it yet.
    pub fn open(&self, url: &str) -> Result<Option<gix::Repository>> {
        let path = self.path(url);
        if !path.join("HEAD").is_file() {
            return Ok(None);
        }
        gix::open(&path).map(Some).map_err(git_error)
    }

    /// Fetches `url`: its `HEAD`, the release branch of each project folder
    /// in `paths` (`""` for the repository's root) and the `ryupdm/` tags,
    /// incrementally, holding the repository's lock file so two daemons
    /// don't fetch into it at once. `dependency_fetch_failed` (exit 4) with
    /// the URL and Git's reason when the remote can't be read.
    pub fn fetch(&self, url: &str, paths: &[&str]) -> Result<Fetched> {
        std::fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, &e))?;
        let path = self.path(url);
        let lock_path = self.dir.join(format!("{}.lock", folder(url)));
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| Error::io(&lock_path, &e))?;
        lock.lock().map_err(|e| Error::io(&lock_path, &e))?;
        let r = self.fetch_locked(url, &path, paths);
        let _ = lock.unlock();
        r
    }

    fn fetch_locked(&self, url: &str, path: &Path, paths: &[&str]) -> Result<Fetched> {
        let repo = match self.open(url)? {
            Some(r) => r,
            None => gix::init_bare(path).map_err(git_error)?,
        };
        let mut branches: BTreeSet<String> = local_branches(&repo)?;
        let before: BTreeMap<String, String> = branches
            .iter()
            .filter_map(|b| Some((b.clone(), tip(&repo, b).ok()??)))
            .collect();
        let mut repo = fetch_into(repo, url, &branches)?;
        // The release branches the projects name at HEAD; fetched again
        // when one is new.
        let mut wanted = BTreeMap::new();
        for p in paths {
            let config = config_at(&repo, HEAD_REF, p)?;
            wanted.insert((*p).to_owned(), config.release_branch);
        }
        let new: BTreeSet<String> = wanted
            .values()
            .filter(|b| !branches.contains(*b))
            .cloned()
            .collect();
        if !new.is_empty() {
            branches.extend(new);
            repo = fetch_into(repo, url, &branches)?;
        }
        let head = tip_of(&repo, HEAD_REF)?.unwrap_or_default();
        let branches = wanted
            .into_iter()
            .map(|(p, b)| {
                let t = tip(&repo, &b)?;
                Ok((p, (b, t)))
            })
            .collect::<Result<_>>()?;
        Ok(Fetched {
            url: url.to_owned(),
            head,
            branches,
            before,
        })
    }
}

/// The branches the cache's repository holds.
fn local_branches(repo: &gix::Repository) -> Result<BTreeSet<String>> {
    let refs = repo.references().map_err(git_error)?;
    let mut out = BTreeSet::new();
    for r in refs.prefixed("refs/heads/").map_err(git_error)? {
        let r = r.map_err(git_error)?;
        let name = r.name().as_bstr().to_string();
        if let Some(b) = name.strip_prefix("refs/heads/") {
            out.insert(b.to_owned());
        }
    }
    Ok(out)
}

/// The tip of branch `b` in the cache's repository, in hex.
pub fn tip(repo: &gix::Repository, b: &str) -> Result<Option<String>> {
    tip_of(repo, &format!("refs/heads/{b}"))
}

fn tip_of(repo: &gix::Repository, full: &str) -> Result<Option<String>> {
    match repo.try_find_reference(full).map_err(git_error)? {
        None => Ok(None),
        Some(mut r) => Ok(Some(r.peel_to_commit().map_err(git_error)?.id.to_string())),
    }
}

/// ryupdm's settings of the project at `prefix` (`""` or `a/b`) in the
/// commit `rev` names; the defaults when it has none.
pub fn config_at(repo: &gix::Repository, rev: &str, prefix: &str) -> Result<Config> {
    let prefix = prefix_of(prefix);
    let Ok(id) = repo.rev_parse_single(rev) else {
        return Ok(Config::default());
    };
    let commit = id
        .object()
        .map_err(git_error)?
        .peel_to_commit()
        .map_err(git_error)?;
    let tree = commit.tree_id().map_err(git_error)?.detach();
    let mut files = BTreeMap::new();
    if let Some(b) = crate::history::entry(repo, tree, &prefix, PDM_CONFIG)? {
        let data = repo.find_object(b).map_err(git_error)?.data.clone();
        files.insert(PDM_CONFIG.to_owned(), data);
    }
    Config::read(&files)
}

/// A project's folder as [`crate::history`] takes it: `""`, or ending in
/// `/`.
pub fn prefix_of(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

/// Fetches `url` into `repo`: `HEAD`, the `ryupdm/` tags and `branches`,
/// each to the same name; returns the repository opened again, so it
/// sees the new refs.
fn fetch_into(
    repo: gix::Repository,
    url: &str,
    branches: &BTreeSet<String>,
) -> Result<gix::Repository> {
    let failed = |e: &dyn std::fmt::Display| {
        Error::new(
            Code::DependencyFetchFailed,
            format!("fetching {url} failed: {e}"),
        )
        .with("url", url)
        .with("reason", e.to_string())
    };
    let mut specs = vec![
        format!("+HEAD:{HEAD_REF}"),
        "+refs/tags/ryupdm/*:refs/tags/ryupdm/*".to_owned(),
    ];
    specs.extend(
        branches
            .iter()
            .map(|b| format!("+refs/heads/{b}:refs/heads/{b}")),
    );
    let remote = repo
        .remote_at(url)
        .map_err(|e| failed(&e))?
        .with_refspecs(
            specs.iter().map(String::as_str),
            gix::remote::Direction::Fetch,
        )
        .map_err(|e| failed(&e))?
        // Only ryupdm's tags, which the refspecs name: no others follow.
        .with_fetch_tags(gix::remote::fetch::Tags::None);
    let connection = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(|e| failed(&e))?;
    connection
        .prepare_fetch(gix::progress::Discard, Default::default())
        .map_err(|e| failed(&e))?
        .receive(
            gix::progress::Discard,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .map_err(|e| failed(&e))?;
    let path = repo.path().to_owned();
    drop(repo);
    gix::open(path).map_err(git_error)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::history::tests::git;

    /// A repository at `dir` holding a project at `path` whose settings
    /// name `branch` as the release branch, on `main`, committed.
    pub(crate) fn upstream(dir: &Path, path: &str, branch: &str) {
        git(dir, &["init", "-q"]);
        let root = dir.join(path);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("ryucad.json"),
            "{\"default_unit\": \"mm\", \"format\": 1, \"kind\": \"ryucad.project\"}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("ryupdm.json"),
            format!(
                "{{\"format\": 1, \"kind\": \"ryupdm.config\", \"release_branch\": \"{branch}\", \"revisions\": \"letters\"}}\n"
            ),
        )
        .unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "project"]);
    }

    pub(crate) fn url(dir: &Path) -> String {
        format!("file://{}", dir.display())
    }

    #[test]
    fn urls_are_keyed_as_one_repository() {
        assert_eq!(
            normalize("HTTPS://GitHub.com/Acme/Fasteners.git/"),
            "https://github.com/Acme/Fasteners"
        );
        assert_eq!(
            folder("https://github.com/acme/x"),
            folder("https://GITHUB.com/acme/x.git")
        );
        assert_ne!(
            folder("https://github.com/acme/x"),
            folder("https://github.com/acme/y")
        );
        assert_eq!(folder("file:///srv/x").len(), 36);
    }

    #[test]
    fn a_fetch_takes_head_the_release_branch_and_tags_incrementally() {
        let up = tempfile::tempdir().unwrap();
        upstream(up.path(), "lib", "release");
        git(up.path(), &["branch", "release"]);
        git(up.path(), &["branch", "other"]);
        git(up.path(), &["tag", "ryupdm/p/A"]);
        git(up.path(), &["tag", "v1"]);
        let cache = Cache::new(tempfile::tempdir().unwrap().keep());
        let u = url(up.path());
        assert!(cache.open(&u).unwrap().is_none());
        let f = cache.fetch(&u, &["lib"]).unwrap();
        let head = git(up.path(), &["rev-parse", "HEAD"]).trim().to_owned();
        assert_eq!(f.head, head);
        assert_eq!(
            f.branches["lib"],
            ("release".to_owned(), Some(head.clone()))
        );
        assert!(f.before.is_empty());
        let repo = cache.open(&u).unwrap().unwrap();
        assert!(repo.is_bare());
        // Only what was asked for: the release branch and ryupdm's tags.
        assert_eq!(
            local_branches(&repo).unwrap(),
            BTreeSet::from(["release".to_owned()])
        );
        assert!(
            repo.try_find_reference("refs/tags/ryupdm/p/A")
                .unwrap()
                .is_some()
        );
        assert!(repo.try_find_reference("refs/tags/v1").unwrap().is_none());
        // Again, after the branch moves on: what was there is `before`.
        std::fs::write(up.path().join("lib/x"), "x").unwrap();
        git(up.path(), &["add", "-A"]);
        git(up.path(), &["commit", "-q", "-m", "x"]);
        git(up.path(), &["branch", "-f", "release"]);
        let f = cache.fetch(&u, &["lib"]).unwrap();
        assert_eq!(f.before["release"], head);
        let now = git(up.path(), &["rev-parse", "HEAD"]).trim().to_owned();
        assert_eq!(f.branches["lib"].1.as_deref(), Some(now.as_str()));
        // A project with no settings reads `main`.
        let f = cache.fetch(&u, &[""]).unwrap();
        assert_eq!(f.branches[""], ("main".to_owned(), Some(now)));
    }

    #[test]
    fn a_fetch_that_fails_says_where_and_why() {
        let cache = Cache::new(tempfile::tempdir().unwrap().keep());
        let gone = tempfile::tempdir().unwrap();
        let u = url(&gone.path().join("nothing"));
        let e = cache.fetch(&u, &[""]).unwrap_err();
        assert_eq!(e.code, Code::DependencyFetchFailed);
        assert_eq!(e.code.exit_code(), 4);
        assert_eq!(e.details["url"], u);
    }
}
