//! Keeping a session fresh against the files. The files are the truth:
//! before every request a session checks each file it depends on. An
//! unchanged size and modification time is trusted unless the time is too
//! recent to be reliable; otherwise the content is read again. The stamps
//! keep every file's bytes, so a snapshot of the project costs no reading.
//! Which files a session depends on is the product's to say.
//!
//! A daemon narrows the check with file events ([`Watch`]): the paths
//! events named since the last request are checked, and the rest are
//! trusted. Events only name paths; what changed is still read from the
//! files.

use ryu_doc::error::{Error, Result};
use ryu_doc::source::relative;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
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
        self.scan_under(root, files, &[root.to_path_buf()])
    }

    /// [`Stamps::scan`] of the files at or under the paths in `under`
    /// (those `files` lists, from the folders and files events named):
    /// a stamp under one of them that `files` doesn't list is deleted,
    /// and stamps elsewhere are left as they are.
    pub fn scan_under(
        &mut self,
        root: &Path,
        files: Vec<(PathBuf, fs::Metadata)>,
        under: &[PathBuf],
    ) -> Result<Vec<(String, &'static str)>> {
        let trusted_before = self.stamped_at.checked_sub(RACY);
        let mut changes = Vec::new();
        let mut listed = BTreeSet::new();
        for (path, meta) in files {
            if !listed.insert(path.clone()) {
                continue;
            }
            let (modified, len) = (meta.modified().ok(), meta.len());
            let old = self.stamps.get(&path);
            let trusted = match (old, modified, trusted_before) {
                (Some(o), Some(m), Some(t)) => o.modified == Some(m) && o.len == len && m < t,
                _ => false,
            };
            if trusted {
                continue;
            }
            let bytes = fs::read(&path).map_err(|e| Error::io(&path, &e))?;
            let stamp = Stamp {
                modified,
                len,
                bytes: bytes.into(),
            };
            match old {
                None => changes.push((path.clone(), "created")),
                Some(o) if o.bytes != stamp.bytes => changes.push((path.clone(), "modified")),
                Some(_) => {}
            }
            self.stamps.insert(path, stamp);
        }
        let mut gone = Vec::new();
        for u in under {
            gone.extend(
                self.stamps
                    .range(u.clone()..)
                    .map(|(p, _)| p)
                    .take_while(|p| p.starts_with(u))
                    .filter(|p| !listed.contains(*p))
                    .cloned(),
            );
        }
        for path in gone {
            if self.stamps.remove(&path).is_some() {
                changes.push((path, "deleted"));
            }
        }
        self.stamped_at = SystemTime::now();
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

/// What file events say changed since the last [`Watch::changes`].
#[derive(Clone, Debug, PartialEq)]
pub enum Changes {
    /// These paths (relative to the project, with `/`, files or folders)
    /// and nothing else.
    Paths(BTreeSet<String>),
    /// Anything may have: check every file. Why: the watch just started,
    /// events overflowed or failed, the project folder itself moved, or
    /// events came too late.
    All(&'static str),
}

/// How long [`Watch::changes`] waits for its own marker's event before it
/// gives up and asks for a full check.
const MARKER_WAIT: Duration = Duration::from_secs(2);

/// Each file a [`Watch`] makes to mark a moment: one round for an event
/// queue every path shares (inotify, FSEvents); on Windows each folder
/// has its own, so a second round catches what the first released.
const ROUNDS: u64 = if cfg!(windows) { 2 } else { 1 };

/// File events for a project folder (inotify, FSEvents,
/// ReadDirectoryChangesW, through the `notify` crate), so a request checks
/// only the paths they name. Events come on a thread of their own and may
/// lag the writes they report, so before a request takes them the watch
/// marks the moment it started: it makes a file in a folder of its own,
/// watched by the same watcher, and waits for that file's event. Events
/// arrive in order, so by then every change made before the request has
/// been reported.
pub struct Watch {
    markers: PathBuf,
    seen: Arc<(Mutex<Seen>, Condvar)>,
    next: u64,
    // Dropped last: it stops the event thread.
    _watcher: notify::RecommendedWatcher,
}

#[derive(Debug)]
struct Seen {
    paths: BTreeSet<String>,
    all: Option<&'static str>,
    marker: u64,
}

impl Watch {
    /// Watches `root` and everything under it, keeping the events whose
    /// paths (relative, with `/`) `keep` accepts, with its markers in
    /// `markers` (a private folder outside the project, made if needed
    /// and emptied). An error says why the folder can't be watched (a
    /// filesystem without events, a watch limit reached).
    pub fn start(
        root: &Path,
        markers: &Path,
        keep: fn(&str) -> bool,
    ) -> std::result::Result<Watch, String> {
        let _ = fs::remove_dir_all(markers);
        fs::create_dir_all(markers).map_err(|e| format!("{}: {e}", markers.display()))?;
        let roots = [
            root.to_path_buf(),
            root.canonicalize().map_err(|e| e.to_string())?,
        ];
        let marks = [
            markers.to_path_buf(),
            markers.canonicalize().map_err(|e| e.to_string())?,
        ];
        let seen = Arc::new((
            Mutex::new(Seen {
                paths: BTreeSet::new(),
                all: Some("the watch started"),
                marker: 0,
            }),
            Condvar::new(),
        ));
        let into = Arc::clone(&seen);
        let (watched, marked) = (roots[1].clone(), marks[1].clone());
        let handler = move |res: notify::Result<notify::Event>| {
            let (lock, ready) = &*into;
            let mut s = lock.lock().unwrap_or_else(|p| p.into_inner());
            match res {
                Err(_) => s.all = Some("file events failed"),
                Ok(ev) => {
                    if ev.need_rescan() {
                        s.all = Some("file events overflowed");
                    }
                    // Opening and reading change nothing, and the
                    // session's own reads would come back as events.
                    if !matches!(ev.kind, notify::EventKind::Access(_)) {
                        for p in &ev.paths {
                            note(&mut s, p, &roots, &marks, keep);
                        }
                    }
                }
            }
            ready.notify_all();
        };
        let mut watcher = notify::recommended_watcher(handler).map_err(|e| e.to_string())?;
        use notify::Watcher;
        watcher
            .watch(&watched, notify::RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;
        watcher
            .watch(&marked, notify::RecursiveMode::NonRecursive)
            .map_err(|e| e.to_string())?;
        Ok(Watch {
            markers: marked,
            seen,
            next: 0,
            _watcher: watcher,
        })
    }

    /// What changed since the last call: once every event of a change
    /// made before this call has come in.
    pub fn changes(&mut self) -> Changes {
        for _ in 0..ROUNDS {
            self.next += 1;
            let n = self.next;
            if fs::write(self.markers.join(n.to_string()), b"").is_err() {
                return self.take(Some("its marker couldn't be written"));
            }
            let _ = fs::remove_file(self.markers.join((n - 1).to_string()));
            let (lock, ready) = &*self.seen;
            let s = lock.lock().unwrap_or_else(|p| p.into_inner());
            let (s, wait) = ready
                .wait_timeout_while(s, MARKER_WAIT, |s| s.marker < n)
                .unwrap_or_else(|p| p.into_inner());
            if wait.timed_out() {
                drop(s);
                return self.take(Some("file events came too late"));
            }
        }
        self.take(None)
    }

    fn take(&mut self, all: Option<&'static str>) -> Changes {
        let mut s = self.seen.0.lock().unwrap_or_else(|p| p.into_inner());
        let paths = std::mem::take(&mut s.paths);
        match all.or(s.all.take()) {
            Some(why) => Changes::All(why),
            None => Changes::Paths(paths),
        }
    }
}

/// Notes what an event says of `path`: a marker's number, a path in the
/// project `keep` accepts, or the project folder itself.
fn note(s: &mut Seen, path: &Path, roots: &[PathBuf], marks: &[PathBuf], keep: fn(&str) -> bool) {
    if let Some(name) = marks.iter().find_map(|m| path.strip_prefix(m).ok()) {
        if let Some(n) = name.to_str().and_then(|n| n.parse::<u64>().ok()) {
            s.marker = s.marker.max(n);
        }
        return;
    }
    let Some(rel) = roots.iter().find_map(|r| path.strip_prefix(r).ok()) else {
        return;
    };
    let rel: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if rel.is_empty() {
        s.all = Some("the project folder itself changed");
        return;
    }
    let rel = rel.join("/");
    if keep(&rel) {
        s.paths.insert(rel);
    }
}

/// A watch's counts for `daemon status`: full checks and why the last
/// was, files that events missed and a full check found, and whether the
/// folder is watched at all.
#[derive(Clone, Debug, Default)]
pub struct WatchStats {
    pub full: u64,
    pub narrowed: u64,
    pub missed: u64,
    pub last_full: Option<&'static str>,
}

impl WatchStats {
    pub fn to_json(&self, unavailable: Option<&str>) -> serde_json::Value {
        let mut v = serde_json::json!({
            "watching": unavailable.is_none(),
            "full_checks": self.full,
            "narrowed_checks": self.narrowed,
            "missed": self.missed,
        });
        if let Some(why) = self.last_full {
            v["last_full"] = serde_json::json!(why);
        }
        if let Some(why) = unavailable {
            v["unavailable"] = serde_json::json!(why);
            v["code"] = serde_json::json!(ryu_doc::error::Code::WatchUnavailable);
        }
        v
    }
}

/// When a watched session checks every file anyway: at most this long
/// after its last full check, as a safety net.
pub const RESCAN: Duration = Duration::from_secs(60);

/// Each entry in `dir` with its metadata, following symlinks, in name
/// order; empty when `dir` doesn't exist. An entry gone by the time it is
/// looked at is left out. The file system's own order isn't stable: a file
/// written again (renamed over) can move in it. A directory listing says what each entry is (and on Windows gives
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
    out.sort_by(|a, b| a.0.cmp(&b.0));
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
    fn listings_come_in_name_order() {
        // Directory order depends on the file system, and a file written
        // again by renaming over it can move: callers compare listings.
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..64).rev().map(|i| format!("f{i:02}")).collect();
        for n in &names {
            fs::write(dir.path().join(n), n).unwrap();
        }
        let tmp = dir.path().join("tmp");
        fs::write(&tmp, "again").unwrap();
        fs::rename(&tmp, dir.path().join("f00")).unwrap();
        let got: Vec<PathBuf> = read_dir(dir.path())
            .unwrap()
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        let mut sorted = got.clone();
        sorted.sort();
        assert_eq!(got.len(), 64);
        assert_eq!(got, sorted);
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

    #[test]
    fn a_narrowed_scan_checks_only_what_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("x")).unwrap();
        for f in ["a", "x/b", "x/c"] {
            fs::write(root.join(f), "1").unwrap();
        }
        let mut s = Stamps::default();
        let all = |root: &Path| {
            let mut out = listed(root);
            out.extend(listed(&root.join("x")));
            out
        };
        assert_eq!(s.scan(root, all(root)).unwrap().len(), 3);
        // Under x: b deleted, c changed. a changed too, but nothing named
        // it, so it isn't looked at.
        fs::remove_file(root.join("x/b")).unwrap();
        fs::write(root.join("x/c"), "22").unwrap();
        fs::write(root.join("a"), "22").unwrap();
        let changes = s
            .scan_under(root, listed(&root.join("x")), &[root.join("x")])
            .unwrap();
        assert_eq!(
            changes,
            [
                ("x/b".to_owned(), "deleted"),
                ("x/c".to_owned(), "modified")
            ]
        );
        assert_eq!(s.bytes(&root.join("a")), Some(&b"1"[..]));
        // A file named twice is checked once.
        let twice = [listed(root), listed(root)].concat();
        assert_eq!(
            s.scan_under(root, twice, &[root.join("a")]).unwrap(),
            [("a".to_owned(), "modified")]
        );
    }

    fn keep_all(_: &str) -> bool {
        true
    }

    fn paths(c: Changes) -> BTreeSet<String> {
        match c {
            Changes::Paths(p) => p,
            Changes::All(why) => panic!("everything: {why}"),
        }
    }

    #[test]
    fn a_watch_names_a_write_made_just_before_it_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        fs::create_dir_all(root.join("parts/a")).unwrap();
        let mut w = Watch::start(&root, &dir.path().join("marks"), keep_all).unwrap();
        assert_eq!(w.changes(), Changes::All("the watch started"));
        assert!(paths(w.changes()).is_empty());
        // A write, then at once the question: every time.
        for k in 0..200 {
            let f = format!("parts/a/f{}.json", k % 7);
            fs::write(root.join(&f), k.to_string()).unwrap();
            let got = paths(w.changes());
            assert!(got.contains(&f), "{k}: {got:?}");
        }
    }

    #[test]
    fn a_watch_names_renames_and_folders_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        fs::create_dir_all(root.join("parts/a")).unwrap();
        fs::write(root.join("parts/a/x.json"), "1").unwrap();
        let mut w =
            Watch::start(&root, &dir.path().join("marks"), |p| p.starts_with("parts")).unwrap();
        w.changes();
        fs::rename(root.join("parts/a/x.json"), root.join("parts/a/y.json")).unwrap();
        let got = paths(w.changes());
        assert!(
            got.contains("parts/a/x.json") && got.contains("parts/a/y.json"),
            "{got:?}"
        );
        // What a checkout does to a folder: gone, and back with other files.
        fs::remove_dir_all(root.join("parts")).unwrap();
        fs::create_dir_all(root.join("parts/b")).unwrap();
        fs::write(root.join("parts/b/z.json"), "2").unwrap();
        let got = paths(w.changes());
        assert!(got.contains("parts"), "{got:?}");
        // Outside what it keeps: nothing.
        fs::write(root.join("notes.txt"), "3").unwrap();
        assert!(paths(w.changes()).is_empty());
    }

    #[test]
    fn a_folder_that_isnt_there_cant_be_watched() {
        let dir = tempfile::tempdir().unwrap();
        let why = Watch::start(&dir.path().join("gone"), &dir.path().join("m"), keep_all)
            .err()
            .unwrap();
        assert!(!why.is_empty());
    }
}
