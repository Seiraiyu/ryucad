//! The per-machine cache of part records, never in a project: one
//! canonical JSON entry per record under `records/<2 hex>/<64 hex>.json`
//! (BLAKE3 of the source and key), each written to a temporary file and
//! renamed. An entry that doesn't read is a miss and goes; one from a
//! newer ryueda is a miss left for it. A source whose terms keep nothing
//! is never written, and a record past its time to live is never read.
//! 6.B adds each catalogue's index beside `records/`.

use crate::error::{Code, Error, Result};
use crate::record::{Record, Store, Terms};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const KIND: &str = "ryueda.part";
pub const FORMAT: u64 = 1;
/// Moves ryueda's cache folder (the parts cache is `parts` under it).
pub const DIR_VAR: &str = "RYUEDA_CACHE_DIR";

/// ryueda's cache folder: `RYUEDA_CACHE_DIR`, else the OS's
/// (`%LOCALAPPDATA%\ryueda\cache`, `~/Library/Caches/ryueda`,
/// `$XDG_CACHE_HOME/ryueda`).
pub fn ryueda_dir() -> Result<PathBuf> {
    dir_from(|k| std::env::var_os(k), std::env::consts::OS)
}

/// [`ryueda_dir`] from an environment and an OS name, for tests.
pub fn dir_from(env: impl Fn(&str) -> Option<OsString>, os: &str) -> Result<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = get(DIR_VAR) {
        return Ok(d);
    }
    let none = || Error::new(Code::Io, "can't find the user's cache folder");
    match os {
        "windows" => get("LOCALAPPDATA")
            .map(|d| d.join("ryueda").join("cache"))
            .ok_or_else(none),
        "macos" => get("HOME")
            .map(|h| h.join("Library/Caches/ryueda"))
            .ok_or_else(none),
        _ => get("XDG_CACHE_HOME")
            .or_else(|| get("HOME").map(|h| h.join(".cache")))
            .map(|d| d.join("ryueda"))
            .ok_or_else(none),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    record: Record,
    terms: Terms,
}

/// An entry's bytes as read.
#[derive(Debug)]
pub enum Decoded {
    Entry(Box<Record>, Terms),
    /// Written by a newer ryueda.
    Newer,
    Bad,
}

/// An entry's canonical text.
pub fn encode(record: &Record, terms: &Terms) -> String {
    let mut v = serde_json::to_value(Entry {
        record: record.clone(),
        terms: terms.clone(),
    })
    .expect("entries serialize");
    v["kind"] = Value::from(KIND);
    v["format"] = Value::from(FORMAT);
    ryu_doc::canonical::to_string(&v)
}

