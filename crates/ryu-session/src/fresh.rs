//! Keeping a session fresh against the files. The files are the truth:
//! before every request a session checks each file it depends on. An
//! unchanged size and modification time is trusted unless the time is too
//! recent to be reliable; otherwise the content is read again. The stamps
//! keep every file's bytes, so a snapshot of the project costs no reading.
//! Which files a session depends on is the product's to say.

use ryu_doc::error::{Error, Result};
use ryu_doc::source::relative;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// A modification time this close to when it was read may be followed by
/// another write in the same timestamp tick, so it isn't trusted.
const RACY: Duration = Duration::from_secs(2);

/// A file's content, shared: snapshots, dry runs and the activity log hold
/// the very bytes the stamps do.
pub type Bytes = Arc<[u8]>;

/// A project's files and their contents, by relative path with `/`.
pub type Files = BTreeMap<String, Bytes>;

#[derive(Clone, Debug, PartialEq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    bytes: Arc<[u8]>,
}

/// Every file a session depends on, as last read.
#[derive(Clone, Debug)]
pub struct Stamps {
    stamps: BTreeMap<PathBuf, Stamp>,
    stamped_at: SystemTime,
}

impl Default for Stamps {
    fn default() -> Self {
        Stamps {
            stamps: BTreeMap::new(),
            stamped_at: SystemTime::now(),
        }
    }
}

impl Stamps {
    /// The stamps of files that never change on disk (a Git revision's,
    /// under `root`): a scan would read every one again.
    pub fn frozen(root: &Path, files: Files) -> Stamps {
        let stamps = files
            .into_iter()
            .map(|(rel, bytes)| {
                let path = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
                let stamp = Stamp {
                    modified: None,
                    len: bytes.len() as u64,
                    bytes,
                };
                (path, stamp)
            })
            .collect();
        Stamps {
            stamps,
            stamped_at: SystemTime::now(),
        }
    }

    /// Checks every file in `files` (each with its metadata, as
    /// [`read_dir`] gives them) against its stamp, reading only those whose
    /// size or time moved or whose time is too recent to trust. Returns
    /// what changed, relative to `root`, sorted: `created`, `modified` or
    /// `deleted`.
    pub fn scan(
        &mut self,
        root: &Path,
        files: Vec<(PathBuf, fs::Metadata)>,
    ) -> Result<Vec<(String, &'static str)>> {
        let trusted_before = self.stamped_at.checked_sub(RACY);
        let mut changes = Vec::new();
        let mut stamps = BTreeMap::new();
        for (path, meta) in files {
            let (modified, len) = (meta.modified().ok(), meta.len());
            let old = self.stamps.get(&path);
            let trusted = match (old, modified, trusted_before) {
                (Some(o), Some(m), Some(t)) => o.modified == Some(m) && o.len == len && m < t,
                _ => false,
            };
            let stamp = match old {
                Some(o) if trusted => o.clone(),
                _ => {
                    let bytes = fs::read(&path).map_err(|e| Error::io(&path, &e))?;
                    Stamp {
                        modified,
                        len,
                        bytes: bytes.into(),
                    }
                }
            };
            match old {
                None => changes.push((path.clone(), "created")),
                Some(o) if o.bytes != stamp.bytes => changes.push((path.clone(), "modified")),
                Some(_) => {}
            }
            stamps.insert(path, stamp);
        }
        for path in self.stamps.keys().filter(|p| !stamps.contains_key(*p)) {
            changes.push((path.clone(), "deleted"));
        }
        self.stamped_at = SystemTime::now();
        self.stamps = stamps;
        let mut changes: Vec<(String, &'static str)> = changes
            .into_iter()
            .map(|(p, c)| (relative(root, &p), c))
            .collect();
        changes.sort();
        Ok(changes)
    }

    /// Every file as last read, relative to `root`.
    pub fn snapshot(&self, root: &Path) -> Files {
        self.stamps
            .iter()
            .map(|(p, s)| (relative(root, p), Arc::clone(&s.bytes)))
            .collect()
    }

    /// A hash of every file as last read.
    pub fn digest(&self) -> String {
        let mut h = blake3::Hasher::new();
        for (path, s) in &self.stamps {
            let p = path.to_string_lossy();
            h.update(&(p.len() as u64).to_le_bytes());
            h.update(p.as_bytes());
            h.update(&(s.bytes.len() as u64).to_le_bytes());
            h.update(&s.bytes);
        }
        h.finalize().to_hex().to_string()
    }

    /// The bytes of the file at `path`, as last read.
    pub fn bytes(&self, path: &Path) -> Option<&[u8]> {
        self.stamps.get(path).map(|s| &*s.bytes)
    }

    /// [`Stamps::bytes`], shared.
    pub fn shared(&self, path: &Path) -> Option<Bytes> {
        self.stamps.get(path).map(|s| Arc::clone(&s.bytes))
    }

    /// The paths of the files at or after `from`, in order: those under a
    /// folder come first among them.
    pub fn paths_from(&self, from: &Path) -> impl Iterator<Item = &PathBuf> {
        self.stamps.range(from.to_path_buf()..).map(|(p, _)| p)
    }
}

/// Each entry in `dir` with its metadata, following symlinks; empty when
/// `dir` doesn't exist. An entry gone by the time it is looked at is left
/// out. A directory listing says what each entry is (and on Windows gives
/// its size and time) without opening it, so checking a project costs at
/// most one `stat` per file.
pub fn read_dir(dir: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(dir, &e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(dir, &e))?;
        let path = entry.path();
        let meta = match entry.file_type() {
            Ok(t) if t.is_symlink() => fs::metadata(&path),
            Ok(_) => entry.metadata(),
            Err(e) => Err(e),
        };
        match meta {
            Ok(m) => out.push((path, m)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&path, &e)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(dir: &Path) -> Vec<(PathBuf, fs::Metadata)> {
        read_dir(dir)
            .unwrap()
            .into_iter()
            .filter(|(_, m)| m.is_file())
            .collect()
    }

    #[test]
    fn scans_see_created_modified_and_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a"), "1").unwrap();
        fs::write(root.join("b"), "2").unwrap();
        let mut s = Stamps::default();
        assert_eq!(
            s.scan(root, listed(root)).unwrap(),
            [("a".to_owned(), "created"), ("b".to_owned(), "created")]
        );
        assert!(s.scan(root, listed(root)).unwrap().is_empty());
        // The same size and a time too recent to trust: read again.
        fs::write(root.join("a"), "3").unwrap();
        fs::remove_file(root.join("b")).unwrap();
        assert_eq!(
            s.scan(root, listed(root)).unwrap(),
            [("a".to_owned(), "modified"), ("b".to_owned(), "deleted")]
        );
        assert_eq!(
            s.snapshot(root),
            Files::from([("a".into(), b"3".as_slice().into())])
        );
        assert_eq!(s.bytes(&root.join("a")), Some(&b"3"[..]));
        assert_eq!(s.paths_from(root).count(), 1);
    }

    #[test]
    fn frozen_stamps_hash_as_their_files() {
        let files = Files::from([
            ("x/y".into(), b"1".as_slice().into()),
            ("z".into(), Bytes::from(Vec::new())),
        ]);
        let root = Path::new("/r");
        let s = Stamps::frozen(root, files.clone());
        assert_eq!(s.snapshot(root), files);
        assert_eq!(s.digest(), Stamps::frozen(root, files).digest());
        assert_ne!(s.digest(), Stamps::default().digest());
        assert!(read_dir(Path::new("/no/such/folder")).unwrap().is_empty());
    }
}
