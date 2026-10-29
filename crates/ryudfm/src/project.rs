//! A project: a folder with `ryudfm.json` and its boards, each a folder
//! `boards/<id>/` holding `board.json` and the input as given.
//!
//! ```text
//! ryudfm.json                      the manifest
//! boards/<id>/board.json           a board: its input, chosen step and name
//! boards/<id>/input.tgz            the input as given (or input.tar, input.zip)
//! boards/<id>/input/…              or a folder input, file for file
//! .gitattributes                   written at init
//! ```
//!
//! A board whose `board.json` doesn't read, or whose input is missing or
//! isn't the bytes it names, fails alone: the project still loads.

use crate::doc;
use crate::error::{Code, Error, Result};
use crate::id::{Id, ProjectId};
use crate::input::{Container, Input};
use crate::source::{Kind, Source, join, plain};
use crate::units::Unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MANIFEST: &str = "ryudfm.json";
pub const KIND: &str = "ryudfm.project";
pub const FORMAT: u64 = 1;
/// Where boards live.
pub const BOARDS: &str = "boards";
pub const BOARD_FILE: &str = "board.json";
pub const BOARD_KIND: &str = "ryudfm.board";
pub const BOARD_FORMAT: u64 = 1;
pub const GITATTRIBUTES: &str = ".gitattributes";

/// The lines `init` writes to `.gitattributes`: JSON files are text with
/// LF endings everywhere, and inputs are binary, so Git keeps their bytes
/// exactly and a Windows checkout doesn't change a hash. A folder input's
/// files need their own line: a pattern naming a folder doesn't reach the
/// files in it.
pub const ATTRIBUTES: [&str; 3] = [
    "*.json text eol=lf",
    "boards/*/input.* binary",
    "boards/*/input/** binary",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// 128 random bits, written at `init`, in the form of a ryucad
    /// manifest's, so ryupdm can name ryudfm projects.
    pub project: ProjectId,
    /// The unit of lengths an agent types as bare numbers.
    pub default_unit: Unit,
}

impl Manifest {
    pub fn read(bytes: &[u8]) -> Result<Manifest> {
        doc::read(bytes, MANIFEST, KIND, FORMAT)
    }

    pub fn to_text(&self) -> String {
        doc::to_string(self, KIND, FORMAT)
    }
}

/// The only format a board's input is in so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    #[serde(rename = "odb++")]
    Odb,
}

/// Where a board's input is and the BLAKE3 that names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRef {
    /// Relative to the board's folder: `input` or `input.<container>`.
    pub path: String,
    pub blake3: String,
    pub format: Format,
    pub container: Container,
}

/// What the input said it was, as read when last imported: for people
/// reading the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInfo {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
}

/// `boards/<id>/board.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardFile {
    pub name: String,
    pub input: InputRef,
    /// The board step, written even when the rule chose it, so a
    /// re-export that adds a panel step doesn't change what is checked
    /// silently.
    pub step: String,
    pub source: SourceInfo,
}

impl BoardFile {
    pub fn read(bytes: &[u8], path: &str) -> Result<BoardFile> {
        let b: BoardFile = doc::read(bytes, path, BOARD_KIND, BOARD_FORMAT)?;
        if b.input.path != b.input.container.file_name() {
            return Err(Error::new(
                Code::InvalidDocument,
                format!(
                    "{path}: a {} input is {}, not {}",
                    serde_json::to_value(b.input.container).expect("serializes"),
                    b.input.container.file_name(),
                    b.input.path
                ),
            )
            .with("path", path)
            .with("pointer", "/input/path"));
        }
        Ok(b)
    }

    pub fn to_text(&self) -> String {
        doc::to_string(self, BOARD_KIND, BOARD_FORMAT)
    }
}

/// The folder of board `id`, relative to the project.
pub fn board_dir(id: &str) -> String {
    join(BOARDS, id)
}

/// A board of the project as its files are.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: Id,
    pub file: Result<BoardFile>,
    /// The input, when `board.json` reads and it is there.
    pub input: Result<Input>,
    /// The input's BLAKE3 as it is now, when it is there.
    pub hash: Option<String>,
    /// A hash of `board.json` and the input's hash: what a write to the
    /// board is guarded by.
    pub revision: String,
}

