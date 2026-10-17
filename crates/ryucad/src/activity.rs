//! The activity log: every change to the project the daemon made or saw,
//! newest last, with who made it. For its own changes it keeps each
//! touched file's content before and after, which is what undo restores.
//! It lives in the daemon's memory only; Git is the durable history.

use crate::doc::project::write_atomic;
use crate::error::{Code, Error, Result};
use crate::live::Live;
use crate::notify::Topic;
use crate::session::Session;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The most entries kept.
pub const MAX_ENTRIES: usize = 1000;
/// The most file content kept, before and after together.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

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
}

/// A file's content before or after a change; `None` when it didn't
/// exist.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    /// Relative to the project root, with `/`.
    pub path: String,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

impl FileChange {
    fn change(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, _) => "created",
            (_, None) => "deleted",
            _ => "modified",
        }
    }

    fn size(&self) -> usize {
        self.before.as_ref().map_or(0, Vec::len) + self.after.as_ref().map_or(0, Vec::len)
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

    fn size(&self) -> usize {
        self.files.iter().map(FileChange::size).sum()
    }
}

/// The project's files and their contents, by relative path.
pub type Files = BTreeMap<String, Vec<u8>>;

/// Reads every file the project's state depends on.
pub fn snapshot(root: &Path) -> Result<Files> {
    crate::session::files(root)?
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).map_err(|e| crate::error::Error::io(&path, &e))?;
            Ok((crate::doc::project::relative(root, &path), bytes))
        })
        .collect()
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

#[derive(Debug, Default)]
pub struct Log {
    entries: VecDeque<Entry>,
    next: u64,
    bytes: usize,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Log {
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
        self.bytes += e.size();
        self.entries.push_back(e);
        while self.entries.len() > MAX_ENTRIES || (self.bytes > MAX_BYTES && self.entries.len() > 1)
        {
            let old = self.entries.pop_front().expect("not empty");
            self.bytes -= old.size();
        }
        self.entries.back().expect("just pushed")
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
pub fn record(
    live: &mut Live,
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
    live.hub.send(Topic::Activity, "activity.appended", &v);
    seq
}

/// Records files that changed behind the daemon's back.
pub fn record_external(live: &mut Live, seen: Vec<(String, &'static str)>) {
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
    live.hub.send(Topic::Activity, "activity.appended", &v);
}

/// Puts each file back the way it was before `files` changed it, but only
/// if every one is still as `files` left it. Returns the changes made.
fn revert(root: &Path, files: &[FileChange]) -> Result<Vec<FileChange>> {
    let current = snapshot(root)?;
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
    // Feature files, then lists and the manifest, then deletions: a crash
    // part way leaves at worst an unlisted feature file, as any write does.
    let index = |p: &str| p.ends_with("/part.json") || !p.contains('/');
    let mut order: Vec<&FileChange> = files.iter().collect();
    order.sort_by_key(|f| (f.before.is_none(), index(&f.path)));
    for f in order {
        let path = root.join(&f.path);
        match &f.before {
            Some(bytes) => write_atomic(&path, bytes)?,
            None => {
                std::fs::remove_file(&path).map_err(|e| Error::io(&path, &e))?;
                remove_empty_dirs(root, &path);
            }
        }
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

/// Removes the folders above `path` that are now empty, up to `parts/`.
fn remove_empty_dirs(root: &Path, path: &Path) {
    let stop = root.join("parts");
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == stop || !d.starts_with(&stop) || std::fs::remove_dir(d).is_err() {
            return;
        }
        dir = d.parent();
    }
}

/// Undoes (or with `redo`, redoes) an entry: `entry`, or the caller's
/// latest that can be. Records the result as a new entry.
pub fn undo(
    live: &mut Live,
    session: &mut Session,
    who: &str,
    entry: Option<u64>,
    redo: bool,
) -> Result<Value> {
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
    let changes = revert(session.root(), &files)?;
    session.reload()?;
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
    let revisions: BTreeMap<String, String> = session
        .project()
        .parts
        .values()
        .map(|p| (p.id.to_string(), p.revision.clone()))
        .collect();
    Ok(json!({
        "entry": live.log.get(seq).expect("just made").to_json(),
        "revisions": revisions,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> Files {
        pairs
            .iter()
            .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
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
            after: Some(vec![0; MAX_BYTES + 1]),
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
}
