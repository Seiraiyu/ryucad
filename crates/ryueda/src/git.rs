//! ryueda's projects in Git: its layout for `ryu-git`, and a project at
//! any revision, read from Git's objects and never from the disk, its
//! library aliases resolved inside the repository at that commit.

use crate::error::{Code, Error, Result};
use crate::id::Id;
use crate::library;
use crate::project::{MANIFEST, Manifest, ProjectKind, ROOT_FILES, STYLE, path_ok};
use crate::source::join;
use ryu_session::fresh::Files;
use std::collections::BTreeMap;
use std::path::Path;

/// The folders a project is made of, below its root.
pub const FOLDERS: [&str; 5] = [
    crate::conn::DESIGNS,
    crate::conn::RAILS,
    crate::conn::CLASSES,
    crate::conn::BLOCKS,
    library::DIR,
];

impl ryu_git::Layout for crate::Eda {
    const NAME: &'static str = "ryueda";
    const MANIFEST: &'static str = MANIFEST;
    const ROOT_FILES: &'static [&'static str] = &ROOT_FILES;
    const FOLDERS: &'static [&'static str] = &FOLDERS;
    fn project_file(rel: &str) -> bool {
        project_file(rel)
    }
}

/// A ryueda project's repository.
pub type Repo = ryu_git::Repo<crate::Eda>;

/// Whether `rel` is a file a session tracks
/// ([`crate::project::Project::tracked`]): the root files, a file of
/// `designs/`, `rails/` or `classes/`, a block's files and its parts'
/// (`blocks/<b>/<f>`, `blocks/<b>/<dir>/<f>`), and the library's
/// (`library/<f>`, `library/<dir>/<f>`).
pub fn project_file(rel: &str) -> bool {
    if ROOT_FILES.contains(&rel) {
        return true;
    }
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        [d, _] => [
            crate::conn::DESIGNS,
            crate::conn::RAILS,
            crate::conn::CLASSES,
            library::DIR,
        ]
        .contains(d),
        [d, _, _] => *d == crate::conn::BLOCKS || *d == library::DIR,
        [d, _, _, _] => *d == crate::conn::BLOCKS,
        _ => false,
    }
}

/// The project as a revision has it.
#[derive(Debug)]
pub struct Revision {
    /// The commit, in hex.
    pub commit: String,
    /// The project folder's tree and each alias's folder's: equal keys,
    /// equal files.
    pub key: String,
    /// The project's files and each usable alias's (`../lib/…`), by path
    /// relative to the project, as a session holds them.
    pub files: Files,
    /// Each alias's folder (as the manifest writes it), or why it can't
    /// be used at this revision.
    pub aliases: BTreeMap<Id, Result<String>>,
}

fn missing(why: String) -> Error {
    Error::new(Code::LibraryMissing, why)
}

/// `path` (relative to the project at `prefix`, as a manifest writes it)
/// relative to the work tree's root; `None` when it leads out of it.
fn in_repository(prefix: &str, path: &str) -> Option<String> {
    let mut at: Vec<&str> = prefix.split('/').filter(|s| !s.is_empty()).collect();
    for s in path.split('/') {
        if s == ".." {
            at.pop()?;
        } else {
            at.push(s);
        }
    }
    Some(at.join("/"))
}

