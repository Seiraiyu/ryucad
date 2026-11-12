//! The project's Git history, read-only: the commits that touched the
//! project (or one part), what is uncommitted, files as `HEAD` has them,
//! the whole project at any revision, and a merge in progress (its step,
//! each side's commit and the files Git left unmerged). No ryu product
//! writes to Git here; committing, and marking files resolved, stay with the
//! agent or user.

use crate::Layout;
use ryu_doc::error::{Code, Error, Result};
use ryu_session::fresh::Files;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// A project's repository, and where the project sits in it.
pub struct Repo<L: Layout> {
    repo: gix::Repository,
    /// The project root relative to the work tree, with `/`, ending in `/`
    /// unless it is the work tree itself (then empty).
    prefix: String,
    /// The product whose layout this reads; `fn() -> L` keeps `Repo`
    /// `Send` and `Sync` whatever `L` is.
    layout: PhantomData<fn() -> L>,
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
    /// The layout's root files and everything under its folders, by path
    /// relative to the project.
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
pub fn same_text(a: &[u8], b: &[u8]) -> bool {
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

impl<L: Layout> Repo<L> {
    /// The repository holding `root`, or `None` when there is none.
    pub fn open(root: &Path) -> Result<Option<Repo<L>>> {
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
        let mut prefix = ryu_doc::source::relative(Path::new(""), rel);
        if !prefix.is_empty() {
            prefix.push('/');
        }
        Ok(Some(Repo {
            repo,
            prefix,
            layout: PhantomData,
        }))
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
    /// project: the layout's root files and everything under its folders.
    fn project_blobs(&self, tree: &gix::Tree<'_>) -> Result<BTreeMap<String, gix::ObjectId>> {
        let mut out = BTreeMap::new();
        for name in L::ROOT_FILES {
            if let Some(id) = self.entry(tree, &format!("{}{name}", self.prefix))? {
                out.insert((*name).to_owned(), id);
            }
        }
        for dir in L::FOLDERS {
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
        let head = self.head().map(|c| c.id.to_string());
        self.changed_since(head.as_deref(), files)
    }

    /// The project's files in `files` that differ from the commit `id`
    /// (hex; `None` before the first commit, when every file is new):
    /// another worktree's uncommitted changes, from its head.
    pub fn changed_since(&self, id: Option<&str>, files: &Files) -> Result<Vec<Value>> {
        let head = match id {
            Some(c) => self.project_blobs(&self.commit(c)?.tree().map_err(git_error)?)?,
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
            files.insert(path, self.read(id)?.into());
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

    /// The size and time of each file Git keeps while a step waits, and
    /// of the index: what a merge status read from can have moved by,
    /// though the state's key reads the same (a file rewritten as it was).
    pub fn merge_stamp(&self) -> String {
        let dir = self.repo.git_dir();
        let stamp = |p: &Path| match std::fs::metadata(p) {
            Ok(m) => {
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                format!("{}:{t}", m.len())
            }
            Err(_) => String::new(),
        };
        let mut out = String::new();
        for name in WAITING {
            out.push_str(&stamp(&dir.join(name)));
            out.push(';');
        }
        out.push_str(&stamp(&self.repo.index_path()));
        out
    }

    /// The merge of `heads` into `HEAD` that `step` makes.
    fn merging(&self, step: Step, heads: &[String]) -> Result<MergeState> {
        let head = match heads {
            [h] => h,
            _ => {
                return Err(Error::new(
                    Code::MergeUnsupported,
                    format!(
                        "an octopus merge of {} heads: {} reports on merges of two",
                        heads.len() + 1,
                        L::NAME
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
                if !L::project_file(rel) {
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

    /// The files of `files` the index holds at no stage: new, never
    /// `git add`ed. [`Repo::unstaged`] lists them too, since a commit
    /// would leave them out; `merge.status` names them apart, as nothing
    /// the merge settled.
    pub fn untracked(&self, files: &Files) -> Result<Vec<String>> {
        let mut held = std::collections::BTreeSet::new();
        if let Ok(index) = self.repo.open_index() {
            for e in index.entries() {
                let path = e.path(&index).to_string();
                if let Some(rel) = path.strip_prefix(&self.prefix) {
                    held.insert(rel.to_owned());
                }
            }
        }
        Ok(files
            .keys()
            .filter(|p| !held.contains(p.as_str()))
            .cloned()
            .collect())
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

/// A worktree of the repository holding the project: the clone's own
/// (the main worktree) or one `git worktree add` made. Each is a project
/// folder of its own, with its own daemon.
#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    /// The project folder in it.
    pub root: PathBuf,
    /// Its work tree.
    pub top: PathBuf,
    /// The clone's own worktree, which `git worktree remove` can't take.
    pub main: bool,
    /// The worktree the repository was opened from.
    pub current: bool,
    /// The branch checked out, or `None` on a detached head.
    pub branch: Option<String>,
    /// The commit `HEAD` points at, in hex; `None` before the first commit.
    pub head: Option<String>,
    /// `git worktree lock` keeps it from being moved or removed.
    pub locked: bool,
    /// Its private Git folder: the repository's own for the main worktree,
    /// `.git/worktrees/<id>` for the others.
    pub git_dir: PathBuf,
}

impl Worktree {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.root,
            "top": self.top,
            "main": self.main,
            "current": self.current,
            "branch": self.branch,
            "head": self.head,
            "locked": self.locked,
        })
    }
}

/// The error for a path that isn't a worktree holding the project.
fn no_worktree(path: &Path, why: &str) -> Error {
    Error::new(
        Code::WorktreeNotFound,
        format!(
            "{} is not a worktree of this repository holding this project: {why}",
            path.display()
        ),
    )
    .with("path", path)
}

impl<L: Layout> Repo<L> {
    /// Every worktree of the repository that holds the project (its
    /// manifest at the same place): the main worktree first, then the
    /// others as Git keeps them. One whose folder is gone, or whose
    /// branch holds no project, is left out.
    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        let here = self
            .repo
            .workdir()
            .and_then(|w| w.canonicalize().ok())
            .unwrap_or_default();
        let mut out = Vec::new();
        let main = self.repo.main_repo().map_err(git_error)?;
        let mut found: Vec<(gix::Repository, bool, bool)> = vec![(main, true, false)];
        for proxy in self.repo.worktrees().map_err(git_error)? {
            let locked = proxy.is_locked();
            let Ok(r) = proxy.into_repo_with_possibly_inaccessible_worktree() else {
                continue;
            };
            found.push((r, false, locked));
        }
        for (r, main, locked) in found {
            let Some(top) = r.workdir().and_then(|w| w.canonicalize().ok()) else {
                continue;
            };
            let root = self
                .prefix
                .split('/')
                .filter(|s| !s.is_empty())
                .fold(top.clone(), |p, s| p.join(s));
            if !root.join(L::MANIFEST).is_file() {
                continue;
            }
            let branch = r
                .head_name()
                .map_err(git_error)?
                .map(|n| n.shorten().to_string());
            let head = r.head_id().ok().map(|id| id.to_string());
            out.push(Worktree {
                root: ryu_doc::source::plain(root),
                current: top == here,
                top: ryu_doc::source::plain(top),
                main,
                branch,
                head,
                locked,
                git_dir: r.git_dir().to_path_buf(),
            });
        }
        Ok(out)
    }

    /// The worktree `path` names: its work tree or the project folder in
    /// it. Anything else is `worktree_not_found`.
    pub fn worktree(&self, path: &Path) -> Result<Worktree> {
        let canon = path
            .canonicalize()
            .map_err(|e| no_worktree(path, &e.to_string()))?;
        let canon = ryu_doc::source::plain(canon);
        let all = self.worktrees()?;
        let listed: Vec<&PathBuf> = all.iter().map(|w| &w.root).collect();
        let listed = json!(listed);
        all.iter()
            .find(|w| w.root == canon || w.top == canon)
            .cloned()
            .ok_or_else(|| {
                no_worktree(path, "Git lists no worktree there that holds it")
                    .with("worktrees", listed)
            })
    }

    /// How many commits `a` has that `b` doesn't, and `b` has that `a`
    /// doesn't (both in hex): ahead and behind.
    pub fn ahead_behind(&self, a: &str, b: &str) -> Result<(usize, usize)> {
        let id = |x: &str| gix::ObjectId::from_hex(x.as_bytes()).map_err(git_error);
        let (a, b) = (id(a)?, id(b)?);
        let count = |from: gix::ObjectId, hide: gix::ObjectId| -> Result<usize> {
            let walk = self
                .repo
                .rev_walk([from])
                .with_hidden([hide])
                .all()
                .map_err(git_error)?;
            let mut n = 0;
            for info in walk {
                info.map_err(git_error)?;
                n += 1;
            }
            Ok(n)
        };
        Ok((count(a, b)?, count(b, a)?))
    }

    /// The merge base of two commits (hex), if they have one.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let id = |x: &str| gix::ObjectId::from_hex(x.as_bytes()).map_err(git_error);
        Ok(self
            .repo
            .merge_base(id(a)?, id(b)?)
            .ok()
            .flatten()
            .map(|b| b.to_string()))
    }

    /// The commit `HEAD` points at, in hex, if there is one.
    pub fn head_commit(&self) -> Option<String> {
        self.head().map(|c| c.id.to_string())
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

/// The folders under `tree` (paths below `at`) that hold the product's
/// manifest.
fn manifests<L: Layout>(
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
            manifests::<L>(repo, e.oid.to_owned(), &format!("{at}{name}/"), out)?;
        } else if name == L::MANIFEST {
            out.push(at.to_owned());
        }
    }
    Ok(())
}

/// The projects of the repository holding `here` that a merge in
/// progress concerns: every folder with the product's manifest in `HEAD` or in
/// the commit being merged that the work tree still has, sorted. Empty,
/// at once, when Git is in no merge (as for every ordinary commit).
pub fn merging_projects<L: Layout>(here: &Path) -> Result<Vec<std::path::PathBuf>> {
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
        manifests::<L>(&repo, tree, "", &mut folders)?;
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
        .filter(|p| p.join(L::MANIFEST).is_file())
        .collect())
}

/// The repository holding the project at `root`, or `not_a_git_repo`.
pub fn repo<L: Layout>(root: &Path) -> Result<Repo<L>> {
    Repo::open(root)?.ok_or_else(|| no_repo(root))
}

/// The project at `root` as `rev` has it: `not_a_git_repo` outside a
/// repository, `rev_not_found` for a revision it doesn't have.
pub fn revision<L: Layout>(root: &Path, rev: &str) -> Result<Revision> {
    repo::<L>(root)?.revision(rev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Toy, git, write};

    fn files(pairs: &[(&str, &str)]) -> Files {
        pairs
            .iter()
            .map(|(p, t)| ((*p).to_owned(), t.as_bytes().into()))
            .collect()
    }

    #[test]
    fn a_layouts_files_at_a_revision_and_in_a_merge() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        git(top, &["init", "-q"]);
        write(top, "sub/toy.json", "{}\n");
        write(top, "sub/things/a.json", "a\n");
        write(top, "sub/other/b.json", "b\n");
        write(top, "README", "outside\n");
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "one"]);
        let root = top.join("sub");
        let repo = Repo::<Toy>::open(&root).unwrap().unwrap();
        assert_eq!(repo.prefix(), "sub/");
        let r = repo.revision("HEAD").unwrap();
        assert_eq!(
            r.files.keys().collect::<Vec<_>>(),
            ["things/a.json", "toy.json"]
        );
        // A merge Git stopped: its state, and the paths it left.
        git(top, &["checkout", "-q", "-b", "theirs"]);
        write(top, "sub/things/a.json", "theirs\n");
        git(top, &["commit", "-q", "-am", "theirs"]);
        git(top, &["checkout", "-q", "main"]);
        write(top, "sub/things/a.json", "ours\n");
        git(top, &["commit", "-q", "-am", "ours"]);
        let out = crate::testing::git_command()
            .args(["merge", "theirs"])
            .current_dir(top)
            .output()
            .unwrap();
        assert!(!out.status.success());
        let s = repo.merge_state().unwrap().unwrap();
        assert_eq!(s.step, Step::Merge);
        assert_eq!(
            repo.unmerged().unwrap().keys().collect::<Vec<_>>(),
            ["things/a.json"]
        );
        let merging: Vec<_> = merging_projects::<Toy>(top)
            .unwrap()
            .iter()
            .map(|p| p.canonicalize().unwrap())
            .collect();
        assert_eq!(merging, [root.canonicalize().unwrap()]);
        // Only project files count as unstaged.
        assert_eq!(
            repo.unstaged(&files(&[("toy.json", "{}\n"), ("things/new.json", "n\n")]))
                .unwrap(),
            ["things/new.json"]
        );
        // An octopus names the product.
        let heads = format!("{}\n{}\n", r.commit, r.commit);
        std::fs::write(top.join(".git/MERGE_HEAD"), heads).unwrap();
        let e = repo.merge_state().unwrap_err();
        assert!(
            e.message.contains("toy reports on merges of two"),
            "{}",
            e.message
        );
    }

    #[test]
    fn crlf_is_lf() {
        assert!(same_text(b"a\r\nb\r\n", b"a\nb\n"));
        assert!(!same_text(b"a\nb", b"a\nc"));
    }
}
