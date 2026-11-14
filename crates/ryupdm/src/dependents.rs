//! The scan for dependents' checkouts on disk: the projects of a product
//! at or under a folder, found by its manifest. Where-used over other
//! checkouts reads each one it finds.

use std::path::{Path, PathBuf};

/// Folders whose names say they hold no project: build output,
/// installed packages. Hidden folders (Git's among them) are skipped too.
pub const SKIPPED: [&str; 2] = ["target", "node_modules"];

/// The projects at or under `folder`, by `manifest` (`ryucad.json`,
/// `ryueda.json`): the folder itself when it holds one, else every
/// folder below holding one, never looking inside a project, a hidden
/// folder, `target` or `node_modules`; sorted. A folder below that can't
/// be read holds nothing to search; `folder` itself unreadable is the
/// error.
pub fn projects_under(folder: &Path, manifest: &str) -> std::io::Result<Vec<PathBuf>> {
    if folder.join(manifest).is_file() {
        return Ok(vec![folder.to_path_buf()]);
    }
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(folder)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !SKIPPED.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect();
    entries.sort();
    for dir in entries {
        out.extend(projects_under(&dir, manifest).unwrap_or_default());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_are_found_by_their_manifest() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for f in [
            "a/toy.json",
            "b/c/toy.json",
            "b/c/inner/toy.json",
            ".hidden/toy.json",
            "target/toy.json",
            "node_modules/x/toy.json",
            "e/other.json",
        ] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "{}").unwrap();
        }
        assert_eq!(
            projects_under(root, "toy.json").unwrap(),
            [root.join("a"), root.join("b/c")]
        );
        assert_eq!(
            projects_under(&root.join("a"), "toy.json").unwrap(),
            [root.join("a")]
        );
        assert!(projects_under(&root.join("nowhere"), "toy.json").is_err());
    }
}