impl Entry {
    /// The board's file and input, or why the board can't be read: its
    /// file doesn't read, its input is missing, or isn't the bytes the file
    /// names.
    pub fn ready(&self) -> Result<(&BoardFile, &Input)> {
        let file = self.file.as_ref().map_err(Clone::clone)?;
        let input = self.input.as_ref().map_err(Clone::clone)?;
        let found = self.hash.as_deref().expect("an input read has a hash");
        if found != file.input.blake3 {
            return Err(Error::new(
                Code::BoardInputChanged,
                format!(
                    "board {}'s input isn't the one board.json names: import it again with --update, or restore it from Git",
                    self.id
                ),
            )
            .with("board", &self.id)
            .with("path", join(&board_dir(self.id.as_str()), &file.input.path))
            .with("expected", &file.input.blake3)
            .with("found", found));
        }
        Ok((file, input))
    }
}

/// A loaded project.
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    /// A hash of the manifest and every board's revision.
    pub revision: String,
    pub boards: BTreeMap<Id, Entry>,
}

/// Adds one length-prefixed piece to a hash.
fn piece(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

fn short(h: blake3::Hasher) -> String {
    h.finalize().to_hex()[..32].to_owned()
}

/// Every file under `dir` of `src`, by path relative to `dir`, sorted.
fn files_under(src: &dyn Source, dir: &str) -> Result<Vec<(String, Arc<[u8]>)>> {
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let at = if rel.is_empty() {
            dir.to_owned()
        } else {
            join(dir, &rel)
        };
        for (name, kind) in src.list(&at)? {
            let at = join(&rel, &name);
            match kind {
                Kind::Dir => stack.push(at),
                Kind::File => {
                    if let Some(b) = src.read_shared(&join(dir, &at))? {
                        out.push((at, b));
                    }
                }
                Kind::Other => {}
            }
        }
    }
    out.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(out)
}

/// Reads board `id` from `src`.
fn entry(src: &dyn Source, id: Id, bytes: &[u8]) -> Result<Entry> {
    let dir = board_dir(id.as_str());
    let path = join(&dir, BOARD_FILE);
    let file = BoardFile::read(bytes, &path);
    let input = match &file {
        Err(e) => Err(e.clone()),
        Ok(f) => {
            let at = join(&dir, &f.input.path);
            let missing = || {
                Error::new(
                    Code::BoardInputMissing,
                    format!("board {id} has no input at {at}: restore it from Git, or import it again with --update"),
                )
                .with("board", &id)
                .with("path", &at)
            };
            match f.input.container {
                Container::Folder => {
                    let files = files_under(src, &at)?;
                    if files.is_empty() {
                        Err(missing())
                    } else {
                        Ok(Input::Folder(files))
                    }
                }
                _ => src
                    .read_shared(&at)?
                    .map(Input::Archive)
                    .ok_or_else(missing),
            }
        }
    };
    let hash = input.as_ref().ok().map(Input::hash);
    let mut h = blake3::Hasher::new();
    h.update(b"ryudfm.board.1");
    piece(&mut h, bytes);
    piece(&mut h, hash.as_deref().unwrap_or("-").as_bytes());
    Ok(Entry {
        id,
        file,
        input,
        hash,
        revision: short(h),
    })
}

