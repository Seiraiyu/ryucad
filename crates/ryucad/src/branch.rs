//! Branches from the app (design 447): the repository's branches as
//! `branch.list` gives them, read through gix, and `branch.switch`, which
//! checks one out in the project's work tree through `git switch`: after
//! `worktree add`, the second place ryucad runs a Git command that writes
//! the working tree. It stages nothing and commits nothing, and refuses
//! while project files differ from `HEAD` or a merge is in progress, so
//! Git never carries the project's uncommitted changes across.

use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// `branch.list`: every local branch, then every remote-tracking branch
/// with no local branch of its name (what `git switch <name>` would make
/// from it), each with its tip, upstream, commits ahead of and behind
/// this worktree's head, whether it is checked out here (`current`), and
/// the project folder of the worktree it is checked out in (`worktree`;
/// this folder for the current branch). With the branch checked out here
/// (`null` on a detached head), the head, and the project revision a
/// switch can be guarded by (`project`).
pub fn list(session: &Session) -> Result<Value> {
    let repo = crate::git::repo(session.root())?;
    let here = repo.head_commit();
    let current = repo.branch()?;
    let held: BTreeMap<String, PathBuf> = repo
        .worktrees()?
        .into_iter()
        .filter_map(|w| Some((w.branch?, w.root)))
        .collect();
    let mut out = Vec::new();
    for b in repo.branches()? {
        let (ahead, behind) = match &here {
            Some(h) => repo.ahead_behind(&b.head, h)?,
            None => (0, 0),
        };
        let local = b.remote.is_none();
        let is_current = local && current.as_deref() == Some(b.name.as_str());
        let worktree = if local {
            held.get(&b.name).cloned()
        } else {
            None
        };
        out.push(json!({
            "name": b.name,
            "ref": b.full,
            "remote": b.remote,
            "head": b.head,
            "upstream": b.upstream,
            "ahead": ahead,
            "behind": behind,
            "current": is_current,
            "worktree": worktree,
        }));
    }
    Ok(json!({
        "current": current,
        "head": here,
        "project": session.project_revision()?,
        "branches": out,
    }))
}

/// `branch.switch`: checks out `branch` in the project's work tree (`git
/// switch`, which makes a local branch from a remote-tracking one of that
/// name). Refused while the project isn't at `expected` (`conflict`),
/// while Git is in a merge, cherry-pick, revert or rebase step
/// (`merge_in_progress`), for a branch the repository doesn't have
/// (`rev_not_found`) or another worktree has checked out
/// (`branch_in_worktree`), and while any project file differs from
/// `HEAD`, staged or not, created or deleted (`checkout_dirty`). Files
/// outside the project are Git's to judge (`git_failed`, with what it
/// said). The session takes in every file before answering, and a live
/// session logs one `switch` entry by the caller, which undo never takes.
/// Answers `{switched, from, to, head, project, files, entry}`; the
/// branch already checked out is `switched: false`, and nothing runs.
pub fn switch(
    session: &mut Session,
    live: Option<&mut Live>,
    me: Option<&str>,
    branch: &str,
    expected: Option<&str>,
) -> Result<Value> {
    let root = session.root().to_path_buf();
    let repo = crate::git::repo(&root)?;
    session.check_project_revision(expected)?;
    if let Some(m) = repo.merge_state()? {
        let step = m.step.as_str();
        return Err(Error::new(
            Code::MergeInProgress,
            format!("a {step} is in progress: finish or abort it in Git before switching branches"),
        )
        .with("step", step));
    }
    let from = repo.branch()?;
    let all = repo.branches()?;
    let Some(target) = all.iter().find(|b| b.name == branch) else {
        let names: Vec<&str> = all.iter().map(|b| b.name.as_str()).collect();
        return Err(Error::new(
            Code::RevNotFound,
            format!("no branch {branch} in the repository"),
        )
        .with("rev", branch)
        .with("branches", names));
    };
    if from.as_deref() == Some(branch) {
        return Ok(json!({
            "switched": false,
            "from": from,
            "to": branch,
            "head": repo.head_commit(),
            "project": session.project_revision()?,
            "files": [],
            "entry": null,
        }));
    }
    if target.remote.is_none()
        && let Some(w) = repo
            .worktrees()?
            .into_iter()
            .find(|w| !w.current && w.branch.as_deref() == Some(branch))
    {
        return Err(Error::new(
            Code::BranchInWorktree,
            format!(
                "{branch} is checked out in the worktree at {}: open it there",
                w.root.display()
            ),
        )
        .with("branch", branch)
        .with("worktree", &w.root));
    }
    let files = session.snapshot();
    let mut dirty: BTreeSet<String> = repo
        .uncommitted(&files)?
        .iter()
        .filter_map(|f| f["path"].as_str().map(str::to_owned))
        .collect();
    dirty.extend(repo.unstaged(&files)?);
    if !dirty.is_empty() {
        return Err(Error::new(
            Code::CheckoutDirty,
            format!(
                "{} project file(s) differ from HEAD: commit or stash them in Git first (ryucad never commits)",
                dirty.len()
            ),
        )
        .with("files", dirty));
    }
    let top = repo
        .gix()
        .workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::new(Code::NotAGitRepo, "the repository has no work tree"))?;
    drop(repo);
    crate::worktree::git(&top, &["switch", "--quiet", branch]).map_err(|e| {
        let said = e.details["stderr"]
            .as_str()
            .unwrap_or(&e.message)
            .to_owned();
        Error::new(
            Code::GitFailed,
            format!("git switch {branch} failed: {said}"),
        )
        .with("git", ["switch", branch])
        .with("stderr", said)
    })?;
    let seen = session.rescan()?;
    let head = crate::git::repo(&root)?.head_commit();
    let changed: Vec<Value> = seen
        .iter()
        .map(|(p, c)| json!({"path": p, "change": c}))
        .collect();
    let entry = live.map(|l| {
        let who = l.caller(me);
        crate::activity::record_switch(l, &who, json!({"from": from, "to": branch}), seen)
    });
    Ok(json!({
        "switched": true,
        "from": from,
        "to": branch,
        "head": head,
        "project": session.project_revision()?,
        "files": changed,
        "entry": entry,
    }))
}

