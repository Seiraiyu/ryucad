//! The activity log and undo: `ryu-session`'s, over ryueda's files.

use crate::error::Result;
use crate::session::Session;
use crate::{Eda, project};
use ryu_session::activity::Files;
use ryu_session::live::Live;
use serde_json::{Value, json};
use std::path::Path;

/// The project as undo sees it: the session over it.
struct Undone<'a>(&'a mut Session);

impl ryu_session::activity::Project for Undone<'_> {
    fn root(&self) -> &Path {
        self.0.root()
    }

    fn current(&self) -> Result<Files> {
        crate::session::current(self.0.root())
    }

    /// Library files go back before the manifest that names their
    /// libraries.
    fn is_index(&self, p: &str) -> bool {
        p == project::MANIFEST || p.ends_with(&format!("/{}", project::MANIFEST))
    }

    fn remove(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path).map_err(|e| crate::error::Error::io(path, &e))
    }

    fn reload(&mut self) -> Result<()> {
        self.0.reload()
    }
}

/// Undoes (or with `redo`, redoes) an entry: `entry`, or the caller's
/// latest that can be. Records the result as a new entry, and answers it
/// with the revisions now.
pub fn undo(
    live: &mut Live<Eda>,
    session: &mut Session,
    who: &str,
    entry: Option<u64>,
    redo: bool,
) -> Result<Value> {
    let seq = ryu_session::activity::undo(live, &mut Undone(session), who, entry, redo)?;
    Ok(json!({
        "entry": live.log.get(seq).expect("just made").to_json(),
        "revisions": crate::server::revisions(session.project()),
    }))
}
