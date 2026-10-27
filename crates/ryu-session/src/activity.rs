//! The activity log: every change to the project the daemon made or saw,
//! newest last, with who made it. For its own changes it keeps each
//! touched file's content before and after, which is what undo restores.
//! It lives in the daemon's memory only; Git is the durable history.
//!
//! Contents are shared, never copied: a snapshot holds the session's own
//! bytes, and an undo's entry the entry it undoes. Each distinct content
//! counts once against its budget: [`MAX_BYTES`] for the project's text
//! files, [`MAX_BULKY_BYTES`] for those under the product's bulky folders
//! ([`crate::Product::BULKY`]: ryucad's imported STEP files), so changing
//! one large file never pushes every other change out.

use crate::Product;
use crate::live::Live;
use crate::notify::Topics;
use ryu_doc::error::{Code, Error, Result};
use ryu_doc::source::write_atomic;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::fresh::{Bytes, Files};

/// The most entries kept.
pub const MAX_ENTRIES: usize = 1000;
/// The most file content kept, before and after together, outside the
/// product's bulky folders.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// The most content kept of files in the product's bulky folders.
pub const MAX_BULKY_BYTES: usize = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A request that changed files.
    Change,
    /// An undo of `target`.
    Undo,
    /// A redo: the undo of the undo `target`.
    Redo,
    /// Files changed behind the daemon's back.
    External,
    /// Git refs a request wrote (`release.sync`'s tags): logged, but not
    /// project files, so never undone.
    Refs,
}

/// A file's content before or after a change; `None` when it didn't
/// exist.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    /// Relative to the project root, with `/`.
    pub path: String,
    pub before: Option<Bytes>,
    pub after: Option<Bytes>,
}

