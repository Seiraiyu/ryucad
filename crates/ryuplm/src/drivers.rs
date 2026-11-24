//! ryuplm with Git: the attribute lines a catalogue needs, and the
//! driver entries, through ryu-git's shared installer.

use crate::error::Result;
use serde_json::Value;
use std::path::Path;

/// The line that marks ryuplm's block in `.gitattributes`.
pub const HEADER: &str =
    "# ryuplm: canonical JSON, merged and diffed by ryuplm (`ryuplm git install`)";

pub const LINES: [&str; 7] = [
    "ryuplm.json merge=ryuplm diff=ryuplm text eol=lf",
    "ryupdm.json merge=ryuplm diff=ryuplm text eol=lf",
    "items/*/*.json merge=ryuplm diff=ryuplm text eol=lf",
    "documents/*/*.json merge=ryuplm diff=ryuplm text eol=lf",
    // Documents' bytes (design 937): binary, never normalized, never
    // merged or diffed as text.
    "documents/*/*/** -text -diff -merge",
    "releases/*/*.json merge=ryuplm diff=ryuplm text eol=lf",
    ".gitattributes text eol=lf",
];

const ATTRS: ryu_git::drivers::Attributes = ryu_git::drivers::Attributes {
    header: HEADER,
    lines: &LINES,
    old: &[],
};

pub const DRIVERS: ryu_git::drivers::Drivers = ryu_git::drivers::Drivers {
    product: "ryuplm",
    textconv: "diff.ryuplm.textconv",
    merge: Some(("merge.ryuplm.name", "merge.ryuplm.driver")),
    hook: true,
};

/// Writes `.gitattributes` with ryuplm's lines kept beside any it has;
/// the lines it lacked.
pub fn write_attributes(root: &Path) -> Result<Vec<&'static str>> {
    let at = root.join(".gitattributes");
    let text = std::fs::read_to_string(&at).unwrap_or_default();
    let missing = ATTRS.missing(&text, None);
    if !missing.is_empty() {
        crate::source::write_atomic(&at, ATTRS.with_lines(&text, None))?;
    }
    Ok(missing)
}

pub fn install(here: &Path, local: bool) -> Result<Value> {
    ryu_git::drivers::install(here, local, &DRIVERS, cfg!(test))
}

/// The repository's setup for ryuplm, or `None` outside one.
pub fn setup(root: &Path) -> Option<ryu_git::drivers::Setup> {
    ryu_git::drivers::setup(root, &DRIVERS, cfg!(test))
}
