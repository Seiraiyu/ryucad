//! The project's Git history, read-only: the commits that touched the
//! project (or one part), what is uncommitted, files as `HEAD` has them,
//! the whole project at any revision, and a merge in progress (its step,
//! each side's commit and the files Git left unmerged). ryucad never
//! writes to Git; committing, and marking files resolved, stay with the
//! agent or user.

use crate::activity::Files;
use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// A project's repository, and where the project sits in it.
pub struct Repo {
    repo: gix::Repository,
    /// The project root relative to the work tree, with `/`, ending in `/`
    /// unless it is the work tree itself (then empty).
    prefix: String,
}

fn git_error(e: impl std::fmt::Display) -> Error {
    Error::new(Code::Io, format!("reading the Git repository: {e}"))
}

/// The project as a revision has it.
#[derive(Clone, Debug)]
pub struct Revision {
    /// The commit the revision names, in hex.
    pub commit: String,
    /// The project folder's tree in that commit, in hex: equal trees, equal
    /// files. Empty when the commit has no such folder.
    pub tree: String,
    /// The manifest, its Git attributes, ryupdm's settings and everything
    /// under `parts/`, `releases/` and `imports/`, by path relative to the
    /// project.
    pub files: Files,
}

/// What Git is in the middle of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Merge,
    CherryPick,
    Revert,
    /// One step of a rebase: a commit being replayed.
    Rebase,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Merge => "merge",
            Step::CherryPick => "cherry-pick",
            Step::Revert => "revert",
            Step::Rebase => "rebase",
        }
    }
}

/// A merge in progress: what Git is doing, and the commits it merges, in
/// hex. Ours is `HEAD`; theirs and the base are what Git merges with it:
/// for a merge the other head and their merge base; for a cherry-pick or
/// a rebase step the picked commit and its parent; for a revert the
/// reverted commit's parent, from the reverted commit as base.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeState {
    pub step: Step,
    pub ours: String,
    pub theirs: String,
    /// `None` when the two have no common ancestor (or a root commit was
    /// picked).
    pub base: Option<String>,
}

/// Each side's version of a file Git couldn't merge, from the index's
/// stages; `None` where that side has no file (a deletion).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Unmerged {
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
}

/// The error for a project outside any repository.
fn no_repo(root: &Path) -> Error {
    Error::new(
        Code::NotAGitRepo,
        format!("{} is not in a Git repository", root.display()),
    )
    .with("path", root)
}

/// The warning for a project outside any repository.
pub fn not_a_repo(root: &Path) -> Value {
    json!(no_repo(root))
}

/// Bytes equal once CRLF line endings are read as LF: a checkout with
/// `core.autocrlf` doesn't make every file look changed.
fn same_text(a: &[u8], b: &[u8]) -> bool {
    let lf = |x: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(x.len());
        let mut i = 0;
        while i < x.len() {
            if x[i] == b'\r' && x.get(i + 1) == Some(&b'\n') {
                i += 1;
                continue;
            }
            out.push(x[i]);
            i += 1;
        }
        out
    };
    a == b || lf(a) == lf(b)
}

impl Repo {
    /// The repository holding `root`, or `None` when there is none.
    pub fn open(root: &Path) -> Result<Option<Repo>> {
        let repo = match gix::discover(root) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        let Some(workdir) = repo.workdir() else {
            return Ok(None);
        };
        let workdir = workdir.canonicalize().map_err(|e| Error::io(workdir, &e))?;
        let root = root.canonicalize().map_err(|e| Error::io(root, &e))?;
        let Ok(rel) = root.strip_prefix(&workdir) else {
            return Ok(None);
        };
        let mut prefix = crate::doc::project::relative(Path::new(""), rel);
        if !prefix.is_empty() {
            prefix.push('/');
        }
        Ok(Some(Repo { repo, prefix }))
    }

    /// The `origin` remote's URL, if the clone has one.
    pub fn origin_url(&self) -> Option<String> {
        self.repo
            .config_snapshot()
            .string("remote.origin.url")
            .map(|s| s.to_string())
    }

    /// The branch checked out (`feature/arm`), or `None` on a detached
    /// head.
    pub fn branch(&self) -> Result<Option<String>> {
        Ok(self
            .repo
            .head_name()
            .map_err(git_error)?
            .map(|n| n.shorten().to_string()))
    }

    /// The repository itself, for ryupdm.
    pub fn gix(&self) -> &gix::Repository {
        &self.repo
    }

