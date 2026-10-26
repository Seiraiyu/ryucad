//! The disk cache: results by the hashes of their inputs, in the OS cache
//! folder (never inside a project), shared by every project and daemon on
//! the machine.
//!
//! An entry is a file named by its key: a BLAKE3 of this build's identity
//! (the versions, the commit, and the executable's size and time, so a
//! development build never reads what another build wrote), the entry's
//! kind and the input hash the session already computes. It holds a
//! header, a BLAKE3 of its payload and the payload, and is written to a
//! temporary file then renamed into place, so a reader sees a whole entry
//! or none. Keys are content: two daemons writing one entry write the same
//! bytes, and a lost race costs nothing. A read whose header, length or
//! hash is wrong is a miss, and the entry is removed. Reading an entry
//! touches its time, and trimming removes the least recently read first,
//! down to nine tenths of `cache.max_bytes`. A cache that can't be written
//! (a full disk) is skipped with one warning; the cache can never change a
//! result, only how soon it comes.

use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// Overrides the cache folder (tests, and anyone who wants another).
pub const CACHE_DIR_VAR: &str = "RYUCAD_CACHE_DIR";

/// The first bytes of every entry.
const MAGIC: &[u8] = b"ryucad.cache.1\n";

/// The folder under the cache folder this format's entries live in.
const LAYOUT: &str = "v1";

/// A temporary file older than this was left by a writer that died.
const STALE_TEMP: Duration = Duration::from_secs(3600);

/// What an entry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A body-making feature's solids.
    Feature,
    /// A join, cut or intersect's own body.
    Tool,
    /// A body's display mesh at one quality.
    Mesh,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Feature, Kind::Tool, Kind::Mesh];

    /// Its folder, and its name in `cache.stats`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Feature => "features",
            Kind::Tool => "tools",
            Kind::Mesh => "meshes",
        }
    }
}

/// How much the cache holds: entries and bytes, all and per kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub entries: u64,
    pub bytes: u64,
    pub kinds: Vec<(Kind, u64, u64)>,
}

impl Stats {
    pub fn to_json(&self) -> Value {
        let kinds: serde_json::Map<String, Value> = self
            .kinds
            .iter()
            .map(|(k, n, b)| (k.as_str().to_owned(), json!({"entries": n, "bytes": b})))
            .collect();
        json!({"entries": self.entries, "bytes": self.bytes, "kinds": kinds})
    }
}

/// A cache folder and the build reading and writing it.
#[derive(Debug)]
pub struct Disk {
    root: PathBuf,
    build: [u8; 32],
    /// Whether the one warning about failed writes has been given.
    warned: AtomicBool,
    /// Whether the one note about a corrupt entry has been given.
    noted: AtomicBool,
    /// Bytes written since the last trim.
    written: AtomicU64,
    /// Whether a background trim is running.
    trimming: Arc<AtomicBool>,
    /// What the cache may hold, in bytes.
    max_bytes: AtomicU64,
}

/// The OS cache folder for ryucad, or `RYUCAD_CACHE_DIR`.
pub fn cache_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os(CACHE_DIR_VAR) {
        return Ok(PathBuf::from(d));
    }
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let none = || Error::new(Code::Io, "can't find the user's cache folder");
    if cfg!(windows) {
        return env("LOCALAPPDATA")
            .map(|d| d.join("ryucad").join("cache"))
            .ok_or_else(none);
    }
    let home = env("HOME");
    if cfg!(target_os = "macos") {
        return home
            .map(|h| h.join("Library/Caches/ryucad"))
            .ok_or_else(none);
    }
    env("XDG_CACHE_HOME")
        .or_else(|| home.map(|h| h.join(".cache")))
        .map(|d| d.join("ryucad"))
        .ok_or_else(none)
}

/// This build's identity: its versions and commit, and the size and
/// modification time of the running executable, which every build
/// changes. `None` when the executable can't be read.
pub fn build_identity() -> Option<[u8; 32]> {
    let v = crate::proto::VersionInfo::current();
    let exe = std::env::current_exe().ok()?;
    let meta = fs::metadata(exe).ok()?;
    let since = meta
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?;
    let mut h = blake3::Hasher::new();
    for piece in [&v.ryucad, &v.ryukei, &v.commit, &v.target] {
        h.update(&(piece.len() as u64).to_le_bytes());
        h.update(piece.as_bytes());
    }
    h.update(&meta.len().to_le_bytes());
    h.update(&since.as_nanos().to_le_bytes());
    Some(*h.finalize().as_bytes())
}