impl Project {
    /// Loads the project at `root` from `src`.
    pub fn load_from(root: &Path, src: &dyn Source) -> Result<Project> {
        let bytes = src.read(MANIFEST)?.ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!("{} has no {MANIFEST}", root.display()),
            )
            .with("path", root)
        })?;
        let manifest = Manifest::read(&bytes)?;
        let mut boards = BTreeMap::new();
        for (name, kind) in src.list(BOARDS)? {
            let Ok(id) = Id::parse(&name, "board") else {
                continue;
            };
            if kind != Kind::Dir {
                continue;
            }
            if let Some(b) = src.read(&join(&board_dir(&name), BOARD_FILE))? {
                boards.insert(id.clone(), entry(src, id, &b)?);
            }
        }
        let mut h = blake3::Hasher::new();
        h.update(b"ryudfm.project.1");
        piece(&mut h, &bytes);
        for (id, e) in &boards {
            piece(&mut h, id.as_str().as_bytes());
            piece(&mut h, e.revision.as_bytes());
        }
        Ok(Project {
            root: root.to_path_buf(),
            manifest,
            revision: short(h),
            boards,
        })
    }

    /// The project's revision and each board's, by `project` and
    /// `board/<id>`: what subscribers are told moved.
    pub fn revisions(&self) -> BTreeMap<String, serde_json::Value> {
        let mut out: BTreeMap<String, serde_json::Value> = self
            .boards
            .iter()
            .map(|(id, b)| (format!("board/{id}"), serde_json::json!(b.revision)))
            .collect();
        out.insert("project".into(), serde_json::json!(self.revision));
        out
    }

    /// Board `id`, or `not_found` naming the boards there are.
    pub fn board(&self, id: &str) -> Result<&Entry> {
        self.boards
            .iter()
            .find(|(k, _)| k.as_str() == id)
            .map(|(_, e)| e)
            .ok_or_else(|| {
                Error::new(Code::NotFound, format!("the project has no board {id}"))
                    .with("board", id)
                    .with("boards", self.boards.keys().collect::<Vec<_>>())
            })
    }

    /// Every file the project's state depends on under `root`, with its
    /// metadata, by path relative to it: the manifest, `.gitattributes`,
    /// and every file under `boards/` (links aren't followed).
    pub fn tracked(root: &Path) -> Vec<(String, std::fs::Metadata)> {
        let mut out = Vec::new();
        for f in [MANIFEST, GITATTRIBUTES] {
            if let Ok(meta) = std::fs::metadata(root.join(f))
                && meta.is_file()
            {
                out.push((f.to_owned(), meta));
            }
        }
        if !out.iter().any(|(p, _)| p == MANIFEST) {
            return Vec::new();
        }
        let mut stack = vec![BOARDS.to_owned()];
        while let Some(rel) = stack.pop() {
            let at = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
            let Ok(entries) = std::fs::read_dir(at) else {
                continue;
            };
            let mut names: Vec<(String, std::fs::Metadata)> = entries
                .flatten()
                .filter_map(|e| {
                    let meta = std::fs::symlink_metadata(e.path()).ok()?;
                    Some((e.file_name().to_string_lossy().into_owned(), meta))
                })
                .collect();
            names.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, meta) in names {
                let p = join(&rel, &name);
                if meta.is_dir() {
                    stack.push(p);
                } else if meta.is_file() {
                    out.push((p, meta));
                }
            }
        }
        out
    }
}

/// Refuses a write when the agent's view of `what` is out of date:
/// `conflict`, with the revision expected and the one there is.
pub fn check_revision(what: &str, actual: &str, expected: Option<&str>) -> Result<()> {
    match expected {
        Some(e) if e != actual => Err(Error::new(
            Code::Conflict,
            format!("{what} changed since revision {e}; read it again and retry"),
        )
        .with("expected", e)
        .with("actual", actual)),
        _ => Ok(()),
    }
}