    /// Where the project sits in the work tree: empty at its root, else
    /// ending in `/`.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The commit `HEAD` points at, or `None` before the first commit.
    fn head(&self) -> Option<gix::Commit<'_>> {
        self.repo.head_commit().ok()
    }

    /// The object at `path` (relative to the work tree) in `tree`.
    fn entry(&self, tree: &gix::Tree<'_>, path: &str) -> Result<Option<gix::ObjectId>> {
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            return Ok(Some(tree.id));
        }
        Ok(tree
            .clone()
            .lookup_entry_by_path(path)
            .map_err(git_error)?
            .map(|e| e.object_id()))
    }

    /// Every blob under the tree `id`, by path below it.
    fn blobs(
        &self,
        id: gix::ObjectId,
        at: &str,
        out: &mut BTreeMap<String, gix::ObjectId>,
    ) -> Result<()> {
        let obj = self.repo.find_object(id).map_err(git_error)?;
        let Ok(tree) = obj.try_into_tree() else {
            out.insert(at.trim_end_matches('/').to_owned(), id);
            return Ok(());
        };
        let decoded = tree.decode().map_err(git_error)?;
        for e in &decoded.entries {
            let path = format!("{at}{}", e.filename);
            if e.mode.is_tree() {
                self.blobs(e.oid.to_owned(), &format!("{path}/"), out)?;
            } else if e.mode.is_blob() {
                out.insert(path, e.oid.to_owned());
            }
        }
        Ok(())
    }

    /// The project's files in a commit's tree, by path relative to the
    /// project: the files at its root a session tracks and everything
    /// under `parts/`, `releases/` and `imports/`.
    fn project_blobs(&self, tree: &gix::Tree<'_>) -> Result<BTreeMap<String, gix::ObjectId>> {
        let mut out = BTreeMap::new();
        for name in crate::doc::project::ROOT_FILES {
            if let Some(id) = self.entry(tree, &format!("{}{name}", self.prefix))? {
                out.insert(name.to_owned(), id);
            }
        }
        for dir in [
            "parts",
            crate::doc::project::RELEASES,
            crate::doc::project::IMPORTS,
        ] {
            if let Some(id) = self.entry(tree, &format!("{}{dir}", self.prefix))? {
                self.blobs(id, &format!("{dir}/"), &mut out)?;
            }
        }
        Ok(out)
    }

    fn read(&self, id: gix::ObjectId) -> Result<Vec<u8>> {
        Ok(self.repo.find_object(id).map_err(git_error)?.data.clone())
    }

    /// A project file as `HEAD` has it, if it does.
    pub fn head_file(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let Some(head) = self.head() else {
            return Ok(None);
        };
        let tree = head.tree().map_err(git_error)?;
        match self.entry(&tree, &format!("{}{rel}", self.prefix))? {
            Some(id) => self.read(id).map(Some),
            None => Ok(None),
        }
    }

    /// The project's files that differ from `HEAD`.
    pub fn uncommitted(&self, files: &Files) -> Result<Vec<Value>> {
        let head = match self.head() {
            Some(h) => self.project_blobs(&h.tree().map_err(git_error)?)?,
            None => BTreeMap::new(),
        };
        let mut out = Vec::new();
        for (path, bytes) in files {
            match head.get(path) {
                None => out.push(json!({"path": path, "change": "created"})),
                Some(id) if !same_text(&self.read(*id)?, bytes) => {
                    out.push(json!({"path": path, "change": "modified"}));
                }
                Some(_) => {}
            }
        }
        for path in head.keys().filter(|p| !files.contains_key(*p)) {
            out.push(json!({"path": path, "change": "deleted"}));
        }
        out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        Ok(out)
    }

    /// Up to `limit` commits, newest first, that changed the project, or
    /// with `under` only that folder of it (`parts/<part>`). A commit
    /// changed it when it differs there from its first parent.
    pub fn log(&self, under: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        let Some(head) = self.head() else {
            return Ok(Vec::new());
        };
        let path = match under {
            Some(u) => format!("{}{u}", self.prefix),
            None => self.prefix.clone(),
        };
        let walk = self
            .repo
            .rev_walk([head.id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                Default::default(),
            ))
            .all()
            .map_err(git_error)?;
        let mut out = Vec::new();
        for info in walk {
            if out.len() >= limit {
                break;
            }
            let info = info.map_err(git_error)?;
            let commit = info.object().map_err(git_error)?;
            let tree = commit.tree().map_err(git_error)?;
            // A shallow clone's oldest commits name parents it doesn't
            // have: they read as first commits, as `git log` shows them.
            let parent = match info.parent_ids.first() {
                Some(p) if self.repo.has_object(*p) => Some(
                    self.repo
                        .find_commit(*p)
                        .map_err(git_error)?
                        .tree()
                        .map_err(git_error)?,
                ),
                _ => None,
            };
            let mine = self.entry(&tree, &path)?;
            let theirs = match &parent {
                Some(t) => self.entry(t, &path)?,
                None => None,
            };
            if mine == theirs {
                continue;
            }
            let files = self.changed(&tree, parent.as_ref())?;
            let author = commit.author().map_err(git_error)?;
            let message = commit.message().map_err(git_error)?;
            out.push(json!({
                "id": commit.id.to_string(),
                "author": author.name.to_string(),
                "time": commit.time().map_err(git_error)?.seconds,
                "subject": message.summary().to_string(),
                "files": files,
            }));
        }
        Ok(out)
    }

    /// The commit a revision expression names (`HEAD`, `main`, `HEAD~2`,
    /// a hash), or `rev_not_found`.
    pub fn commit(&self, rev: &str) -> Result<gix::Commit<'_>> {
        let not_found = |why: String| {
            Error::new(
                Code::RevNotFound,
                format!("no revision {rev} in the repository: {why}"),
            )
            .with("rev", rev)
        };
        let id = self
            .repo
            .rev_parse_single(rev)
            .map_err(|e| not_found(e.to_string()))?;
        let object = id.object().map_err(|e| not_found(e.to_string()))?;
        object
            .peel_to_commit()
            .map_err(|e| not_found(e.to_string()))
    }

    /// When the commit `rev` names was made: its committer's time, in
    /// seconds since the Unix epoch.
    pub fn commit_time(&self, rev: &str) -> Result<i64> {
        Ok(self.commit(rev)?.time().map_err(git_error)?.seconds)
    }

    /// The commit `rev` names and the project folder's tree in it (empty
    /// when the commit has no such folder), both in hex.
    pub fn locate(&self, rev: &str) -> Result<(String, String)> {
        let commit = self.commit(rev)?;
        let tree = commit.tree().map_err(git_error)?;
        let folder = self.entry(&tree, &self.prefix)?;
        Ok((
            commit.id.to_string(),
            folder.map(|t| t.to_string()).unwrap_or_default(),
        ))
    }

    /// The project's files in the commit `id` (hex).
    pub fn files(&self, id: &str) -> Result<Files> {
        let commit = self.commit(id)?;
        let tree = commit.tree().map_err(git_error)?;
        let mut files = Files::new();
        for (path, id) in self.project_blobs(&tree)? {
            files.insert(path, self.read(id)?);
        }
        Ok(files)
    }

    /// The project as `rev` has it.
    pub fn revision(&self, rev: &str) -> Result<Revision> {
        let (commit, tree) = self.locate(rev)?;
        let files = self.files(&commit)?;
        Ok(Revision {
            commit,
            tree,
            files,
        })
    }

    /// The commit `HEAD` points at, in hex.
    fn head_id(&self) -> Result<String> {
        self.head()
            .map(|c| c.id.to_string())
            .ok_or_else(|| Error::new(Code::NotMerging, "the repository has no commits"))
    }

    /// The first parent of the commit `id`, in hex, if it has one.
    fn parent(&self, id: &str) -> Result<Option<String>> {
        let commit = self.commit(id)?;
        Ok(commit.parent_ids().next().map(|p| p.to_string()))
    }

    /// The merge Git is in the middle of, from the files it keeps while it
    /// waits (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, and
    /// `REBASE_HEAD` during a rebase), or, inside the hook Git runs before
    /// committing a merge it made itself, from what it says it merges
    /// ([`hook_heads`]); `None` otherwise. An octopus merge is
    /// `merge_unsupported`.
    pub fn merge_state(&self) -> Result<Option<MergeState>> {
        let dir = self.repo.git_dir();
        let waiting = WAITING.iter().any(|n| dir.join(n).is_file());
        if !waiting {
            let heads = hook_heads(dir);
            if !heads.is_empty() {
                return self.merging(Step::Merge, &heads).map(Some);
            }
        }
        let rebasing = dir.join("rebase-merge").is_dir() || dir.join("rebase-apply").is_dir();
        let steps = [
            ("MERGE_HEAD", Step::Merge),
            ("CHERRY_PICK_HEAD", Step::CherryPick),
            ("REVERT_HEAD", Step::Revert),
            ("REBASE_HEAD", Step::Rebase),
        ];
        for (name, step) in steps {
            if step == Step::Rebase && !rebasing {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
                continue;
            };
            let heads: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
            if heads.is_empty() {
                continue;
            }
            return self.merging(step, &heads).map(Some);
        }
        Ok(None)
    }

    /// The index file's size and time: `git add` and `git rm` change it.
    pub fn index_stamp(&self) -> String {
        match std::fs::metadata(self.repo.index_path()) {
            Ok(m) => {
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                format!("{}:{t}", m.len())
            }
            Err(_) => String::new(),
        }
    }

    /// The merge of `heads` into `HEAD` that `step` makes.
    fn merging(&self, step: Step, heads: &[String]) -> Result<MergeState> {
        let head = match heads {
            [h] => h,
            _ => {
                return Err(Error::new(
                    Code::MergeUnsupported,
                    format!(
                        "an octopus merge of {} heads: ryucad reports on merges of two",
                        heads.len() + 1
                    ),
                )
                .with("heads", heads));
            }
        };
        let ours = self.head_id()?;
        let named = self.commit(head)?.id.to_string();
        let (theirs, base) = match step {
            Step::Merge => {
                let ours_id = gix::ObjectId::from_hex(ours.as_bytes()).map_err(git_error)?;
                let theirs_id = gix::ObjectId::from_hex(named.as_bytes()).map_err(git_error)?;
                let base = self
                    .repo
                    .merge_base(ours_id, theirs_id)
                    .ok()
                    .flatten()
                    .map(|b| b.to_string());
                (named, base)
            }
            Step::CherryPick | Step::Rebase => {
                let base = self.parent(&named)?;
                (named, base)
            }
            Step::Revert => {
                let parent = self.parent(&named)?.ok_or_else(|| {
                    Error::new(
                        Code::MergeUnsupported,
                        "reverting a root commit: there is no parent to merge",
                    )
                })?;
                (parent, Some(named))
            }
        };
        Ok(MergeState {
            step,
            ours,
            theirs,
            base,
        })
    }

    /// The project's files Git left unmerged in the index, by path relative
    /// to the project, with each side's version.
    pub fn unmerged(&self) -> Result<BTreeMap<String, Unmerged>> {
        use gix::index::entry::Stage;
        let mut out: BTreeMap<String, Unmerged> = BTreeMap::new();
        // A repository with no index yet has nothing unmerged.
        let Ok(index) = self.repo.open_index() else {
            return Ok(out);
        };
        for e in index.entries() {
            let stage = e.stage();
            if stage == Stage::Unconflicted {
                continue;
            }
            let path = e.path(&index).to_string();
            let Some(rel) = path.strip_prefix(&self.prefix) else {
                continue;
            };
            let bytes = Some(self.read(e.id)?);
            let entry = out.entry(rel.to_owned()).or_default();
            match stage {
                Stage::Base => entry.base = bytes,
                Stage::Ours => entry.ours = bytes,
                Stage::Theirs => entry.theirs = bytes,
                Stage::Unconflicted => {}
            }
        }
        Ok(out)
    }

    /// The project's files whose working copy isn't what the index holds
    /// (changed, deleted or never added), by path relative to the project,
    /// sorted: a commit would take the index's, not what was checked. Paths
    /// Git left unmerged are [`Repo::unmerged`]'s and left out.
    pub fn unstaged(&self, files: &Files) -> Result<Vec<String>> {
        use gix::index::entry::Stage;
        let mut staged = BTreeMap::new();
        let mut unmerged = std::collections::BTreeSet::new();
        if let Ok(index) = self.repo.open_index() {
            for e in index.entries() {
                let path = e.path(&index).to_string();
                let Some(rel) = path.strip_prefix(&self.prefix) else {
                    continue;
                };
                if !project_file(rel) {
                    continue;
                }
                if e.stage() == Stage::Unconflicted {
                    staged.insert(rel.to_owned(), e.id);
                } else {
                    unmerged.insert(rel.to_owned());
                }
            }
        }
        let mut out = std::collections::BTreeSet::new();
        for (path, bytes) in files {
            match staged.get(path) {
                _ if unmerged.contains(path) => {}
                Some(id) if same_text(&self.read(*id)?, bytes) => {}
                _ => {
                    out.insert(path.clone());
                }
            }
        }
        out.extend(staged.into_keys().filter(|p| !files.contains_key(p)));
        Ok(out.into_iter().collect())
    }

    /// The project files a commit changed relative to its parent.
    fn changed(&self, tree: &gix::Tree<'_>, parent: Option<&gix::Tree<'_>>) -> Result<Vec<Value>> {
        let now = self.project_blobs(tree)?;
        let before = match parent {
            Some(p) => self.project_blobs(p)?,
            None => BTreeMap::new(),
        };
        let mut out = Vec::new();
        for (path, id) in &now {
            match before.get(path) {
                None => out.push(json!({"path": path, "change": "created"})),
                Some(b) if b != id => out.push(json!({"path": path, "change": "modified"})),
                Some(_) => {}
            }
        }
        for path in before.keys().filter(|p| !now.contains_key(*p)) {
            out.push(json!({"path": path, "change": "deleted"}));
        }
        out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        Ok(out)
    }
}

