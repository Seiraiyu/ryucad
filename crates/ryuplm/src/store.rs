//! The local cache of documents' bytes (design 937): service-stored bytes
//! by their BLAKE3, so a command reads them without the network once one
//! has fetched or written them. Derived data, never committed: deleting it
//! loses nothing the service holds. Written whole (a temporary file, then a
//! rename) and read only after the hash is checked.

use crate::error::{Code, Error, Result};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Moves ryuplm's cache folder (documents are `documents/` under it).
pub const DIR_VAR: &str = "RYUPLM_CACHE_DIR";

/// ryuplm's cache folder: `RYUPLM_CACHE_DIR`, else the OS's
/// (`%LOCALAPPDATA%\ryuplm\cache`, `~/Library/Caches/ryuplm`,
/// `$XDG_CACHE_HOME/ryuplm`), as ryueda's is found.
pub fn dir() -> Result<PathBuf> {
    dir_from(|k| std::env::var_os(k), std::env::consts::OS)
}

/// [`dir`] from an environment and an OS name, for tests.
pub fn dir_from(env: impl Fn(&str) -> Option<OsString>, os: &str) -> Result<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = get(DIR_VAR) {
        return Ok(d);
    }
    let none = || {
        Error::new(
            Code::Io,
            "can't find the user's cache folder; set RYUPLM_CACHE_DIR",
        )
    };
    match os {
        "windows" => get("LOCALAPPDATA")
            .map(|d| d.join("ryuplm").join("cache"))
            .ok_or_else(none),
        "macos" => get("HOME")
            .map(|h| h.join("Library/Caches/ryuplm"))
            .ok_or_else(none),
        _ => get("XDG_CACHE_HOME")
            .or_else(|| get("HOME").map(|h| h.join(".cache")))
            .map(|d| d.join("ryuplm"))
            .ok_or_else(none),
    }
}

/// Documents' bytes under a cache folder.
#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
}

