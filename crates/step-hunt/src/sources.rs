//! What a campaign reads: `sources.txt` beside this crate's manifest, one
//! line per corpus.
//!
//! ```text
//! source <name> <root> <glob> <licence> [whole]
//! ```
//!
//! `root` is a folder, relative to the workspace or absolute, and may start
//! with `${VAR:-default}` (`${RYUCAD_CORPUS:-target/corpus}`); `glob`
//! matches paths under it (`/`-separated, `*` and `?` within a name, `**`
//! any number of folders, letters in any case); `licence` is one SPDX
//! token; `whole` also runs each file whole, as well as body by body.

use std::path::{Path, PathBuf};

/// One line of the sources file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    /// As written, before expansion.
    pub root: String,
    pub glob: String,
    pub licence: String,
    pub whole: bool,
}

/// Reads a sources file. Blank lines and `#` comments are skipped; any
/// other line that isn't a source, or a name used twice, is an error.
pub fn parse(text: &str) -> Result<Vec<Source>, String> {
    let mut out: Vec<Source> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let source = match words.as_slice() {
            ["source", name, root, glob, licence, rest @ ..]
                if rest.is_empty() || rest == ["whole"] =>
            {
                Source {
                    name: (*name).to_owned(),
                    root: (*root).to_owned(),
                    glob: (*glob).to_owned(),
                    licence: (*licence).to_owned(),
                    whole: !rest.is_empty(),
                }
            }
            _ => return Err(format!("sources line {}: not a source: {line}", n + 1)),
        };
        expand(&source.root, Path::new(".")).map_err(|e| format!("sources line {}: {e}", n + 1))?;
        if out.iter().any(|s| s.name == source.name) {
            return Err(format!("source {} named twice", source.name));
        }
        out.push(source);
    }
    Ok(out)
}

/// `root` with its leading `${VAR:-default}` replaced by the variable's
/// value (the default when it is unset or empty), relative to `base`.
pub fn expand(root: &str, base: &Path) -> Result<PathBuf, String> {
    let text = match root.strip_prefix("${") {
        Some(rest) => {
            let (inner, tail) = rest
                .split_once('}')
                .ok_or_else(|| format!("{root}: an unclosed ${{"))?;
            let (var, default) = inner.split_once(":-").unwrap_or((inner, ""));
            if var.is_empty() {
                return Err(format!("{root}: no variable named"));
            }
            let value = std::env::var(var)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_owned());
            format!("{value}{tail}")
        }
        None => root.to_owned(),
    };
    Ok(base.join(text))
}

/// Whether the `/`-separated `path` matches `pattern`.
pub fn matches(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segments(&p, &s)
}

fn segments(p: &[&str], s: &[&str]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((&"**", rest)) => (0..=s.len()).any(|i| segments(rest, &s[i..])),
        Some((first, rest)) => {
            !s.is_empty() && name(first.as_bytes(), s[0].as_bytes()) && segments(rest, &s[1..])
        }
    }
}

fn name(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| name(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && name(rest, &s[1..]),
        Some((c, rest)) => !s.is_empty() && c.eq_ignore_ascii_case(&s[0]) && name(rest, &s[1..]),
    }
}

/// The files under `root` that `glob` matches, as `/`-separated paths
/// relative to it, sorted.
pub fn files(root: &Path, glob: &str) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    walk(root, "", &mut out)?;
    out.retain(|p| matches(glob, p));
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else if kind.is_file() {
            out.push(rel);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_read_and_refuse() {
        let s = parse("# a comment\n\nsource a crates/x *.stp LicenseRef-ryucad\nsource b ${V:-t/c} **/*.step GPL-3.0 whole\n").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].glob, "*.stp");
        assert!(!s[0].whole);
        assert!(s[1].whole);
        assert!(parse("source a x").unwrap_err().contains("line 1"));
        assert!(parse("source a x * L extra").is_err());
        assert!(
            parse("source a x * L\nsource a y * L")
                .unwrap_err()
                .contains("twice")
        );
        assert!(parse("source a ${V x * L").is_err());
    }

    #[test]
    fn roots_expand_with_defaults() {
        let base = Path::new("/w");
        assert_eq!(expand("a/b", base).unwrap(), Path::new("/w/a/b"));
        assert_eq!(
            expand("${STEP_HUNT_TEST_UNSET:-target/corpus}/voron", base).unwrap(),
            Path::new("/w/target/corpus/voron")
        );
        assert_eq!(expand("/abs", base).unwrap(), Path::new("/abs"));
    }

    #[test]
    fn globs_match_names_and_folders() {
        assert!(matches("*.stp", "a.STP"));
        assert!(!matches("*.stp", "d/a.stp"));
        assert!(matches("**/*.stp", "a.stp"));
        assert!(matches("**/*.stp", "d/e/a.stp"));
        assert!(matches(
            "*.3dshapes/*.step",
            "Resistor_SMD.3dshapes/R_0402.step"
        ));
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
    }

    #[test]
    fn files_are_found_sorted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("d")).unwrap();
        for f in ["b.stp", "a.stp", "d/c.stp", "d/x.txt"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        assert_eq!(
            files(dir.path(), "**/*.stp").unwrap(),
            ["a.stp", "b.stp", "d/c.stp"]
        );
    }
}
