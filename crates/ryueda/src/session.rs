//! A loaded project, kept fresh against the files. Before every request
//! the session checks each file it depends on (the manifest, this
//! project's library, and each alias's library project), as ryucad's
//! does, and loads the project again when any changed. It keeps every
//! file's bytes, so the activity log's snapshots cost no reading.

use crate::error::{Code, Error, Result};
use crate::project::{MANIFEST, Manifest, Project, resolve};
use ryu_session::fresh::{Files, Stamps};
use std::path::{Path, PathBuf};

pub struct Session {
    root: PathBuf,
    project: Project,
    stamps: Stamps,
}

/// `rel` (relative to the project, with `/`) as a path under `root`.
pub fn path_of(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

/// Every file the project's state depends on, with its metadata, as
/// paths under `root` (an alias's files by way of `..`).
fn listed(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    Project::tracked(root)
        .into_iter()
        .map(|(rel, m)| (path_of(root, &rel), m))
        .collect()
}

impl Session {
    /// Loads the project at `root`.
    pub fn open(root: &Path) -> Result<Session> {
        let mut stamps = Stamps::default();
        stamps.scan(root, listed(root))?;
        let project = load(root, &stamps)?;
        Ok(Session {
            root: root.to_path_buf(),
            project,
            stamps,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Brings the session up to date with the files, returning those that
    /// changed (relative, with `/`) and how.
    pub fn refresh_changes(&mut self) -> Result<Vec<(String, &'static str)>> {
        let changes = self.stamps.scan(&self.root, listed(&self.root))?;
        if !changes.is_empty() {
            self.project = load(&self.root, &self.stamps)?;
        }
        Ok(changes)
    }

    /// Takes in what this session just wrote.
    pub fn reload(&mut self) -> Result<()> {
        self.refresh_changes().map(drop)
    }

    /// Takes in the files this session just wrote at `written`, as
    /// `project`: the project already read over them in memory. Reads it
    /// again only if anything else changed too.
    pub fn adopt(&mut self, project: Project, written: &[String]) -> Result<()> {
        let changes = self.stamps.scan(&self.root, listed(&self.root))?;
        if changes.iter().all(|(p, _)| written.contains(p)) {
            self.project = project;
        } else {
            self.project = load(&self.root, &self.stamps)?;
        }
        Ok(())
    }

    /// Every file the project's state depends on, as last read.
    pub fn snapshot(&self) -> Files {
        self.stamps.snapshot(&self.root)
    }
}

/// The project from the stamps' bytes, its aliases resolved on disk.
fn load(root: &Path, stamps: &Stamps) -> Result<Project> {
    let files = stamps.snapshot(root);
    let bytes = files.get(MANIFEST).ok_or_else(|| {
        Error::new(
            Code::ProjectNotFound,
            format!("{} has no {MANIFEST}", root.display()),
        )
        .with("path", root)
    })?;
    let aliases = resolve(root, &Manifest::read(bytes)?);
    Project::load_from(root, &files, &aliases)
}

/// Every file the project's state depends on, read now: what undo checks
/// a revert against.
pub fn current(root: &Path) -> Result<Files> {
    listed(root)
        .into_iter()
        .map(|(path, _)| {
            let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, &e))?;
            Ok((crate::source::relative(root, &path), bytes.into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectKind;
    use crate::project::tests::{library_in, manifest};

    #[test]
    fn sessions_see_changes_in_aliases_too() {
        let dir = tempfile::tempdir().unwrap();
        let (board, lib) = (dir.path().join("board"), dir.path().join("lib"));
        manifest(&lib, ProjectKind::Library, &[]);
        library_in(&lib);
        manifest(&board, ProjectKind::Design, &[("passives", "../lib")]);
        let mut s = Session::open(&board).unwrap();
        let before = s.project().library("passives").unwrap().revision.clone();
        assert!(s.snapshot().contains_key("../lib/library/symbols/r.json"));
        assert!(s.refresh_changes().unwrap().is_empty());
        std::fs::remove_file(lib.join("library/symbols/r.json")).unwrap();
        let changes = s.refresh_changes().unwrap();
        assert_eq!(
            changes,
            [("../lib/library/symbols/r.json".to_owned(), "deleted")]
        );
        assert_ne!(s.project().library("passives").unwrap().revision, before);
        assert_eq!(current(&board).unwrap(), s.snapshot());
    }

    #[test]
    fn sessions_see_blocks_change() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board");
        manifest(&board, ProjectKind::Design, &[]);
        crate::project::tests::design_in(&board);
        let mut s = Session::open(&board).unwrap();
        let top = crate::design::tests::id("top");
        let before = s.project().conn.blocks[&top].revision.clone();
        std::fs::remove_file(board.join("blocks/top/nets/sda.json")).unwrap();
        let changes = s.refresh_changes().unwrap();
        assert_eq!(
            changes,
            [("blocks/top/nets/sda.json".to_owned(), "deleted")]
        );
        assert_ne!(s.project().conn.blocks[&top].revision, before);
    }
}
