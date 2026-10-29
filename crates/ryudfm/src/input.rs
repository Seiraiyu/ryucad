//! A board's input: the job as it was given, committed byte for byte, an
//! archive as one file (`input.tgz`, `input.tar`, `input.zip`) or a
//! folder file for file (`input/…`), and the BLAKE3 that names it.
//!
//! An archive's hash is its bytes'. A folder's is BLAKE3 over
//! `ryudfm.tree.1` and, for each file in byte order of its path relative
//! to `input/`, the path's length and bytes, then the content's length and
//! BLAKE3 (lengths as 8 bytes, little-endian).

use crate::odb::{self, Job, Limits};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// What an input came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Folder,
    Tgz,
    Tar,
    Zip,
}

impl Container {
    /// The input's name in its board's folder.
    pub fn file_name(self) -> &'static str {
        match self {
            Container::Folder => "input",
            Container::Tgz => "input.tgz",
            Container::Tar => "input.tar",
            Container::Zip => "input.zip",
        }
    }

    pub fn of(c: odb::Container) -> Container {
        match c {
            odb::Container::Folder => Container::Folder,
            odb::Container::Tgz => Container::Tgz,
            odb::Container::Tar => Container::Tar,
            odb::Container::Zip => Container::Zip,
        }
    }
}

/// An input's bytes, shared with the session's snapshot of the files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Archive(Arc<[u8]>),
    /// Each file by its path relative to `input/`, sorted.
    Folder(Vec<(String, Arc<[u8]>)>),
}

/// A hash's text: 64 lower-case hex digits.
fn hex(h: blake3::Hash) -> String {
    h.to_hex().to_string()
}

/// The BLAKE3 of an archive's bytes.
pub fn archive_hash(bytes: &[u8]) -> String {
    hex(blake3::hash(bytes))
}

/// The BLAKE3 of a folder's files, by path relative to it.
pub fn tree_hash<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut files: Vec<(&str, &[u8])> = files.into_iter().collect();
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = blake3::Hasher::new();
    h.update(b"ryudfm.tree.1");
    for (path, bytes) in files {
        h.update(&(path.len() as u64).to_le_bytes());
        h.update(path.as_bytes());
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(blake3::hash(bytes).as_bytes());
    }
    hex(h.finalize())
}

impl Input {
    pub fn hash(&self) -> String {
        match self {
            Input::Archive(b) => archive_hash(b),
            Input::Folder(files) => tree_hash(files.iter().map(|(p, b)| (p.as_str(), &b[..]))),
        }
    }

    /// The job the input holds, named `stem` where the job doesn't name
    /// itself.
    pub fn job(&self, stem: &str) -> Result<Job, odb::Error> {
        let limits = Limits::default();
        match self {
            Input::Archive(b) => Job::from_archive(stem, b, limits),
            Input::Folder(files) => Job::from_files(
                odb::Container::Folder,
                stem,
                files.iter().map(|(p, b)| (p.clone(), b.to_vec())).collect(),
                limits,
            ),
        }
    }

    /// How many bytes it holds.
    pub fn len(&self) -> u64 {
        match self {
            Input::Archive(b) => b.len() as u64,
            Input::Folder(files) => files.iter().map(|(_, b)| b.len() as u64).sum(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_hash_by_path_and_content() {
        let a = tree_hash([("b", &b"2"[..]), ("a/x", &b"1"[..])]);
        // Order given doesn't matter; paths and contents do.
        assert_eq!(a, tree_hash([("a/x", &b"1"[..]), ("b", &b"2"[..])]));
        assert_ne!(a, tree_hash([("a/y", &b"1"[..]), ("b", &b"2"[..])]));
        assert_ne!(a, tree_hash([("a/x", &b"1"[..]), ("b", &b"3"[..])]));
        // A path's bytes end where its length says: "ab" + "" isn't "a" + "b".
        assert_ne!(tree_hash([("ab", &b""[..])]), tree_hash([("a", &b"b"[..])]));
        // Pinned, so the format never moves.
        assert_eq!(
            tree_hash([("matrix/matrix", &b"STEP {\n}\n"[..])]),
            hex({
                let mut h = blake3::Hasher::new();
                h.update(b"ryudfm.tree.1");
                h.update(&13u64.to_le_bytes());
                h.update(b"matrix/matrix");
                h.update(&9u64.to_le_bytes());
                h.update(blake3::hash(b"STEP {\n}\n").as_bytes());
                h.finalize()
            })
        );
        assert_eq!(archive_hash(b""), hex(blake3::hash(b"")));
        assert_eq!(archive_hash(b"").len(), 64);
    }

    #[test]
    fn names_by_container() {
        assert_eq!(Container::Folder.file_name(), "input");
        assert_eq!(Container::of(odb::Container::Tgz).file_name(), "input.tgz");
        assert_eq!(
            serde_json::to_value(Container::Zip).unwrap(),
            serde_json::json!("zip")
        );
    }
}