impl FileChange {
    fn change(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, _) => "created",
            (_, None) => "deleted",
            _ => "modified",
        }
    }

    /// Its contents, and whether they are in one of `bulky`'s folders.
    fn contents<'a>(
        &'a self,
        bulky: &'a [&'a str],
    ) -> impl Iterator<Item = (&'a Bytes, bool)> + 'a {
        let big = bulky.iter().any(|b| self.path.starts_with(b));
        [&self.before, &self.after]
            .into_iter()
            .flatten()
            .map(move |b| (b, big))
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub seq: u64,
    pub time_ms: u64,
    pub kind: EntryKind,
    /// Who: `{id, kind, name}`, or null for external changes.
    pub participant: Value,
    pub method: String,
    pub params: Value,
    pub target: Option<u64>,
    /// The entry that undid this one, if any.
    pub undone_by: Option<u64>,
    /// Contents before and after (empty for external entries).
    pub files: Vec<FileChange>,
    /// For external entries: `(path, created | modified | deleted)`.
    pub seen: Vec<(String, &'static str)>,
}

impl Entry {
    pub fn to_json(&self) -> Value {
        let files: Vec<Value> = if self.kind == EntryKind::External {
            self.seen
                .iter()
                .map(|(p, c)| json!({"path": p, "change": c}))
                .collect()
        } else {
            self.files
                .iter()
                .map(|f| json!({"path": f.path, "change": f.change()}))
                .collect()
        };
        json!({
            "seq": self.seq,
            "time_ms": self.time_ms,
            "kind": self.kind,
            "participant": self.participant,
            "method": self.method,
            "params": self.params,
            "target": self.target,
            "undone_by": self.undone_by,
            "files": files,
        })
    }
}

/// The files that differ between two snapshots.
pub fn diff(before: &Files, after: &Files) -> Vec<FileChange> {
    let paths: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    paths
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .map(|p| FileChange {
            path: p.clone(),
            before: before.get(p).cloned(),
            after: after.get(p).cloned(),
        })
        .collect()
}

#[derive(Debug)]
pub struct Log {
    entries: VecDeque<Entry>,
    next: u64,
    /// The budgets: [`MAX_BYTES`] and [`MAX_BULKY_BYTES`] but in tests.
    budgets: (usize, usize),
    /// The folders whose files count as bulky (`imports/`).
    bulky: &'static [&'static str],
    /// How many times the entries hold each distinct content, by its
    /// address (held, it can't move or be freed).
    held: BTreeMap<usize, usize>,
    /// What the distinct contents come to: the project's, the bulky.
    bytes: usize,
    bulky_bytes: usize,
}

impl Default for Log {
    fn default() -> Log {
        Log::with_budgets(MAX_BYTES, MAX_BULKY_BYTES, &[])
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Log {
    /// A log keeping at most `bytes` of the project's files' contents and
    /// `bulky_bytes` of those in `bulky`'s folders (`imports/`).
    pub fn with_budgets(bytes: usize, bulky_bytes: usize, bulky: &'static [&'static str]) -> Log {
        Log {
            entries: VecDeque::new(),
            next: 0,
            budgets: (bytes, bulky_bytes),
            bulky,
            held: BTreeMap::new(),
            bytes: 0,
            bulky_bytes: 0,
        }
    }

    /// Appends an entry and returns its sequence number. The oldest
    /// entries go when the log is over its limits.
    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &mut self,
        kind: EntryKind,
        participant: Value,
        method: &str,
        params: Value,
        target: Option<u64>,
        files: Vec<FileChange>,
        seen: Vec<(String, &'static str)>,
    ) -> &Entry {
        self.next += 1;
        let e = Entry {
            seq: self.next,
            time_ms: now_ms(),
            kind,
            participant,
            method: method.to_owned(),
            params,
            target,
            undone_by: None,
            files,
            seen,
        };
        self.hold(&e, true);
        self.entries.push_back(e);
        while self.entries.len() > MAX_ENTRIES
            || ((self.bytes > self.budgets.0 || self.bulky_bytes > self.budgets.1)
                && self.entries.len() > 1)
        {
            let old = self.entries.pop_front().expect("not empty");
            self.hold(&old, false);
        }
        self.entries.back().expect("just pushed")
    }

    /// Counts `e`'s contents in (`add`) or out: each distinct content
    /// once, however many entries hold it.
    fn hold(&mut self, e: &Entry, add: bool) {
        let bulky = self.bulky;
        for (b, big) in e.files.iter().flat_map(|f| f.contents(bulky)) {
            let key = b.as_ptr() as usize;
            let change = if add {
                let n = self.held.entry(key).or_insert(0);
                *n += 1;
                (*n == 1).then_some(b.len() as isize)
            } else {
                let Some(n) = self.held.get_mut(&key) else {
                    continue;
                };
                *n -= 1;
                let last = *n == 0;
                if last {
                    self.held.remove(&key);
                }
                last.then_some(-(b.len() as isize))
            };
            if let Some(d) = change {
                let total = if big {
                    &mut self.bulky_bytes
                } else {
                    &mut self.bytes
                };
                *total = total.wrapping_add_signed(d);
            }
        }
    }

    /// What the entries' distinct contents come to: the project's files'
    /// and the bulky files'.
    pub fn held_bytes(&self) -> (usize, usize) {
        (self.bytes, self.bulky_bytes)
    }

    pub fn get(&self, seq: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.seq == seq)
    }

    pub fn get_mut(&mut self, seq: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.seq == seq)
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter()
    }

    /// Up to `limit` entries older than `before`, oldest first.
    pub fn page(&self, limit: usize, before: Option<u64>) -> Vec<Value> {
        let mut page: Vec<Value> = self
            .entries
            .iter()
            .rev()
            .filter(|e| before.is_none_or(|b| e.seq < b))
            .take(limit)
            .map(Entry::to_json)
            .collect();
        page.reverse();
        page
    }
}

/// Records a change the daemon made and tells subscribers.
pub fn record<P: Product>(
    live: &mut Live<P>,
    kind: EntryKind,
    who: &str,
    method: &str,
    params: Value,
    target: Option<u64>,
    files: Vec<FileChange>,
) -> u64 {
    let participant = live
        .get(who)
        .map(|p| json!({"id": p.id, "kind": p.kind, "name": p.name}))
        .unwrap_or(Value::Null);
    let e = live
        .log
        .append(kind, participant, method, params, target, files, Vec::new());
    let (seq, v) = (e.seq, e.to_json());
    live.hub.send(P::Topic::ACTIVITY, "activity.appended", &v);
    seq
}

