//! The project's Git history, read-only: the commits that touched the
//! project (or one part), what is uncommitted, files as `HEAD` has them,
//! the whole project at any revision, and a merge in progress (its step,
//! each side's commit and the files Git left unmerged). ryucad never
//! writes to Git; committing, and marking files resolved, stay with the
//! agent or user.

use crate::activity::Files;
use crate::error::Result;
use serde_json::{Value, json};
use std::path::Path;

pub use ryu_git::repo::{
    Branch, MergeState, Revision, Step, Unmerged, Worktree, not_a_repo, same_text,
};

/// ryucad's layout: the files at its root, everything under `parts/`,
/// `releases/`, `imports/`, `annotations/` and `templates/`.
impl ryu_git::Layout for crate::Cad {
    const NAME: &'static str = "ryucad";
    const MANIFEST: &'static str = crate::doc::project::MANIFEST;
    const ROOT_FILES: &'static [&'static str] = &crate::doc::project::ROOT_FILES;
    const FOLDERS: &'static [&'static str] = &[
        "parts",
        crate::doc::project::RELEASES,
        crate::doc::project::IMPORTS,
        crate::doc::annotation::ANNOTATIONS,
        crate::doc::drawing_template::TEMPLATES,
    ];
    fn project_file(rel: &str) -> bool {
        project_file(rel)
    }
}

/// A project's repository, and where the project sits in it.
pub type Repo = ryu_git::Repo<crate::Cad>;

/// The ryucad projects of the repository holding `here` that a merge in
/// progress concerns: every folder with a `ryucad.json` in `HEAD` or in
/// the commit being merged that the work tree still has, sorted. Empty,
/// at once, when Git is in no merge (as for every ordinary commit).
pub fn merging_projects(here: &Path) -> Result<Vec<std::path::PathBuf>> {
    ryu_git::merging_projects::<crate::Cad>(here)
}

/// The repository holding the project at `root`, or `not_a_git_repo`.
pub fn repo(root: &Path) -> Result<Repo> {
    ryu_git::repo::repo(root)
}

/// The project at `root` as `rev` has it: `not_a_git_repo` outside a
/// repository, `rev_not_found` for a revision it doesn't have.
pub fn revision(root: &Path, rev: &str) -> Result<Revision> {
    ryu_git::repo::revision::<crate::Cad>(root, rev)
}

