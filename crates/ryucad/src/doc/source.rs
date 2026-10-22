//! Where a project's files come from: the folder on disk, or a set of
//! files held in memory (a Git revision's, a snapshot). Paths are relative
//! to the project root, with `/`; the root itself is `""`.

use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// What an entry of a folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    /// Neither: a broken link, a socket.
    Other,
}

/// A project's files, read only.
pub trait Source {
    /// The entries of the folder `dir`, sorted by name; empty when there is
    /// no such folder.
    fn list(&self, dir: &str) -> Result<Vec<(String, Kind)>>;
    /// The bytes of the file at `path`, or `None` when there is none.
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>>;
}

/// `dir/name`, or `name` at the root.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The project folder on disk.
pub struct Folder<'a>(pub &'a Path);

impl Folder<'_> {
    fn path(&self, rel: &str) -> std::path::PathBuf {
        rel.split('/')
            .filter(|s| !s.is_empty())
            .fold(self.0.to_path_buf(), |p, s| p.join(s))
    }
}

impl Source for Folder<'_> {
    fn list(&self, dir: &str) -> Result<Vec<(String, Kind)>> {
        let path = self.path(dir);
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io(&path, &e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(&path, &e))?;
            // Links are followed, as the files they point at are read.
            let p = entry.path();
            let kind = if p.is_dir() {
                Kind::Dir
            } else if p.is_file() {
                Kind::File
            } else {
                Kind::Other
            };
            out.push((entry.file_name().to_string_lossy().into_owned(), kind));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path(rel);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            // A folder where a file should be reads as no file.
            Err(_) if path.is_dir() => Ok(None),
            Err(e) => Err(Error::io(&path, &e)),
        }
    }
}

/// Files in memory, by path: a folder is every path below it.
impl Source for BTreeMap<String, Vec<u8>> {
    fn list(&self, dir: &str) -> Result<Vec<(String, Kind)>> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut out: Vec<(String, Kind)> = Vec::new();
        for path in self.range(prefix.clone()..).map(|(p, _)| p) {
            let Some(rest) = path.strip_prefix(&prefix) else {
                break;
            };
            let entry = match rest.split_once('/') {
                Some((name, _)) => (name.to_owned(), Kind::Dir),
                None => (rest.to_owned(), Kind::File),
            };
            if out.last() != Some(&entry) {
                out.push(entry);
            }
        }
        // `a.json` sorts before `a/` among paths, not among names.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.get(path).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_in_memory_list_as_folders() {
        let files: BTreeMap<String, Vec<u8>> = [
            ("ryucad.json", "m"),
            ("parts/a/part.json", "a"),
            ("parts/a/features/x.json", "x"),
            ("parts/b/part.json", "b"),
            ("partsy", "not under parts"),
        ]
        .into_iter()
        .map(|(p, b)| (p.to_owned(), b.as_bytes().to_vec()))
        .collect();
        let names = |dir: &str| -> Vec<(String, Kind)> { files.list(dir).unwrap() };
        assert_eq!(
            names(""),
            [
                ("parts".to_owned(), Kind::Dir),
                ("partsy".to_owned(), Kind::File),
                ("ryucad.json".to_owned(), Kind::File)
            ]
        );
        assert_eq!(
            names("parts"),
            [("a".to_owned(), Kind::Dir), ("b".to_owned(), Kind::Dir)]
        );
        assert_eq!(
            names("parts/a"),
            [
                ("features".to_owned(), Kind::Dir),
                ("part.json".to_owned(), Kind::File)
            ]
        );
        assert!(names("parts/c").is_empty());
        assert_eq!(
            files.read("parts/b/part.json").unwrap(),
            Some(b"b".to_vec())
        );
        assert_eq!(files.read("parts/b").unwrap(), None);
    }

    #[test]
    fn the_folder_lists_and_reads() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("parts/a")).unwrap();
        fs::write(dir.path().join("parts/a/part.json"), "a").unwrap();
        let f = Folder(dir.path());
        assert_eq!(f.list("").unwrap(), [("parts".to_owned(), Kind::Dir)]);
        assert_eq!(
            f.list("parts/a").unwrap(),
            [("part.json".to_owned(), Kind::File)]
        );
        assert!(f.list("nowhere").unwrap().is_empty());
        assert_eq!(f.read("parts/a/part.json").unwrap(), Some(b"a".to_vec()));
        assert_eq!(f.read("parts/a/none.json").unwrap(), None);
        assert_eq!(f.read("parts/a").unwrap(), None);
    }
}
