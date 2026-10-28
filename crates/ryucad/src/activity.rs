//! The activity log: every change to the project the daemon made or saw,
//! newest last, with who made it: `ryu-session`'s, over ryucad's project
//! files. For its own changes it keeps each touched file's content before
//! and after, which is what undo restores. It lives in the daemon's memory
//! only; Git is the durable history.

use crate::error::Result;
use crate::live::Live;
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub use ryu_session::activity::{
    Bytes, Entry, EntryKind, FileChange, Files, Log, MAX_BULKY_BYTES, MAX_BYTES, MAX_ENTRIES, diff,
    record, record_external,
};

/// Reads every file the project's state depends on.
pub fn snapshot(root: &Path) -> Result<Files> {
    crate::session::files(root)?
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).map_err(|e| crate::error::Error::io(&path, &e))?;
            Ok((crate::doc::project::relative(root, &path), bytes.into()))
        })
        .collect()
}

/// The project as undo sees it: the session over it.
struct Undone<'a>(&'a mut Session);

impl ryu_session::activity::Project for Undone<'_> {
    fn root(&self) -> &Path {
        self.0.root()
    }

    fn current(&self) -> Result<Files> {
        snapshot(self.0.root())
    }

    /// Feature files go back before lists and the manifest.
    fn is_index(&self, p: &str) -> bool {
        p.ends_with("/part.json") || !p.contains('/')
    }

    fn remove(&self, path: &Path) -> Result<()> {
        crate::doc::project::remove_file(self.0.root(), path)
    }

    fn reload(&mut self) -> Result<()> {
        self.0.reload()
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
    let seq = ryu_session::activity::undo(live, &mut Undone(session), who, entry, redo)?;
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
