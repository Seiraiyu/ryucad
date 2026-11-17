//! Git's drivers for a catalogue (replaced by Task 21).

pub const LINES: [&str; 5] = [
    "ryuplm.json merge=ryuplm diff=ryuplm text eol=lf",
    "ryupdm.json merge=ryuplm diff=ryuplm text eol=lf",
    "items/*/*.json merge=ryuplm diff=ryuplm text eol=lf",
    "releases/*/*.json merge=ryuplm diff=ryuplm text eol=lf",
    ".gitattributes text eol=lf",
];
pub fn write_attributes(root: &std::path::Path) -> crate::error::Result<Vec<&'static str>> {
    let at = root.join(".gitattributes");
    let text = std::fs::read_to_string(&at).unwrap_or_default();
    let missing: Vec<&'static str> = LINES
        .iter()
        .copied()
        .filter(|l| !text.lines().any(|x| x == *l))
        .collect();
    if !missing.is_empty() {
        let mut t = text.clone();
        if !t.is_empty() && !t.ends_with('\n') {
            t.push('\n');
        }
        for l in &missing {
            t.push_str(l);
            t.push('\n');
        }
        crate::source::write_atomic(&at, t)?;
    }
    Ok(missing)
}