fn hex_ok(h: &str) -> bool {
    h.len() == 64
        && h.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Cache {
    /// The cache in [`dir`].
    pub fn open() -> Result<Cache> {
        Ok(Cache::at(&dir()?))
    }

    pub fn at(root: &Path) -> Cache {
        Cache {
            root: root.to_path_buf(),
        }
    }

    /// Where the bytes of `blake3` are, there or not.
    pub fn path(&self, blake3: &str) -> PathBuf {
        self.root.join("documents").join(blake3)
    }

    /// Whether an entry of `size` bytes is there (its hash unchecked:
    /// [`Cache::get`] checks it).
    pub fn has(&self, blake3: &str, size: u64) -> bool {
        hex_ok(blake3) && std::fs::metadata(self.path(blake3)).is_ok_and(|m| m.len() == size)
    }

    /// Copies `src` into the cache, hashing as it goes: its BLAKE3 and
    /// length. Never holds the file whole in memory.
    pub fn put_file(&self, src: &Path) -> Result<(String, u64)> {
        let mut f = std::fs::File::open(src).map_err(|e| Error::io(src, &e))?;
        self.put_from(&mut f, src)
    }

    pub fn put_bytes(&self, bytes: &[u8]) -> Result<String> {
        self.put_from(&mut &bytes[..], &self.root).map(|(h, _)| h)
    }

    fn put_from(&self, r: &mut dyn Read, named: &Path) -> Result<(String, u64)> {
        let dir = self.root.join("documents");
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, &e))?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| Error::io(&dir, &e))?;
        let mut h = blake3::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut size = 0u64;
        loop {
            let n = r.read(&mut buf).map_err(|e| Error::io(named, &e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            tmp.write_all(&buf[..n])
                .map_err(|e| Error::io(tmp.path(), &e))?;
            size += n as u64;
        }
        tmp.as_file()
            .sync_all()
            .map_err(|e| Error::io(tmp.path(), &e))?;
        let hex = h.finalize().to_hex().to_string();
        let at = self.path(&hex);
        // Always over whatever is there: an entry of the right length with
        // the wrong bytes is replaced by the bytes just hashed.
        tmp.persist(&at).map_err(|e| Error::io(&at, &e.error))?;
        Ok((hex, size))
    }

    /// Whether the entry of `blake3` is there and is `size` bytes hashing
    /// to it, read in pieces, never whole; a bad entry is removed.
    pub fn verify(&self, blake3: &str, size: u64) -> Result<bool> {
        if !self.has(blake3, size) {
            return Ok(false);
        }
        let at = self.path(blake3);
        let mut f = std::fs::File::open(&at).map_err(|e| Error::io(&at, &e))?;
        let mut h = blake3::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf).map_err(|e| Error::io(&at, &e))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        if h.finalize().to_hex().as_str() != blake3 {
            drop(f);
            let _ = std::fs::remove_file(&at);
            return Ok(false);
        }
        Ok(true)
    }

    /// The bytes of `blake3`, `None` when they aren't there or aren't
    /// `size` bytes hashing to it (a bad entry is removed).
    pub fn get(&self, blake3: &str, size: u64) -> Result<Option<Arc<[u8]>>> {
        if !self.has(blake3, size) {
            return Ok(None);
        }
        let at = self.path(blake3);
        let b = std::fs::read(&at).map_err(|e| Error::io(&at, &e))?;
        if b.len() as u64 != size || blake3::hash(&b).to_hex().as_str() != blake3 {
            let _ = std::fs::remove_file(&at);
            return Ok(None);
        }
        Ok(Some(Arc::from(b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_cache_folder_is_the_variables_or_the_oss() {
        assert_eq!(
            dir_from(env(&[(DIR_VAR, "/c")]), "linux").unwrap(),
            Path::new("/c")
        );
        assert_eq!(
            dir_from(env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")]), "linux").unwrap(),
            Path::new("/x/ryuplm")
        );
        assert_eq!(
            dir_from(env(&[("HOME", "/h")]), "linux").unwrap(),
            Path::new("/h/.cache/ryuplm")
        );
        assert_eq!(
            dir_from(env(&[("HOME", "/h")]), "macos").unwrap(),
            Path::new("/h/Library/Caches/ryuplm")
        );
        assert_eq!(
            dir_from(env(&[("LOCALAPPDATA", "C:/L")]), "windows").unwrap(),
            Path::new("C:/L/ryuplm/cache")
        );
    }

    #[test]
    fn entries_are_written_whole_and_read_only_when_their_hash_holds() {
        let d = tempfile::tempdir().unwrap();
        let c = Cache::at(d.path());
        let src = d.path().join("src.pdf");
        std::fs::write(&src, b"%PDF one").unwrap();
        let (h, size) = c.put_file(&src).unwrap();
        assert_eq!(
            (h.as_str(), size),
            (blake3::hash(b"%PDF one").to_hex().as_str(), 8)
        );
        assert_eq!(&*c.get(&h, 8).unwrap().unwrap(), b"%PDF one");
        assert!(c.has(&h, 8));
        assert_eq!(
            c.put_bytes(b"%PDF one").unwrap(),
            h,
            "putting again is the same entry"
        );
        // A corrupt entry is dropped, never answered.
        std::fs::write(c.path(&h), b"tampered").unwrap();
        assert!(c.get(&h, 8).unwrap().is_none());
        assert!(!c.path(&h).exists());
        assert!(c.get(&"0".repeat(64), 1).unwrap().is_none());
        // An entry of the right length with the wrong bytes: putting the
        // bytes again replaces it, and verify drops it.
        std::fs::write(c.path(&h), b"%PDF onE").unwrap();
        assert!(c.has(&h, 8), "has looks at the length only");
        assert!(!c.verify(&h, 8).unwrap());
        assert!(!c.path(&h).exists());
        std::fs::write(c.path(&h), b"%PDF onE").unwrap();
        assert_eq!(c.put_file(&src).unwrap().0, h);
        assert!(c.verify(&h, 8).unwrap(), "the good bytes replaced the bad");
        // Nothing but finished entries is ever there.
        let names: Vec<String> = std::fs::read_dir(d.path().join("documents"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| n.len() == 64), "{names:?}");
    }
}