/// The project folder holding `start`, or above it.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    let start = plain(start.canonicalize().map_err(|e| Error::io(start, &e))?);
    start
        .ancestors()
        .find(|d| d.join(MANIFEST).is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            Error::new(
                Code::ProjectNotFound,
                format!(
                    "no {MANIFEST} in {} or any folder above it; run `ryudfm init` or pass -C <dir>",
                    start.display()
                ),
            )
            .with("path", &start)
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::input::archive_hash;
    use serde_json::json;

    pub fn manifest() -> Manifest {
        Manifest {
            project: ProjectId::from_bytes([7; 16]),
            default_unit: Unit::Mm,
        }
    }

    pub fn board_file(container: Container, blake3: &str) -> BoardFile {
        BoardFile {
            name: "Board".into(),
            input: InputRef {
                path: container.file_name().into(),
                blake3: blake3.into(),
                format: Format::Odb,
                container,
            },
            step: "pcb".into(),
            source: SourceInfo {
                version: "8.1".into(),
                application: None,
            },
        }
    }

    fn files(pairs: &[(&str, &[u8])]) -> BTreeMap<String, Arc<[u8]>> {
        pairs
            .iter()
            .map(|(p, b)| (p.to_string(), Arc::from(*b)))
            .collect()
    }

    #[test]
    fn files_read_and_write_canonically() {
        let text = manifest().to_text();
        assert_eq!(
            text,
            format!(
                "{{\n  \"default_unit\": \"mm\",\n  \"format\": 1,\n  \"kind\": \"ryudfm.project\",\n  \"project\": \"{}\"\n}}\n",
                "07".repeat(16)
            )
        );
        assert_eq!(Manifest::read(text.as_bytes()).unwrap(), manifest());
        let b = board_file(Container::Tgz, "ab");
        let v: serde_json::Value = serde_json::from_str(&b.to_text()).unwrap();
        assert_eq!(
            v,
            json!({"format": 1, "kind": "ryudfm.board", "name": "Board", "step": "pcb",
                "input": {"path": "input.tgz", "blake3": "ab", "format": "odb++", "container": "tgz"},
                "source": {"version": "8.1"}})
        );
        assert_eq!(BoardFile::read(b.to_text().as_bytes(), "b").unwrap(), b);
        let mut wrong = b.clone();
        wrong.input.path = "other.tgz".into();
        let e = BoardFile::read(wrong.to_text().as_bytes(), "b").unwrap_err();
        assert_eq!(
            (e.code, &e.details["pointer"]),
            (Code::InvalidDocument, &json!("/input/path"))
        );
    }

    #[test]
    fn boards_fail_alone() {
        let good = board_file(Container::Zip, &archive_hash(b"zip"));
        let changed = board_file(Container::Zip, &archive_hash(b"other"));
        let folder = board_file(
            Container::Folder,
            &crate::input::tree_hash([("matrix/matrix", &b"m"[..])]),
        );
        let src = files(&[
            (MANIFEST, manifest().to_text().as_bytes()),
            ("boards/a/board.json", good.to_text().as_bytes()),
            ("boards/a/input.zip", b"zip"),
            ("boards/b/board.json", changed.to_text().as_bytes()),
            ("boards/b/input.zip", b"zip"),
            ("boards/c/board.json", good.to_text().as_bytes()),
            ("boards/d/board.json", b"{"),
            ("boards/e/board.json", folder.to_text().as_bytes()),
            ("boards/e/input/matrix/matrix", b"m"),
            ("boards/Not An Id/board.json", b"{"),
        ]);
        let p = Project::load_from(Path::new("/p"), &src).unwrap();
        assert_eq!(
            p.boards.keys().map(Id::as_str).collect::<Vec<_>>(),
            ["a", "b", "c", "d", "e"]
        );
        assert!(p.board("a").unwrap().ready().is_ok());
        let e = p.board("b").unwrap().ready().unwrap_err();
        assert_eq!(e.code, Code::BoardInputChanged);
        assert_eq!(e.details["found"], json!(archive_hash(b"zip")));
        assert_eq!(
            p.board("c").unwrap().ready().unwrap_err().code,
            Code::BoardInputMissing
        );
        assert_eq!(
            p.board("d").unwrap().ready().unwrap_err().code,
            Code::InvalidJson
        );
        let (_, input) = p.board("e").unwrap().ready().unwrap();
        assert!(matches!(input, Input::Folder(f) if f.len() == 1));
        assert_eq!(p.board("z").unwrap_err().code, Code::NotFound);
        // Revisions move with the board's input, and the project's with
        // any board's.
        let mut src2 = src.clone();
        src2.insert("boards/a/input.zip".into(), Arc::from(&b"zap"[..]));
        let p2 = Project::load_from(Path::new("/p"), &src2).unwrap();
        assert_ne!(
            p2.board("a").unwrap().revision,
            p.board("a").unwrap().revision
        );
        assert_eq!(
            p2.board("b").unwrap().revision,
            p.board("b").unwrap().revision
        );
        assert_ne!(p2.revision, p.revision);
    }
}
