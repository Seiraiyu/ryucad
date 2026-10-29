//! A loaded project, kept fresh against the files, and the boards read
//! from it. Before every request the session checks each file it depends
//! on (the manifest, `.gitattributes` and everything under `boards/`), as
//! ryucad's and ryueda's do, and loads the project again when any changed.
//! It keeps every file's bytes, so the activity log's snapshots and the
//! boards' inputs cost no reading.
//!
//! Boards read are kept by their input's hash and step, the eight most
//! recently used, so `inspect` and `render` after the first read cost
//! milliseconds, and so do the shapes they build.

use crate::codes;
use crate::error::{Code, Error, Result};
use crate::project::{MANIFEST, Project};
use crate::source::relative;
use ryu_board::model::Board;
use ryu_session::fresh::{Files, Stamps};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How many boards a session keeps read.
pub const KEPT: usize = 8;

pub struct Session {
    root: PathBuf,
    project: Project,
    stamps: Stamps,
    /// Boards read, most recently used first, by input hash and step.
    boards: Vec<(String, Arc<Board>)>,
}

/// `rel` (relative to the project, with `/`) as a path under `root`.
pub fn path_of(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

fn listed(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    Project::tracked(root)
        .into_iter()
        .map(|(rel, m)| (path_of(root, &rel), m))
        .collect()
}

fn load(root: &Path, stamps: &Stamps) -> Result<Project> {
    let files = stamps.snapshot(root);
    if !files.contains_key(MANIFEST) {
        return Err(Error::new(
            Code::ProjectNotFound,
            format!("{} has no {MANIFEST}", root.display()),
        )
        .with("path", root));
    }
    Project::load_from(root, &files)
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
            boards: Vec::new(),
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

    /// Every file the project's state depends on, as last read.
    pub fn snapshot(&self) -> Files {
        self.stamps.snapshot(&self.root)
    }

    /// Board `id` read, from the session's boards when its input and step
    /// were read before; refused as the board's file or input fails, or
    /// as the reader refuses the job.
    pub fn board(&mut self, id: &str) -> Result<Arc<Board>> {
        let entry = self.project.board(id)?;
        let (file, input) = entry.ready()?;
        let key = format!("{}\0{}", entry.hash.as_deref().expect("ready"), file.step);
        if let Some(b) = recall(&mut self.boards, &key) {
            return Ok(b);
        }
        let board = Arc::new(read(input, id, &file.step)?);
        remember(&mut self.boards, key, Arc::clone(&board));
        Ok(board)
    }

    /// How many boards the session keeps read.
    pub fn boards_kept(&self) -> usize {
        self.boards.len()
    }
}

/// The board kept under `key`, moved to the front.
fn recall<T>(kept: &mut Vec<(String, Arc<T>)>, key: &str) -> Option<Arc<T>> {
    let i = kept.iter().position(|(k, _)| k == key)?;
    let found = kept.remove(i);
    let b = Arc::clone(&found.1);
    kept.insert(0, found);
    Some(b)
}

/// Keeps `board` under `key` in front, and no more than [`KEPT`].
fn remember<T>(kept: &mut Vec<(String, Arc<T>)>, key: String, board: Arc<T>) {
    kept.insert(0, (key, board));
    kept.truncate(KEPT);
}

/// Reads an input's job with `step` as the board step: a job that can't
/// be read is refused with the reader's code, naming the board.
pub fn read(input: &crate::input::Input, id: &str, step: &str) -> Result<Board> {
    let options = crate::odb::ReadOptions {
        step: Some(step.to_owned()),
        ..Default::default()
    };
    input
        .job(id)
        .and_then(|job| crate::odb::read(&job, &options))
        .map_err(|e| codes::error(&e).with("board", id))
}

/// Deletes the file at `path`, and the folders under `root` that leaves
/// empty: undoing an import takes its board's folder too.
pub fn remove_file(root: &Path, path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(|e| Error::io(path, &e))?;
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

/// Every file the project's state depends on, read now: what undo checks
/// a revert against.
pub fn current(root: &Path) -> Result<Files> {
    listed(root)
        .into_iter()
        .map(|(path, _)| {
            let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, &e))?;
            Ok((relative(root, &path), bytes.into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{BOARD_FILE, BoardFile, Format, InputRef, Manifest, SourceInfo};
    use crate::source::write_atomic;

    #[test]
    fn removing_a_file_takes_the_folders_it_empties() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let deep = root.join("boards/b/input/matrix/matrix");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        std::fs::write(&deep, "m").unwrap();
        std::fs::write(root.join("boards/b/board.json"), "{}").unwrap();
        remove_file(root, &deep).unwrap();
        assert!(!root.join("boards/b/input").exists());
        assert!(root.join("boards/b").exists());
        remove_file(root, &root.join("boards/b/board.json")).unwrap();
        assert!(!root.join("boards").exists());
        assert!(root.exists());
    }

    #[test]
    fn the_most_recently_used_are_kept() {
        let mut kept: Vec<(String, Arc<u32>)> = Vec::new();
        for n in 0..10u32 {
            remember(&mut kept, n.to_string(), Arc::new(n));
        }
        assert_eq!(kept.len(), KEPT);
        assert_eq!(recall(&mut kept, "0"), None);
        assert_eq!(recall(&mut kept, "2").as_deref(), Some(&2));
        assert_eq!(kept[0].0, "2");
        remember(&mut kept, "x".into(), Arc::new(99));
        assert!(kept.iter().any(|(k, _)| k == "2"));
        assert!(!kept.iter().any(|(k, _)| k == "3"));
    }

    #[test]
    fn boards_are_read_once_per_input() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let m = Manifest {
            project: crate::id::ProjectId::from_bytes([1; 16]),
            default_unit: crate::units::Unit::Mm,
        };
        write_atomic(&root.join(crate::project::MANIFEST), m.to_text()).unwrap();
        let tgz = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/minimal-clean.tgz"),
        )
        .unwrap();
        for id in ["a", "b"] {
            let file = BoardFile {
                name: id.into(),
                input: InputRef {
                    path: "input.tgz".into(),
                    blake3: crate::input::archive_hash(&tgz),
                    format: Format::Odb,
                    container: crate::input::Container::Tgz,
                },
                step: "pcb".into(),
                source: SourceInfo {
                    version: "8.1".into(),
                    application: None,
                },
            };
            let dir = root.join("boards").join(id);
            write_atomic(&dir.join(BOARD_FILE), file.to_text()).unwrap();
            write_atomic(&dir.join("input.tgz"), &tgz).unwrap();
        }
        let mut s = Session::open(root).unwrap();
        let a = s.board("a").unwrap();
        assert!(Arc::ptr_eq(&a, &s.board("a").unwrap()));
        // The same input and step: one board read.
        assert!(Arc::ptr_eq(&a, &s.board("b").unwrap()));
        assert_eq!(s.boards_kept(), 1);
        assert_eq!(s.board("c").unwrap_err().code, Code::NotFound);
    }
}
