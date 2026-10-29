//! Agents on branches: the project's worktrees, each a project folder of
//! its own with its own daemon, all sharing the machine's disk cache. Git's
//! worktree list is the registry; ryucad keeps none of its own. `list`
//! reads it and, for each worktree, what its working files and its daemon
//! (only if one is running) say; `add` and `remove` run `git worktree`,
//! the one place ryucad runs a Git command that writes the repository.

use crate::error::{Code, Error, Result};
use crate::git::Worktree;
use crate::live::Live;
use crate::session::{Session, WORKTREE};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The file in a worktree's private Git folder naming the participant an
/// agent will use there (`worktree add --as`), shown until its daemon
/// says who is attached. Git removes it with the worktree.
pub const AS_FILE: &str = "ryucad-as";

/// The name `worktree add --as` recorded for a worktree, if any.
pub fn recorded_as(w: &Worktree) -> Option<String> {
    let text = std::fs::read_to_string(w.git_dir.join(AS_FILE)).ok()?;
    let name = text.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// `worktree.list`: every worktree of the repository that holds the
/// project, this one first among equals marked `current`: its branch and
/// head, how many commits it is ahead of and behind this worktree's head,
/// how many project files its working files change from its own head, a
/// digest of them (`state`: equal states, equal files), and, only when its
/// daemon is already running, who is attached and what was done last.
/// It never starts a daemon. `live` is this daemon's session, if this
/// request came through one: it answers for this worktree.
pub fn list(session: &mut Session, live: Option<&Live>) -> Result<Value> {
    let repo = crate::git::repo(session.root())?;
    let here = repo.head_commit();
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for w in repo.worktrees()? {
        let mut v = w.to_json();
        let (files, digest) = if w.current {
            (session.snapshot(), session.digest())
        } else {
            let rev = format!("{WORKTREE}{}", w.root.display());
            match session.worktree_files(&rev)? {
                Some((_, f, d)) => (f, d),
                None => continue,
            }
        };
        let (ahead, behind) = match (&w.head, &here) {
            (Some(a), Some(b)) => repo.ahead_behind(a, b)?,
            _ => (0, 0),
        };
        v["ahead"] = json!(ahead);
        v["behind"] = json!(behind);
        let key = format!("{}\0{digest}", w.head.as_deref().unwrap_or_default());
        let changes = match session.worktree_changes.get(&key) {
            Some(n) => *n,
            None => {
                let n = repo.changed_since(w.head.as_deref(), &files)?.len();
                session.worktree_changes.insert(key.clone(), n);
                n
            }
        };
        seen.insert(key);
        v["changes"] = json!(changes);
        v["dirty"] = json!(changes > 0);
        v["state"] = json!(&digest[..16]);
        v["as"] = json!(recorded_as(&w));
        v["daemon"] = match (w.current, live) {
            (true, Some(l)) => json!({
                "running": true,
                "pid": std::process::id(),
                "participants": l.participants()
                    .map(|p| json!({"id": p.id, "kind": p.kind, "name": p.name,
                                    "status": p.status}))
                    .collect::<Vec<_>>(),
                "last_activity": l.log.entries().next_back().map(|e| json!({
                    "seq": e.seq, "time_ms": e.time_ms, "kind": e.kind,
                    "participant": e.participant, "method": e.method})),
            }),
            _ => daemon(&w.root),
        };
        out.push(v);
    }
    let current = out.iter().find(|w| w["current"] == true).cloned();
    session.worktree_changes.retain(|k, _| seen.contains(k));
    Ok(json!({
        "path": session.root(),
        "branch": current.as_ref().map(|c| c["branch"].clone()),
        "head": here,
        "worktrees": out,
    }))
}

/// What a worktree's daemon says of itself, without starting one: `null`
/// when none is running, else its pid, version, participants and last
/// activity (`busy` while a request holds it, when it can't say who is
/// attached).
fn daemon(root: &Path) -> Value {
    match crate::client::status(root) {
        Ok(s) if s["running"] == true => {
            let mut v = json!({"running": true});
            for k in ["pid", "version", "busy", "participants", "last_activity"] {
                if let Some(x) = s.get(k) {
                    v[k] = x.clone();
                }
            }
            v
        }
        _ => Value::Null,
    }
}

/// The folder a new worktree goes in: `path`, made absolute against
/// `here`; it must not exist yet (Git makes it).
fn new_folder(here: &Path, path: &Path) -> Result<PathBuf> {
    let p = here.join(path);
    if p.exists() {
        return Err(Error::new(
            Code::InvalidParam,
            format!(
                "{} exists already: a new worktree needs a new folder",
                p.display()
            ),
        )
        .with("path", &p));
    }
    Ok(p)
}

/// Runs `git` in `dir`; its standard output, or `git_failed`'s error with
/// what Git said.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| Error::new(Code::Io, format!("running git: {e}")))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(Error::new(
            Code::InvalidParam,
            format!("git {} failed: {said}", args.join(" ")),
        )
        .with("git", args)
        .with("stderr", said));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What `worktree add` is asked for.
