//! The activity log and undo: `ryu-session`'s, over ryuplm's files.

use crate::Plm;
use crate::error::Result;
use crate::session::Session;
use ryu_session::activity::Files;
use ryu_session::live::Live;
use serde_json::{Value, json};
use std::path::Path;

/// The catalogue as undo sees it: the session over it.
struct Undone<'a>(&'a mut Session);

impl ryu_session::activity::Project for Undone<'_> {
    fn root(&self) -> &Path {
        self.0.root()
    }

    fn current(&self) -> Result<Files> {
        crate::session::current(self.0.root())
    }

    /// The manifest and the settings go back before the items they judge.
    fn is_index(&self, p: &str) -> bool {
        p == crate::catalogue::MANIFEST || p == ryupdm::PDM_CONFIG
    }

    fn remove(&self, path: &Path) -> Result<()> {
        crate::session::remove_file(self.0.root(), path)
    }

    fn reload(&mut self) -> Result<()> {
        self.0.reload()
    }
}

/// Undoes (or with `redo`, redoes) an entry: `entry`, or the caller's
/// latest that can be. Records the result as a new entry, and answers it
/// with the catalogue's revision now.
pub fn undo(
    live: &mut Live<Plm>,
    session: &mut Session,
    who: &str,
    entry: Option<u64>,
    redo: bool,
) -> Result<Value> {
    let seq = ryu_session::activity::undo(live, &mut Undone(session), who, entry, redo)?;
    Ok(json!({
        "entry": live.log.get(seq).expect("just made").to_json(),
        "revision": session.revision(),
    }))
}