/// An alias's library project at `commit`: its folder's tree and its
/// files (manifest, attributes, style, `library/` two levels deep), or why
/// it can't be used there, as [`crate::project::resolve`] judges it on disk.
fn alias_at(
    repo: &Repo,
    commit: &str,
    path: &str,
    rev: &str,
) -> Result<(String, BTreeMap<String, Vec<u8>>)> {
    if !path_ok(path) {
        return Err(missing(format!(
            "library path {path:?} isn't a relative path of folder names joined by /"
        )));
    }
    let Some(folder) = in_repository(repo.prefix(), path) else {
        return Err(missing(format!("{path} leads out of the repository")));
    };
    if folder == repo.prefix().trim_end_matches('/') {
        return Err(missing(format!(
            "{path} is this project, not a library project beside it"
        )));
    }
    let Some(tree) = repo.tree_at(commit, &folder)? else {
        return Err(missing(format!("no folder at {path} at {rev}")));
    };
    let bytes = repo
        .file_at(commit, &join(&folder, MANIFEST))?
        .ok_or_else(|| {
            missing(format!(
                "{path} holds no ryueda project ({MANIFEST}) at {rev}"
            ))
        })?;
    let m = Manifest::read(&bytes)
        .map_err(|e| missing(format!("{path}/{MANIFEST} at {rev}: {}", e.message)))?;
    if m.project_kind != ProjectKind::Library {
        return Err(missing(format!(
            "{path} is a design project at {rev}, not a library project"
        )));
    }
    if !m.libraries.is_empty() {
        return Err(Error::new(
            Code::LibraryNested,
            format!("{path} names libraries of its own at {rev}; a library project can't"),
        ));
    }
    let mut out = BTreeMap::new();
    out.insert(MANIFEST.to_owned(), bytes);
    for f in [crate::project::ATTRIBUTES, STYLE] {
        if let Some(b) = repo.file_at(commit, &join(&folder, f))? {
            out.insert(f.to_owned(), b);
        }
    }
    for (p, b) in repo.files_at(commit, &join(&folder, library::DIR))? {
        if p.matches('/').count() <= 1 {
            out.insert(join(library::DIR, &p), b);
        }
    }
    Ok((tree, out))
}

/// The commit `rev` names and the key of what the project reads there,
/// reading only trees and the manifest: what a session's cache is keyed
/// by.
pub fn key(root: &Path, rev: &str) -> Result<(String, String)> {
    let repo: Repo = ryu_git::repo::repo(root)?;
    let (commit, tree) = repo.locate(rev)?;
    let mut key = tree;
    let manifest = repo.file_at(
        &commit,
        &join(repo.prefix().trim_end_matches('/'), MANIFEST),
    )?;
    if let Some(m) = manifest.and_then(|b| Manifest::read(&b).ok()) {
        for (alias, a) in &m.libraries {
            let t = in_repository(repo.prefix(), &a.path)
                .map(|f| repo.tree_at(&commit, &f))
                .transpose()?
                .flatten()
                .unwrap_or_default();
            key.push_str(&format!(";{alias}={t}"));
        }
    }
    Ok((commit, key))
}