pub struct Add<'a> {
    /// The new worktree's folder, relative to where the command runs.
    pub path: &'a Path,
    /// A branch to create there, from `from`.
    pub branch: Option<&'a str>,
    /// What to check out (or to start the branch from): `HEAD` by
    /// default.
    pub from: Option<&'a str>,
    /// The participant name an agent will use there.
    pub name: Option<&'a str>,
    /// Rebuild the new worktree once, so its daemon starts warm: from the
    /// disk cache this worktree filled, almost nothing builds.
    pub warm: bool,
}

/// `ryucad worktree add`: a new worktree of the project's repository,
/// through `git worktree add` (a branch only when asked for, else the
/// revision detached), checked for ryucad's drivers and hooks (each
/// repository's, so every worktree shares them; what is missing is
/// reported with `ryucad git install`), its participant name recorded, and
/// its cache warmed. Answers the new project folder.
pub fn add(root: &Path, here: &Path, ask: &Add) -> Result<Value> {
    let repo = crate::git::repo(root)?;
    let folder = new_folder(here, ask.path)?;
    let from = ask.from.unwrap_or("HEAD");
    repo.commit(from)?;
    let top = repo
        .gix()
        .workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::new(Code::NotAGitRepo, "the repository has no work tree"))?;
    let target = folder.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add", "--quiet"];
    match ask.branch {
        Some(b) => args.extend(["-b", b]),
        None => args.push("--detach"),
    }
    args.extend([target.as_str(), from]);
    git(&top, &args)?;
    let repo = crate::git::repo(root)?;
    let w = repo.worktree(&folder)?;
    if let Some(name) = ask.name {
        std::fs::write(w.git_dir.join(AS_FILE), format!("{name}\n"))
            .map_err(|e| Error::io(&w.git_dir, &e))?;
    }
    let mut warnings = Vec::new();
    if let Some(setup) = crate::drivers::setup(&w.root) {
        let hooks: Vec<&str> = setup
            .hooks
            .iter()
            .filter(|(_, h)| !matches!(h, crate::drivers::Hook::Installed(_)))
            .map(|(n, _)| *n)
            .collect();
        if !setup.missing.is_empty() || !hooks.is_empty() {
            warnings.push(json!({
                "code": "git_drivers_missing",
                "message": "the repository lacks ryucad's Git drivers or hooks: run `ryucad git install` so merges between worktrees go through ryucad",
                "missing": setup.missing,
                "hooks": hooks,
                "repair": "ryucad git install",
            }));
        }
    }
    let warm = if ask.warm {
        let req = crate::proto::Request::CacheWarm { part: None };
        let a = crate::server::handle_safely(&mut None, None, None, &w.root, req)?;
        Some(a.result)
    } else {
        None
    };
    let mut v = w.to_json();
    v["as"] = json!(ask.name);
    v["warm"] = json!(warm);
    v["warnings"] = json!(warnings);
    Ok(v)
}

/// `ryucad worktree remove`: stops the worktree's daemon and removes it
/// through `git worktree remove`, refused (`worktree_dirty`) while its
/// project files differ from its head, as Git refuses for any change,
/// unless `force`. The main worktree and this one can't be removed.
pub fn remove(root: &Path, path: &Path, force: bool) -> Result<Value> {
    let repo = crate::git::repo(root)?;
    let w = repo.worktree(path)?;
    if w.main || w.current {
        let which = if w.main {
            "the repository's own"
        } else {
            "this one"
        };
        return Err(Error::new(
            Code::InvalidParam,
            format!("{} is {which} worktree: remove another", w.root.display()),
        )
        .with("path", &w.root));
    }
    if !force {
        let files = crate::activity::snapshot(&w.root)?;
        let changed = repo.changed_since(w.head.as_deref(), &files)?;
        if !changed.is_empty() {
            return Err(Error::new(
                Code::WorktreeDirty,
                format!(
                    "{} has {} uncommitted change(s) to the project: commit them, or remove it with --force",
                    w.root.display(),
                    changed.len()
                ),
            )
            .with("path", &w.root)
            .with("changes", changed));
        }
    }
    let stopped = crate::client::stop(&w.root)?["stopped"] == true;
    let target = w.top.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(target.as_str());
    let top = repo
        .gix()
        .workdir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.to_path_buf());
    let git_said = git(&top, &args);
    match git_said {
        Ok(_) => Ok(json!({"removed": w.root, "top": w.top, "daemon_stopped": stopped})),
        // Git refuses for changes outside the project too.
        Err(e) => Err(Error::new(
            Code::WorktreeDirty,
            format!("git refused to remove {}: {}", w.top.display(), e.message),
        )
        .with("path", &w.root)
        .with("stderr", e.details["stderr"].clone())),
    }
}