/// Whether `rel` (relative to a project) is one of the files its state
/// depends on, as `session::files` lists them: the files at its root,
/// files two and three levels under `parts/`, files two levels under
/// `releases/`, and files under `imports/`, `annotations/` and
/// `templates/`.
fn project_file(rel: &str) -> bool {
    if crate::doc::project::ROOT_FILES.contains(&rel) {
        return true;
    }
    let mut parts = rel.split('/');
    match parts.next() {
        Some("parts") => matches!(parts.count(), 2 | 3),
        Some(crate::doc::project::RELEASES) => parts.count() == 2,
        Some(crate::doc::project::IMPORTS) => parts.count() == 1,
        Some(crate::doc::annotation::ANNOTATIONS) => parts.count() == 1,
        Some(crate::doc::drawing_template::TEMPLATES) => parts.count() == 1,
        _ => false,
    }
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
    use crate::testing::{call, fresh};

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
    use crate::error::Code;
    use crate::session::Session;
    use std::path::PathBuf;
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
            then.parts[&crate::part::part_id("a").unwrap()]
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
        // Of those, only the new file is untracked.
        assert_eq!(
            repo.untracked(&files(r)).unwrap(),
            ["parts/p/features/x.json"]
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
    fn the_worktrees_that_hold_the_project() {
        // The project sits in a subfolder; one worktree's branch has none.
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().join("repo");
        std::fs::create_dir(&top).unwrap();
        git(&top, &["init", "-q", "-b", "main"]);
        let root = top.join("cad");
        std::fs::create_dir(&root).unwrap();
        crate::server::init(&root, None).unwrap();
        git(&top, &["add", "."]);
        git(&top, &["commit", "-q", "-m", "init"]);
        let wt = dir.path().join("wt");
        let w = wt.to_str().unwrap();
        git(&top, &["worktree", "add", "-q", "-b", "arm", w]);
        std::fs::write(wt.join("README"), "arm\n").unwrap();
        git(&wt, &["add", "."]);
        git(&wt, &["commit", "-q", "-m", "on arm"]);
        git(
            &top,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                dir.path().join("bare").to_str().unwrap(),
            ],
        );
        let empty = dir.path().join("empty");
        git(
            &top,
            &[
                "worktree",
                "add",
                "-q",
                "--orphan",
                "-b",
                "nothing",
                empty.to_str().unwrap(),
            ],
        );
        git(&top, &["worktree", "lock", "bare"]);

        let canon = |p: &Path| crate::doc::project::plain(p.canonicalize().unwrap());
        let repo = Repo::open(&root).unwrap().unwrap();
        let all = repo.worktrees().unwrap();
        let roots: Vec<&PathBuf> = all.iter().map(|w| &w.root).collect();
        assert_eq!(
            roots,
            [
                &canon(&root),
                &canon(&dir.path().join("bare/cad")),
                &canon(&wt.join("cad"))
            ]
        );
        let (main, bare, arm) = (&all[0], &all[1], &all[2]);
        assert!(main.main && main.current && !main.locked);
        assert_eq!(main.branch.as_deref(), Some("main"));
        assert_eq!(main.top, canon(&top));
        assert!(!bare.main && !bare.current && bare.locked);
        assert_eq!(bare.branch, None);
        assert_eq!(bare.head, main.head);
        assert_eq!(arm.branch.as_deref(), Some("arm"));
        // Seen from the other worktree, it is the current one.
        let from_arm = Repo::open(&wt.join("cad")).unwrap().unwrap();
        let seen = from_arm.worktrees().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen[2].current && !seen[0].current);
        // Either folder names it; anything else is refused.
        assert_eq!(repo.worktree(&wt).unwrap().root, arm.root);
        assert_eq!(repo.worktree(&wt.join("cad")).unwrap().root, arm.root);
        for bad in [
            empty.clone(),
            dir.path().join("nowhere"),
            wt.join("cad/parts"),
        ] {
            let e = repo.worktree(&bad).unwrap_err();
            assert_eq!(e.code, Code::WorktreeNotFound, "{bad:?}");
        }
        // Commits either way, and where they part.
        let (m, a) = (main.head.clone().unwrap(), arm.head.clone().unwrap());
        assert_eq!(repo.ahead_behind(&a, &m).unwrap(), (1, 0));
        assert_eq!(repo.ahead_behind(&m, &a).unwrap(), (0, 1));
        assert_eq!(repo.merge_base(&m, &a).unwrap(), Some(m.clone()));
        assert_eq!(repo.head_commit(), Some(m));
    }

    #[test]
    fn branches_local_and_remote_tracking_with_their_upstreams() {
        let dir = tempfile::tempdir().unwrap();
        let up = dir.path().join("up");
        std::fs::create_dir(&up).unwrap();
        git(&up, &["init", "-q", "-b", "main"]);
        crate::server::init(&up, None).unwrap();
        git(&up, &["add", "."]);
        git(&up, &["commit", "-q", "-m", "init"]);
        git(&up, &["branch", "arm"]);
        git(&up, &["branch", "leg"]);
        let url = format!("file://{}", up.display());
        git(dir.path(), &["clone", "-q", &url, "down"]);
        let down = dir.path().join("down");
        // A local branch tracking origin/leg, and one with no upstream.
        git(&down, &["branch", "--track", "leg", "origin/leg"]);
        git(&down, &["branch", "solo"]);
        let repo = Repo::open(&down).unwrap().unwrap();
        let all = repo.branches().unwrap();
        let seen: Vec<(&str, Option<&str>, Option<&str>)> = all
            .iter()
            .map(|b| (b.name.as_str(), b.remote.as_deref(), b.upstream.as_deref()))
            .collect();
        // Local ones by name, then origin's arm, which has no local
        // branch; origin/HEAD, origin/main and origin/leg aren't listed.
        assert_eq!(
            seen,
            [
                ("leg", None, Some("origin/leg")),
                ("main", None, Some("origin/main")),
                ("solo", None, None),
                ("arm", Some("origin"), None),
            ]
        );
        assert_eq!(all[0].full, "refs/heads/leg");
        assert_eq!(all[3].full, "refs/remotes/origin/arm");
        assert!(
            all.iter()
                .all(|b| Some(&b.head) == repo.head_commit().as_ref())
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