#[cfg(test)]
mod tests {
    use crate::error::Code;
    use crate::git::tests::git;
    use crate::session::Session;
    use crate::testing::{call, fresh};
    use serde_json::{Value, json};

    /// A project in a repository of its own on `main` with a box
    /// committed, and branch `arm` one commit ahead, the box 15 mm long
    /// there.
    pub(crate) fn two_branches() -> (tempfile::TempDir, Session) {
        let (dir, mut s) = fresh();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        call(&mut s, "part.create", json!({"part": "p"})).unwrap();
        let params = json!({"x": 10, "y": 10, "z": 5});
        call(
            &mut s,
            "feature.add",
            json!({"part": "p", "type": "box", "id": "b", "params": params}),
        )
        .unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "a box"]);
        git(&root, &["switch", "-q", "-c", "arm"]);
        s.reload().unwrap();
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 15}}),
        )
        .unwrap();
        git(&root, &["commit", "-q", "-am", "longer"]);
        git(&root, &["switch", "-q", "main"]);
        s.reload().unwrap();
        (dir, s)
    }

    /// The row of `branch.list`'s answer named `name`.
    pub(crate) fn row<'a>(v: &'a Value, name: &str) -> &'a Value {
        v["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == name)
            .unwrap_or_else(|| panic!("{name} not in {v}"))
    }

    #[test]
    fn branches_say_how_they_stand_and_where_they_are_checked_out() {
        let (dir, mut s) = two_branches();
        let v = call(&mut s, "branch.list", json!({})).unwrap();
        assert_eq!(v["current"], "main");
        assert_eq!(v["project"], json!(s.project_revision().unwrap()));
        let here = crate::doc::project::plain(dir.path().canonicalize().unwrap());
        let main = row(&v, "main");
        assert_eq!(main["ref"], "refs/heads/main");
        assert_eq!(
            (&main["current"], &main["ahead"], &main["behind"]),
            (&json!(true), &json!(0), &json!(0))
        );
        assert_eq!(main["worktree"], json!(here));
        let arm = row(&v, "arm");
        assert_eq!(
            (&arm["current"], &arm["ahead"], &arm["behind"]),
            (&json!(false), &json!(1), &json!(0))
        );
        assert_eq!(arm["worktree"], Value::Null);
        assert_eq!(arm["upstream"], Value::Null);
        assert_eq!(arm["remote"], Value::Null);
        // Checked out in another worktree, a branch says where.
        let other = tempfile::tempdir().unwrap();
        let wt = other.path().join("wt");
        git(
            dir.path(),
            &["worktree", "add", "-q", wt.to_str().unwrap(), "arm"],
        );
        let v = call(&mut s, "branch.list", json!({})).unwrap();
        let held = crate::doc::project::plain(wt.canonicalize().unwrap());
        assert_eq!(row(&v, "arm")["worktree"], json!(held));
        git(dir.path(), &["worktree", "remove", wt.to_str().unwrap()]);
        // Outside a repository there are none.
        let (_alone_dir, mut alone) = fresh();
        let e = call(&mut alone, "branch.list", json!({})).unwrap_err();
        assert_eq!(e.code, crate::error::Code::NotAGitRepo);
    }

    #[test]
    fn a_clean_switch_checks_the_branch_out_and_reloads_at_once() {
        let (dir, mut s) = two_branches();
        let a = call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap();
        assert_eq!(
            (&a["switched"], &a["from"], &a["to"]),
            (&json!(true), &json!("main"), &json!("arm"))
        );
        let files = a["files"].as_array().unwrap();
        assert!(
            files
                .iter()
                .any(|f| f["path"] == "parts/p/features/b.json" && f["change"] == "modified"),
            "{a}"
        );
        // In process there is no log to write to.
        assert_eq!(a["entry"], Value::Null);
        let repo = crate::git::repo(dir.path()).unwrap();
        assert_eq!(repo.branch().unwrap().as_deref(), Some("arm"));
        assert_eq!(a["head"], json!(repo.head_commit()));
        // The session holds arm's files before it answers.
        let on_disk = std::fs::read(dir.path().join("parts/p/features/b.json")).unwrap();
        assert_eq!(
            &*s.snapshot()["parts/p/features/b.json"],
            on_disk.as_slice()
        );
        assert_eq!(a["project"], json!(s.project_revision().unwrap()));
        // Again: nothing to do.
        let again = call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap();
        assert_eq!(again["switched"], false);
        // And back.
        let back = call(&mut s, "branch.switch", json!({"branch": "main"})).unwrap();
        assert_eq!(
            (&back["from"], &back["to"]),
            (&json!("arm"), &json!("main"))
        );
    }

    #[test]
    fn project_changes_from_head_refuse_a_switch() {
        let (dir, mut s) = two_branches();
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        call(&mut s, "part.create", json!({"part": "q"})).unwrap();
        let e = call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap_err();
        assert_eq!(e.code, Code::CheckoutDirty);
        let files = e.details["files"].as_array().unwrap();
        assert!(
            files.contains(&json!("parts/p/features/b.json")),
            "{files:?}"
        );
        assert!(files.contains(&json!("parts/q/part.json")), "{files:?}");
        // Staged is still uncommitted.
        git(dir.path(), &["add", "."]);
        let e = call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap_err();
        assert_eq!(e.code, Code::CheckoutDirty);
        let repo = crate::git::repo(dir.path()).unwrap();
        assert_eq!(repo.branch().unwrap().as_deref(), Some("main"));
        // Committed, it goes; a file outside the project is Git's to
        // judge, and Git keeps an untracked one.
        git(dir.path(), &["commit", "-q", "-m", "q and shorter"]);
        std::fs::write(dir.path().join("notes.txt"), "mine\n").unwrap();
        call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap();
        assert!(dir.path().join("notes.txt").is_file());
        assert!(!s.project().parts.keys().any(|id| id.to_string() == "q"));
    }

    #[test]
    fn a_merge_another_worktree_and_unknown_branches_refuse_a_switch() {
        let (dir, mut s) = two_branches();
        let root = dir.path();
        let e = call(&mut s, "branch.switch", json!({"branch": "nope"})).unwrap_err();
        assert_eq!(e.code, Code::RevNotFound);
        assert_eq!(e.details["branches"], json!(["arm", "main"]));
        // Not at the revision the caller read.
        let stale = json!({"branch": "arm", "expected_revision": "0000"});
        let e = call(&mut s, "branch.switch", stale).unwrap_err();
        assert_eq!(e.code, Code::Conflict);
        // Checked out in another worktree.
        let other = tempfile::tempdir().unwrap();
        let wt = other.path().join("wt");
        git(
            root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "arm"],
        );
        let e = call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap_err();
        assert_eq!(e.code, Code::BranchInWorktree);
        let held = crate::doc::project::plain(wt.canonicalize().unwrap());
        assert_eq!(e.details["worktree"], json!(held));
        assert_eq!(e.details["branch"], "arm");
        git(root, &["worktree", "remove", wt.to_str().unwrap()]);
        // In a merge that stopped for conflicts: both sides set the box's
        // length.
        call(
            &mut s,
            "feature.edit",
            json!({"part": "p", "id": "b", "params": {"x": 12}}),
        )
        .unwrap();
        git(root, &["commit", "-q", "-am", "shorter"]);
        let merged = crate::git::tests::git_command()
            .args(["-c", "user.name=Test", "-c", "user.email=t@example.com"])
            .args(["merge", "arm"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(!merged.status.success(), "the merge should stop");
        let e = call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap_err();
        assert_eq!(e.code, Code::MergeInProgress);
        assert_eq!(e.details["step"], "merge");
        git(root, &["merge", "--abort"]);
    }

    #[test]
    fn a_remote_tracking_branch_is_checked_out_as_git_guesses() {
        let (dir, _s) = two_branches();
        let other = tempfile::tempdir().unwrap();
        let url = format!("file://{}", dir.path().display());
        git(other.path(), &["clone", "-q", &url, "down"]);
        let mut s = Session::open(&other.path().join("down")).unwrap();
        let v = call(&mut s, "branch.list", json!({})).unwrap();
        let arm = row(&v, "arm");
        assert_eq!(
            (&arm["remote"], &arm["ref"], &arm["current"]),
            (
                &json!("origin"),
                &json!("refs/remotes/origin/arm"),
                &json!(false)
            )
        );
        assert_eq!(row(&v, "main")["upstream"], "origin/main");
        call(&mut s, "branch.switch", json!({"branch": "arm"})).unwrap();
        let v = call(&mut s, "branch.list", json!({})).unwrap();
        let arm = row(&v, "arm");
        assert_eq!(
            (&arm["remote"], &arm["upstream"], &arm["current"]),
            (&Value::Null, &json!("origin/arm"), &json!(true))
        );
    }

    #[test]
    fn a_switch_is_logged_once_and_never_undone() {
        use crate::live::{Kind, Live};
        use crate::testing::call_live;
        let (dir, mut s) = two_branches();
        let mut live = Live::new();
        let me = live.hello(Kind::Agent, Some("claude-1"), None).unwrap();
        // The agent's change, committed: undoable until its file moves.
        let edit = json!({"part": "p", "id": "b", "params": {"x": 12}});
        call_live(&mut s, &mut live, Some(&me), "feature.edit", edit).unwrap();
        git(dir.path(), &["commit", "-q", "-am", "shorter"]);
        let to_arm = json!({"branch": "arm"});
        let a = call_live(&mut s, &mut live, Some(&me), "branch.switch", to_arm).unwrap();
        let (seq, v) = {
            let e = live.log.entries().next_back().unwrap();
            (e.seq, e.to_json())
        };
        assert_eq!(a["entry"], json!(seq));
        assert_eq!(
            (&v["kind"], &v["method"]),
            (&json!("switch"), &json!("branch.switch"))
        );
        assert_eq!(v["params"], json!({"from": "main", "to": "arm"}));
        assert_eq!(v["participant"]["name"], "claude-1");
        assert!(
            v["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["path"] == "parts/p/features/b.json")
        );
        // One entry: what the switch changed isn't logged again as an
        // outside change on the next request.
        let n = live.log.entries().count();
        call_live(&mut s, &mut live, Some(&me), "part.list", json!({})).unwrap();
        assert_eq!(live.log.entries().count(), n);
        // Never undone; and the change before it moved with the files.
        let named = json!({"entry": seq});
        let e = call_live(&mut s, &mut live, Some(&me), "undo", named).unwrap_err();
        assert_eq!(e.code, Code::NothingToUndo);
        let e = call_live(&mut s, &mut live, Some(&me), "undo", json!({})).unwrap_err();
        assert_eq!(e.code, Code::UndoConflict);
        // A person in View is refused (a window opens in View); agents
        // never are.
        let dana = live.hello(Kind::Human, Some("Dana"), None).unwrap();
        let back = json!({"branch": "main"});
        let e = call_live(&mut s, &mut live, Some(&dana), "branch.switch", back).unwrap_err();
        assert_eq!(e.code, Code::ParticipantMode);
    }
}
