//! What the start screen does (design 199, Start screen): New project…
//! runs `ryucad init` in an empty folder, and Open sample… writes the
//! bundled sample there first (`core::sample`). Both go through the
//! `ryucad` binary the app starts daemons from, as an agent's would, and
//! a folder that isn't empty is refused before anything is written.

use crate::core::doctor::cli;
use ryucad::client::Daemon;
use ryucad::error::{Code, Error, Result};
use std::path::Path;

/// Whether a new project can go in `dir`: it holds nothing (made if it
/// isn't there yet). Otherwise an `invalid_param` naming the folder.
pub fn empty_folder(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, &e));
    }
    let mut entries = std::fs::read_dir(dir).map_err(|e| Error::io(dir, &e))?;
    match entries.next() {
        None => Ok(()),
        Some(_) => Err(Error::new(
            Code::InvalidParam,
            format!(
                "{} isn't empty: choose an empty folder for a new project",
                dir.display()
            ),
        )
        .with("path", dir)),
    }
}

/// Makes `dir`, which must be empty, a new project: `ryucad init`.
pub fn new_project(daemon: &Daemon, dir: &Path) -> Result<()> {
    empty_folder(dir)?;
    cli(daemon, dir, &["init".to_owned()]).map(drop)
}

/// Writes the sample into `dir`, which must be empty, and makes it a
/// project there: its files, then `ryucad init` (the manifest and
/// `.gitattributes`).
pub fn open_sample(daemon: &Daemon, dir: &Path) -> Result<()> {
    empty_folder(dir)?;
    crate::core::sample::write(dir)?;
    cli(daemon, dir, &["init".to_owned()]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_empty_folder_takes_a_new_project() {
        let dir = tempfile::tempdir().unwrap();
        assert!(empty_folder(dir.path()).is_ok());
        // One that isn't there yet is made.
        let new = dir.path().join("a").join("b");
        assert!(empty_folder(&new).is_ok());
        assert!(new.is_dir());
        // Anything in it refuses it, a hidden file too.
        std::fs::write(new.join(".keep"), b"").unwrap();
        let e = empty_folder(&new).unwrap_err();
        assert_eq!(e.code, Code::InvalidParam);
        assert!(e.message.contains("isn't empty"), "{}", e.message);
    }

    #[test]
    fn nothing_is_written_into_a_folder_that_isn_t_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"mine").unwrap();
        // Refused before the binary is run: this test's own will do.
        let daemon = Daemon::this().unwrap();
        assert_eq!(
            open_sample(&daemon, dir.path()).unwrap_err().code,
            Code::InvalidParam
        );
        assert_eq!(
            new_project(&daemon, dir.path()).unwrap_err().code,
            Code::InvalidParam
        );
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1);
    }
}
