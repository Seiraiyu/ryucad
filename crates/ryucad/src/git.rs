//! The project's Git history, read-only: the commits that touched the
//! project (or one part), what is uncommitted, and files as `HEAD` has
//! them. ryucad never writes to Git; committing stays with the agent or
//! user.

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

/// The warning for a project outside any repository.
pub fn not_a_repo(root: &Path) -> Value {
    json!(
        Error::new(
            Code::NotAGitRepo,
            format!("{} is not in a Git repository", root.display())
        )
        .with("path", root)
    )
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
    /// project: the manifest and everything under `parts/`.
    fn project_blobs(&self, tree: &gix::Tree<'_>) -> Result<BTreeMap<String, gix::ObjectId>> {
        let mut out = BTreeMap::new();
        if let Some(id) = self.entry(
            tree,
            &format!("{}{}", self.prefix, crate::doc::project::MANIFEST),
        )? {
            out.insert(crate::doc::project::MANIFEST.to_owned(), id);
        }
        if let Some(id) = self.entry(tree, &format!("{}parts", self.prefix))? {
            self.blobs(id, "parts/", &mut out)?;
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
    use crate::session::Session;
    use std::process::Command;

    /// Runs git in `dir` with a fixed identity.
    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
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
    fn a_repository_with_no_commits() {
        let (dir, _s) = fresh();
        git(dir.path(), &["init", "-q"]);
        let v = history(dir.path(), &files(dir.path()), None, 10).unwrap();
        assert_eq!(v["repository"], true);
        assert_eq!(v["commits"], json!([]));
        assert_eq!(
            v["uncommitted"],
            json!([{"path": "ryucad.json", "change": "created"}])
        );
    }
}