/// Whether `rel` (relative to a project) is one of the files its state
/// depends on, as `session::files` lists them: the files at its root,
/// files two and three levels under `parts/`, files two levels under
/// `releases/`, and files under `imports/`.
fn project_file(rel: &str) -> bool {
    if crate::doc::project::ROOT_FILES.contains(&rel) {
        return true;
    }
    let mut parts = rel.split('/');
    match parts.next() {
        Some("parts") => matches!(parts.count(), 2 | 3),
        Some(crate::doc::project::RELEASES) => parts.count() == 2,
        Some(crate::doc::project::IMPORTS) => parts.count() == 1,
        _ => false,
    }
}

/// The files Git keeps while a merge, cherry-pick, revert or rebase step
/// waits, and the heads they name.
const WAITING: [&str; 4] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
];

/// What Git is merging when it runs a hook before committing a merge it
/// made itself, before it writes `MERGE_HEAD`: the commits its reflog
/// action names (`merge <commit>…`, options left out), or for a pull, the
/// heads `FETCH_HEAD` marks for merging. Empty anywhere else.
fn hook_heads(git_dir: &Path) -> Vec<String> {
    let Ok(action) = std::env::var("GIT_REFLOG_ACTION") else {
        return Vec::new();
    };
    let mut words = action.split_whitespace();
    match words.next() {
        Some("merge") => words
            .filter(|w| !w.starts_with('-'))
            .map(str::to_owned)
            .collect(),
        Some("pull") => std::fs::read_to_string(git_dir.join("FETCH_HEAD"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                let (id, mark) = (f.next()?, f.next()?);
                mark.is_empty().then(|| id.to_owned())
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The folders under `tree` (paths below `at`) that hold a `ryucad.json`.
fn manifests(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    at: &str,
    out: &mut Vec<String>,
) -> Result<()> {
    let obj = repo.find_object(tree).map_err(git_error)?;
    let Ok(tree) = obj.try_into_tree() else {
        return Ok(());
    };
    let decoded = tree.decode().map_err(git_error)?;
    for e in &decoded.entries {
        let name = e.filename.to_string();
        if e.mode.is_tree() {
            manifests(repo, e.oid.to_owned(), &format!("{at}{name}/"), out)?;
        } else if name == crate::doc::project::MANIFEST {
            out.push(at.to_owned());
        }
    }
    Ok(())
}

/// The ryucad projects of the repository holding `here` that a merge in
/// progress concerns: every folder with a `ryucad.json` in `HEAD` or in
/// the commit being merged that the work tree still has, sorted. Empty,
/// at once, when Git is in no merge (as for every ordinary commit).
pub fn merging_projects(here: &Path) -> Result<Vec<std::path::PathBuf>> {
    let Ok(repo) = gix::discover(here) else {
        return Ok(Vec::new());
    };
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
        return Ok(Vec::new());
    };
    let dir = repo.git_dir().to_path_buf();
    let mut heads: Vec<String> = WAITING
        .iter()
        .filter_map(|n| std::fs::read_to_string(dir.join(n)).ok())
        .flat_map(|t| t.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .collect();
    if heads.is_empty() {
        heads = hook_heads(&dir);
    }
    if heads.is_empty() {
        return Ok(Vec::new());
    }
    let mut folders = Vec::new();
    for rev in std::iter::once("HEAD".to_owned()).chain(heads) {
        let Ok(id) = repo.rev_parse_single(rev.as_str()) else {
            continue;
        };
        let commit = id
            .object()
            .map_err(git_error)?
            .peel_to_commit()
            .map_err(git_error)?;
        let tree = commit.tree_id().map_err(git_error)?.detach();
        manifests(&repo, tree, "", &mut folders)?;
    }
    folders.sort();
    folders.dedup();
    Ok(folders
        .into_iter()
        .map(|f| {
            f.split('/')
                .filter(|s| !s.is_empty())
                .fold(workdir.clone(), |p, s| p.join(s))
        })
        .filter(|p| p.join(crate::doc::project::MANIFEST).is_file())
        .collect())
}

/// What a commit of the index would get wrong about releases, for every
/// ryucad project the index holds (a folder with a `ryucad.json`): each
/// project's folder in the work tree with ryupdm's commit guard findings
/// (`ryupdm::check::staged`), those with none left out. Nothing outside a
/// repository.
pub fn release_guard(
    here: &Path,
) -> Result<Vec<(std::path::PathBuf, Vec<ryupdm::check::Finding>)>> {
    let Ok(repo) = gix::discover(here) else {
        return Ok(Vec::new());
    };
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
        return Ok(Vec::new());
    };
    let Ok(index) = repo.open_index() else {
        return Ok(Vec::new());
    };
    let manifest = format!("/{}", crate::doc::project::MANIFEST);
    let mut prefixes: Vec<String> = index
        .entries()
        .iter()
        .filter(|e| e.stage() == gix::index::entry::Stage::Unconflicted)
        .filter_map(|e| {
            let path = e.path(&index).to_string();
            if path == crate::doc::project::MANIFEST {
                return Some(String::new());
            }
            path.strip_suffix(&manifest).map(|dir| format!("{dir}/"))
        })
        .collect();
    prefixes.sort();
    prefixes.dedup();
    let mut out = Vec::new();
    for prefix in prefixes {
        let found = ryupdm::check::staged(&repo, &prefix)?;
        if !found.is_empty() {
            let root = prefix
                .split('/')
                .filter(|s| !s.is_empty())
                .fold(workdir.clone(), |p, s| p.join(s));
            out.push((root, found));
        }
    }
    Ok(out)
}

/// The GitHub owner and repository a remote URL names (https, ssh or
/// scp-like, with or without `.git`), or `None` for other hosts.
pub fn github_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let path = [
        "https://github.com/",
        "http://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ]
    .iter()
    .find_map(|p| url.strip_prefix(p))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    (ok(owner) && ok(repo)).then(|| (owner.to_owned(), repo.to_owned()))
}

/// The repository holding the project at `root`, or `not_a_git_repo`.
pub fn repo(root: &Path) -> Result<Repo> {
    Repo::open(root)?.ok_or_else(|| no_repo(root))
}

/// The project at `root` as `rev` has it: `not_a_git_repo` outside a
/// repository, `rev_not_found` for a revision it doesn't have.
pub fn revision(root: &Path, rev: &str) -> Result<Revision> {
    repo(root)?.revision(rev)
}

/// `history.log`: the project's commits (or a part's), and what is
/// uncommitted.
pub fn history(root: &Path, files: &Files, part: Option<&str>, limit: usize) -> Result<Value> {
    let Some(repo) = Repo::open(root)? else {
        return Ok(
            json!({"repository": false, "commits": [], "uncommitted": [],
                         "warnings": [not_a_repo(root)]}),
        );
    };
    let under = part.map(|p| format!("parts/{p}"));
    let commits = repo.log(under.as_deref(), limit)?;
    let mut uncommitted = repo.uncommitted(files)?;
    if let Some(u) = &under {
        let prefix = format!("{u}/");
        uncommitted.retain(|f| f["path"].as_str().is_some_and(|p| p.starts_with(&prefix)));
    }
    Ok(json!({"repository": true, "commits": commits, "uncommitted": uncommitted, "warnings": []}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::server::tests::{call, fresh};

    #[test]
    fn github_remotes() {
        let want = Some(("acme".to_owned(), "widget".to_owned()));
        for url in [
            "https://github.com/acme/widget.git",
            "https://github.com/acme/widget",
            "https://github.com/acme/widget/",
            "git@github.com:acme/widget.git",
            "ssh://git@github.com/acme/widget.git",
        ] {
            assert_eq!(github_repo(url), want, "{url}");
        }
        for url in [
            "https://gitlab.com/acme/widget",
            "git@github.com:acme",
            "https://github.com/acme/wid get",
            "/srv/widget.git",
        ] {
            assert_eq!(github_repo(url), None, "{url}");
        }
    }

    #[test]
    fn origin_and_branch_of_a_clone() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        let repo = Repo::open(dir.path()).unwrap().unwrap();
        assert_eq!(repo.origin_url(), None);
        assert_eq!(repo.branch().unwrap().as_deref(), Some("main"));
        git(&["remote", "add", "origin", "git@github.com:acme/widget.git"]);
        git(&["checkout", "-q", "-b", "feature/arm"]);
        let repo = Repo::open(dir.path()).unwrap().unwrap();
        assert_eq!(
            repo.origin_url().as_deref(),
            Some("git@github.com:acme/widget.git")
        );
        assert_eq!(repo.branch().unwrap().as_deref(), Some("feature/arm"));
    }
    use crate::session::Session;
    use std::process::Command;

    /// `git`, reading neither the user's config nor the system's: where
    /// the installers registered ryucad's drivers, or Git for Windows set
    /// `core.autocrlf`, tests would merge and check out otherwise.
    pub(crate) fn git_command() -> Command {
        let mut git = Command::new("git");
        git.env(
            "GIT_CONFIG_GLOBAL",
            std::env::temp_dir().join("ryucad-no-gitconfig"),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1");
        git
    }

    /// Runs git in `dir` with a fixed identity.
    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let st = git_command()
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    fn files(root: &Path) -> Files {
        crate::activity::snapshot(root).unwrap()
    }

    #[test]
    fn crlf_is_lf() {
        assert!(same_text(b"a\r\nb\r\n", b"a\nb\n"));
        assert!(!same_text(b"a\nb", b"a\nc"));
    }

    #[test]
    fn outside_a_repository_there_is_no_history() {
        let (dir, _s) = fresh();
        let v = history(dir.path(), &files(dir.path()), None, 10).unwrap();
        assert_eq!(v["repository"], false);
        assert_eq!(v["warnings"][0]["code"], "not_a_git_repo");
    }

    #[test]
    fn a_shallow_clone_has_the_history_it_holds() {
        // CI checkouts and many agents' clones stop after a commit or so.
        let dir = tempfile::tempdir().unwrap();
        let full = dir.path().join("full");
        std::fs::create_dir(&full).unwrap();
        git(&full, &["init", "-q", "-b", "main"]);
        crate::server::init(&full, None).unwrap();
        git(&full, &["add", "."]);
        git(&full, &["commit", "-q", "-m", "init"]);
        let mut s = Session::open(&full).unwrap();
        for part in ["a", "b"] {
            call(&mut s, "part.create", serde_json::json!({"part": part})).unwrap();
            git(&full, &["add", "."]);
            git(&full, &["commit", "-q", "-m", &format!("part {part}")]);
        }
        let url = format!("file://{}", full.display());
        git(
            dir.path(),
            &["clone", "-q", "--depth", "1", &url, "shallow"],
        );
        let root = dir.path().join("shallow");
        let v = history(&root, &files(&root), None, 10).unwrap();
        let subjects: Vec<&str> = v["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["subject"].as_str().unwrap())
            .collect();
        assert_eq!(subjects, ["part b"]);
    }

    #[test]
    fn a_revision_holds_ryupdm_files() {
        let (dir, _s) = fresh();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("ryupdm.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("parts/p")).unwrap();
        std::fs::write(root.join("parts/p/properties.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("releases/p")).unwrap();
        std::fs::write(root.join("releases/p/A.json"), "{}").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "pdm"]);
        let repo = repo(root).unwrap();
        let r = repo.revision("HEAD").unwrap();
        assert_eq!(r.files, files(root));
        assert!(r.files.contains_key("ryupdm.json"));
        assert!(r.files.contains_key("parts/p/properties.json"));
        assert!(r.files.contains_key("releases/p/A.json"));
        // A record changed and one never added are what a commit would
        // leave out.
        std::fs::write(root.join("releases/p/A.json"), "{ }").unwrap();
        std::fs::write(root.join("releases/p/B.json"), "{}").unwrap();
        assert_eq!(
            repo.unstaged(&files(root)).unwrap(),
            ["releases/p/A.json", "releases/p/B.json"]
        );
    }

    #[test]
    fn commits_that_touch_the_project_or_a_part() {
        // The project sits in a subfolder of the repository.
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        git(top, &["init", "-q", "-b", "main"]);
        std::fs::write(top.join("README"), "hi").unwrap();
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "readme"]);
        let root = top.join("cad");
        std::fs::create_dir(&root).unwrap();
        crate::server::init(&root, None).unwrap();
        let mut s = Session::open(&root).unwrap();
        call(&mut s, "part.create", serde_json::json!({"part": "a"})).unwrap();
        call(&mut s, "part.create", serde_json::json!({"part": "b"})).unwrap();
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "two parts"]);
        call(
            &mut s,
            "feature.add",
            serde_json::json!({"part": "b", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "a sphere in b\n\nwith a body"]);
        std::fs::write(top.join("README"), "unrelated").unwrap();
        git(top, &["commit", "-q", "-am", "readme again"]);
        // Uncommitted: an edit, and a new part.
        call(
            &mut s,
            "feature.edit",
            serde_json::json!({"part": "b", "id": "s", "params": {"radius": 2}}),
        )
        .unwrap();
        call(&mut s, "part.create", serde_json::json!({"part": "c"})).unwrap();

        let v = history(&root, &files(&root), None, 10).unwrap();
        let subjects: Vec<&str> = v["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["subject"].as_str().unwrap())
            .collect();
        assert_eq!(subjects, ["a sphere in b", "two parts"]);
        assert_eq!(
            v["commits"][0]["files"],
            json!([{"path": "parts/b/features/s.json", "change": "created"},
                   {"path": "parts/b/part.json", "change": "modified"}])
        );
        assert_eq!(v["commits"][0]["author"], "Test");
        assert_eq!(
            v["uncommitted"],
            json!([{"path": "parts/b/features/s.json", "change": "modified"},
                   {"path": "parts/c/part.json", "change": "created"}])
        );
        let a = history(&root, &files(&root), Some("a"), 10).unwrap();
        assert_eq!(a["commits"].as_array().unwrap().len(), 1);
        assert_eq!(a["uncommitted"], json!([]));
        let one = history(&root, &files(&root), None, 1).unwrap();
        assert_eq!(one["commits"].as_array().unwrap().len(), 1);
        // HEAD's copy of a file, for repairs.
        let repo = Repo::open(&root).unwrap().unwrap();
        let committed = repo.head_file("parts/b/features/s.json").unwrap().unwrap();
        assert!(String::from_utf8(committed).unwrap().contains("1 mm"));
        assert_eq!(repo.head_file("parts/c/part.json").unwrap(), None);
    }

    #[test]
    fn the_project_at_any_revision() {
        // The project sits in a subfolder, beside files that aren't its.
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        git(top, &["init", "-q", "-b", "main"]);
        let root = top.join("cad");
        std::fs::create_dir(&root).unwrap();
        crate::server::init(&root, None).unwrap();
        let mut s = Session::open(&root).unwrap();
        call(&mut s, "part.create", serde_json::json!({"part": "a"})).unwrap();
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "a"]);
        let first = files(&root);
        call(
            &mut s,
            "feature.add",
            serde_json::json!({"part": "a", "type": "sphere", "id": "s", "params": {"radius": 1}}),
        )
        .unwrap();
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "a sphere"]);
        std::fs::write(top.join("README"), "not the project's").unwrap();
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "readme"]);
        let r = revision(&root, "HEAD~2").unwrap();
        assert_eq!(r.files, first);
        assert_eq!(revision(&root, "HEAD").unwrap().files, files(&root));
        // A commit that leaves the project alone has the same tree.
        assert_eq!(
            revision(&root, "HEAD").unwrap().tree,
            revision(&root, "HEAD~1").unwrap().tree
        );
        assert_ne!(r.tree, revision(&root, "HEAD").unwrap().tree);
        assert_eq!(r.commit.len(), 40);
        assert_eq!(revision(&root, &r.commit).unwrap().files, first);
        assert_eq!(revision(&root, "main~2").unwrap().tree, r.tree);
        // The files load as the project did then.
        let then = crate::doc::project::Project::load_from(&root, &r.files).unwrap();
        assert!(
            then.parts[&crate::server::part_id("a").unwrap()]
                .features
                .is_empty()
        );
        for bad in ["nowhere", "HEAD~9", "0123456789abcdef"] {
            let e = revision(&root, bad).unwrap_err();
            assert_eq!(
                (e.code, e.details["rev"].as_str()),
                (Code::RevNotFound, Some(bad))
            );
        }
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(
            revision(outside.path(), "HEAD").unwrap_err().code,
            Code::NotAGitRepo
        );
    }

    /// A repository with a project in `cad/` holding two plain files, and
    /// branch `theirs` that edits the first and deletes the second, while
    /// `main` edits both: a merge leaves both unmerged.
    fn diverged() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().to_path_buf();
        git(&top, &["init", "-q", "-b", "main"]);
        let root = top.join("cad");
        std::fs::create_dir(&root).unwrap();
        crate::server::init(&root, None).unwrap();
        let f = |name: &str, text: &str| {
            let p = root.join("parts/p/features").join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        f("x.json", "a\n");
        f("y.json", "y\n");
        std::fs::write(top.join("README"), "outside\n").unwrap();
        git(&top, &["add", "."]);
        git(&top, &["commit", "-q", "-m", "base"]);
        git(&top, &["checkout", "-q", "-b", "theirs"]);
        f("x.json", "b\n");
        std::fs::remove_file(root.join("parts/p/features/y.json")).unwrap();
        std::fs::write(top.join("README"), "theirs\n").unwrap();
        git(&top, &["commit", "-q", "-am", "theirs"]);
        git(&top, &["checkout", "-q", "main"]);
        f("x.json", "c\n");
        f("y.json", "ours\n");
        std::fs::write(top.join("README"), "ours\n").unwrap();
        git(&top, &["commit", "-q", "-am", "ours"]);
        (dir, root)
    }

    /// Runs git, which may fail.
    fn try_git(dir: &Path, args: &[&str]) -> bool {
        git_command()
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("GIT_EDITOR", "true")
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn a_merge_in_progress_and_what_it_left() {
        let (dir, root) = diverged();
        let top = dir.path();
        let repo = Repo::open(&root).unwrap().unwrap();
        assert_eq!(repo.merge_state().unwrap(), None);
        assert!(repo.unmerged().unwrap().is_empty());
        let rev = |r: &str| repo.commit(r).unwrap().id.to_string();
        let (base, ours, theirs) = (rev("main~1"), rev("main"), rev("theirs"));
        assert!(!try_git(top, &["merge", "theirs"]));
        let repo = Repo::open(&root).unwrap().unwrap();
        assert_eq!(
            repo.merge_state().unwrap(),
            Some(MergeState {
                step: Step::Merge,
                ours: ours.clone(),
                theirs: theirs.clone(),
                base: Some(base.clone()),
            })
        );
        // Only the project's files, by path in the project.
        let u = repo.unmerged().unwrap();
        assert_eq!(
            u.keys().collect::<Vec<_>>(),
            ["parts/p/features/x.json", "parts/p/features/y.json"]
        );
        let x = &u["parts/p/features/x.json"];
        assert_eq!(
            (x.base.as_deref(), x.ours.as_deref(), x.theirs.as_deref()),
            (Some(&b"a\n"[..]), Some(&b"c\n"[..]), Some(&b"b\n"[..]))
        );
        let y = &u["parts/p/features/y.json"];
        assert_eq!(
            (y.ours.as_deref(), y.theirs.as_deref()),
            (Some(&b"ours\n"[..]), None)
        );
        git(top, &["merge", "--abort"]);
        assert_eq!(repo.merge_state().unwrap(), None);
        // A cherry-pick merges the picked commit from its parent.
        assert!(!try_git(top, &["cherry-pick", "theirs"]));
        let s = repo.merge_state().unwrap().unwrap();
        assert_eq!(
            (s.step, s.theirs.as_str(), s.base.as_deref()),
            (Step::CherryPick, theirs.as_str(), Some(base.as_str()))
        );
        git(top, &["cherry-pick", "--abort"]);
        // A rebase step replays a commit the same way.
        git(top, &["checkout", "-q", "theirs"]);
        assert!(!try_git(top, &["rebase", "main"]));
        let s = repo.merge_state().unwrap().unwrap();
        assert_eq!(
            (
                s.step,
                s.ours.as_str(),
                s.theirs.as_str(),
                s.base.as_deref()
            ),
            (
                Step::Rebase,
                ours.as_str(),
                theirs.as_str(),
                Some(base.as_str())
            )
        );
        git(top, &["rebase", "--abort"]);
        // A revert merges the parent, from the reverted commit.
        git(top, &["checkout", "-q", "main"]);
        std::fs::write(root.join("parts/p/features/x.json"), "d\n").unwrap();
        git(top, &["commit", "-q", "-am", "after"]);
        assert!(!try_git(top, &["revert", "--no-edit", "main~1"]));
        let s = repo.merge_state().unwrap().unwrap();
        assert_eq!(
            (s.step, s.theirs.as_str(), s.base.as_deref()),
            (Step::Revert, base.as_str(), Some(ours.as_str()))
        );
        git(top, &["revert", "--abort"]);
        // An octopus is refused.
        let git_dir = top.join(".git");
        std::fs::write(git_dir.join("MERGE_HEAD"), format!("{theirs}\n{base}\n")).unwrap();
        assert_eq!(repo.merge_state().unwrap_err().code, Code::MergeUnsupported);
    }

    #[test]
    fn a_merge_concerns_every_project_either_side_has() {
        let (dir, root) = diverged();
        let top = dir.path();
        assert!(merging_projects(top).unwrap().is_empty());
        // A second project, only on theirs, and one only in the work tree.
        git(top, &["checkout", "-q", "theirs"]);
        let other = top.join("deep/other");
        std::fs::create_dir_all(&other).unwrap();
        crate::server::init(&other, None).unwrap();
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "another"]);
        git(top, &["checkout", "-q", "main"]);
        let stray = top.join("stray");
        std::fs::create_dir_all(&stray).unwrap();
        crate::server::init(&stray, None).unwrap();
        assert!(!try_git(top, &["merge", "theirs"]));
        let found = merging_projects(&root.join("parts")).unwrap();
        let canon = |p: &Path| p.canonicalize().unwrap();
        assert_eq!(
            found.iter().map(|p| canon(p)).collect::<Vec<_>>(),
            [canon(&root), canon(&other)]
        );
    }

    #[test]
    fn files_the_index_doesnt_hold_as_they_are() {
        let (dir, _s) = fresh();
        let r = dir.path();
        git(r, &["init", "-q"]);
        git(r, &["add", "."]);
        git(r, &["commit", "-q", "-m", "one"]);
        let repo = Repo::open(r).unwrap().unwrap();
        assert!(repo.unstaged(&files(r)).unwrap().is_empty());
        // Changed, never added, and not a project file at all.
        std::fs::write(r.join(crate::doc::project::MANIFEST), "{}\n").unwrap();
        std::fs::create_dir_all(r.join("parts/p/features/deeper")).unwrap();
        std::fs::write(r.join("parts/p/features/x.json"), "{}\n").unwrap();
        std::fs::write(r.join("parts/p/features/deeper/y.json"), "{}\n").unwrap();
        assert_eq!(
            repo.unstaged(&files(r)).unwrap(),
            ["parts/p/features/x.json", "ryucad.json"]
        );
        // Staged is settled; deleted but still staged isn't.
        git(r, &["add", "ryucad.json"]);
        std::fs::remove_file(r.join(crate::drivers::ATTRIBUTES)).unwrap();
        assert_eq!(
            repo.unstaged(&files(r)).unwrap(),
            [".gitattributes", "parts/p/features/x.json"]
        );
    }

    #[test]
    fn a_repository_with_no_commits() {
        let (dir, _s) = fresh();
        git(dir.path(), &["init", "-q"]);
        let v = history(dir.path(), &files(dir.path()), None, 10).unwrap();
        assert_eq!(v["repository"], true);
        assert_eq!(v["commits"], json!([]));
        assert_eq!(
            v["uncommitted"],
            json!([{"path": ".gitattributes", "change": "created"},
                   {"path": "ryucad.json", "change": "created"}])
        );
    }
}
