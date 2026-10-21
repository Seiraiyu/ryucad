//! Registering ryucad with Git, so that `git diff` reads feature files as
//! values: the project's `.gitattributes`, which says which files Git hands
//! to ryucad, and the driver entry in Git's config, which says how. The
//! attributes are a project file, committed; the config is each user's,
//! written by the installers and `ryucad git install`. These are, with
//! nothing else, the Git config ryucad writes.

use crate::error::{Code, Error, Result};
use serde_json::{Value, json};
use std::path::Path;

/// The project's attributes file, beside its manifest; its patterns are
/// relative to the project folder.
pub const ATTRIBUTES: &str = ".gitattributes";

/// The lines ryucad needs in it.
pub const LINES: [&str; 2] = ["ryucad.json diff=ryucad", "parts/**/*.json diff=ryucad"];

/// The comment that heads ryucad's lines.
const HEADER: &str = "# ryucad: Git reads these files through ryucad (`ryucad git install`)";

/// The config entry Git's diff runs.
pub const TEXTCONV: &str = "diff.ryucad.textconv";

/// The lines of [`LINES`] that `text` doesn't have.
pub fn missing(text: &str) -> Vec<&'static str> {
    LINES
        .into_iter()
        .filter(|l| !text.lines().any(|t| t.trim() == *l))
        .collect()
}

/// `text` (the attributes file as it is, or empty) with every missing line
/// added at its end.
pub fn with_lines(text: &str) -> String {
    let lines = missing(text);
    if lines.is_empty() {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.lines().any(|l| l == HEADER) {
        out.push_str(HEADER);
        out.push('\n');
    }
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Writes the project's attributes file with ryucad's lines, keeping
/// whatever else it holds. Writes nothing when they are all there.
pub fn write_attributes(root: &Path) -> Result<()> {
    let path = root.join(ATTRIBUTES);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&path, &e)),
    };
    let new = with_lines(&text);
    if new != text {
        crate::doc::project::write_atomic(&path, new)?;
    }
    Ok(())
}

/// Whether Git, as configured for the repository at `root`, has ryucad's
/// diff driver; `None` outside a repository.
pub fn configured(root: &Path) -> Option<bool> {
    let repo = gix::discover(root).ok()?;
    Some(repo.config_snapshot().string(TEXTCONV).is_some())
}

/// The command Git runs for the driver: this binary, quoted for the shell
/// Git runs it in, with forward slashes even on Windows.
pub fn textconv_command() -> Result<String> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::new(Code::Io, format!("finding the ryucad binary: {e}")))?;
    let exe = crate::doc::project::plain(exe.canonicalize().unwrap_or(exe));
    let path = exe
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', r"'\''");
    Ok(format!("'{path}' git textconv"))
}

/// `ryucad git install`: writes the driver entry to the user's global Git
/// config, or with `local` to the config of the repository at `here`.
pub fn install(here: &Path, local: bool) -> Result<Value> {
    let command = textconv_command()?;
    let mut git = std::process::Command::new("git");
    if local {
        let repo = gix::discover(here).map_err(|_| {
            Error::new(
                Code::NotAGitRepo,
                format!("{} is not in a Git repository", here.display()),
            )
            .with("path", here)
        })?;
        let dir = repo.workdir().unwrap_or(repo.git_dir()).to_path_buf();
        git.current_dir(dir).args(["config", "--local"]);
    } else {
        git.args(["config", "--global"]);
    }
    let out = git
        .args([TEXTCONV, &command])
        .output()
        .map_err(|e| Error::new(Code::Io, format!("running git: {e}; is Git installed?")))?;
    if !out.status.success() {
        return Err(Error::new(
            Code::Io,
            format!(
                "git config failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let scope = if local { "local" } else { "global" };
    Ok(json!({"scope": scope, "config": {TEXTCONV: command}}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_added_once_and_others_kept() {
        assert_eq!(missing(""), LINES);
        let ours = with_lines("");
        assert!(missing(&ours).is_empty());
        assert_eq!(with_lines(&ours), ours);
        let theirs = "*.png binary";
        let both = with_lines(theirs);
        assert!(both.starts_with("*.png binary\n# ryucad"), "{both}");
        assert_eq!(both.lines().count(), 4);
        // One line already there, written by hand with spaces.
        let one = with_lines("  ryucad.json diff=ryucad  \n");
        assert_eq!(missing(&one), Vec::<&str>::new());
        assert_eq!(one.matches("ryucad.json").count(), 1);
    }

    #[test]
    fn the_command_is_quoted_with_forward_slashes() {
        let c = textconv_command().unwrap();
        assert!(c.starts_with('\'') && c.ends_with("' git textconv"), "{c}");
        assert!(!c.contains('\\'), "{c}");
    }

    #[test]
    fn writing_keeps_the_file_when_nothing_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_attributes(dir.path()).unwrap();
        let path = dir.path().join(ATTRIBUTES);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(missing(&text).is_empty());
        std::fs::write(&path, format!("{text}*.bin binary\n")).unwrap();
        write_attributes(dir.path()).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("*.bin binary\n")
        );
        assert_eq!(configured(dir.path()), None);
    }
}
