//! The token the client sends a shared cache: `RYUCAD_CACHE_TOKEN`, else
//! the file `cache-token` beside the user's preferences, readable by its
//! owner alone. The preferences file never holds secrets: `config get`
//! prints it, and the app shows it.

use crate::error::{Code, Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// The token, ahead of the file (CI sets it from a secret).
pub const TOKEN_VAR: &str = "RYUCAD_CACHE_TOKEN";

/// The file's name, in the user's config folder.
pub const TOKEN_FILE: &str = "cache-token";

/// Where a token came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Variable,
    File,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Variable => "variable",
            Origin::File => "file",
        }
    }
}

/// The token file's path.
pub fn path() -> Result<PathBuf> {
    Ok(crate::config::config_dir()?.join(TOKEN_FILE))
}

/// The token to send, and where it came from; `None` without one.
pub fn token() -> Option<(String, Origin)> {
    if let Ok(t) = std::env::var(TOKEN_VAR)
        && !t.is_empty()
    {
        return Some((t, Origin::Variable));
    }
    let dir = crate::config::config_dir().ok()?;
    read_in(&dir).map(|t| (t, Origin::File))
}

/// The token in `dir`'s token file, if it holds a valid one.
pub fn read_in(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(TOKEN_FILE)).ok()?;
    let t = text.trim();
    valid(t).then(|| t.to_owned())
}

/// Whether `t` can be a token: 1 to 4,096 visible ASCII characters, since
/// it travels in a header.
pub fn valid(t: &str) -> bool {
    (1..=4096).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_graphic())
}

/// Writes `token` to the token file in the user's config folder, or removes
/// the file for `None`.
pub fn store(token: Option<&str>) -> Result<()> {
    store_in(&crate::config::config_dir()?, token)
}

/// [`store`] in `dir`: written through a temporary file made readable by
/// its owner alone, then renamed.
pub fn store_in(dir: &Path, token: Option<&str>) -> Result<()> {
    let path = dir.join(TOKEN_FILE);
    let Some(t) = token else {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::io(&path, &e)),
            _ => Ok(()),
        };
    };
    if !valid(t) {
        return Err(Error::new(
            Code::InvalidParam,
            "a cache token is 1 to 4096 visible ASCII characters",
        ));
    }
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, &e))?;
    let tmp = dir.join(format!("{TOKEN_FILE}.{}.tmp", std::process::id()));
    let written = (|| {
        use std::io::Write;
        let mut o = fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&tmp)?;
        f.write_all(format!("{t}\n").as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &path)
    })();
    written.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(&path, &e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_stored_for_its_owner_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_in(dir.path()), None);
        store_in(dir.path(), Some("ryc_abc123")).unwrap();
        assert_eq!(read_in(dir.path()).as_deref(), Some("ryc_abc123"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(TOKEN_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store_in(dir.path(), Some("ryc_other")).unwrap();
        assert_eq!(read_in(dir.path()).as_deref(), Some("ryc_other"));
        store_in(dir.path(), None).unwrap();
        assert_eq!(read_in(dir.path()), None);
        store_in(dir.path(), None).unwrap();
    }

    #[test]
    fn tokens_are_visible_ascii() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "has space", "new\nline", "é"] {
            assert!(!valid(bad), "{bad:?}");
            assert_eq!(
                store_in(dir.path(), Some(bad)).unwrap_err().code,
                Code::InvalidParam
            );
        }
        assert!(!valid(&"x".repeat(4097)));
        fs::write(dir.path().join(TOKEN_FILE), "  spaced\n\n").unwrap();
        assert_eq!(read_in(dir.path()).as_deref(), Some("spaced"));
        fs::write(dir.path().join(TOKEN_FILE), "two words").unwrap();
        assert_eq!(read_in(dir.path()), None);
    }
}