/// Records files that changed behind the daemon's back.
pub fn record_external<P: Product>(live: &mut Live<P>, seen: Vec<(String, &'static str)>) {
    let e = live.log.append(
        EntryKind::External,
        Value::Null,
        "external",
        Value::Null,
        None,
        Vec::new(),
        seen,
    );
    let v = e.to_json();
    live.hub.send(P::Topic::ACTIVITY, "activity.appended", &v);
}

/// How undo reaches a product's project: its files as they are on disk,
/// which of them list others (written back after them), how a file is
/// deleted, and reloading after a revert.
pub trait Project {
    fn root(&self) -> &Path;
    /// Every file the project's state depends on, as it is on disk now.
    fn current(&self) -> Result<Files>;
    /// Whether the file at `path` (relative, with `/`) lists others, so is
    /// written back after them.
    fn is_index(&self, path: &str) -> bool;
    /// Deletes the project file at `path`, and what that leaves empty.
    fn remove(&self, path: &Path) -> Result<()>;
    /// Takes in the files undo just wrote.
    fn reload(&mut self) -> Result<()>;
}

/// Puts each file back the way it was before `files` changed it, but only
/// if every one is still as `files` left it. Returns the changes made.
fn revert(project: &dyn Project, files: &[FileChange]) -> Result<Vec<FileChange>> {
    let current = project.current()?;
    let moved: Vec<&str> = files
        .iter()
        .filter(|f| current.get(&f.path) != f.after.as_ref())
        .map(|f| f.path.as_str())
        .collect();
    if !moved.is_empty() {
        return Err(Error::new(
            Code::UndoConflict,
            format!("{} file(s) changed since; nothing was undone", moved.len()),
        )
        .with("files", moved));
    }
    // Content files, then the files that list them, then deletions: a
    // crash part way leaves at worst an unlisted file, as any write does.
    // The files of each step are written side by side: each waits on its
    // disk, and undoing an import writes thousands.
    let root = project.root();
    let index: Vec<bool> = files.iter().map(|f| project.is_index(&f.path)).collect();
    for lists in [false, true] {
        use rayon::prelude::*;
        let written: Vec<Result<()>> = files
            .par_iter()
            .zip(&index)
            .filter(|(_, i)| **i == lists)
            .filter_map(|(f, _)| Some(write_atomic(&root.join(&f.path), f.before.as_ref()?)))
            .collect();
        written.into_iter().collect::<Result<()>>()?;
    }
    let mut gone: Vec<(&FileChange, bool)> = files
        .iter()
        .zip(index)
        .filter(|(f, _)| f.before.is_none())
        .collect();
    gone.sort_by_key(|(_, i)| *i);
    for (f, _) in gone {
        project.remove(&root.join(&f.path))?;
    }
    Ok(files
        .iter()
        .map(|f| FileChange {
            path: f.path.clone(),
            before: f.after.clone(),
            after: f.before.clone(),
        })
        .collect())
}

/// Undoes (or with `redo`, redoes) an entry: `entry`, or the caller's
/// latest that can be. Records the result as a new entry and returns its
/// sequence number.
pub fn undo<P: Product>(
    live: &mut Live<P>,
    project: &mut dyn Project,
    who: &str,
    entry: Option<u64>,
    redo: bool,
) -> Result<u64> {
    let what = if redo { "redo" } else { "undo" };
    let undoable = |e: &Entry| {
        e.undone_by.is_none()
            && if redo {
                e.kind == EntryKind::Undo
            } else {
                matches!(e.kind, EntryKind::Change | EntryKind::Redo)
            }
    };
    let target = match entry {
        Some(seq) => {
            let e = live.log.get(seq).ok_or_else(|| {
                Error::new(Code::NotFound, format!("no activity entry {seq}")).with("entry", seq)
            })?;
            if !undoable(e) {
                return Err(Error::new(
                    Code::NothingToUndo,
                    format!("entry {seq} can't be {what}ne"),
                )
                .with("entry", seq));
            }
            seq
        }
        None => {
            let mine = |e: &&Entry| e.participant["id"] == who;
            let latest = live.log.entries().rev().filter(mine).find(|e| {
                // A new change of one's own ends what can be redone.
                undoable(e) || (redo && e.kind == EntryKind::Change)
            });
            match latest {
                Some(e) if undoable(e) => e.seq,
                _ => {
                    return Err(Error::new(
                        Code::NothingToUndo,
                        format!("nothing of yours to {what}"),
                    ));
                }
            }
        }
    };
    let files = live.log.get(target).expect("found above").files.clone();
    let changes = revert(project, &files)?;
    project.reload()?;
    let kind = if redo {
        EntryKind::Redo
    } else {
        EntryKind::Undo
    };
    let seq = record(
        live,
        kind,
        who,
        what,
        json!({"entry": target}),
        Some(target),
        changes,
    );
    live.log.get_mut(target).expect("still there").undone_by = Some(seq);
    Ok(seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn files(pairs: &[(&str, &str)]) -> Files {
        pairs
            .iter()
            .map(|(p, c)| (p.to_string(), c.as_bytes().into()))
            .collect()
    }

    #[test]
    fn diffs_name_created_modified_and_deleted_files() {
        let a = files(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let b = files(&[("a", "1"), ("b", "two"), ("d", "4")]);
        let d = diff(&a, &b);
        let changes: Vec<(&str, &str)> = d.iter().map(|f| (f.path.as_str(), f.change())).collect();
        assert_eq!(
            changes,
            [("b", "modified"), ("c", "deleted"), ("d", "created")]
        );
        assert_eq!(d[0].before.as_deref(), Some(&b"2"[..]));
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn the_log_keeps_its_limits() {
        let mut log = Log::default();
        for i in 0..MAX_ENTRIES + 5 {
            log.append(
                EntryKind::Change,
                Value::Null,
                "feature.edit",
                json!({"i": i}),
                None,
                Vec::new(),
                Vec::new(),
            );
        }
        assert_eq!(log.entries().count(), MAX_ENTRIES);
        assert_eq!(log.entries().next().unwrap().seq, 6);
        let page = log.page(3, Some(10));
        let seqs: Vec<u64> = page.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, [7, 8, 9]);
        // One huge entry pushes out everything older, but stays itself.
        let big = FileChange {
            path: "x".into(),
            before: None,
            after: Some(vec![0; MAX_BYTES + 1].into()),
        };
        log.append(
            EntryKind::Change,
            Value::Null,
            "m",
            Value::Null,
            None,
            vec![big],
            Vec::new(),
        );
        assert_eq!(log.entries().count(), 1);
    }

    fn change(log: &mut Log, kind: EntryKind, files: Vec<FileChange>) -> u64 {
        log.append(kind, Value::Null, "m", Value::Null, None, files, Vec::new())
            .seq
    }

    #[test]
    fn bulky_files_have_a_budget_of_their_own_and_count_once() {
        let mut log = Log::with_budgets(100, 1000, &["imports/"]);
        let small = FileChange {
            path: "parts/p/features/f.json".into(),
            before: None,
            after: Some(b"{}".as_slice().into()),
        };
        let first = change(&mut log, EntryKind::Change, vec![small]);
        // A re-import of a 400-byte file holds 800 bytes: past the
        // project's budget, not the bulky files'.
        let (old, new): (Bytes, Bytes) = (vec![1; 400].into(), vec![2; 400].into());
        let update = FileChange {
            path: "imports/big.step".into(),
            before: Some(Arc::clone(&old)),
            after: Some(Arc::clone(&new)),
        };
        change(&mut log, EntryKind::Change, vec![update.clone()]);
        assert_eq!(log.entries().next().unwrap().seq, first, "kept");
        assert_eq!(log.held_bytes(), (2, 800));
        // Its undo holds the same contents the other way round: nothing
        // more.
        let undone = FileChange {
            path: update.path.clone(),
            before: update.after.clone(),
            after: update.before.clone(),
        };
        change(&mut log, EntryKind::Undo, vec![undone]);
        assert_eq!(log.held_bytes(), (2, 800));
        // Past the bulky files' budget the oldest entries go: the change
        // and its undo share their contents, so they go together.
        let more = FileChange {
            path: "imports/other.step".into(),
            before: None,
            after: Some(vec![3; 300].into()),
        };
        let last = change(&mut log, EntryKind::Change, vec![more]);
        let left: Vec<u64> = log.entries().map(|e| e.seq).collect();
        assert_eq!(left, [last]);
        assert_eq!(log.held_bytes(), (0, 300));
    }
}