/// The default of `cache.max_bytes`: 10 GiB.
pub const DEFAULT_MAX_BYTES: u64 = 10 << 30;

fn io(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(Code::Io, format!("can't {what} {}: {e}", path.display()))
}

impl Disk {
    /// The cache in `root` (its entries under `root/v1`), for `build`.
    pub fn open(root: impl Into<PathBuf>, build: [u8; 32]) -> Disk {
        Disk {
            root: root.into().join(LAYOUT),
            build,
            warned: AtomicBool::new(false),
            noted: AtomicBool::new(false),
            written: AtomicU64::new(0),
            trimming: Arc::new(AtomicBool::new(false)),
            max_bytes: AtomicU64::new(DEFAULT_MAX_BYTES),
        }
    }

    /// The user's cache, for this build, bounded by `cache.max_bytes`;
    /// `None` when there is no cache folder or executable to name it by.
    pub fn for_user() -> Option<Disk> {
        let disk = Disk::open(cache_dir().ok()?, build_identity()?);
        disk.set_max_bytes(max_bytes_preference());
        Some(disk)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn set_max_bytes(&self, max: u64) {
        self.max_bytes.store(max, Ordering::Relaxed);
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// The key of `input`'s entry of `kind`: 64 hex digits.
    pub fn key(&self, kind: Kind, input: &[u8]) -> String {
        let mut h = blake3::Hasher::new();
        h.update(&self.build);
        h.update(kind.as_str().as_bytes());
        h.update(&[0]);
        h.update(input);
        h.finalize().to_hex().to_string()
    }

    fn path(&self, kind: Kind, key: &str) -> PathBuf {
        self.root.join(kind.as_str()).join(&key[..2]).join(key)
    }

    /// The payload of `input`'s entry of `kind`, if the cache holds a
    /// sound one. A corrupt entry is removed.
    pub fn get(&self, kind: Kind, input: &[u8]) -> Option<Vec<u8>> {
        let path = self.path(kind, &self.key(kind, input));
        let bytes = fs::read(&path).ok()?;
        match payload(&bytes) {
            Some(p) => {
                // Reading counts as using: trimming goes by this time.
                if let Ok(f) = fs::File::options().write(true).open(&path) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Some(p.to_vec())
            }
            None => {
                self.forget(&path);
                None
            }
        }
    }

    /// Removes a corrupt entry: one that failed its hash here, or whose
    /// payload a caller couldn't decode.
    pub fn discard(&self, kind: Kind, input: &[u8]) {
        self.forget(&self.path(kind, &self.key(kind, input)));
    }

    fn forget(&self, path: &Path) {
        let _ = fs::remove_file(path);
        if !self.noted.swap(true, Ordering::Relaxed) {
            eprintln!(
                "ryucad: removed a corrupt cache entry, {}; it will be rebuilt",
                path.display()
            );
        }
    }

    /// Writes `payload` as `input`'s entry of `kind`, unless one is there.
    /// A failure is warned about once and otherwise ignored.
    pub fn put(&self, kind: Kind, input: &[u8], payload_bytes: &[u8]) {
        let path = self.path(kind, &self.key(kind, input));
        if path.exists() {
            return;
        }
        match write_entry(&path, payload_bytes) {
            Ok(n) => {
                let total = self.written.fetch_add(n, Ordering::Relaxed) + n;
                if total > self.max_bytes() / 16 {
                    self.trim_in_background();
                }
            }
            Err(e) => {
                if !self.warned.swap(true, Ordering::Relaxed) {
                    eprintln!("ryucad: not writing to the cache: {}", e.message);
                }
            }
        }
    }

    /// Every entry file, with its kind, size and last use.
    fn entries(&self) -> Vec<(Kind, PathBuf, u64, SystemTime)> {
        let mut out = Vec::new();
        for kind in Kind::ALL {
            let Ok(shards) = fs::read_dir(self.root.join(kind.as_str())) else {
                continue;
            };
            for shard in shards.flatten() {
                let Ok(files) = fs::read_dir(shard.path()) else {
                    continue;
                };
                for f in files.flatten() {
                    let path = f.path();
                    let Ok(meta) = f.metadata() else {
                        continue;
                    };
                    let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    if path.extension().is_some_and(|e| e == "tmp") {
                        // A writer's, or a dead writer's.
                        if used.elapsed().is_ok_and(|age| age > STALE_TEMP) {
                            let _ = fs::remove_file(&path);
                        }
                        continue;
                    }
                    out.push((kind, path, meta.len(), used));
                }
            }
        }
        out
    }

    /// What the cache holds.
    pub fn stats(&self) -> Stats {
        let mut s = Stats {
            kinds: Kind::ALL.iter().map(|&k| (k, 0, 0)).collect(),
            ..Stats::default()
        };
        for (kind, _, len, _) in self.entries() {
            s.entries += 1;
            s.bytes += len;
            let k = s
                .kinds
                .iter_mut()
                .find(|(x, _, _)| *x == kind)
                .expect("every kind");
            k.1 += 1;
            k.2 += len;
        }
        s
    }

    /// Removes every entry; returns what was removed.
    pub fn clear(&self) -> Stats {
        let before = self.stats();
        for (_, path, _, _) in self.entries() {
            let _ = fs::remove_file(path);
        }
        before
    }

    /// Removes the least recently used entries (ties by path) until the
    /// cache holds at most nine tenths of `max`, if it holds more than
    /// `max`. Returns how many it removed.
    pub fn trim(&self, max: u64) -> usize {
        let mut entries = self.entries();
        let mut total: u64 = entries.iter().map(|e| e.2).sum();
        if total <= max {
            return 0;
        }
        entries.sort_by(|a, b| (a.3, &a.1).cmp(&(b.3, &b.1)));
        let goal = max / 10 * 9;
        let mut removed = 0;
        for (_, path, len, _) in entries {
            if total <= goal {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                removed += 1;
            }
        }
        removed
    }

    /// Trims on a thread of its own, unless one is already trimming.
    pub fn trim_in_background(&self) {
        if self.trimming.swap(true, Ordering::AcqRel) {
            return;
        }
        self.written.store(0, Ordering::Relaxed);
        let trimmer = Disk {
            root: self.root.clone(),
            build: self.build,
            warned: AtomicBool::new(true),
            noted: AtomicBool::new(true),
            written: AtomicU64::new(0),
            trimming: Arc::clone(&self.trimming),
            max_bytes: AtomicU64::new(self.max_bytes()),
        };
        std::thread::spawn(move || {
            trimmer.trim(trimmer.max_bytes());
            trimmer.trimming.store(false, Ordering::Release);
        });
    }
}

/// `cache.max_bytes` from the user's preferences.
pub fn max_bytes_preference() -> u64 {
    crate::config::user("cache.max_bytes")
        .as_u64()
        .unwrap_or(DEFAULT_MAX_BYTES)
}

/// The payload of an entry's bytes, if its header and hash are sound.
fn payload(bytes: &[u8]) -> Option<&[u8]> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (hash, rest) = rest.split_first_chunk::<32>()?;
    let (len, payload) = rest.split_first_chunk::<8>()?;
    if u64::from_le_bytes(*len) != payload.len() as u64 {
        return None;
    }
    (blake3::hash(payload).as_bytes() == hash).then_some(payload)
}

/// Writes an entry at `path` through a temporary file beside it; returns
/// the bytes written.
fn write_entry(path: &Path, payload: &[u8]) -> Result<u64> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().expect("entries live in a shard");
    fs::create_dir_all(dir).map_err(|e| io("create", dir, e))?;
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut bytes = Vec::with_capacity(MAGIC.len() + 40 + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(blake3::hash(payload).as_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(payload);
    let written = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_data()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io("write", path, e));
    }
    Ok(bytes.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(dir: &Path) -> Disk {
        Disk::open(dir, [7; 32])
    }

    fn set_used(d: &Disk, kind: Kind, input: &[u8], secs: u64) {
        let path = d.path(kind, &d.key(kind, input));
        let f = fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn entries_come_back_and_misses_are_none() {
        let dir = tempfile::tempdir().unwrap();
        let d = disk(dir.path());
        assert_eq!(d.get(Kind::Feature, b"a"), None);
        d.put(Kind::Feature, b"a", b"payload");
        assert_eq!(d.get(Kind::Feature, b"a").as_deref(), Some(&b"payload"[..]));
        assert_eq!(d.get(Kind::Tool, b"a"), None, "kinds are apart");
        let other = Disk::open(dir.path(), [8; 32]);
        assert_eq!(other.get(Kind::Feature, b"a"), None, "builds are apart");
        let again = disk(dir.path());
        assert_eq!(
            again.get(Kind::Feature, b"a").as_deref(),
            Some(&b"payload"[..]),
            "another daemon of the same build shares it"
        );
        d.put(Kind::Feature, b"empty", b"");
        assert_eq!(d.get(Kind::Feature, b"empty").as_deref(), Some(&b""[..]));
    }

    #[test]
    fn corrupt_entries_are_misses_and_go() {
        let dir = tempfile::tempdir().unwrap();
        let d = disk(dir.path());
        d.put(Kind::Mesh, b"m", b"some bytes");
        let path = d.path(Kind::Mesh, &d.key(Kind::Mesh, b"m"));
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert_eq!(d.get(Kind::Mesh, b"m"), None);
        assert!(!path.exists());
        d.put(Kind::Mesh, b"m", b"some bytes");
        fs::write(&path, b"ryucad.cache.0\n").unwrap();
        assert_eq!(d.get(Kind::Mesh, b"m"), None);
        d.put(Kind::Mesh, b"m", b"some bytes");
        let whole = fs::read(&path).unwrap();
        fs::write(&path, &whole[..whole.len() - 2]).unwrap();
        assert_eq!(d.get(Kind::Mesh, b"m"), None, "cut short");
        d.put(Kind::Mesh, b"m", b"some bytes");
        d.discard(Kind::Mesh, b"m");
        assert!(!path.exists());
    }

    #[test]
    fn stats_count_and_clear_empties() {
        let dir = tempfile::tempdir().unwrap();
        let d = disk(dir.path());
        d.put(Kind::Feature, b"a", &[1; 100]);
        d.put(Kind::Feature, b"b", &[1; 10]);
        d.put(Kind::Mesh, b"a", &[1; 1000]);
        let s = d.stats();
        assert_eq!(s.entries, 3);
        let header = (MAGIC.len() + 40) as u64;
        assert_eq!(s.bytes, 1110 + 3 * header);
        assert_eq!(s.kinds[0], (Kind::Feature, 2, 110 + 2 * header));
        assert_eq!(s.kinds[1], (Kind::Tool, 0, 0));
        assert_eq!(
            s.to_json()["kinds"]["meshes"],
            json!({"entries": 1, "bytes": 1000 + header})
        );
        assert_eq!(d.clear(), s);
        assert_eq!(d.stats().entries, 0);
        assert_eq!(d.get(Kind::Mesh, b"a"), None);
    }

    #[test]
    fn trimming_removes_the_least_recently_read_first() {
        let dir = tempfile::tempdir().unwrap();
        let d = disk(dir.path());
        let header = (MAGIC.len() + 40) as u64;
        for (i, input) in [b"a", b"b", b"c", b"d"].iter().enumerate() {
            d.put(Kind::Feature, *input, &[0; 1000]);
            set_used(&d, Kind::Feature, *input, 1000 + i as u64);
        }
        // Read `a` again: it is now the most recent.
        assert!(d.get(Kind::Feature, b"a").is_some());
        let each = 1000 + header;
        assert_eq!(d.trim(4 * each), 0, "within the bound");
        assert_eq!(d.trim(3 * each), 2, "down to nine tenths");
        assert!(d.get(Kind::Feature, b"b").is_none());
        assert!(d.get(Kind::Feature, b"c").is_none());
        assert!(d.get(Kind::Feature, b"d").is_some());
        assert!(d.get(Kind::Feature, b"a").is_some());
    }

    #[test]
    fn a_dead_writers_temporary_files_go_and_others_stay() {
        let dir = tempfile::tempdir().unwrap();
        let d = disk(dir.path());
        d.put(Kind::Tool, b"t", b"x");
        let shard = d.path(Kind::Tool, &d.key(Kind::Tool, b"t"));
        let shard = shard.parent().unwrap();
        let old = shard.join("dead.1.0.tmp");
        let new = shard.join("live.2.0.tmp");
        fs::write(&old, b"half").unwrap();
        fs::write(&new, b"half").unwrap();
        let f = fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(d.stats().entries, 1);
        assert!(!old.exists());
        assert!(new.exists());
    }

    #[test]
    fn an_unwritable_cache_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the cache's folder should be.
        fs::write(dir.path().join("v1"), b"in the way").unwrap();
        let d = disk(dir.path());
        d.put(Kind::Feature, b"a", b"x");
        assert_eq!(d.get(Kind::Feature, b"a"), None);
        assert!(d.warned.load(Ordering::Relaxed));
        assert_eq!(d.stats().entries, 0);
    }

    #[test]
    fn the_folder_follows_the_variable() {
        // Read only: the variable is set for the whole test process by
        // nothing else here.
        if std::env::var_os(CACHE_DIR_VAR).is_none() {
            let d = cache_dir().unwrap();
            assert!(d.ends_with("ryucad") || d.ends_with("cache"), "{d:?}");
        }
        assert!(build_identity().is_some());
    }
}