/// The project at `root` as `rev` has it, with its aliases: what
/// [`crate::project::Project::load_from`] takes. `not_a_git_repo` outside
/// a repository, `rev_not_found` for a revision it doesn't have; an alias
/// that can't be used there fails only what names it, as on disk.
pub fn revision(root: &Path, rev: &str) -> Result<Revision> {
    let repo: Repo = ryu_git::repo::repo(root)?;
    let r = repo.revision(rev)?;
    let (_, key) = key(root, &r.commit)?;
    let mut files: Files = r
        .files
        .into_iter()
        .filter(|(p, _)| project_file(p))
        .collect();
    let mut aliases = BTreeMap::new();
    let manifest = files.get(MANIFEST).and_then(|b| Manifest::read(b).ok());
    for (alias, a) in manifest.map(|m| m.libraries).unwrap_or_default() {
        let found = alias_at(&repo, &r.commit, &a.path, rev)
            .map_err(|e| e.with("alias", &alias).with("path", &a.path));
        let base = found.map(|(_, lib)| {
            for (p, b) in lib {
                files.insert(join(&a.path, &p), b.into());
            }
            a.path.clone()
        });
        aliases.insert(alias, base);
    }
    Ok(Revision {
        commit: r.commit,
        key,
        files,
        aliases,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::project::tests::{library_in, manifest};
    use crate::project::{Project, ProjectKind};

    /// `git`, reading no user's or system's config, with a fixed identity
    /// and dates.
    pub(crate) fn git_command() -> std::process::Command {
        let mut git = std::process::Command::new("git");
        git.env(
            "GIT_CONFIG_GLOBAL",
            std::env::temp_dir().join("ryueda-no-gitconfig"),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.autocrlf=false",
        ]);
        git
    }

    /// Runs git in `dir`; panics on failure.
    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let out = git_command().args(args).current_dir(dir).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn the_project_and_its_library_at_a_revision() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        git(top, &["init", "-q"]);
        let (board, lib) = (top.join("board"), top.join("lib"));
        manifest(&lib, ProjectKind::Library, &[]);
        library_in(&lib);
        manifest(&board, ProjectKind::Design, &[("passives", "../lib")]);
        git(top, &["add", "."]);
        git(top, &["commit", "-q", "-m", "one"]);
        let first = Project::load(&board).unwrap();
        // The library changes on disk and in a second commit.
        std::fs::remove_file(lib.join("library/symbols/r.json")).unwrap();
        git(top, &["commit", "-q", "-am", "two"]);
        let r = revision(&board, "HEAD~1").unwrap();
        assert_eq!(r.commit.len(), 40);
        assert!(r.files.contains_key("../lib/library/symbols/r.json"));
        assert!(r.files.contains_key("ryueda.json"));
        let p = Project::load_from(&board, &r.files, &r.aliases).unwrap();
        assert_eq!(p.revision, first.revision);
        assert_eq!(
            p.library("passives").unwrap().revision,
            first.library("passives").unwrap().revision
        );
        // Equal trees, equal keys; a changed library, another key.
        assert_eq!(key(&board, "HEAD~1").unwrap().1, r.key);
        assert_ne!(key(&board, "HEAD").unwrap().1, r.key);
        assert!(matches!(revision(&board, "nope"), Err(e) if e.code == Code::RevNotFound));
    }

    #[test]
    fn an_alias_out_of_the_repository_is_missing_at_the_revision() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().join("repo");
        std::fs::create_dir_all(&top).unwrap();
        git(&top, &["init", "-q"]);
        let board = top.join("board");
        manifest(
            &board,
            ProjectKind::Design,
            &[("far", "../../far"), ("gone", "../gone")],
        );
        git(&top, &["add", "."]);
        git(&top, &["commit", "-q", "-m", "one"]);
        let r = revision(&board, "HEAD").unwrap();
        let far = r.aliases[&Id::parse("far", "t").unwrap()]
            .as_ref()
            .unwrap_err();
        assert_eq!(far.code, Code::LibraryMissing);
        assert!(
            far.message.contains("out of the repository"),
            "{}",
            far.message
        );
        let gone = r.aliases[&Id::parse("gone", "t").unwrap()]
            .as_ref()
            .unwrap_err();
        assert!(gone.message.contains("at HEAD"), "{}", gone.message);
    }

    #[test]
    fn outside_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        manifest(dir.path(), ProjectKind::Design, &[]);
        assert_eq!(
            revision(dir.path(), "HEAD").unwrap_err().code,
            Code::NotAGitRepo
        );
    }

    #[test]
    fn project_files_are_what_a_session_tracks() {
        for f in [
            "ryueda.json",
            ".gitattributes",
            "erc.json",
            "lint.json",
            "parts.json",
            "style.json",
            "designs/d.json",
            "rails/r.json",
            "classes/c.json",
            "blocks/b/block.json",
            "blocks/b/layout.json",
            "blocks/b/components/c.json",
            "library/standards.json",
            "library/symbols/r.json",
            "library/models/r.step",
        ] {
            assert!(project_file(f), "{f}");
        }
        for f in [
            "README",
            "designs/x/d.json",
            "blocks/b/components/x/c.json",
            "library/a/b/c.json",
        ] {
            assert!(!project_file(f), "{f}");
        }
    }
}
