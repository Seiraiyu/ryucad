//! Registering ryueda with Git (design 518), through `ryu-git`: the
//! project's `.gitattributes`, which hands ryueda's files to it, and the
//! textconv and merge driver entries in Git's config with
//! `merge.renames=false` in the clone (design 518; plan 4.C).

use crate::error::{Error, Result};
use serde_json::Value;
use std::path::Path;

pub use crate::project::ATTRIBUTES;

/// The lines ryueda needs, beside its manifest. `eol=lf` keeps files
/// canonical where Git for Windows checks text out with CRLF; model files
/// are kept byte for byte (their BLAKE3s name them).
pub const LINES: [&str; 12] = [
    "ryueda.json merge=ryueda diff=ryueda text eol=lf",
    "erc.json merge=ryueda diff=ryueda text eol=lf",
    "lint.json merge=ryueda diff=ryueda text eol=lf",
    "parts.json merge=ryueda diff=ryueda text eol=lf",
    "style.json merge=ryueda diff=ryueda text eol=lf",
    "designs/*.json merge=ryueda diff=ryueda text eol=lf",
    "rails/*.json merge=ryueda diff=ryueda text eol=lf",
    "classes/*.json merge=ryueda diff=ryueda text eol=lf",
    "blocks/**/*.json merge=ryueda diff=ryueda text eol=lf",
    "library/**/*.json merge=ryueda diff=ryueda text eol=lf",
    crate::kicad::import::MODELS_LINE,
    ".gitattributes text eol=lf",
];

/// The comment above them.
const HEADER: &str = "# ryueda: Git reads these files through ryueda (`ryueda git install`)";

const ATTRS: ryu_git::drivers::Attributes = ryu_git::drivers::Attributes {
    header: HEADER,
    lines: &LINES,
    old: &[],
};

/// The textconv entry.
pub const TEXTCONV: &str = "diff.ryueda.textconv";

/// The merge driver's name and command entries.
pub const MERGE_NAME: &str = "merge.ryueda.name";
pub const MERGE_DRIVER: &str = "merge.ryueda.driver";

/// What ryueda registers with Git: textconv and its merge driver, and
/// from Task 12 its hook line.
pub const DRIVERS: ryu_git::drivers::Drivers = ryu_git::drivers::Drivers {
    product: "ryueda",
    textconv: TEXTCONV,
    merge: Some((MERGE_NAME, MERGE_DRIVER)),
    hook: false,
};

/// The lines `text` doesn't have.
pub fn missing(text: &str) -> Vec<&'static str> {
    ATTRS.missing(text, None)
}

/// `text` with every missing line added.
pub fn with_lines(text: &str) -> String {
    ATTRS.with_lines(text, None)
}

/// The attributes file at `root` with ryueda's lines (whatever else it
/// holds kept), or `None` when nothing is missing.
pub fn attributes_wanted(root: &Path) -> Result<Option<String>> {
    let path = root.join(ATTRIBUTES);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&path, &e)),
    };
    let new = with_lines(&text);
    Ok((new != text).then_some(new))
}

/// Writes the attributes file when a line is missing.
pub fn write_attributes(root: &Path) -> Result<()> {
    if let Some(text) = attributes_wanted(root)? {
        crate::source::write_atomic(&root.join(ATTRIBUTES), text)?;
    }
    Ok(())
}

/// The repository's setup for ryueda, or `None` outside one.
pub fn setup(root: &Path) -> Option<ryu_git::drivers::Setup> {
    ryu_git::drivers::setup(root, &DRIVERS, cfg!(test))
}

/// `ryueda git install`: CLI only, no session.
pub fn install(here: &Path, local: bool) -> Result<Value> {
    ryu_git::drivers::install(here, local, &DRIVERS, cfg!(test))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_kind_reads_through_ryueda() {
        for l in [
            "ryueda.json merge=ryueda diff=ryueda text eol=lf",
            "blocks/**/*.json merge=ryueda diff=ryueda text eol=lf",
            "library/**/*.json merge=ryueda diff=ryueda text eol=lf",
            "library/models/** binary",
            ".gitattributes text eol=lf",
        ] {
            assert!(LINES.contains(&l), "{l}");
        }
        let text = with_lines("");
        assert!(missing(&text).is_empty());
        // The import's own line, written before 4.A, counts.
        let old = format!("{}\n", crate::kicad::import::MODELS_LINE);
        assert_eq!(
            with_lines(&old).matches("library/models/** binary").count(),
            1
        );
    }
}