pub fn decode(bytes: &[u8]) -> Decoded {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Decoded::Bad;
    };
    let Ok(Value::Object(mut o)) = ryu_doc::canonical::parse(text) else {
        return Decoded::Bad;
    };
    if o.remove("kind").as_ref().and_then(Value::as_str) != Some(KIND) {
        return Decoded::Bad;
    }
    match o.remove("format").and_then(|f| f.as_u64()) {
        Some(f) if f > FORMAT => return Decoded::Newer,
        Some(f) if f >= 1 => {}
        _ => return Decoded::Bad,
    }
    match serde_json::from_value::<Entry>(Value::Object(o)) {
        Ok(e) => Decoded::Entry(Box::new(e.record), e.terms),
        Err(_) => Decoded::Bad,
    }
}

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// The parts cache in ryueda's cache folder.
    pub fn open() -> Result<Cache> {
        Ok(Cache::at(ryueda_dir()?.join("parts")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Cache {
        Cache { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entry_path(&self, source: &str, key: &str) -> PathBuf {
        let mut h = blake3::Hasher::new();
        h.update(b"ryueda.part.1\0");
        h.update(source.as_bytes());
        h.update(b"\0");
        h.update(key.as_bytes());
        let hex = h.finalize().to_hex();
        self.root
            .join("records")
            .join(&hex[..2])
            .join(format!("{hex}.json"))
    }

    /// Keeps `record` as `terms` allow: `Ok(false)`, with nothing written,
    /// for a source that keeps nothing.
    pub fn put(&self, record: &Record, terms: &Terms) -> Result<bool> {
        if terms.store == Store::Never {
            return Ok(false);
        }
        let path = self.entry_path(&record.source, &record.key);
        let dir = path.parent().expect("an entry is in a folder");
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, &e))?;
        static N: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            ".{}.{}.tmp",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, encode(record, terms)).map_err(|e| Error::io(&tmp, &e))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            Error::io(&path, &e)
        })?;
        Ok(true)
    }

    /// The record kept for `source`'s `key`, if it reads, is that part's
    /// and is still fresh at `now` (UTC seconds). Anything else that
    /// isn't a newer ryueda's is removed.
    pub fn get(&self, source: &str, key: &str, now: u64) -> Option<(Record, Terms)> {
        let path = self.entry_path(source, key);
        let bytes = std::fs::read(&path).ok()?;
        match decode(&bytes) {
            Decoded::Newer => None,
            Decoded::Entry(r, t)
                if r.source == source && r.key == key && t.fresh(r.fetched_at, now) =>
            {
                Some((*r, t))
            }
            _ => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Takes `source`'s `key` out; whether it was there.
    pub fn remove(&self, source: &str, key: &str) -> Result<bool> {
        let path = self.entry_path(source, key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::io(&path, &e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn terms(store: Store) -> Terms {
        Terms {
            store,
            write_parameters: false,
            attribution: "Data from a test".into(),
        }
    }

    fn record(key: &str, fetched_at: Option<u64>) -> Record {
        let mut r = Record::new("jlc", key);
        r.mpn = "CL05B104KO5NNNC".into();
        r.stock = Some(1_200_000);
        r.fetched_at = fetched_at;
        r
    }

    #[test]
    fn records_keep_as_their_terms_allow() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("parts"));
        let r = record("C1525", Some(1_000));
        assert!(cache.put(&r, &terms(Store::Cache)).unwrap());
        let at = cache.entry_path("jlc", "C1525");
        assert!(at.starts_with(dir.path().join("parts").join("records")));
        assert_eq!(
            cache.get("jlc", "C1525", 9_999_999),
            Some((r.clone(), terms(Store::Cache)))
        );
        assert!(cache.get("jlc", "C9", 0).is_none());
        // A source that keeps nothing never touches the disk.
        let never = Cache::at(dir.path().join("never"));
        assert!(!never.put(&r, &terms(Store::Never)).unwrap());
        assert!(!dir.path().join("never").exists());
        // A record past its time to live is gone.
        let short = record("C2", Some(1_000));
        cache.put(&short, &terms(Store::Ttl(100))).unwrap();
        assert!(cache.get("jlc", "C2", 1_100).is_some());
        assert!(cache.get("jlc", "C2", 1_101).is_none());
        assert!(!cache.entry_path("jlc", "C2").exists());
        // No temporary file is left behind.
        let left: Vec<_> = std::fs::read_dir(at.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1);
        assert!(cache.remove("jlc", "C1525").unwrap());
        assert!(!cache.remove("jlc", "C1525").unwrap());
    }

    #[test]
    fn bad_entries_are_misses_and_go_but_newer_ones_stay() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path());
        let r = record("C1525", Some(1));
        cache.put(&r, &terms(Store::Cache)).unwrap();
        let at = cache.entry_path("jlc", "C1525");
        std::fs::write(&at, b"{not json").unwrap();
        assert!(cache.get("jlc", "C1525", 2).is_none());
        assert!(!at.exists());
        let newer = encode(&r, &terms(Store::Cache)).replace("\"format\": 1", "\"format\": 2");
        std::fs::write(&at, &newer).unwrap();
        assert!(cache.get("jlc", "C1525", 2).is_none());
        assert!(at.exists(), "a newer ryueda's entry is its own");
        // An entry naming another part than its path's is not used.
        std::fs::write(&at, encode(&record("C9", Some(1)), &terms(Store::Cache))).unwrap();
        assert!(cache.get("jlc", "C1525", 2).is_none());
        assert!(!at.exists());
        assert!(matches!(decode(b"\xff"), Decoded::Bad));
        assert!(matches!(
            decode(encode(&r, &terms(Store::Cache)).as_bytes()),
            Decoded::Entry(..)
        ));
    }

    #[test]
    fn the_folder_is_ryuedas_cache_folders() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        assert_eq!(
            dir_from(env(&[(DIR_VAR, "/c"), ("HOME", "/h")]), "linux").unwrap(),
            PathBuf::from("/c")
        );
        assert_eq!(
            dir_from(env(&[("HOME", "/h")]), "linux").unwrap(),
            PathBuf::from("/h/.cache/ryueda")
        );
        assert_eq!(
            dir_from(env(&[("XDG_CACHE_HOME", "/x"), ("HOME", "/h")]), "linux").unwrap(),
            PathBuf::from("/x/ryueda")
        );
        assert_eq!(
            dir_from(env(&[("HOME", "/h")]), "macos").unwrap(),
            PathBuf::from("/h/Library/Caches/ryueda")
        );
        assert_eq!(
            dir_from(env(&[("LOCALAPPDATA", "C:/L")]), "windows").unwrap(),
            PathBuf::from("C:/L").join("ryueda").join("cache")
        );
        assert!(dir_from(env(&[]), "linux").is_err());
    }
}
