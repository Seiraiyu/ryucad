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
    /// libraries, and a block's files before its `block.json`.
    fn is_index(&self, p: &str) -> bool {
        p == project::MANIFEST
            || p.ends_with(&format!("/{}", project::MANIFEST))
            || (p.starts_with("blocks/") && p.ends_with(&format!("/{}", crate::conn::BLOCK_FILE)))
    }

    fn remove(&self, path: &Path) -> Result<()> {
        remove_file(self.0.root(), path)
    }

    fn reload(&mut self) -> Result<()> {
        self.0.reload()
    }
}

/// Deletes the file at `path`, and the folders under `root` that leaves
/// empty: undoing a block's creation takes its folder too.
pub fn remove_file(root: &Path, path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(|e| crate::error::Error::io(path, &e))?;
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_file_takes_the_folders_it_empties() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let comp = root.join("blocks/b/components/r1.json");
        std::fs::create_dir_all(comp.parent().unwrap()).unwrap();
        std::fs::write(&comp, "{}").unwrap();
        std::fs::write(root.join("blocks/b/block.json"), "{}").unwrap();
        remove_file(root, &comp).unwrap();
        assert!(!root.join("blocks/b/components").exists());
        assert!(root.join("blocks/b").exists());
        remove_file(root, &root.join("blocks/b/block.json")).unwrap();
        assert!(!root.join("blocks").exists());
        assert!(root.exists());
    }
}
